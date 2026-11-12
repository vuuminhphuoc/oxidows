//! The real registry: Ctrl+Shift+Esc takeover and its undo. Runs on the `windows-latest` CI
//! runner, which is elevated; elsewhere the test is skipped.
#![cfg(windows)]

use std::process::Command;

use taskmgr_sys::{TaskmgrHandler, set_replaces_taskmgr_in, taskmgr_handler_in};

/// The test works on a scratch copy of the IFEO key, so it never touches this machine's real
/// Ctrl+Shift+Esc setting (a developer's PC may have its own takeover or Process Explorer).
fn scratch_root() -> String {
    format!(r"SOFTWARE\oxidows-test\{}\IFEO", std::process::id())
}

fn reg(args: &[&str]) -> (bool, String) {
    let out = Command::new("reg").args(args).output().unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Removes the scratch key before and after the test, even when an assert fails half-way.
struct Cleanup(String);
impl Cleanup {
    fn new() -> Self {
        let c = Cleanup(format!(r"SOFTWARE\oxidows-test\{}", std::process::id()));
        c.clear();
        c
    }
    fn clear(&self) {
        let _ = reg(&["delete", &format!(r"HKLM\{}", self.0), "/f"]);
        // The shared parent only if no other run's key is in it; RegDeleteKeyW fails on a non-empty key.
        use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RegDeleteKeyW};
        let parent: Vec<u16> = r"SOFTWARE\oxidows-test".encode_utf16().chain(Some(0)).collect();
        // SAFETY: NUL-terminated key path under a predefined root.
        unsafe { RegDeleteKeyW(HKEY_LOCAL_MACHINE, parent.as_ptr()) };
    }
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        self.clear();
    }
}

#[test]
fn taskmgr_takeover_and_undo() {
    if !taskmgr_sys::is_elevated() {
        eprintln!("skipped: needs an elevated runner");
        return;
    }
    let root = scratch_root();
    let _c = Cleanup::new();
    let key = format!(r"HKLM\{root}\taskmgr.exe");
    let key = key.as_str();
    assert_eq!(taskmgr_handler_in(&root), TaskmgrHandler::BuiltIn, "the scratch key starts empty");

    // Clean start: take over, undo, and nothing of ours is left.
    set_replaces_taskmgr_in(true, &root).unwrap();
    assert_eq!(taskmgr_handler_in(&root), TaskmgrHandler::ThisExe);
    let (_, q) = reg(&["query", key, "/v", "Debugger"]);
    let me = std::env::current_exe().unwrap().display().to_string();
    assert!(q.to_lowercase().contains(&me.to_lowercase()), "{q}");
    set_replaces_taskmgr_in(false, &root).unwrap();
    assert_eq!(taskmgr_handler_in(&root), TaskmgrHandler::BuiltIn);
    let (exists, q) = reg(&["query", key]);
    assert!(!exists || !q.contains("Debugger") && !q.contains("oxidows"), "left behind: {q}");

    // Another tool had it (Process Explorer): undo gives it back to that tool.
    let procexp = r"C:\Sys\procexp64.exe";
    assert!(reg(&["add", key, "/v", "Debugger", "/d", &format!("\"{procexp}\""), "/f"]).0);
    assert_eq!(taskmgr_handler_in(&root), TaskmgrHandler::OtherTool(procexp.into()));
    set_replaces_taskmgr_in(true, &root).unwrap();
    assert_eq!(taskmgr_handler_in(&root), TaskmgrHandler::ThisExe);
    set_replaces_taskmgr_in(false, &root).unwrap();
    assert_eq!(taskmgr_handler_in(&root), TaskmgrHandler::OtherTool(procexp.into()));
    let (_, q) = reg(&["query", key]);
    assert!(!q.contains("oxidows"), "left behind: {q}");

    // Undo never takes out a tool that came after us.
    assert!(reg(&["delete", key, "/v", "Debugger", "/f"]).0);
    set_replaces_taskmgr_in(true, &root).unwrap();
    assert!(reg(&["add", key, "/v", "Debugger", "/d", &format!("\"{procexp}\""), "/f"]).0);
    set_replaces_taskmgr_in(false, &root).unwrap();
    assert_eq!(taskmgr_handler_in(&root), TaskmgrHandler::OtherTool(procexp.into()));
}

/// Settings > Run as administrator > Always, without a prompt: the task is accepted by Task
/// Scheduler as written, has what it needs, and is gone after the undo.
#[test]
fn admin_task_add_and_remove() {
    if !taskmgr_sys::is_elevated() {
        eprintln!("skipped: needs an elevated runner");
        return;
    }
    let me = std::env::current_exe().unwrap().display().to_string();
    let name = taskmgr_sys::admin_task_name(&me);
    struct Remove;
    impl Drop for Remove {
        fn drop(&mut self) {
            let _ = taskmgr_sys::set_admin_task(false);
        }
    }
    let _r = Remove;
    assert!(!taskmgr_sys::admin_task_exists(), "left over from an earlier run: {name}");
    taskmgr_sys::set_admin_task(true).unwrap();
    assert!(taskmgr_sys::admin_task_exists());
    // Again (Settings chosen twice, or a fix after the exe was replaced): replaces it.
    taskmgr_sys::set_admin_task_as_admin(true).unwrap();
    let out = Command::new("schtasks").args(["/query", "/tn", &name, "/xml"]).output().unwrap();
    assert!(out.status.success());
    // schtasks writes the XML as UTF-16 when piped on some versions, as ANSI on others.
    let xml = if out.stdout.iter().filter(|b| **b == 0).count() > out.stdout.len() / 4 {
        let w: Vec<u16> = out.stdout.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&w)
    } else {
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    eprintln!("{xml}");
    assert!(xml.contains("HighestAvailable"), "{xml}");
    assert!(xml.contains("--from-task"), "{xml}");
    assert!(xml.to_lowercase().contains(&me.to_lowercase()), "{xml}");
    taskmgr_sys::set_admin_task(false).unwrap();
    assert!(!taskmgr_sys::admin_task_exists());
    // Removing what is not there is fine (Settings switched away twice).
    taskmgr_sys::set_admin_task(false).unwrap();
}
