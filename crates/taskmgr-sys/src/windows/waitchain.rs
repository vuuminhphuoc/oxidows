//! "Analyze wait chain" (Wait Chain Traversal) and the UAC virtualization switch on Details.

use std::io;
use std::mem::{size_of, zeroed};
use std::ptr::null_mut;

use taskmgr_core::WaitChain;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::{
    GetTokenInformation, SetTokenInformation, TOKEN_ADJUST_DEFAULT, TOKEN_QUERY, TokenVirtualizationAllowed,
    TokenVirtualizationEnabled,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    CloseThreadWaitChainSession, GetThreadWaitChain, OpenThreadWaitChainSession, WAITCHAIN_NODE_INFO,
    WCT_MAX_NODE_COUNT, WctAlpcType, WctComActivationType, WctComType, WctCriticalSectionType, WctMutexType,
    WctProcessWaitType, WctSendMessageType, WctSmbIoType, WctSocketIoType, WctStatusBlocked, WctThreadType,
    WctThreadWaitType,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::Threading::{OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: handle we own.
        unsafe { CloseHandle(self.0) };
    }
}

fn threads_of(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    // SAFETY: snapshot closed by Handle; entry sized as the API requires.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let snap = Handle(snap);
        let mut e: THREADENTRY32 = zeroed();
        e.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut ok = Thread32First(snap.0, &mut e) != 0;
        while ok {
            if e.th32OwnerProcessID == pid {
                out.push(e.th32ThreadID);
            }
            ok = Thread32Next(snap.0, &mut e) != 0;
        }
    }
    out
}

#[allow(non_upper_case_globals)]
fn kind(t: i32) -> &'static str {
    match t {
        WctCriticalSectionType => "critical section",
        WctSendMessageType => "SendMessage",
        WctMutexType => "mutex",
        WctAlpcType => "ALPC",
        WctComType => "COM call",
        WctThreadWaitType => "thread",
        WctProcessWaitType => "process",
        WctComActivationType => "COM activation",
        WctSocketIoType => "socket I/O",
        WctSmbIoType => "SMB I/O",
        _ => "object",
    }
}

/// The threads of `pid` that are blocked, each with what it waits on and which processes hold it.
pub fn wait_chain(pid: u32) -> io::Result<Vec<WaitChain>> {
    // SAFETY: synchronous session (no callback), closed below.
    let wct = unsafe { OpenThreadWaitChainSession(0, None) };
    if wct.is_null() {
        return Err(io::Error::last_os_error());
    }
    let mut out = Vec::new();
    for tid in threads_of(pid) {
        let mut nodes: [WAITCHAIN_NODE_INFO; WCT_MAX_NODE_COUNT as usize] = Default::default();
        let mut count = WCT_MAX_NODE_COUNT;
        let mut cycle = 0;
        // SAFETY: array of `count` nodes, valid session.
        let ok = unsafe { GetThreadWaitChain(wct, 0, 0, tid, &mut count, nodes.as_mut_ptr(), &mut cycle) };
        if ok == 0 || count < 2 {
            continue;
        }
        let nodes = &nodes[..count.min(WCT_MAX_NODE_COUNT) as usize];
        // The first node is the thread itself; a chain only matters when it is blocked.
        if nodes[0].ObjectStatus != WctStatusBlocked {
            continue;
        }
        let mut chain = WaitChain { thread: tid, cycle: cycle != 0, ..Default::default() };
        for n in &nodes[1..] {
            if n.ObjectType == WctThreadType {
                // SAFETY: thread nodes use the ThreadObject arm.
                let t = unsafe { n.Anonymous.ThreadObject };
                chain.steps.push(format!("thread {} of process {}", t.ThreadId, t.ProcessId));
                if t.ProcessId != pid && t.ProcessId != 0 && !chain.blocking.contains(&t.ProcessId) {
                    chain.blocking.push(t.ProcessId);
                }
            } else {
                // SAFETY: other nodes use the LockObject arm.
                let name = unsafe { n.Anonymous.LockObject.ObjectName };
                let len = name.iter().position(|c| *c == 0).unwrap_or(name.len());
                let name = String::from_utf16_lossy(&name[..len]);
                chain.steps.push(if name.is_empty() {
                    kind(n.ObjectType).to_string()
                } else {
                    format!("{} {name}", kind(n.ObjectType))
                });
            }
        }
        out.push(chain);
    }
    // SAFETY: session opened above.
    unsafe { CloseThreadWaitChainSession(wct) };
    Ok(out)
}

fn token(pid: u32, access: u32) -> io::Result<Handle> {
    // SAFETY: plain calls; handles closed by Handle.
    unsafe {
        let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if p.is_null() {
            return Err(io::Error::last_os_error());
        }
        let p = Handle(p);
        let mut t = null_mut();
        if OpenProcessToken(p.0, access, &mut t) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Handle(t))
    }
}

/// None when the process may not be virtualized (elevated, services, 64-bit with a manifest...).
pub fn uac_virtualization(pid: u32) -> Option<bool> {
    let t = token(pid, TOKEN_QUERY).ok()?;
    let get = |class| {
        let (mut v, mut len) = (0u32, 0u32);
        // SAFETY: 4-byte buffer.
        (unsafe { GetTokenInformation(t.0, class, (&raw mut v).cast(), 4, &mut len) } != 0).then_some(v)
    };
    if get(TokenVirtualizationAllowed)? == 0 {
        return None;
    }
    Some(get(TokenVirtualizationEnabled)? != 0)
}

pub fn set_uac_virtualization(pid: u32, on: bool) -> io::Result<()> {
    let t = token(pid, TOKEN_ADJUST_DEFAULT | TOKEN_QUERY)?;
    let v: u32 = on.into();
    // SAFETY: 4-byte value.
    if unsafe { SetTokenInformation(t.0, TokenVirtualizationEnabled, (&raw const v).cast(), 4) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
