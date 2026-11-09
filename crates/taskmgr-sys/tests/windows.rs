//! The real registry: Ctrl+Shift+Esc takeover and its undo. Runs on the `windows-latest` CI
//! runner, which is elevated; elsewhere the test is skipped.
#![cfg(windows)]

use std::process::Command;

use taskmgr_sys::{TaskmgrHandler, set_replaces_taskmgr, set_replaces_taskmgr_as_admin, taskmgr_handler};

const KEY: &str = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\taskmgr.exe";

fn reg(args: &[&str]) -> (bool, String) {
    let out = Command::new("reg").args(args).output().unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Leaves the runner's Ctrl+Shift+Esc alone even when an assert fails half-way.
struct Cleanup;
impl Drop for Cleanup {
    fn drop(&mut self) {
        for v in ["Debugger", "oxidows.PreviousDebugger", "oxidows.Debugger"] {
            let _ = reg(&["delete", KEY, "/v", v, "/f"]);
        }
    }
}

#[test]
fn taskmgr_takeover_and_undo() {
    if !taskmgr_sys::is_elevated() {
        eprintln!("skipped: needs an elevated runner");
        return;
    }
    assert_eq!(taskmgr_handler(), TaskmgrHandler::BuiltIn, "the runner should start with Windows Task Manager");
    let _c = Cleanup;

    // Clean start: take over, undo, and nothing of ours is left.
    set_replaces_taskmgr_as_admin(true).unwrap();
    assert_eq!(taskmgr_handler(), TaskmgrHandler::ThisExe);
    let (_, q) = reg(&["query", KEY, "/v", "Debugger"]);
    let me = std::env::current_exe().unwrap().display().to_string();
    assert!(q.to_lowercase().contains(&me.to_lowercase()), "{q}");
    set_replaces_taskmgr_as_admin(false).unwrap();
    assert_eq!(taskmgr_handler(), TaskmgrHandler::BuiltIn);
    let (exists, q) = reg(&["query", KEY]);
    assert!(!exists || !q.contains("Debugger") && !q.contains("oxidows"), "left behind: {q}");

    // Another tool had it (Process Explorer): undo gives it back to that tool.
    let procexp = r"C:\Sys\procexp64.exe";
    assert!(reg(&["add", KEY, "/v", "Debugger", "/d", &format!("\"{procexp}\""), "/f"]).0);
    assert_eq!(taskmgr_handler(), TaskmgrHandler::OtherTool(procexp.into()));
    set_replaces_taskmgr(true).unwrap();
    assert_eq!(taskmgr_handler(), TaskmgrHandler::ThisExe);
    set_replaces_taskmgr(false).unwrap();
    assert_eq!(taskmgr_handler(), TaskmgrHandler::OtherTool(procexp.into()));
    let (_, q) = reg(&["query", KEY]);
    assert!(!q.contains("oxidows"), "left behind: {q}");

    // Undo never takes out a tool that came after us.
    assert!(reg(&["delete", KEY, "/v", "Debugger", "/f"]).0);
    set_replaces_taskmgr(true).unwrap();
    assert!(reg(&["add", KEY, "/v", "Debugger", "/d", &format!("\"{procexp}\""), "/f"]).0);
    set_replaces_taskmgr(false).unwrap();
    assert_eq!(taskmgr_handler(), TaskmgrHandler::OtherTool(procexp.into()));
}
