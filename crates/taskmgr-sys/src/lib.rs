//! Raw readers for the operating system.
//!
//! Windows is the real target: one `NtQuerySystemInformation(SystemProcessInformation)` call returns
//! every process with its CPU time, memory and I/O counters, so there is no `OpenProcess` per process.
//! PDH supplies the disk and GPU rates. The Linux `/proc` reader exists only so the app can be
//! developed and screenshotted off Windows.
//!
//! All `unsafe` in the project lives in this crate.

use std::io;

pub use taskmgr_core::RawSample;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as imp;

/// Reads one [`RawSample`] per call. Keep it alive between calls: it holds open OS handles
/// (PDH query, reusable buffers) so each call allocates as little as possible.
pub struct Collector {
    inner: imp::Collector,
}

impl Collector {
    pub fn new() -> io::Result<Self> {
        Ok(Self { inner: imp::Collector::new()? })
    }

    pub fn collect(&mut self) -> io::Result<RawSample> {
        self.inner.collect()
    }
}

/// Ends a process immediately (Windows `TerminateProcess`, Unix `SIGKILL`).
pub fn end_process(pid: u32) -> io::Result<()> {
    imp::end_process(pid)
}
