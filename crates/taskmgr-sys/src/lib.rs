//! Raw readers for the operating system.
//!
//! Windows is the real target: one `NtQuerySystemInformation(SystemProcessInformation)` call returns
//! every process with its CPU time, memory and I/O counters, so there is no `OpenProcess` per process
//! per tick. Path, command line and user are read once per new process and cached. PDH supplies the
//! disk and GPU rates. The Linux `/proc` reader exists only so the app can be developed and
//! screenshotted off Windows.
//!
//! All `unsafe` in the project lives in this crate.

use std::collections::HashMap;
use std::io;
use std::sync::Arc;

pub use taskmgr_core::RawSample;
use taskmgr_core::{CpuInfo, Extras, Service, Session, StartupItem, StaticInfo};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as imp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriorityClass {
    Idle,
    BelowNormal,
    Normal,
    AboveNormal,
    High,
    Realtime,
}

impl PriorityClass {
    pub const ALL: [PriorityClass; 6] =
        [Self::Realtime, Self::High, Self::AboveNormal, Self::Normal, Self::BelowNormal, Self::Idle];

    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Low",
            Self::BelowNormal => "Below normal",
            Self::Normal => "Normal",
            Self::AboveNormal => "Above normal",
            Self::High => "High",
            Self::Realtime => "Realtime",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
}

/// Reads one [`RawSample`] per call. Keep it alive between calls: it holds open OS handles
/// (PDH query, reusable buffers) and the per-process info cache.
pub struct Collector {
    inner: imp::Collector,
    info: HashMap<(u32, u64), Arc<StaticInfo>>,
}

/// What [`Collector::collect`] returns alongside the sample when [`Extras`] asked for it.
#[derive(Default)]
pub struct ExtraData {
    pub services: Option<Vec<Service>>,
    pub sessions: Option<Vec<Session>>,
}

impl Collector {
    pub fn new() -> io::Result<Self> {
        Ok(Self { inner: imp::Collector::new()?, info: HashMap::new() })
    }

    pub fn collect(&mut self, extras: Extras) -> io::Result<(RawSample, ExtraData)> {
        let mut sample = self.inner.collect()?;
        let mut seen = HashMap::with_capacity(sample.processes.len());
        for p in &mut sample.processes {
            let key = (p.pid, p.start_time);
            let info = self.info.remove(&key).unwrap_or_else(|| Arc::new(imp::static_info(p.pid)));
            p.info = info.clone();
            seen.insert(key, info);
        }
        // Exited processes drop out of the cache here.
        self.info = seen;
        let extra = ExtraData {
            services: extras.services.then(imp::list_services).and_then(Result::ok),
            sessions: extras.sessions.then(imp::list_sessions).and_then(Result::ok),
        };
        Ok((sample, extra))
    }
}

/// IP addresses, DNS suffix and Wi-Fi SSID of the adapter named `name` (its alias).
pub fn adapter_info(name: &str) -> taskmgr_core::AdapterInfo {
    imp::adapter_info(name)
}

/// Notification-area icon showing CPU usage (Windows only; no-ops elsewhere).
pub mod tray {
    /// Adds the icon for the window with this raw handle (HWND).
    pub fn install(hwnd: isize) -> bool {
        super::imp::tray::install(hwnd)
    }
    /// Redraws the icon for this CPU percentage, with a tooltip. Callable from any thread.
    pub fn update(cpu_pct: f32, tip: &str) {
        super::imp::tray::update(cpu_pct, tip);
    }
    /// Hides the main window; the icon brings it back.
    pub fn hide_window() {
        super::imp::tray::hide_window();
    }
}

/// Frames per second of the apps drawing with DirectX (Windows only; needs administrator).
pub mod fps {
    pub use taskmgr_core::fps::{FpsStatus, RawFps};

    /// Starts recording present calls the first time; afterwards returns how that went.
    pub fn start() -> FpsStatus {
        #[cfg(windows)]
        {
            super::windows::fps::start()
        }
        #[cfg(not(windows))]
        {
            if fake() { FpsStatus::Running } else { FpsStatus::Unavailable }
        }
    }

    /// Recent present times per process. Empty until [`start`] has returned `Running`.
    pub fn read() -> Vec<RawFps> {
        #[cfg(windows)]
        {
            super::windows::fps::read()
        }
        #[cfg(not(windows))]
        {
            if fake() { fake_frames() } else { Vec::new() }
        }
    }

    /// OXI_TASKMGR_FAKE_FPS=1 (off Windows): made-up frames for the parent process (about 144
    /// FPS with a stutter) and this one, so the page can be laid out and screenshotted.
    #[cfg(not(windows))]
    fn fake() -> bool {
        std::env::var_os("OXI_TASKMGR_FAKE_FPS").is_some()
    }

    #[cfg(not(windows))]
    fn fake_frames() -> Vec<RawFps> {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let frames = |fps: f64, wobble: f64| -> Vec<f64> {
            let mut t = now - taskmgr_core::fps::WINDOW_SECS;
            let mut out = Vec::new();
            let mut i = 0u32;
            while t < now {
                out.push(t);
                i += 1;
                let jitter = (f64::from(i) * 0.7 + now).sin() * wobble;
                t += (1.0 + jitter) / fps + if i.is_multiple_of(300) { 0.03 } else { 0.0 };
            }
            out
        };
        vec![
            RawFps {
                pid: std::os::unix::process::parent_id(),
                api: Default::default(),
                times: frames(144.0, 0.15),
                now,
            },
            RawFps { pid: std::process::id(), api: Default::default(), times: frames(2.0, 0.0), now },
        ]
    }
}

/// App history from Windows' resource usage database (SRUM). Needs administrator; takes seconds
/// (it reads a shadow copy), so call it off the UI thread.
/// A small icon as straight-alpha RGBA pixels, top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// The program icon shown next to a process (the shell's small icon of its exe). An empty
/// `path` gives the generic program icon. None off Windows. Slow (reads the file): call it
/// off the UI thread.
pub fn exe_icon(path: &str) -> Option<IconImage> {
    #[cfg(windows)]
    {
        imp::exe_icon(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

pub fn app_history() -> io::Result<taskmgr_core::AppHistory> {
    imp::app_history()
}

/// "Switch to": brings the process's main window to the front.
pub fn switch_to(pid: u32) -> io::Result<()> {
    imp::switch_to(pid)
}

/// Opens the file's Properties dialog.
pub fn file_properties(path: &str) -> io::Result<()> {
    imp::file_properties(path)
}

/// Model, capacity, SSD / HDD and bus of physical disk `number`.
pub fn disk_info(number: u32) -> io::Result<taskmgr_core::DiskInfo> {
    imp::disk_info(number)
}

/// Memory speed, slots and form factor from the firmware tables.
pub fn memory_info() -> taskmgr_core::MemoryInfo {
    imp::memory_info()
}

/// °C, where the drive reports it.
pub fn disk_temperature(number: u32) -> Option<f32> {
    imp::disk_temperature(number)
}

/// °C for the GPU with this LUID (as in [`taskmgr_core::GpuStats::luid`]), where the driver reports it.
pub fn gpu_temperature(luid: &str) -> Option<f32> {
    imp::gpu_temperature(luid)
}

/// Blocked threads of a process and what they wait on ("Analyze wait chain").
pub fn wait_chain(pid: u32) -> io::Result<Vec<taskmgr_core::WaitChain>> {
    imp::wait_chain(pid)
}

/// None when UAC virtualization is not allowed for the process.
pub fn uac_virtualization(pid: u32) -> Option<bool> {
    imp::uac_virtualization(pid)
}

pub fn set_uac_virtualization(pid: u32, on: bool) -> io::Result<()> {
    imp::set_uac_virtualization(pid, on)
}

/// Stops background work (the ETW network trace on Windows). Call on exit.
pub fn shutdown() {
    imp::shutdown();
}

/// Ends a process immediately (Windows `TerminateProcess`, Unix `SIGKILL`).
pub fn end_process(pid: u32) -> io::Result<()> {
    imp::end_process(pid)
}

pub fn set_priority(pid: u32, class: PriorityClass) -> io::Result<()> {
    imp::set_priority(pid, class)
}

/// Blocks until the service manager accepts the request; call it off the UI thread.
pub fn service_control(name: &str, action: ServiceAction) -> io::Result<()> {
    imp::service_control(name, action)
}

pub fn startup_items() -> Vec<StartupItem> {
    imp::startup_items()
}

pub fn set_startup_enabled(item: &StartupItem, enabled: bool) -> io::Result<()> {
    imp::set_startup_enabled(item, enabled)
}

/// Processor name, clocks, topology and caches; read once.
pub fn cpu_info() -> CpuInfo {
    imp::cpu_info()
}

pub fn disconnect_session(id: u32) -> io::Result<()> {
    imp::disconnect_session(id)
}

/// Signs the session's user out; their unsaved work is lost.
pub fn logoff_session(id: u32) -> io::Result<()> {
    imp::logoff_session(id)
}

pub fn last_bios_ms() -> Option<u32> {
    imp::last_bios_ms()
}

/// Opens the classic Services console.
pub fn open_services_console() -> io::Result<()> {
    imp::run_new_task("services.msc", false)
}

/// Program path of a startup command (quotes stripped, `%VARS%` expanded).
pub fn startup_exe(command: &str) -> String {
    #[cfg(windows)]
    {
        windows::startup_exe(command)
    }
    #[cfg(not(windows))]
    {
        split_command(command).0
    }
}

/// Who Ctrl+Shift+Esc / taskmgr.exe starts now, from the IFEO `Debugger` value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskmgrHandler {
    /// No replacement: Windows Task Manager.
    BuiltIn,
    /// This exe.
    ThisExe,
    /// Another copy of oxi-taskmgr, by path.
    OtherCopy(String),
    /// Some other tool (Process Explorer, System Informer), by its command.
    OtherTool(String),
}

/// Whether an IFEO `Debugger` value starts one of our exes (this copy or another).
pub fn is_oxidows_debugger(debugger: &str) -> bool {
    debugger.to_ascii_lowercase().contains("oxi-taskmgr")
}

/// Sorts a `Debugger` value (`None`: there is none) relative to `me`, this exe's path.
pub fn classify_debugger(debugger: Option<&str>, me: &str) -> TaskmgrHandler {
    let Some(d) = debugger.map(str::trim).filter(|d| !d.is_empty()) else { return TaskmgrHandler::BuiltIn };
    let path = d.trim_matches('"').to_string();
    if !me.is_empty() && d.to_lowercase().contains(&me.to_lowercase()) {
        TaskmgrHandler::ThisExe
    } else if is_oxidows_debugger(d) {
        TaskmgrHandler::OtherCopy(path)
    } else {
        TaskmgrHandler::OtherTool(path)
    }
}

pub fn taskmgr_handler() -> TaskmgrHandler {
    let me = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    classify_debugger(imp::taskmgr_replacement().as_deref(), &me)
}

/// True when Ctrl+Shift+Esc / taskmgr.exe currently opens this exe.
pub fn replaces_taskmgr() -> bool {
    taskmgr_handler() == TaskmgrHandler::ThisExe
}

/// Makes Ctrl+Shift+Esc open this exe (`true`) or gives it back (`false`): to Windows Task
/// Manager, or to the tool that had it before. Needs admin; see [`set_replaces_taskmgr_as_admin`].
pub fn set_replaces_taskmgr(on: bool) -> io::Result<()> {
    if on {
        let me = std::env::current_exe()?;
        imp::set_taskmgr_replacement(Some(&me.display().to_string()))
    } else {
        imp::set_taskmgr_replacement(None)
    }
}

/// [`set_replaces_taskmgr`], through the administrator prompt when this process is not
/// elevated: it runs this exe again as `--set-default` / `--restore-default --quiet` elevated
/// and waits for it. Blocks until the prompt is answered.
pub fn set_replaces_taskmgr_as_admin(on: bool) -> io::Result<()> {
    if imp::is_elevated() {
        return set_replaces_taskmgr(on);
    }
    let args = if on { "--set-default --quiet" } else { "--restore-default --quiet" };
    match imp::run_self_elevated(args)? {
        0 => Ok(()),
        code => Err(io::Error::other(format!("the elevated helper failed (exit code {code})"))),
    }
}

/// One Task Manager at a time: true when this process should go on, false when another
/// instance is running (its window was brought to the front; exit). `wait_for_previous` is for
/// a restart (`--restarted`), which waits for the instance that started it to exit.
pub fn single_instance(wait_for_previous: bool) -> bool {
    imp::single_instance(wait_for_previous)
}

pub fn is_elevated() -> bool {
    imp::is_elevated()
}

/// Relaunches the app through the administrator prompt; the caller should exit on `Ok`.
pub fn restart_elevated() -> io::Result<()> {
    imp::restart_elevated()
}

/// Starts this exe with `params` through the administrator prompt, without waiting for it.
pub fn start_elevated(params: &str) -> io::Result<()> {
    imp::start_elevated(params)
}

// ---- "Always run as administrator" ----

/// True when this exe's run-as-administrator task exists (Settings > Run as administrator).
pub fn admin_task_exists() -> bool {
    imp::admin_task_exists()
}

/// Registers or removes this exe's run-as-administrator task. Needs admin; see
/// [`set_admin_task_as_admin`].
pub fn set_admin_task(on: bool) -> io::Result<()> {
    imp::set_admin_task(on)
}

/// [`set_admin_task`], through the administrator prompt when this process is not elevated (it
/// runs this exe again as `--admin-task-on` / `--admin-task-off`, hidden, and waits for it).
pub fn set_admin_task_as_admin(on: bool) -> io::Result<()> {
    if imp::is_elevated() {
        return set_admin_task(on);
    }
    match imp::run_self_elevated(if on { "--admin-task-on" } else { "--admin-task-off" })? {
        0 => Ok(()),
        code => Err(io::Error::other(format!("the elevated helper failed (exit code {code})"))),
    }
}

/// Starts this exe elevated through its task, with no prompt. The new instance gets
/// `--from-task` and nothing else on its command line. An error: there is no such task.
pub fn run_admin_task() -> io::Result<()> {
    imp::run_admin_task()
}

/// The task's name: a folder of its own in Task Scheduler, and a hash of the exe's path, so a
/// moved or second copy never starts another copy's exe.
pub fn admin_task_name(exe: &str) -> String {
    // FNV-1a: stable across builds and Rust versions (unlike `DefaultHasher`).
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in exe.to_lowercase().bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
    }
    format!(r"oxidows\Task Manager as administrator {:08x}", h as u32)
}

/// The task definition (Task Scheduler XML): start `exe --from-task` as `user`, in their
/// session, with their full token. No triggers (it only runs on request), no time limit (the
/// default stops a task after 72 hours) and normal priority (the default, 7, is below normal).
pub fn admin_task_xml(exe: &str, user: &str) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let dir = exe.rsplit_once(['\\', '/']).map(|(d, _)| d).unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>oxidows</Author>
    <Description>Starts Task Manager (oxidows) as administrator without a UAC prompt. Made by its Settings page (Run as administrator); choosing another option there removes it.</Description>
  </RegistrationInfo>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>Parallel</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>"{exe}"</Command>
      <Arguments>--from-task</Arguments>
      <WorkingDirectory>{dir}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#,
        user = esc(user),
        exe = esc(exe),
        dir = esc(dir),
    )
}

pub fn affinity(pid: u32) -> io::Result<(u64, u64)> {
    imp::affinity(pid)
}

pub fn set_affinity(pid: u32, mask: u64) -> io::Result<()> {
    imp::set_affinity(pid, mask)
}

pub fn efficiency_mode(pid: u32) -> io::Result<bool> {
    imp::efficiency_mode(pid)
}

pub fn set_efficiency_mode(pid: u32, on: bool) -> io::Result<()> {
    imp::set_efficiency_mode(pid, on)
}

/// Writes a full memory dump; slow for large processes, so call it off the UI thread.
pub fn create_dump(pid: u32, path: &std::path::Path) -> io::Result<()> {
    imp::create_dump(pid, path)
}

/// "Run new task": a program, document, folder or URL, optionally through the UAC prompt.
pub fn run_new_task(command: &str, admin: bool) -> io::Result<()> {
    imp::run_new_task(command.trim(), admin)
}

/// True if a process with this image name (case-insensitive) is running.
pub fn is_running(name: &str) -> bool {
    imp::is_running(name)
}

pub fn search_online(term: &str) -> io::Result<()> {
    let q: String = term
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    imp::open_url(&format!("https://www.bing.com/search?q={q}"))
}

/// Opens a URL or system protocol URI (e.g. `ms-settings:otherusers`) using the default handler.
pub fn open_url(url: &str) -> io::Result<()> {
    imp::open_url(url)
}

/// `"C:\Program Files\x.exe" -a b` -> (`C:\Program Files\x.exe`, `-a b`).
pub(crate) fn split_command(cmd: &str) -> (String, String) {
    let cmd = cmd.trim();
    if let Some(rest) = cmd.strip_prefix('"')
        && let Some(end) = rest.find('"')
    {
        return (rest[..end].to_string(), rest[end + 1..].trim().to_string());
    }
    match cmd.split_once(' ') {
        Some((f, p)) => (f.to_string(), p.trim().to_string()),
        None => (cmd.to_string(), String::new()),
    }
}

/// Opens Explorer with the file selected.
pub fn open_file_location(path: &str) -> io::Result<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer").arg(format!("/select,{path}")).spawn().map(|_| ())
    }
    #[cfg(not(windows))]
    {
        let dir = std::path::Path::new(path).parent().map(|p| p.display().to_string()).unwrap_or_default();
        std::process::Command::new("xdg-open").arg(dir).spawn().map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::{TaskmgrHandler, admin_task_name, admin_task_xml, classify_debugger, split_command};

    #[test]
    fn admin_task_per_exe() {
        let a = admin_task_name(r"C:\Users\A\AppData\Local\Programs\oxidows\oxi-taskmgr.exe");
        assert!(a.starts_with(r"oxidows\Task Manager as administrator "), "{a}");
        // Windows paths are case-insensitive; another folder is another task.
        assert_eq!(a, admin_task_name(r"c:\users\a\appdata\local\programs\oxidows\OXI-TASKMGR.EXE"));
        assert_ne!(a, admin_task_name(r"D:\Tools\oxi-taskmgr.exe"));
        let x = admin_task_xml(r"D:\R&D <x>\oxi-taskmgr.exe", r"PC\Phuoc");
        assert!(x.contains(r#"<Command>"D:\R&amp;D &lt;x&gt;\oxi-taskmgr.exe"</Command>"#), "{x}");
        assert!(x.contains(r"<WorkingDirectory>D:\R&amp;D &lt;x&gt;</WorkingDirectory>"), "{x}");
        assert!(x.contains(r"<UserId>PC\Phuoc</UserId>") && x.contains("<RunLevel>HighestAvailable</RunLevel>"));
    }

    #[test]
    fn classifies_debugger() {
        let me = r"C:\Tools\oxi-taskmgr.exe";
        assert_eq!(classify_debugger(None, me), TaskmgrHandler::BuiltIn);
        assert_eq!(classify_debugger(Some("  "), me), TaskmgrHandler::BuiltIn);
        assert_eq!(classify_debugger(Some(r#""c:\tools\OXI-TASKMGR.exe""#), me), TaskmgrHandler::ThisExe);
        assert_eq!(
            classify_debugger(Some(r#""D:\old\oxi-taskmgr.exe""#), me),
            TaskmgrHandler::OtherCopy(r"D:\old\oxi-taskmgr.exe".into())
        );
        assert_eq!(
            classify_debugger(Some(r#""C:\Sys\procexp64.exe""#), me),
            TaskmgrHandler::OtherTool(r"C:\Sys\procexp64.exe".into())
        );
    }

    #[test]
    fn splits_commands() {
        assert_eq!(split_command("notepad"), ("notepad".into(), String::new()));
        assert_eq!(split_command("cmd /k echo hi"), ("cmd".into(), "/k echo hi".into()));
        assert_eq!(
            split_command(r#""C:\Program Files\a b.exe" --x"#),
            (r"C:\Program Files\a b.exe".into(), "--x".into())
        );
    }
}
