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
