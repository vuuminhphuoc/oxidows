//! Per-process facts read once (path, command line, user), priority changes, elevation.

use std::ffi::c_void;
use std::io;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use taskmgr_core::StaticInfo;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::{
    GetTokenInformation, LookupAccountSidW, SID_NAME_USE, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER, TokenElevation,
    TokenUser,
};
use windows_sys::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, HIGH_PRIORITY_CLASS,
    IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, QueryFullProcessImageNameW, REALTIME_PRIORITY_CLASS,
    SetPriorityClass,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::PriorityClass;

const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(h: HANDLE, class: u32, buf: *mut c_void, len: u32, ret: *mut u32) -> i32;
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: a handle we opened.
        unsafe { CloseHandle(self.0) };
    }
}

fn open(pid: u32, access: u32) -> io::Result<Handle> {
    // SAFETY: plain Win32 call.
    let h = unsafe { OpenProcess(access, 0, pid) };
    if h.is_null() { Err(io::Error::last_os_error()) } else { Ok(Handle(h)) }
}

pub fn static_info(pid: u32) -> StaticInfo {
    let Ok(h) = open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
        return StaticInfo::default();
    };
    let path = image_path(&h);
    let (description, publisher) = version_strings(&path);
    StaticInfo { command_line: command_line(&h), user: token_user(h.0), description, publisher, path }
}

fn image_path(h: &Handle) -> String {
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: buffer of `len` u16.
    if unsafe { QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) } == 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..len as usize])
}

fn command_line(h: &Handle) -> String {
    let mut buf = vec![0u64; 512];
    loop {
        let mut ret = 0u32;
        let len = (buf.len() * 8) as u32;
        // SAFETY: 8-byte aligned buffer of `len` bytes.
        let st = unsafe {
            NtQueryInformationProcess(h.0, PROCESS_COMMAND_LINE_INFORMATION, buf.as_mut_ptr().cast(), len, &mut ret)
        };
        if st < 0 {
            // STATUS_INFO_LENGTH_MISMATCH / BUFFER_TOO_SMALL: grow once to the reported size.
            if ret as usize > buf.len() * 8 && buf.len() < 1 << 16 {
                buf.resize((ret as usize).div_ceil(8), 0);
                continue;
            }
            return String::new();
        }
        // The buffer starts with a UNICODE_STRING pointing just past itself.
        #[repr(C)]
        struct Us {
            length: u16,
            max: u16,
            buffer: *const u16,
        }
        // SAFETY: the kernel wrote a UNICODE_STRING whose buffer lies inside our allocation.
        unsafe {
            let us = &*buf.as_ptr().cast::<Us>();
            if us.buffer.is_null() {
                return String::new();
            }
            return String::from_utf16_lossy(std::slice::from_raw_parts(us.buffer, usize::from(us.length) / 2));
        }
    }
}

fn token_user(process: HANDLE) -> String {
    let mut tok = null_mut();
    // SAFETY: out-pointer to a local.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut tok) } == 0 {
        return String::new();
    }
    let tok = Handle(tok);
    let mut buf = vec![0u64; 64];
    let mut ret = 0u32;
    // SAFETY: aligned buffer of the given size.
    let ok =
        unsafe { GetTokenInformation(tok.0, TokenUser, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32, &mut ret) };
    if ok == 0 {
        return String::new();
    }
    // SAFETY: GetTokenInformation(TokenUser) wrote a TOKEN_USER at the start.
    let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let (mut name, mut domain) = (vec![0u16; 256], vec![0u16; 256]);
    let (mut nlen, mut dlen) = (name.len() as u32, domain.len() as u32);
    let mut use_: SID_NAME_USE = 0;
    // SAFETY: buffers sized by nlen / dlen.
    let ok = unsafe {
        LookupAccountSidW(null(), sid, name.as_mut_ptr(), &mut nlen, domain.as_mut_ptr(), &mut dlen, &mut use_)
    };
    if ok == 0 {
        return String::new();
    }
    let name = String::from_utf16_lossy(&name[..nlen as usize]);
    let domain = String::from_utf16_lossy(&domain[..dlen as usize]);
    if domain.is_empty() { name } else { format!("{domain}\\{name}") }
}

pub fn set_priority(pid: u32, class: PriorityClass) -> io::Result<()> {
    let h = open(pid, PROCESS_SET_INFORMATION)?;
    let c = match class {
        PriorityClass::Idle => IDLE_PRIORITY_CLASS,
        PriorityClass::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS,
        PriorityClass::Normal => NORMAL_PRIORITY_CLASS,
        PriorityClass::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
        PriorityClass::High => HIGH_PRIORITY_CLASS,
        PriorityClass::Realtime => REALTIME_PRIORITY_CLASS,
    };
    // SAFETY: valid handle.
    if unsafe { SetPriorityClass(h.0, c) } == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

pub fn is_elevated() -> bool {
    let mut tok = null_mut();
    // SAFETY: pseudo-handle for the current process, out-pointer to a local.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) } == 0 {
        return false;
    }
    let tok = Handle(tok);
    // SAFETY: POD out-struct of the declared size.
    unsafe {
        let mut e: TOKEN_ELEVATION = zeroed();
        let mut ret = 0u32;
        GetTokenInformation(
            tok.0,
            TokenElevation,
            (&mut e as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret,
        ) != 0
            && e.TokenIsElevated != 0
    }
}

/// Starts this exe again through the UAC prompt. The caller exits on success.
pub fn restart_elevated() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let exe: Vec<u16> = exe.as_os_str().encode_wide_nul();
    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    // SAFETY: NUL-terminated wide strings.
    let r = unsafe { ShellExecuteW(null_mut(), verb.as_ptr(), exe.as_ptr(), null(), null(), SW_SHOWNORMAL) };
    // Values above 32 mean success.
    if r as usize > 32 { Ok(()) } else { Err(io::Error::other("the administrator prompt was cancelled or failed")) }
}

trait EncodeWideNul {
    fn encode_wide_nul(&self) -> Vec<u16>;
}
impl EncodeWideNul for std::ffi::OsStr {
    fn encode_wide_nul(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(Some(0)).collect()
    }
}

// ---- Process actions: affinity, efficiency mode, dump file, run new task ----

use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::System::Diagnostics::Debug::{MiniDumpWithFullMemory, MiniDumpWriteDump};
use windows_sys::Win32::System::Threading::{
    GetProcessAffinityMask, GetProcessInformation, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_INFORMATION,
    PROCESS_VM_READ, ProcessPowerThrottling, SetProcessAffinityMask, SetProcessInformation,
};

/// (process mask, system mask).
pub fn affinity(pid: u32) -> io::Result<(u64, u64)> {
    let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let (mut p, mut s) = (0usize, 0usize);
    // SAFETY: out-pointers to locals.
    if unsafe { GetProcessAffinityMask(h.0, &mut p, &mut s) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((p as u64, s as u64))
}

pub fn set_affinity(pid: u32, mask: u64) -> io::Result<()> {
    let h = open(pid, PROCESS_SET_INFORMATION)?;
    // SAFETY: valid handle.
    if unsafe { SetProcessAffinityMask(h.0, mask as usize) } == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

pub fn efficiency_mode(pid: u32) -> io::Result<bool> {
    let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    // SAFETY: POD out-struct with the version set, as the API requires.
    unsafe {
        let mut st: PROCESS_POWER_THROTTLING_STATE = zeroed();
        st.Version = PROCESS_POWER_THROTTLING_CURRENT_VERSION;
        if GetProcessInformation(
            h.0,
            ProcessPowerThrottling,
            (&mut st as *mut PROCESS_POWER_THROTTLING_STATE).cast(),
            size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(st.ControlMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0
            && st.StateMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0)
    }
}

/// Like the built-in tool: EcoQoS throttling plus Low priority on, Normal priority off.
pub fn set_efficiency_mode(pid: u32, on: bool) -> io::Result<()> {
    let h = open(pid, PROCESS_SET_INFORMATION)?;
    let st = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: if on { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
    };
    // SAFETY: valid handle, struct of the declared size.
    unsafe {
        if SetProcessInformation(
            h.0,
            ProcessPowerThrottling,
            (&st as *const PROCESS_POWER_THROTTLING_STATE).cast(),
            size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        if SetPriorityClass(h.0, if on { IDLE_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS }) == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Full-memory dump, the same kind "Create memory dump file" writes. Slow for big processes.
pub fn create_dump(pid: u32, path: &std::path::Path) -> io::Result<()> {
    let h = open(pid, PROCESS_QUERY_INFORMATION | PROCESS_VM_READ)?;
    let file = std::fs::File::create(path)?;
    // SAFETY: valid process and file handles; no optional streams.
    let ok =
        unsafe { MiniDumpWriteDump(h.0, pid, file.as_raw_handle(), MiniDumpWithFullMemory, null(), null(), null()) };
    if ok == 0 {
        let e = io::Error::last_os_error();
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(e);
    }
    Ok(())
}

/// `ShellExecuteW` of a program (first token, quotes allowed) with the rest as its arguments.
pub fn run_new_task(command: &str, admin: bool) -> io::Result<()> {
    let (file, params) = crate::split_command(command);
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
    let (f, p) = (wide(&file), wide(&params));
    let verb = wide(if admin { "runas" } else { "open" });
    // SAFETY: NUL-terminated wide strings.
    let r = unsafe {
        ShellExecuteW(
            null_mut(),
            verb.as_ptr(),
            f.as_ptr(),
            if params.is_empty() { null() } else { p.as_ptr() },
            null(),
            SW_SHOWNORMAL,
        )
    };
    if r as usize > 32 {
        Ok(())
    } else {
        Err(io::Error::other(format!("Windows cannot find '{file}' (code {})", r as usize)))
    }
}

pub fn open_url(url: &str) -> io::Result<()> {
    run_new_task(url, false)
}

// ---- Apps (processes with a visible window) and version-resource strings ----

use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetWindow, GetWindowTextLengthW, GetWindowThreadProcessId, IsWindowVisible,
};

unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
    // SAFETY: lparam is the &mut HashSet passed by `window_pids`, alive for the EnumWindows call.
    unsafe {
        let set = &mut *(lparam as *mut std::collections::HashSet<u32>);
        if IsWindowVisible(hwnd) == 0 || !GetWindow(hwnd, GW_OWNER).is_null() || GetWindowTextLengthW(hwnd) == 0 {
            return 1;
        }
        let mut cloaked = 0u32;
        DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as u32, (&mut cloaked as *mut u32).cast(), 4);
        if cloaked != 0 {
            return 1;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        set.insert(pid);
    }
    1
}

/// PIDs owning a visible, unowned, titled top-level window: what Task Manager lists under "Apps".
pub fn window_pids() -> std::collections::HashSet<u32> {
    let mut set = std::collections::HashSet::new();
    // SAFETY: callback only touches `set` during this call.
    unsafe { EnumWindows(Some(collect_window), &mut set as *mut _ as LPARAM) };
    set
}

/// (FileDescription, CompanyName) from the exe's version resource.
pub fn version_strings(path: &str) -> (String, String) {
    if path.is_empty() {
        return Default::default();
    }
    let wpath: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    // SAFETY: buffers sized by the API; VerQueryValueW returns pointers into `data`.
    unsafe {
        let size = GetFileVersionInfoSizeW(wpath.as_ptr(), null_mut());
        if size == 0 {
            return Default::default();
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(wpath.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
            return Default::default();
        }
        // First language/codepage pair, falling back to US English / Unicode.
        let mut lang = (0x0409u16, 0x04B0u16);
        let mut p: *mut c_void = null_mut();
        let mut len = 0u32;
        let tr: Vec<u16> = r"\VarFileInfo\Translation".encode_utf16().chain(Some(0)).collect();
        if VerQueryValueW(data.as_ptr().cast(), tr.as_ptr(), &mut p, &mut len) != 0 && len >= 4 {
            let pair = p.cast::<u16>();
            lang = (*pair, *pair.add(1));
        }
        let query = |key: &str| -> String {
            let q: Vec<u16> =
                format!(r"\StringFileInfo\{:04x}{:04x}\{key}", lang.0, lang.1).encode_utf16().chain(Some(0)).collect();
            let mut p: *mut c_void = null_mut();
            let mut len = 0u32;
            if VerQueryValueW(data.as_ptr().cast(), q.as_ptr(), &mut p, &mut len) == 0 || len == 0 {
                return String::new();
            }
            let s = std::slice::from_raw_parts(p.cast::<u16>(), len as usize);
            String::from_utf16_lossy(s).trim_end_matches('\0').trim().to_string()
        };
        (query("FileDescription"), query("CompanyName"))
    }
}

use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};

/// True if a process with this image name (case-insensitive) is running.
pub fn is_running(name: &str) -> bool {
    // SAFETY: snapshot handle closed below; entry sized as the API requires.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut e: PROCESSENTRY32W = zeroed();
        e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            if String::from_utf16_lossy(&e.szExeFile[..n]).eq_ignore_ascii_case(name) {
                found = true;
                break;
            }
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
        found
    }
}
