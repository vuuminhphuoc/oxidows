//! One Task Manager at a time, as Windows' own: starting it again (Ctrl+Shift+Esc, the taskbar,
//! a second double-click) brings the open window to the front instead of opening another.
//!
//! A named mutex marks the running instance. Its owner releases it by exiting (the system
//! abandons it), so a restart ("Run as administrator", the renderer fallback) can wait for it.

use std::ptr::null;

use windows_sys::core::BOOL;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentProcessId, WaitForSingleObject};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsIconic, SW_RESTORE, SW_SHOW, SetForegroundWindow,
    ShowWindowAsync,
};

const NAME: &str = "Local\\oxidows.taskmgr.instance";
/// The start of the main window's title (see the app's `ViewportBuilder::with_title`).
const TITLE: &str = "Task Manager (oxidows)";

/// True when this process is now the one Task Manager; false when another is running (its
/// window has been brought forward and this process should exit). `wait_for_previous`: this is
/// a restart of an instance that is exiting, so wait for it instead.
pub fn claim(wait_for_previous: bool) -> bool {
    let name: Vec<u16> = NAME.encode_utf16().chain(Some(0)).collect();
    // SAFETY: NUL-terminated name; the handle is kept for the life of the process (the
    // system closes it at exit, which is what frees the name for the next instance).
    let h = unsafe { CreateMutexW(null(), 1, name.as_ptr()) };
    // SAFETY: no arguments; read right after the call.
    let err = unsafe { GetLastError() };
    if h.is_null() {
        // An elevated instance's mutex is closed to a non-elevated process: it is running.
        if err == ERROR_ACCESS_DENIED {
            if bring_forward() {
                return false;
            }
            // No window: it is exiting or still starting. Give it a moment, then go on.
            std::thread::sleep(std::time::Duration::from_secs(2));
            return !bring_forward();
        }
        return true; // can't tell: better two windows than none
    }
    if err != ERROR_ALREADY_EXISTS {
        return true; // we own it
    }
    if !wait_for_previous && bring_forward() {
        // SAFETY: our handle.
        unsafe { CloseHandle(h) };
        return false;
    }
    // A restart, or an instance with no window yet (still starting, or being ended): wait.
    let ms = if wait_for_previous { 15_000 } else { 3_000 };
    // SAFETY: our mutex handle.
    let r = unsafe { WaitForSingleObject(h, ms) };
    if r == WAIT_OBJECT_0 || r == WAIT_ABANDONED {
        return true;
    }
    if bring_forward() {
        // SAFETY: our handle.
        unsafe { CloseHandle(h) };
        return false;
    }
    true
}

/// Shows, restores and focuses another process's Task Manager window. False if there is none.
fn bring_forward() -> bool {
    unsafe extern "system" fn each(h: HWND, found: LPARAM) -> BOOL {
        let mut pid = 0u32;
        let mut buf = [0u16; 128];
        // SAFETY: valid window handle from EnumWindows; buffers sized as passed.
        unsafe {
            GetWindowThreadProcessId(h, &mut pid);
            if pid == GetCurrentProcessId() {
                return 1;
            }
            let n = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
            if String::from_utf16_lossy(&buf[..n]).starts_with(TITLE) {
                *(found as *mut HWND) = h;
                return 0;
            }
        }
        1
    }
    let mut found: HWND = std::ptr::null_mut();
    // SAFETY: the callback writes only through the pointer to `found`, which outlives the call.
    unsafe { EnumWindows(Some(each), &mut found as *mut HWND as LPARAM) };
    if found.is_null() {
        return false;
    }
    // SAFETY: a top-level window handle; the async form never blocks on the other process.
    unsafe {
        // Hidden to the notification area, or minimized: show it as Task Manager does.
        ShowWindowAsync(found, SW_SHOW);
        if IsIconic(found) != 0 {
            ShowWindowAsync(found, SW_RESTORE);
        }
        SetForegroundWindow(found);
    }
    true
}
