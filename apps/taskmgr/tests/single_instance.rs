//! One Task Manager at a time. Runs on the `windows-latest` CI runner (it has a desktop).
#![cfg(windows)]

use std::process::{Child, Command};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_oxi-taskmgr");

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start(args: &[&str]) -> Kill {
    Kill(Command::new(EXE).args(args).env("OXI_RENDERER", "wgpu").spawn().unwrap())
}

/// Waits up to `secs` for the process to exit; its exit code, or None if still running.
fn exit_within(p: &mut Kill, secs: u64) -> Option<i32> {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        if let Some(s) = p.0.try_wait().unwrap() {
            return Some(s.code().unwrap_or(-1));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

#[test]
fn second_start_hands_over_to_the_first() {
    let mut first = start(&[]);
    // Long enough for the window to exist, even under WARP.
    std::thread::sleep(Duration::from_secs(8));
    assert_eq!(exit_within(&mut first, 0), None, "the first instance should be running");

    let mut second = start(&[]);
    assert_eq!(exit_within(&mut second, 15), Some(0), "the second start should exit at once");
    assert_eq!(exit_within(&mut first, 0), None, "the first instance should still be running");

    // A restart (Run as administrator, renderer fallback) waits for the old one to go.
    let mut restarted = start(&["--restarted"]);
    std::thread::sleep(Duration::from_secs(2));
    drop(first);
    std::thread::sleep(Duration::from_secs(4));
    assert_eq!(exit_within(&mut restarted, 0), None, "the restarted instance should take over");
    drop(restarted);

    // Once nothing runs, a start is the instance again.
    std::thread::sleep(Duration::from_secs(1));
    let mut again = start(&[]);
    assert_eq!(exit_within(&mut again, 5), None, "a start with none running should stay");
}
