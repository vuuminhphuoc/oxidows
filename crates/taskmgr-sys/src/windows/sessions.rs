//! Logged-on sessions for the Users tab (WTS API).

use std::io;
use std::ptr::null_mut;

use taskmgr_core::Session;
use windows_sys::Win32::System::RemoteDesktop::{
    WTS_SESSION_INFOW, WTSDomainName, WTSEnumerateSessionsW, WTSFreeMemory, WTSQuerySessionInformationW, WTSUserName,
};

fn query(id: u32, class: i32) -> String {
    let mut p: *mut u16 = null_mut();
    let mut bytes = 0u32;
    // SAFETY: current server; WTS allocates `p`, freed below.
    unsafe {
        if WTSQuerySessionInformationW(null_mut(), id, class, &mut p, &mut bytes) == 0 || p.is_null() {
            return String::new();
        }
        let n = (bytes as usize / 2).saturating_sub(1);
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
        WTSFreeMemory(p.cast());
        s.trim_end_matches('\0').to_string()
    }
}

pub fn list_sessions() -> io::Result<Vec<Session>> {
    let mut p: *mut WTS_SESSION_INFOW = null_mut();
    let mut count = 0u32;
    // SAFETY: current server; WTS allocates the array, freed below.
    unsafe {
        if WTSEnumerateSessionsW(null_mut(), 0, 1, &mut p, &mut count) == 0 {
            return Err(io::Error::last_os_error());
        }
        let list = std::slice::from_raw_parts(p, count as usize);
        let mut out = Vec::new();
        for s in list {
            let user = query(s.SessionId, WTSUserName);
            // Session 0 and listener sessions have no user; the Users tab shows people.
            if user.is_empty() {
                continue;
            }
            let domain = query(s.SessionId, WTSDomainName);
            let state = match s.State {
                0 => "Active",
                1 => "Connected",
                4 => "Disconnected",
                6 => "Listening",
                _ => "Other",
            };
            out.push(Session {
                id: s.SessionId,
                user: if domain.is_empty() { user } else { format!("{domain}\\{user}") },
                state: state.into(),
            });
        }
        WTSFreeMemory(p.cast());
        Ok(out)
    }
}
