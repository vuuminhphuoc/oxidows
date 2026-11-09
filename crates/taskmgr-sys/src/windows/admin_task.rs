//! "Always run as administrator" without a UAC prompt each time: a scheduled task that starts
//! this exe with the user's full (elevated) token. Making or removing it needs admin once; running
//! it does not. The task is named after this exe's path (see [`crate::admin_task_name`]), so a
//! moved or second copy never starts another one's exe.

use std::io;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

/// No console window flashing up for schtasks.exe.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn schtasks(args: &[&str]) -> io::Result<std::process::Output> {
    Command::new("schtasks.exe").args(args).stdin(Stdio::null()).creation_flags(CREATE_NO_WINDOW).output()
}

fn name() -> io::Result<String> {
    Ok(crate::admin_task_name(&std::env::current_exe()?.display().to_string()))
}

/// The tool's own words on failure (stderr, else stdout), for the status line.
fn failure(out: &std::process::Output) -> io::Error {
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
    let mut msg = text(&out.stderr);
    if msg.is_empty() {
        msg = text(&out.stdout);
    }
    if msg.is_empty() {
        msg = format!("schtasks exited with {}", out.status);
    }
    io::Error::other(msg)
}

/// True when this exe's task is registered.
pub fn admin_task_exists() -> bool {
    name().and_then(|n| schtasks(&["/query", "/tn", &n])).map(|o| o.status.success()).unwrap_or(false)
}

/// Registers (`true`) or removes (`false`) this exe's task. Needs an elevated process.
pub fn set_admin_task(on: bool) -> io::Result<()> {
    let name = name()?;
    if !on {
        if !admin_task_exists() {
            return Ok(());
        }
        let out = schtasks(&["/delete", "/tn", &name, "/f"])?;
        return if out.status.success() { Ok(()) } else { Err(failure(&out)) };
    }
    let exe = std::env::current_exe()?;
    let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(d), Ok(u)) if !d.is_empty() => format!("{d}\\{u}"),
        (_, Ok(u)) => u,
        _ => return Err(io::Error::other("unknown user name")),
    };
    let xml = crate::admin_task_xml(&exe.display().to_string(), &user);
    // schtasks reads the definition from a file; UTF-16 with a byte-order mark, as it expects.
    let path = std::env::temp_dir().join(format!("oxidows-taskmgr-task-{}.xml", std::process::id()));
    let bytes: Vec<u8> = [0xFEFFu16].into_iter().chain(xml.encode_utf16()).flat_map(u16::to_le_bytes).collect();
    std::fs::write(&path, bytes)?;
    let out = schtasks(&["/create", "/tn", &name, "/xml", &path.display().to_string(), "/f"]);
    let _ = std::fs::remove_file(&path);
    let out = out?;
    if out.status.success() { Ok(()) } else { Err(failure(&out)) }
}

/// Starts this exe elevated through its task. An error means there is no task (or it would not
/// start); the caller then falls back to the UAC prompt.
pub fn run_admin_task() -> io::Result<()> {
    // The task's process is started by the Task Scheduler, not by us, so it would not be allowed
    // to bring its window to the front. We were started by the user (Ctrl+Shift+Esc), so we may
    // pass that right on.
    // SAFETY: plain call, no pointers.
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    let out = schtasks(&["/run", "/tn", &name()?])?;
    if out.status.success() { Ok(()) } else { Err(failure(&out)) }
}
