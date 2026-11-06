//! Services: one EnumServicesStatusExW call for the list; start / stop / restart by name.

use std::io;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use taskmgr_core::{Service, ServiceState};
use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, GetLastError};
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, ControlService, ENUM_SERVICE_STATUS_PROCESSW, EnumServicesStatusExW, OpenSCManagerW,
    OpenServiceW, QueryServiceStatus, SC_ENUM_PROCESS_INFO, SC_HANDLE, SC_MANAGER_CONNECT,
    SC_MANAGER_ENUMERATE_SERVICE, SERVICE_CONTROL_STOP, SERVICE_QUERY_STATUS, SERVICE_START, SERVICE_STATE_ALL,
    SERVICE_STATUS, SERVICE_STOP, SERVICE_STOPPED, SERVICE_WIN32, StartServiceW,
};

use crate::ServiceAction;

struct Sc(SC_HANDLE);
impl Drop for Sc {
    fn drop(&mut self) {
        // SAFETY: handle from OpenSCManagerW / OpenServiceW.
        unsafe { CloseServiceHandle(self.0) };
    }
}

fn manager(access: u32) -> io::Result<Sc> {
    // SAFETY: local machine, active database.
    let h = unsafe { OpenSCManagerW(null(), null(), access) };
    if h.is_null() { Err(io::Error::last_os_error()) } else { Ok(Sc(h)) }
}

fn state(v: u32) -> ServiceState {
    match v {
        1 => ServiceState::Stopped,
        2 => ServiceState::StartPending,
        3 => ServiceState::StopPending,
        4 => ServiceState::Running,
        5 => ServiceState::ContinuePending,
        6 => ServiceState::PausePending,
        7 => ServiceState::Paused,
        _ => ServiceState::Unknown,
    }
}

unsafe fn wstr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    // SAFETY: caller passes a NUL-terminated string.
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }
}

pub fn list_services() -> io::Result<Vec<Service>> {
    let scm = manager(SC_MANAGER_ENUMERATE_SERVICE)?;
    let mut buf: Vec<u64> = vec![0; 32 * 1024];
    loop {
        let (mut needed, mut count, mut resume) = (0u32, 0u32, 0u32);
        // SAFETY: aligned buffer of the stated size.
        let ok = unsafe {
            EnumServicesStatusExW(
                scm.0,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                buf.as_mut_ptr().cast(),
                (buf.len() * 8) as u32,
                &mut needed,
                &mut count,
                &mut resume,
                null(),
            )
        };
        // SAFETY: plain call.
        if ok == 0 && unsafe { GetLastError() } == ERROR_MORE_DATA && resume == 0 {
            buf.resize(buf.len() + (needed as usize).div_ceil(8) + 1024, 0);
            continue;
        }
        if ok == 0 && count == 0 {
            return Err(io::Error::last_os_error());
        }
        let items = buf.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>();
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            // SAFETY: the call wrote `count` entries; their strings live later in the same buffer.
            unsafe {
                let e = &*items.add(i);
                out.push(Service {
                    name: wstr(e.lpServiceName),
                    display_name: wstr(e.lpDisplayName),
                    state: state(e.ServiceStatusProcess.dwCurrentState),
                    pid: e.ServiceStatusProcess.dwProcessId,
                });
            }
        }
        out.sort_by_cached_key(|s| s.name.to_lowercase());
        return Ok(out);
    }
}

/// Blocks until done (a restart waits up to 20 s for the stop); call it off the UI thread.
pub fn service_control(name: &str, action: ServiceAction) -> io::Result<()> {
    let scm = manager(SC_MANAGER_CONNECT)?;
    let wname: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: NUL-terminated name.
    let h = unsafe { OpenServiceW(scm.0, wname.as_ptr(), SERVICE_START | SERVICE_STOP | SERVICE_QUERY_STATUS) };
    if h.is_null() {
        return Err(io::Error::last_os_error());
    }
    let svc = Sc(h);
    let stop = || -> io::Result<()> {
        // SAFETY: zeroed POD out-struct.
        let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        // SAFETY: valid service handle.
        if unsafe { ControlService(svc.0, SERVICE_CONTROL_STOP, &mut st) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    };
    let start = || -> io::Result<()> {
        // SAFETY: no arguments.
        if unsafe { StartServiceW(svc.0, 0, null_mut()) } == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    };
    match action {
        ServiceAction::Start => start(),
        ServiceAction::Stop => stop(),
        ServiceAction::Restart => {
            stop()?;
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                // SAFETY: zeroed POD out-struct, valid handle.
                let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
                if unsafe { QueryServiceStatus(svc.0, &mut st) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                if st.dwCurrentState == SERVICE_STOPPED {
                    break;
                }
                if Instant::now() > deadline {
                    return Err(io::Error::other("the service did not stop within 20 s"));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            start()
        }
    }
}
