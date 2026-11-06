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
use taskmgr_core::{Extras, Service, Session, StartupItem, StaticInfo};

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

pub fn is_elevated() -> bool {
    imp::is_elevated()
}

/// Relaunches the app through the administrator prompt; the caller should exit on `Ok`.
pub fn restart_elevated() -> io::Result<()> {
    imp::restart_elevated()
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
