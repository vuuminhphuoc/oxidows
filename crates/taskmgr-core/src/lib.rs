#![forbid(unsafe_code)]

//! Platform-independent part of the Task Manager.
//!
//! `taskmgr-sys` produces [`RawSample`]s (cumulative counters read straight from the OS).
//! [`Sampler`] turns two consecutive raw samples into a [`Snapshot`] of rates (CPU %, bytes/s)
//! and keeps a [`History`] of the last [`HISTORY_LEN`] system-wide points for the graphs.
//! Nothing here touches the OS, so it is unit-tested on any platform.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub mod fps;
pub use fps::{Fps, FpsApi, FpsApp, FpsStatus, FpsTracker, RawFps};

/// Points kept for every Performance graph (one per sample: 60 s at the default 1 s interval).
pub const HISTORY_LEN: usize = 60;

/// Cumulative CPU time of one logical processor (or of all of them), in any fixed unit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub idle: u64,
    /// Idle + busy time.
    pub total: u64,
    /// Busy time spent in kernel mode (excludes idle).
    pub kernel: u64,
}

impl CpuTimes {
    /// Busy share in percent between `prev` and `self`.
    pub fn usage_since(&self, prev: &CpuTimes) -> f32 {
        let total = self.total.saturating_sub(prev.total);
        if total == 0 {
            return 0.0;
        }
        let idle = self.idle.saturating_sub(prev.idle).min(total);
        ((total - idle) as f64 * 100.0 / total as f64) as f32
    }

    /// Kernel-mode share in percent between `prev` and `self`.
    pub fn kernel_since(&self, prev: &CpuTimes) -> f32 {
        let total = self.total.saturating_sub(prev.total);
        if total == 0 {
            return 0.0;
        }
        (self.kernel.saturating_sub(prev.kernel).min(total) as f64 * 100.0 / total as f64) as f32
    }
}

/// Extra per-process counters for the Details columns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcCounters {
    pub working_set: u64,
    pub peak_working_set: u64,
    /// Commit size (private bytes).
    pub commit: u64,
    pub paged_pool: u64,
    pub nonpaged_pool: u64,
    pub page_faults: u32,
    pub hard_faults: u32,
    pub read_ops: u64,
    pub write_ops: u64,
    pub other_ops: u64,
    pub other_bytes: u64,
}

/// One process as the OS reports it: cumulative counters, no rates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawProcess {
    pub pid: u32,
    pub parent_pid: u32,
    /// Distinguishes a reused PID (Windows FILETIME, or start ticks on Linux).
    pub start_time: u64,
    pub name: String,
    /// User + kernel time in 100 ns units.
    pub cpu_time: u64,
    /// Private working set in bytes (what Windows Task Manager shows as "Memory").
    pub memory: u64,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
    /// Network bytes sent + received since monitoring started (0 when unavailable).
    pub net_bytes: u64,
    /// Every thread is suspended.
    pub suspended: bool,
    /// Efficiency mode (EcoQoS) is on.
    pub efficiency: bool,
    pub counters: ProcCounters,
    pub threads: u32,
    pub handles: u32,
    pub session_id: u32,
    pub has_window: bool,
    /// Base priority as the kernel reports it (4 idle, 8 normal, 13 high, 24 realtime...).
    pub base_priority: i32,
    /// Path, command line and user: read once per process, shared between samples.
    pub info: Arc<StaticInfo>,
}

/// Process facts that never change while it runs. Empty strings when access is denied.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StaticInfo {
    pub path: String,
    pub command_line: String,
    /// `DOMAIN\user` on Windows, the user name or uid on Linux.
    pub user: String,
    /// Version-resource FileDescription ("Google Chrome"); the name shown for apps.
    pub description: String,
    /// Version-resource CompanyName.
    pub publisher: String,
    /// Runs with an elevated (administrator) token.
    pub elevated: Option<bool>,
    /// "Not allowed", "Disabled" or "Enabled".
    pub uac_virtualization: String,
    /// "64-bit", "32-bit" or "ARM64".
    pub platform: String,
    /// GDI object count from GetGuiResources.
    pub gdi_objects: Option<u32>,
    /// USER object count from GetGuiResources.
    pub user_objects: Option<u32>,
    /// Architecture from IsWow64Process2: "x64", "x86", "ARM64".
    pub architecture: String,
    /// Power throttling from GetProcessInformation: "Enabled", "Disabled".
    pub power_throttling: String,
    /// Package full name from GetPackageFullName, empty when not packaged.
    pub package_name: String,
}

/// System-wide counters for one sample.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawSystem {
    /// Per-process network bytes are being measured (needs administrator on Windows).
    pub per_process_net: bool,
    pub cpu: CpuTimes,
    pub cores: Vec<CpuTimes>,
    pub mem_total: u64,
    pub mem_available: u64,
    pub commit_used: u64,
    pub commit_limit: u64,
    /// Cumulative bytes over all physical network adapters.
    pub net_rx_bytes: u64,
    pub net_tx_bytes: u64,
    /// Already a rate when the OS gives one (PDH); `None` where not available.
    pub disk_active_pct: Option<f32>,
    pub disk_read_bps: Option<f64>,
    pub disk_write_bps: Option<f64>,
    pub gpu_pct: Option<f32>,
    /// GPU % per process (busiest engine), from the same PDH read.
    pub gpu_per_pid: Vec<(u32, f32)>,
    /// Busiest engine type per process.
    pub gpu_engine_per_pid: Vec<(u32, String)>,
    /// Utilization per engine type ("3D", "Copy", "VideoDecode"...), summed over processes.
    pub gpu_engines: Vec<(String, f32)>,
    pub gpu_dedicated_used: Option<u64>,
    pub gpu_shared_used: Option<u64>,
    /// One entry per adapter with GPU engine counters.
    pub gpus: Vec<GpuStats>,
    pub uptime: Duration,
    /// Current clock in MHz, when the OS reports it.
    pub cpu_mhz: Option<f32>,
    pub mem_cached: Option<u64>,
    pub mem_paged_pool: Option<u64>,
    pub mem_nonpaged_pool: Option<u64>,
    /// Installed RAM minus what the OS can use (firmware, integrated GPU).
    pub mem_hw_reserved: Option<u64>,
    /// One per physical disk; PDH already gives rates.
    pub disks: Vec<DiskRates>,
    /// One per physical network adapter, cumulative bytes.
    pub adapters: Vec<RawAdapter>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiskRates {
    /// e.g. "0 C:".
    pub name: String,
    pub active_pct: f32,
    pub read_bps: f64,
    pub write_bps: f64,
    /// Average response time in milliseconds.
    pub response_ms: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawAdapter {
    pub name: String,
    pub description: String,
    /// "Ethernet", "Wi-Fi", "Cellular" or "Network".
    pub kind: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    /// Link speed in bits per second, 0 when unknown.
    pub link_bps: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AdapterRates {
    pub name: String,
    pub description: String,
    pub kind: String,
    pub rx_bps: f64,
    pub tx_bps: f64,
    pub link_bps: u64,
}

/// Facts about one physical disk, read on demand.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiskInfo {
    pub model: String,
    pub capacity: u64,
    /// "SSD" or "HDD", empty when unknown.
    pub kind: String,
    /// "NVMe", "SATA", "USB"...
    pub bus: String,
}

/// Memory modules from the firmware (SMBIOS).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryInfo {
    /// MT/s; 0 when unknown.
    pub speed_mts: u32,
    pub slots_used: u32,
    pub slots_total: u32,
    pub form_factor: String,
}

/// One app's totals in App history.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppUsage {
    pub name: String,
    pub cpu_cycles: u64,
    /// CPU time from cycles at the base clock, in 100 ns units.
    pub cpu_time_100ns: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    /// Sent and received over connections Windows treats as metered.
    pub metered_bytes: u64,
    /// True for packaged (UWP / MSIX / Store) apps, false for classic processes.
    pub packaged: bool,
}

/// App history as Windows records it (SRUM), from the oldest record kept.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppHistory {
    /// Oldest record, as an OLE automation date (days since 1899-12-30).
    pub since_ole_date: Option<f64>,
    pub apps: Vec<AppUsage>,
}

/// An OLE automation date as `YYYY-MM-DD`.
pub fn format_ole_date(d: f64) -> String {
    // Days since 1970-01-01, then the civil-from-days algorithm (Howard Hinnant).
    let z = (d.floor() as i64) - 25569 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// A FILETIME (100 ns intervals since 1601-01-01 UTC) formatted as `YYYY-MM-DD HH:MM`.
pub fn format_filetime(ft: u64) -> String {
    const UNIX_EPOCH_FILETIME: u64 = 116_444_736_000_000_000;
    if ft < UNIX_EPOCH_FILETIME {
        return String::new();
    }
    let unix_100ns = ft - UNIX_EPOCH_FILETIME;
    let unix_secs = unix_100ns / 10_000_000;
    let days = (unix_secs / 86_400) as i64;
    let day_secs = unix_secs % 86_400;
    let hour = day_secs / 3600;
    let min = (day_secs % 3600) / 60;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}")
}

/// Addresses and network name of one adapter, read on demand.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdapterInfo {
    pub ipv4: Vec<String>,
    pub ipv6: Vec<String>,
    pub dns_suffix: String,
    /// Wi-Fi network name, for wireless adapters.
    pub ssid: Option<String>,
}

/// Facts about the processor that do not change while running.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CpuInfo {
    pub name: String,
    pub base_mhz: u32,
    pub sockets: u32,
    pub cores: u32,
    pub logical: u32,
    pub l1_bytes: u64,
    pub l2_bytes: u64,
    pub l3_bytes: u64,
    pub virtualization: Option<bool>,
}

/// From `GPU Engine(*)` instances like `pid_1234_luid_0x0_0x1_phys_0_eng_0_engtype_3D`:
/// the system figure is the busiest engine type summed over processes (what Task Manager shows),
/// and each process gets its own busiest engine type.
/// Result of [`gpu_from_engines`].
pub struct GpuUsage {
    /// Busiest engine type, summed over processes.
    pub total: f32,
    pub per_pid: Vec<(u32, f32)>,
    pub engines: Vec<(String, f32)>,
    /// Busiest engine type per process (only processes using the GPU).
    pub engine_per_pid: Vec<(u32, String)>,
}

/// A display adapter as DXGI reports it. `luid` is formatted as in PDH instance names:
/// `0xHIGHPART_0xLOWPART`, eight hex digits each.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GpuAdapter {
    pub luid: String,
    pub name: String,
    pub dedicated_total: u64,
    pub shared_total: u64,
    /// "31.0.101.4502"; empty when unknown.
    pub driver_version: String,
    /// "12 (FL 12.1)"; empty when the adapter has no Direct3D 12 support.
    pub directx: String,
    /// As the driver package states it, "9/14/2023".
    pub driver_date: String,
    /// "PCI bus 0, device 2, function 0"; empty for software adapters.
    pub location: String,
    /// A neural processor (machine learning only, no graphics), shown as "NPU n".
    pub npu: bool,
}

/// One GPU's figures for a sample.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuStats {
    pub name: String,
    /// "0xHIGH_0xLOW", as in PDH instance names.
    pub luid: String,
    /// Busiest engine type, percent.
    pub util: f32,
    pub engines: Vec<(String, f32)>,
    pub dedicated_used: Option<u64>,
    pub shared_used: Option<u64>,
    pub dedicated_total: u64,
    pub shared_total: u64,
    pub driver_version: String,
    pub directx: String,
    pub driver_date: String,
    pub location: String,
    pub npu: bool,
}

/// "GPU 0", "GPU 1", "NPU 0"...: GPUs and NPUs are numbered separately, as Task Manager does.
pub fn gpu_titles(gpus: &[GpuStats]) -> Vec<String> {
    let (mut g, mut n) = (0, 0);
    gpus.iter()
        .map(|x| {
            let (kind, i) = if x.npu { ("NPU", &mut n) } else { ("GPU", &mut g) };
            *i += 1;
            format!("{kind} {}", *i - 1)
        })
        .collect()
}

fn luid_of(instance: &str) -> Option<&str> {
    let rest = &instance[instance.find("luid_")? + 5..];
    rest.get(..21)
}

/// Splits PDH GPU counters by adapter. Adapters without engine counters (such as the
/// software "Basic Render Driver") are left out; unknown LUIDs get a numbered name.
pub fn split_gpus(
    adapters: &[GpuAdapter],
    engines: &[(String, f64)],
    dedicated: &[(String, f64)],
    shared: &[(String, f64)],
) -> Vec<GpuStats> {
    let mut luids: Vec<&str> = Vec::new();
    for (n, _) in engines {
        if let Some(l) = luid_of(n)
            && !luids.contains(&l)
        {
            luids.push(l);
        }
    }
    let rank = |l: &str| adapters.iter().position(|a| a.luid.eq_ignore_ascii_case(l)).unwrap_or(usize::MAX);
    luids.sort_by_key(|l| (rank(l), l.to_string()));
    let sum = |items: &[(String, f64)], l: &str| {
        let v: Vec<f64> = items.iter().filter(|(n, _)| luid_of(n) == Some(l)).map(|(_, b)| *b).collect();
        (!v.is_empty()).then(|| v.iter().sum::<f64>() as u64)
    };
    luids
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let mine: Vec<(String, f64)> = engines.iter().filter(|(n, _)| luid_of(n) == Some(*l)).cloned().collect();
            let usage = gpu_from_engines(&mine);
            let a = adapters.iter().find(|a| a.luid.eq_ignore_ascii_case(l));
            GpuStats {
                name: a.map_or_else(|| format!("GPU {i}"), |a| a.name.clone()),
                luid: l.to_string(),
                util: usage.total,
                engines: usage.engines,
                dedicated_used: sum(dedicated, l),
                shared_used: sum(shared, l),
                dedicated_total: a.map_or(0, |a| a.dedicated_total),
                shared_total: a.map_or(0, |a| a.shared_total),
                driver_version: a.map(|a| a.driver_version.clone()).unwrap_or_default(),
                directx: a.map(|a| a.directx.clone()).unwrap_or_default(),
                driver_date: a.map(|a| a.driver_date.clone()).unwrap_or_default(),
                location: a.map(|a| a.location.clone()).unwrap_or_default(),
                npu: a.is_some_and(|a| a.npu),
            }
        })
        .collect()
}

pub fn gpu_from_engines(items: &[(String, f64)]) -> GpuUsage {
    let mut by_type: HashMap<&str, f64> = HashMap::new();
    let mut by_pid_type: HashMap<(u32, &str), f64> = HashMap::new();
    for (name, v) in items {
        let pid = name.strip_prefix("pid_").and_then(|r| r.split('_').next()).and_then(|p| p.parse::<u32>().ok());
        let ty = name.rsplit_once("engtype_").map_or("", |(_, t)| t);
        *by_type.entry(ty).or_default() += v;
        if let Some(pid) = pid {
            *by_pid_type.entry((pid, ty)).or_default() += v;
        }
    }
    let total = by_type.values().copied().fold(0.0, f64::max).min(100.0) as f32;
    let mut per_pid: HashMap<u32, f64> = HashMap::new();
    let mut engine_of: HashMap<u32, (&str, f64)> = HashMap::new();
    for ((pid, ty), v) in by_pid_type {
        let e = per_pid.entry(pid).or_default();
        *e = e.max(v);
        if v > 0.0 {
            let best = engine_of.entry(pid).or_insert((ty, v));
            if v > best.1 {
                *best = (ty, v);
            }
        }
    }
    let mut engine_per_pid: Vec<(u32, String)> = engine_of.into_iter().map(|(p, (t, _))| (p, t.to_string())).collect();
    engine_per_pid.sort_by_key(|(p, _)| *p);
    let mut v: Vec<(u32, f32)> = per_pid.into_iter().map(|(p, g)| (p, g.min(100.0) as f32)).collect();
    v.sort_by_key(|(p, _)| *p);
    let mut engines: Vec<(String, f32)> =
        by_type.into_iter().filter(|(t, _)| !t.is_empty()).map(|(t, u)| (t.to_string(), u.min(100.0) as f32)).collect();
    engines.sort_by(|a, b| a.0.cmp(&b.0));
    GpuUsage { total, per_pid: v, engines, engine_per_pid }
}

#[derive(Clone, Debug)]
pub struct RawSample {
    pub at: Instant,
    pub processes: Vec<RawProcess>,
    pub system: RawSystem,
}

/// A process with its rates, ready to show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcessRow {
    pub pid: u32,
    pub parent_pid: u32,
    pub start_time: u64,
    pub name: String,
    pub cpu_pct: f32,
    pub memory: u64,
    /// Read + write bytes per second.
    pub disk_bps: f64,
    /// Sent + received bytes per second.
    pub net_bps: f64,
    pub suspended: bool,
    pub efficiency: bool,
    pub counters: ProcCounters,
    /// Busiest GPU engine type ("3D", "VideoDecode"...), empty when idle.
    pub gpu_engine: String,
    /// Total CPU time (user + kernel) in 100 ns units.
    pub cpu_time: u64,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
    pub threads: u32,
    pub handles: u32,
    pub session_id: u32,
    pub base_priority: i32,
    pub info: Arc<StaticInfo>,
    pub gpu_pct: f32,
    /// Owns a visible top-level window (an "App" in the Processes tab).
    pub has_window: bool,
    /// Estimated power draw now, see [`power_score`].
    pub power: f32,
    /// The same averaged over about two minutes ("Power usage trend").
    pub power_trend: f32,
}

/// An estimate of a process's power draw from what it uses, on the scale [`power_label`] reads.
/// Windows' own column is also an estimate (its Energy Estimation Engine weighs the same four).
pub fn power_score(cpu_pct: f32, gpu_pct: f32, disk_bps: f64, net_bps: f64) -> f32 {
    cpu_pct + gpu_pct * 0.5 + (disk_bps / 1e6) as f32 * 0.4 + (net_bps * 8.0 / 1e6) as f32 * 0.05
}

/// Startup impact by Microsoft's published thresholds: High above 1 s of CPU or 3 MB of disk I/O,
/// Medium above 300 ms or 300 KB, Low below both.
pub fn startup_impact(cpu_100ns: u64, io_bytes: u64) -> &'static str {
    if cpu_100ns > 10_000_000 || io_bytes > 3_000_000 {
        "High"
    } else if cpu_100ns > 3_000_000 || io_bytes > 300_000 {
        "Medium"
    } else {
        "Low"
    }
}

/// Task Manager's five power levels.
pub fn power_label(score: f32) -> &'static str {
    match score {
        s if s < 0.5 => "Very low",
        s if s < 3.0 => "Low",
        s if s < 10.0 => "Moderate",
        s if s < 25.0 => "High",
        _ => "Very high",
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemRates {
    pub per_process_net: bool,
    pub cpu_pct: f32,
    pub cores_pct: Vec<f32>,
    /// Kernel-mode share of all CPU time, and per logical processor.
    pub kernel_pct: f32,
    pub cores_kernel_pct: Vec<f32>,
    pub mem_used: u64,
    pub mem_total: u64,
    pub commit_used: u64,
    pub commit_limit: u64,
    pub net_rx_bps: f64,
    pub net_tx_bps: f64,
    pub disk_active_pct: Option<f32>,
    pub disk_read_bps: f64,
    pub disk_write_bps: f64,
    pub gpu_pct: Option<f32>,
    pub gpu_engines: Vec<(String, f32)>,
    pub gpu_dedicated_used: Option<u64>,
    pub gpu_shared_used: Option<u64>,
    pub gpus: Vec<GpuStats>,
    pub uptime: Duration,
    pub cpu_mhz: Option<f32>,
    pub mem_cached: Option<u64>,
    pub mem_paged_pool: Option<u64>,
    pub mem_nonpaged_pool: Option<u64>,
    pub mem_hw_reserved: Option<u64>,
    pub disks: Vec<DiskRates>,
    pub adapters: Vec<AdapterRates>,
    pub process_count: usize,
    pub thread_count: u64,
    pub handle_count: u64,
}

/// The machine-wide figures Task Manager prints above its CPU, Memory, Disk, Network and GPU
/// columns, in percent.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ColumnTotals {
    pub cpu: f32,
    pub memory: f32,
    pub disk: f32,
    pub network: f32,
    pub gpu: f32,
}

impl ColumnTotals {
    pub fn of(s: &SystemRates) -> Self {
        let link: f64 = s.adapters.iter().map(|a| a.link_bps as f64).sum();
        let moved: f64 = s.adapters.iter().map(|a| a.rx_bps + a.tx_bps).sum();
        let pct = |v: f64| (v as f32).clamp(0.0, 100.0);
        Self {
            cpu: pct(f64::from(s.cpu_pct)),
            memory: if s.mem_total == 0 { 0.0 } else { pct(s.mem_used as f64 * 100.0 / s.mem_total as f64) },
            disk: pct(f64::from(s.disk_active_pct.unwrap_or(0.0))),
            network: if link <= 0.0 { 0.0 } else { pct(moved * 8.0 * 100.0 / link) },
            gpu: pct(f64::from(s.gpu_pct.unwrap_or(0.0))),
        }
    }

    /// "37%", as the header shows it.
    pub fn label(v: f32) -> String {
        format!("{:.0}%", v)
    }
}

/// Everything the UI needs for one frame. Sent from the sampler thread as an `Arc`.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub processes: Vec<ProcessRow>,
    pub system: SystemRates,
    pub history: History,
    /// Filled only while a tab that needs it is open (see [`Extras`]).
    pub services: Option<Arc<Vec<Service>>>,
    pub sessions: Option<Arc<Vec<Session>>>,
    /// Frames per second of the apps drawing with DirectX (filled by the app's sampler thread).
    pub fps: Arc<Fps>,
}

/// Which optional lists the sampler should read this tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Extras {
    pub services: bool,
    pub sessions: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    Stopped,
    StartPending,
    StopPending,
    Running,
    ContinuePending,
    PausePending,
    Paused,
    Unknown,
}

impl ServiceState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Stopped => "Stopped",
            Self::StartPending => "Starting",
            Self::StopPending => "Stopping",
            Self::Running => "Running",
            Self::ContinuePending => "Continuing",
            Self::PausePending => "Pausing",
            Self::Paused => "Paused",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub display_name: String,
    pub state: ServiceState,
    /// 0 when not running.
    pub pid: u32,
    /// The svchost group (`-k` argument of the image path), empty for services with their own exe.
    pub group: String,
}

/// The svchost group in a service image path: `svchost.exe -k LocalServiceNetworkRestricted -p` → the name.
pub fn service_group(image_path: &str) -> String {
    let mut it = image_path.split_whitespace();
    while let Some(t) = it.next() {
        if t.eq_ignore_ascii_case("-k") {
            return it.next().unwrap_or_default().trim_matches('"').to_string();
        }
    }
    String::new()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub id: u32,
    pub user: String,
    /// "Active", "Disconnected"...
    pub state: String,
    /// WTSWinStationName ("Console", "RDP-Tcp#0"...).
    pub session_name: String,
    /// WTSClientName (client machine for remote desktop sessions).
    pub client_name: String,
}

/// A blocked thread found by "Analyze wait chain".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WaitChain {
    pub thread: u32,
    /// What the thread waits on, in order ("mutex X", "thread 12 of process 34").
    pub steps: Vec<String>,
    /// Other processes along the chain; ending one of them releases the thread.
    pub blocking: Vec<u32>,
    /// The chain loops back: a deadlock.
    pub cycle: bool,
}

/// One entry of the Startup apps tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupItem {
    pub name: String,
    pub command: String,
    /// Where it is registered, e.g. `HKCU\...\Run` or the Startup folder path.
    pub location: String,
    pub enabled: bool,
    /// False when changing it needs administrator rights (machine-wide entries).
    pub per_user: bool,
    pub publisher: String,
    /// "Registry", "Folder", or "Task".
    pub startup_type: String,
    /// FILETIME when disabled, read from StartupApproved.
    pub disabled_time: Option<u64>,
}

/// Users tab row: processes grouped by session.
#[derive(Clone, Debug, PartialEq)]
pub struct UserUsage {
    pub session_id: u32,
    pub user: String,
    pub state: String,
    pub processes: usize,
    pub cpu_pct: f32,
    pub memory: u64,
    pub session_name: String,
    pub client_name: String,
}

/// Groups processes by session; sessions with no processes still appear.
pub fn users_view(rows: &[ProcessRow], sessions: &[Session]) -> Vec<UserUsage> {
    let mut out: Vec<UserUsage> = sessions
        .iter()
        .map(|s| UserUsage {
            session_id: s.id,
            user: s.user.clone(),
            state: s.state.clone(),
            processes: 0,
            cpu_pct: 0.0,
            memory: 0,
            session_name: s.session_name.clone(),
            client_name: s.client_name.clone(),
        })
        .collect();
    for p in rows {
        let i = match out.iter().position(|u| u.session_id == p.session_id) {
            Some(i) => i,
            None => {
                out.push(UserUsage {
                    session_id: p.session_id,
                    user: String::new(),
                    state: String::new(),
                    processes: 0,
                    cpu_pct: 0.0,
                    memory: 0,
                    session_name: String::new(),
                    client_name: String::new(),
                });
                out.len() - 1
            }
        };
        let u = &mut out[i];
        u.processes += 1;
        u.cpu_pct += p.cpu_pct;
        u.memory += p.memory;
        // A session without a WTS user name (e.g. on Linux) takes the user of its processes.
        if u.user.is_empty() && !p.info.user.is_empty() {
            u.user = p.info.user.clone();
        }
    }
    out.sort_by_key(|u| u.session_id);
    out
}

/// Windows priority class name from a base priority.
pub fn priority_label(base: i32) -> &'static str {
    match base {
        i32::MIN..=5 => "Low",
        6..=7 => "Below normal",
        8 => "Normal",
        9..=12 => "Above normal",
        13..=23 => "High",
        _ => "Realtime",
    }
}

/// Fixed-size ring of the most recent values.
#[derive(Clone, Debug, PartialEq)]
pub struct Ring {
    buf: Vec<f32>,
    head: usize,
    len: usize,
}

impl Default for Ring {
    fn default() -> Self {
        Self::new(HISTORY_LEN)
    }
}

impl Ring {
    pub fn new(cap: usize) -> Self {
        Self { buf: vec![0.0; cap.max(1)], head: 0, len: 0 }
    }

    pub fn push(&mut self, v: f32) {
        self.buf[self.head] = v;
        self.head = (self.head + 1) % self.buf.len();
        self.len = (self.len + 1).min(self.buf.len());
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    pub fn last(&self) -> Option<f32> {
        (self.len > 0).then(|| self.buf[(self.head + self.buf.len() - 1) % self.buf.len()])
    }

    /// Oldest to newest.
    pub fn iter(&self) -> impl Iterator<Item = f32> + '_ {
        let cap = self.buf.len();
        let start = (self.head + cap - self.len) % cap;
        (0..self.len).map(move |i| self.buf[(start + i) % cap])
    }

    pub fn max(&self) -> f32 {
        self.iter().fold(0.0, f32::max)
    }
}

/// System-wide history for the Performance tab.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    pub cpu: Ring,
    pub cores: Vec<Ring>,
    pub cpu_kernel: Ring,
    pub cores_kernel: Vec<Ring>,
    /// Memory in use, percent of total.
    pub mem_pct: Ring,
    pub disk_active_pct: Ring,
    /// Bytes per second.
    pub disk_bps: Ring,
    pub net_rx_bps: Ring,
    pub net_tx_bps: Ring,
    pub gpu_pct: Ring,
    /// Per physical disk: active time %.
    pub disks: Vec<Ring>,
    /// Per physical disk: (read, write) bytes per second.
    pub disk_io: Vec<(Ring, Ring)>,
    /// Per adapter: (receive, send) bytes per second.
    pub adapters: Vec<(Ring, Ring)>,
    /// Per GPU: utilization %.
    pub gpus: Vec<Ring>,
}

impl History {
    fn push(&mut self, s: &SystemRates) {
        self.cpu.push(s.cpu_pct);
        self.cpu_kernel.push(s.kernel_pct);
        if self.cores_kernel.len() != s.cores_kernel_pct.len() {
            self.cores_kernel = vec![Ring::default(); s.cores_kernel_pct.len()];
        }
        for (r, v) in self.cores_kernel.iter_mut().zip(&s.cores_kernel_pct) {
            r.push(*v);
        }
        if self.cores.len() != s.cores_pct.len() {
            self.cores = vec![Ring::default(); s.cores_pct.len()];
        }
        for (ring, v) in self.cores.iter_mut().zip(&s.cores_pct) {
            ring.push(*v);
        }
        let mem_pct = if s.mem_total == 0 { 0.0 } else { (s.mem_used as f64 * 100.0 / s.mem_total as f64) as f32 };
        self.mem_pct.push(mem_pct);
        self.disk_active_pct.push(s.disk_active_pct.unwrap_or(0.0));
        self.disk_bps.push((s.disk_read_bps + s.disk_write_bps) as f32);
        self.net_rx_bps.push(s.net_rx_bps as f32);
        self.net_tx_bps.push(s.net_tx_bps as f32);
        self.gpu_pct.push(s.gpu_pct.unwrap_or(0.0));
        if self.gpus.len() != s.gpus.len() {
            self.gpus = vec![Ring::default(); s.gpus.len()];
        }
        for (ring, g) in self.gpus.iter_mut().zip(&s.gpus) {
            ring.push(g.util);
        }
        if self.disks.len() != s.disks.len() {
            self.disks = vec![Ring::default(); s.disks.len()];
        }
        for (ring, d) in self.disks.iter_mut().zip(&s.disks) {
            ring.push(d.active_pct);
        }
        if self.disk_io.len() != s.disks.len() {
            self.disk_io = vec![(Ring::default(), Ring::default()); s.disks.len()];
        }
        for ((r, w), d) in self.disk_io.iter_mut().zip(&s.disks) {
            r.push(d.read_bps as f32);
            w.push(d.write_bps as f32);
        }
        if self.adapters.len() != s.adapters.len() {
            self.adapters = vec![(Ring::default(), Ring::default()); s.adapters.len()];
        }
        for ((rx, tx), a) in self.adapters.iter_mut().zip(&s.adapters) {
            rx.push(a.rx_bps as f32);
            tx.push(a.tx_bps as f32);
        }
    }
}

#[derive(Clone, Copy)]
struct PrevProc {
    cpu_time: u64,
    io: u64,
    net: u64,
    power_trend: f32,
}

/// Turns raw cumulative samples into rates. Keep one per collector.
pub struct Sampler {
    prev: Option<(Instant, RawSystem)>,
    prev_procs: HashMap<(u32, u64), PrevProc>,
    history: History,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    pub fn new() -> Self {
        Self { prev: None, prev_procs: HashMap::new(), history: History::default() }
    }

    /// Feed the next raw sample. The first call has no previous sample, so its rates are 0.
    pub fn update(&mut self, raw: RawSample) -> Snapshot {
        let elapsed = self.prev.as_ref().map(|(t, _)| raw.at.saturating_duration_since(*t));
        let secs = elapsed.map(|d| d.as_secs_f64()).filter(|s| *s > 0.0);
        let ncpu = raw.system.cores.len().max(1) as f64;

        let gpu_map: HashMap<u32, f32> = raw.system.gpu_per_pid.iter().copied().collect();
        let engine_map: HashMap<u32, &str> =
            raw.system.gpu_engine_per_pid.iter().map(|(p, e)| (*p, e.as_str())).collect();
        let gpu_of = |pid: u32| gpu_map.get(&pid).copied().unwrap_or(0.0);
        let mut procs_now = HashMap::with_capacity(raw.processes.len());
        let mut thread_count = 0u64;
        let mut handle_count = 0u64;
        let processes: Vec<ProcessRow> = raw
            .processes
            .into_iter()
            .map(|p| {
                let key = (p.pid, p.start_time);
                let io = p.io_read_bytes.saturating_add(p.io_write_bytes);
                let (cpu_pct, disk_bps, net_bps) = match (secs, self.prev_procs.get(&key)) {
                    (Some(secs), Some(prev)) => {
                        let cpu_100ns = p.cpu_time.saturating_sub(prev.cpu_time) as f64;
                        let cpu = cpu_100ns / (secs * 1e7 * ncpu) * 100.0;
                        (
                            cpu.clamp(0.0, 100.0) as f32,
                            io.saturating_sub(prev.io) as f64 / secs,
                            p.net_bytes.saturating_sub(prev.net) as f64 / secs,
                        )
                    }
                    _ => (0.0, 0.0, 0.0),
                };
                let power = power_score(cpu_pct, gpu_of(p.pid), disk_bps, net_bps);
                // Exponential average with a two-minute time constant; a new process starts at its current draw.
                let power_trend = match (secs, self.prev_procs.get(&key)) {
                    (Some(secs), Some(prev)) => {
                        let a = (secs / 120.0).min(1.0) as f32;
                        prev.power_trend + (power - prev.power_trend) * a
                    }
                    _ => power,
                };
                procs_now.insert(key, PrevProc { cpu_time: p.cpu_time, io, net: p.net_bytes, power_trend });
                thread_count += u64::from(p.threads);
                handle_count += u64::from(p.handles);
                ProcessRow {
                    pid: p.pid,
                    parent_pid: p.parent_pid,
                    start_time: p.start_time,
                    name: p.name,
                    cpu_pct,
                    memory: p.memory,
                    disk_bps,
                    net_bps,
                    suspended: p.suspended,
                    efficiency: p.efficiency,
                    counters: p.counters,
                    gpu_engine: engine_map.get(&p.pid).map(|e| e.to_string()).unwrap_or_default(),
                    cpu_time: p.cpu_time,
                    io_read_bytes: p.io_read_bytes,
                    io_write_bytes: p.io_write_bytes,
                    threads: p.threads,
                    handles: p.handles,
                    session_id: p.session_id,
                    base_priority: p.base_priority,
                    info: p.info,
                    gpu_pct: gpu_of(p.pid),
                    has_window: p.has_window,
                    power,
                    power_trend,
                }
            })
            .collect();
        self.prev_procs = procs_now;

        let sys = &raw.system;
        let (cpu_pct, cores_pct, net_rx_bps, net_tx_bps) = match (&self.prev, secs) {
            (Some((_, prev)), Some(secs)) => (
                sys.cpu.usage_since(&prev.cpu),
                sys.cores
                    .iter()
                    .enumerate()
                    .map(|(i, c)| prev.cores.get(i).map_or(0.0, |p| c.usage_since(p)))
                    .collect(),
                sys.net_rx_bytes.saturating_sub(prev.net_rx_bytes) as f64 / secs,
                sys.net_tx_bytes.saturating_sub(prev.net_tx_bytes) as f64 / secs,
            ),
            _ => (0.0, vec![0.0; sys.cores.len()], 0.0, 0.0),
        };
        let (kernel_pct, cores_kernel_pct) = match &self.prev {
            Some((_, prev)) if secs.is_some() => (
                sys.cpu.kernel_since(&prev.cpu),
                sys.cores
                    .iter()
                    .enumerate()
                    .map(|(i, c)| prev.cores.get(i).map_or(0.0, |p| c.kernel_since(p)))
                    .collect(),
            ),
            _ => (0.0, vec![0.0; sys.cores.len()]),
        };
        let disk_from_procs: f64 = processes.iter().map(|p| p.disk_bps).sum();
        let adapters = sys
            .adapters
            .iter()
            .map(|a| {
                let prev = self.prev.as_ref().and_then(|(_, p)| p.adapters.iter().find(|x| x.name == a.name));
                let (rx, tx) = match (prev, secs) {
                    (Some(p), Some(secs)) => (
                        a.rx_bytes.saturating_sub(p.rx_bytes) as f64 / secs,
                        a.tx_bytes.saturating_sub(p.tx_bytes) as f64 / secs,
                    ),
                    _ => (0.0, 0.0),
                };
                AdapterRates {
                    name: a.name.clone(),
                    description: a.description.clone(),
                    kind: a.kind.clone(),
                    rx_bps: rx,
                    tx_bps: tx,
                    link_bps: a.link_bps,
                }
            })
            .collect();
        let system = SystemRates {
            per_process_net: sys.per_process_net,
            kernel_pct,
            cores_kernel_pct,
            cpu_pct,
            cores_pct,
            mem_used: sys.mem_total.saturating_sub(sys.mem_available),
            mem_total: sys.mem_total,
            commit_used: sys.commit_used,
            commit_limit: sys.commit_limit,
            net_rx_bps,
            net_tx_bps,
            disk_active_pct: sys.disk_active_pct.map(|v| v.clamp(0.0, 100.0)),
            disk_read_bps: sys.disk_read_bps.unwrap_or(disk_from_procs),
            disk_write_bps: sys.disk_write_bps.unwrap_or(0.0),
            gpu_pct: sys.gpu_pct.map(|v| v.clamp(0.0, 100.0)),
            gpu_engines: sys.gpu_engines.clone(),
            gpu_dedicated_used: sys.gpu_dedicated_used,
            gpu_shared_used: sys.gpu_shared_used,
            gpus: sys.gpus.clone(),
            uptime: sys.uptime,
            cpu_mhz: sys.cpu_mhz,
            mem_cached: sys.mem_cached,
            mem_paged_pool: sys.mem_paged_pool,
            mem_nonpaged_pool: sys.mem_nonpaged_pool,
            mem_hw_reserved: sys.mem_hw_reserved,
            disks: sys.disks.clone(),
            adapters,
            process_count: processes.len(),
            thread_count,
            handle_count,
        };
        // The very first sample has no rates yet; graphing its zeros would draw a false dip.
        if secs.is_some() {
            self.history.push(&system);
        }
        self.prev = Some((raw.at, raw.system));
        Snapshot {
            processes,
            system,
            history: self.history.clone(),
            services: None,
            sessions: None,
            fps: Arc::default(),
        }
    }
}

/// Column the process table is sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SortKey {
    Name,
    Pid,
    #[default]
    Cpu,
    Memory,
    Disk,
    Network,
    Gpu,
    Threads,
    Power,
    PowerTrend,
    /// Running, then Efficiency mode, then Suspended.
    Status,
    Publisher,
    Type,
    ProcessName,
    CommandLine,
    GpuEngine,
}

impl SortKey {
    pub const ALL: [SortKey; 16] = [
        SortKey::Name,
        SortKey::Pid,
        SortKey::Cpu,
        SortKey::Memory,
        SortKey::Disk,
        SortKey::Network,
        SortKey::Gpu,
        SortKey::Threads,
        SortKey::Power,
        SortKey::PowerTrend,
        SortKey::Status,
        SortKey::Publisher,
        SortKey::Type,
        SortKey::ProcessName,
        SortKey::CommandLine,
        SortKey::GpuEngine,
    ];
    /// Name in the settings file.
    pub fn key(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Pid => "pid",
            SortKey::Cpu => "cpu",
            SortKey::Memory => "memory",
            SortKey::Disk => "disk",
            SortKey::Network => "network",
            SortKey::Gpu => "gpu",
            SortKey::Threads => "threads",
            SortKey::Power => "power",
            SortKey::PowerTrend => "powertrend",
            SortKey::Status => "status",
            SortKey::Publisher => "publisher",
            SortKey::Type => "type",
            SortKey::ProcessName => "processname",
            SortKey::CommandLine => "commandline",
            SortKey::GpuEngine => "gpuengine",
        }
    }

    pub fn from_key(k: &str) -> Option<SortKey> {
        match k {
            "process_name" => Some(SortKey::ProcessName),
            "command_line" => Some(SortKey::CommandLine),
            "gpu_engine" => Some(SortKey::GpuEngine),
            _ => SortKey::ALL.into_iter().find(|s| s.key() == k),
        }
    }

    /// Text reads best A to Z; numbers biggest first, as Task Manager does.
    pub fn descending_first(self) -> bool {
        !matches!(
            self,
            SortKey::Name
                | SortKey::Pid
                | SortKey::Status
                | SortKey::Publisher
                | SortKey::Type
                | SortKey::ProcessName
                | SortKey::CommandLine
                | SortKey::GpuEngine
        )
    }
}

/// The three sections of the Processes tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProcGroup {
    Apps,
    Background,
    Windows,
}

impl ProcGroup {
    pub fn label(self) -> &'static str {
        match self {
            Self::Apps => "Apps",
            Self::Background => "Background processes",
            Self::Windows => "Windows processes",
        }
    }
}

/// Kernel and session-0 system processes that have no image path a normal user can read.
const WINDOWS_NAMES: &[&str] = &[
    "System",
    "Registry",
    "Memory Compression",
    "Secure System",
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "services.exe",
    "lsass.exe",
    "winlogon.exe",
    "fontdrvhost.exe",
    "dwm.exe",
    "svchost.exe",
    "LsaIso.exe",
];

/// Critical Windows system processes that ask for confirmation before ending.
pub const CRITICAL_PROCESSES: &[&str] =
    &["csrss.exe", "wininit.exe", "winlogon.exe", "smss.exe", "services.exe", "lsass.exe"];

/// True when `name` is a critical Windows system process (case-insensitive).
pub fn is_critical_process(name: &str) -> bool {
    CRITICAL_PROCESSES.iter().any(|n| n.eq_ignore_ascii_case(name))
}

/// Case-insensitive text order, as Windows sorts names.
pub fn cmp_text(a: &str, b: &str) -> std::cmp::Ordering {
    a.chars().flat_map(char::to_lowercase).cmp(b.chars().flat_map(char::to_lowercase))
}

/// Display label for a process type: "App", "Background process", or "Windows process".
pub fn type_label(p: &ProcessRow, windir: &str) -> &'static str {
    match group_of(p, windir) {
        ProcGroup::Apps => "App",
        ProcGroup::Background => "Background process",
        ProcGroup::Windows => "Windows process",
    }
}

/// Apps own a visible window; Windows processes run from the Windows folder (except Explorer,
/// which Task Manager lists as an app when it has a window); everything else is background.
pub fn group_of(p: &ProcessRow, windir: &str) -> ProcGroup {
    if p.has_window {
        return ProcGroup::Apps;
    }
    let path = p.info.path.to_lowercase();
    let windir = windir.trim_end_matches('\\').to_lowercase();
    let in_windows = !windir.is_empty() && path.starts_with(&windir) && path[windir.len()..].starts_with('\\');
    if in_windows || (path.is_empty() && WINDOWS_NAMES.iter().any(|n| n.eq_ignore_ascii_case(&p.name))) {
        ProcGroup::Windows
    } else {
        ProcGroup::Background
    }
}

/// Name shown in the Processes tab: the version-resource description when there is one.
pub fn display_name(p: &ProcessRow) -> &str {
    if p.info.description.is_empty() { &p.name } else { &p.info.description }
}

/// Indices of `rows` that match `filter` (case-insensitive name or exact PID), sorted by `key`.
/// Returns indices so the UI never clones rows.
pub fn sorted_view(rows: &[ProcessRow], filter: &str, key: SortKey, descending: bool) -> Vec<usize> {
    let filter = filter.trim().to_lowercase();
    let pid_filter: Option<u32> = filter.parse().ok();
    let mut idx: Vec<usize> = (0..rows.len())
        .filter(|&i| {
            filter.is_empty() || pid_filter == Some(rows[i].pid) || rows[i].name.to_lowercase().contains(&filter)
        })
        .collect();
    // Only the Type column needs the Windows folder; this runs every frame.
    let windir = if key == SortKey::Type {
        std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())
    } else {
        String::new()
    };
    idx.sort_by(|&a, &b| {
        let (a, b) = (&rows[a], &rows[b]);
        let ord = match key {
            SortKey::Name => cmp_text(display_name(a), display_name(b)),
            SortKey::Pid => a.pid.cmp(&b.pid),
            SortKey::Status => (a.suspended, a.efficiency).cmp(&(b.suspended, b.efficiency)),
            SortKey::Publisher => cmp_text(&a.info.publisher, &b.info.publisher),
            SortKey::Cpu => a.cpu_pct.total_cmp(&b.cpu_pct),
            SortKey::Memory => a.memory.cmp(&b.memory),
            SortKey::Disk => a.disk_bps.total_cmp(&b.disk_bps),
            SortKey::Gpu => a.gpu_pct.total_cmp(&b.gpu_pct),
            SortKey::Network => a.net_bps.total_cmp(&b.net_bps),
            SortKey::Threads => a.threads.cmp(&b.threads),
            SortKey::Power => a.power.total_cmp(&b.power),
            SortKey::PowerTrend => a.power_trend.total_cmp(&b.power_trend),
            SortKey::Type => cmp_text(type_label(a, &windir), type_label(b, &windir)),
            SortKey::ProcessName => cmp_text(&a.name, &b.name),
            SortKey::CommandLine => cmp_text(&a.info.command_line, &b.info.command_line),
            SortKey::GpuEngine => cmp_text(&a.gpu_engine, &b.gpu_engine),
        };
        let ord = if descending { ord.reverse() } else { ord };
        ord.then(a.pid.cmp(&b.pid))
    });
    idx
}

/// "1.2 GB", "340 KB" (binary units, as Task Manager does).
pub fn format_bytes(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes.max(0.0);
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 || v >= 100.0 { format!("{v:.0} {}", UNITS[u]) } else { format!("{v:.1} {}", UNITS[u]) }
}

/// Bits per second for network, like "12.4 Mbps".
pub fn format_bits_per_sec(bytes_per_sec: f64) -> String {
    const UNITS: [&str; 4] = ["bps", "Kbps", "Mbps", "Gbps"];
    let mut v = (bytes_per_sec * 8.0).max(0.0);
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 { format!("{v:.0} {}", UNITS[u]) } else { format!("{v:.1} {}", UNITS[u]) }
}

pub fn format_uptime(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}:{:02}:{:02}", s / 86_400, (s / 3600) % 24, (s / 60) % 60, s % 60)
}

#[cfg(test)]
mod tests {

    #[test]
    fn column_totals() {
        let s = SystemRates {
            cpu_pct: 12.4,
            mem_used: 3,
            mem_total: 8,
            disk_active_pct: Some(140.0),
            gpu_pct: None,
            adapters: vec![
                AdapterRates { rx_bps: 1_000_000.0, tx_bps: 250_000.0, link_bps: 100_000_000, ..Default::default() },
                AdapterRates { link_bps: 0, ..Default::default() },
            ],
            ..Default::default()
        };
        let t = ColumnTotals::of(&s);
        assert_eq!(ColumnTotals::label(t.cpu), "12%");
        assert_eq!(ColumnTotals::label(t.memory), "38%");
        assert_eq!(t.disk, 100.0);
        assert_eq!(t.network, 10.0);
        assert_eq!(t.gpu, 0.0);
        assert_eq!(ColumnTotals::of(&SystemRates::default()), ColumnTotals::default());
    }
    use super::*;

    #[test]
    fn startup_impact_levels() {
        assert_eq!(startup_impact(2_000_000, 100_000), "Low");
        assert_eq!(startup_impact(5_000_000, 0), "Medium");
        assert_eq!(startup_impact(0, 4_000_000), "High");
    }

    #[test]
    fn npus_numbered_apart() {
        let g = |npu| GpuStats { npu, ..Default::default() };
        assert_eq!(gpu_titles(&[g(false), g(true), g(false)]), ["GPU 0", "NPU 0", "GPU 1"]);
    }

    #[test]
    fn service_groups() {
        assert_eq!(
            service_group(r"C:\Windows\system32\svchost.exe -k LocalServiceNetworkRestricted -p"),
            "LocalServiceNetworkRestricted"
        );
        assert_eq!(service_group(r"C:\Windows\System32\spoolsv.exe"), "");
    }

    #[test]
    fn ole_dates() {
        assert_eq!(format_ole_date(25569.0), "1970-01-01");
        assert_eq!(format_ole_date(46_000.75), "2025-12-09");
        assert_eq!(format_ole_date(36_526.0), "2000-01-01");
    }

    #[test]
    fn gpus_split_by_luid() {
        let l1 = "luid_0x00000000_0x0000D1B2_phys_0";
        let l2 = "luid_0x00000000_0x0000E000_phys_0";
        let engines = vec![
            (format!("pid_10_{l1}_eng_0_engtype_3D"), 40.0),
            (format!("pid_11_{l1}_eng_0_engtype_3D"), 20.0),
            (format!("pid_10_{l2}_eng_1_engtype_VideoDecode"), 5.0),
        ];
        let adapters = vec![GpuAdapter {
            luid: "0x00000000_0x0000E000".into(),
            name: "Intel".into(),
            dedicated_total: 128,
            shared_total: 0,
            ..Default::default()
        }];
        let dedicated = vec![(l1.to_string(), 1000.0)];
        let g = split_gpus(&adapters, &engines, &dedicated, &[]);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].name, "Intel");
        assert!((g[0].util - 5.0).abs() < 0.01);
        assert_eq!(g[1].name, "GPU 1");
        assert!((g[1].util - 60.0).abs() < 0.01);
        assert_eq!(g[1].dedicated_used, Some(1000));
        assert_eq!(g[0].dedicated_used, None);
    }

    fn proc(pid: u32, cpu_time: u64, io: u64) -> RawProcess {
        RawProcess {
            pid,
            start_time: 1,
            name: format!("p{pid}"),
            cpu_time,
            io_read_bytes: io,
            memory: u64::from(pid) * 1000,
            threads: 2,
            handles: 10,
            ..Default::default()
        }
    }

    fn system(idle: u64, total: u64, cores: usize, rx: u64) -> RawSystem {
        RawSystem {
            cpu: CpuTimes { idle, total, kernel: 0 },
            cores: vec![CpuTimes { idle: idle / cores as u64, total: total / cores as u64, kernel: 0 }; cores],
            mem_total: 1000,
            mem_available: 250,
            net_rx_bytes: rx,
            ..Default::default()
        }
    }

    #[test]
    fn cpu_usage_from_deltas() {
        let a = CpuTimes { idle: 100, total: 200, kernel: 10 };
        let b = CpuTimes { idle: 125, total: 300, kernel: 30 };
        assert_eq!(b.usage_since(&a), 75.0);
        assert_eq!(a.usage_since(&a), 0.0);
        // Counter going backwards (should not happen) must not panic or overflow.
        assert_eq!(a.usage_since(&b), 0.0);
    }

    #[test]
    fn process_cpu_and_disk_rates() {
        let t0 = Instant::now();
        let mut s = Sampler::new();
        let first = s.update(RawSample { at: t0, processes: vec![proc(4, 0, 0)], system: system(0, 0, 4, 0) });
        assert_eq!(first.processes[0].cpu_pct, 0.0);

        // 1 s later, process used 2 s of CPU time (2e7 * 100 ns) on 4 cores = 50 %.
        let second = s.update(RawSample {
            at: t0 + Duration::from_secs(1),
            processes: vec![proc(4, 20_000_000, 4096)],
            system: system(50, 100, 4, 1_000),
        });
        let p = &second.processes[0];
        assert!((p.cpu_pct - 50.0).abs() < 0.01, "{}", p.cpu_pct);
        assert!((p.disk_bps - 4096.0).abs() < 0.01);
        assert_eq!(second.system.cpu_pct, 50.0);
        assert_eq!(second.system.net_rx_bps, 1000.0);
        assert_eq!(second.system.mem_used, 750);
        assert_eq!(second.history.cpu.len(), 1);
        assert_eq!(second.history.mem_pct.last(), Some(75.0));
    }

    #[test]
    fn reused_pid_starts_from_zero() {
        let t0 = Instant::now();
        let mut s = Sampler::new();
        s.update(RawSample { at: t0, processes: vec![proc(8, 1_000_000_000, 0)], system: system(0, 0, 1, 0) });
        let mut reused = proc(8, 5, 0);
        reused.start_time = 2;
        let snap = s.update(RawSample {
            at: t0 + Duration::from_secs(1),
            processes: vec![reused],
            system: system(0, 10, 1, 0),
        });
        assert_eq!(snap.processes[0].cpu_pct, 0.0);
    }

    #[test]
    fn ring_keeps_newest() {
        let mut r = Ring::new(3);
        for v in 1..=5 {
            r.push(v as f32);
        }
        assert_eq!(r.iter().collect::<Vec<_>>(), vec![3.0, 4.0, 5.0]);
        assert_eq!(r.last(), Some(5.0));
        assert_eq!(r.max(), 5.0);
    }

    #[test]
    fn sort_and_filter() {
        let rows: Vec<ProcessRow> = [("chrome.exe", 1, 5.0), ("Code.exe", 2, 20.0), ("chrome.exe", 3, 1.0)]
            .into_iter()
            .map(|(n, pid, cpu)| ProcessRow {
                pid,
                parent_pid: 0,
                start_time: 0,
                name: n.into(),
                cpu_pct: cpu,
                memory: 0,
                disk_bps: 0.0,
                net_bps: 0.0,
                suspended: false,
                efficiency: false,
                counters: Default::default(),
                gpu_engine: String::new(),
                cpu_time: 0,
                io_read_bytes: 0,
                io_write_bytes: 0,
                threads: 0,
                handles: 0,
                session_id: 0,
                base_priority: 8,
                info: Arc::default(),
                gpu_pct: 0.0,
                has_window: false,
                power: 0.0,
                power_trend: 0.0,
            })
            .collect();
        assert_eq!(sorted_view(&rows, "", SortKey::Cpu, true), vec![1, 0, 2]);
        assert_eq!(sorted_view(&rows, "CHROME", SortKey::Cpu, false), vec![2, 0]);
        assert_eq!(sorted_view(&rows, "3", SortKey::Name, false), vec![2]);
    }
    #[test]
    fn critical_processes_list() {
        for name in ["csrss.exe", "wininit.exe", "winlogon.exe", "smss.exe", "services.exe", "lsass.exe"] {
            assert!(is_critical_process(name), "{name} should be critical");
            assert!(is_critical_process(&name.to_uppercase()), "{name} uppercase should be critical");
        }
        assert!(!is_critical_process("svchost.exe"));
        assert!(!is_critical_process("explorer.exe"));
        assert!(!is_critical_process("notepad.exe"));
    }

    #[test]
    fn sort_by_new_keys() {
        let p1 = ProcessRow {
            pid: 1,
            name: "beta.exe".into(),
            gpu_engine: "3D".into(),
            info: Arc::new(StaticInfo {
                command_line: "beta --arg".into(),
                path: r"C:\Windows\System32\beta.exe".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let p2 = ProcessRow {
            pid: 2,
            name: "alpha.exe".into(),
            gpu_engine: "Video".into(),
            has_window: true,
            info: Arc::new(StaticInfo {
                command_line: "alpha -v".into(),
                path: r"C:\Program Files\alpha.exe".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let p3 = ProcessRow {
            pid: 3,
            name: "gamma.exe".into(),
            gpu_engine: "Copy".into(),
            has_window: false,
            info: Arc::new(StaticInfo {
                command_line: "gamma".into(),
                path: r"C:\Tools\gamma.exe".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let rows = vec![p1, p2, p3];
        // ProcessName: alpha (2), beta (1), gamma (3)
        assert_eq!(sorted_view(&rows, "", SortKey::ProcessName, false), vec![1, 0, 2]);
        // CommandLine: "alpha -v" (2), "beta --arg" (1), "gamma" (3)
        assert_eq!(sorted_view(&rows, "", SortKey::CommandLine, false), vec![1, 0, 2]);
        // GpuEngine: "3D" (1), "Copy" (3), "Video" (2)
        assert_eq!(sorted_view(&rows, "", SortKey::GpuEngine, false), vec![0, 2, 1]);
        // Type: p2 is App ("App"), p3 is Background ("Background process"), p1 is in C:\Windows ("Windows process")
        assert_eq!(sorted_view(&rows, "", SortKey::Type, false), vec![1, 2, 0]);
        assert_eq!(cmp_text("Apple", "banana"), std::cmp::Ordering::Less);
        assert_eq!(cmp_text("banana", "Banana"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn users_group_by_session() {
        let row = |pid, session, cpu, mem| ProcessRow {
            pid,
            parent_pid: 0,
            start_time: 0,
            name: String::new(),
            cpu_pct: cpu,
            memory: mem,
            disk_bps: 0.0,
            net_bps: 0.0,
            suspended: false,
            efficiency: false,
            counters: Default::default(),
            gpu_engine: String::new(),
            cpu_time: 0,
            io_read_bytes: 0,
            io_write_bytes: 0,
            threads: 1,
            handles: 1,
            session_id: session,
            base_priority: 8,
            info: Arc::new(StaticInfo { user: "svc".into(), ..Default::default() }),
            gpu_pct: 0.0,
            has_window: false,
            power: 0.0,
            power_trend: 0.0,
        };
        let rows = [row(1, 0, 1.0, 10), row(2, 1, 2.0, 20), row(3, 1, 3.0, 30)];
        let sessions = [Session {
            id: 1,
            user: "PC\\phuoc".into(),
            state: "Active".into(),
            session_name: "Console".into(),
            client_name: String::new(),
        }];
        let v = users_view(&rows, &sessions);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].session_id, v[0].processes, v[0].user.as_str()), (0, 1, "svc"));
        assert_eq!((v[1].processes, v[1].memory, v[1].cpu_pct), (2, 50, 5.0));
        assert_eq!(v[1].user, "PC\\phuoc");
    }

    #[test]
    fn adapter_rates() {
        let t0 = Instant::now();
        let mut s = Sampler::new();
        let adapter = |rx| RawAdapter { name: "Ethernet".into(), rx_bytes: rx, ..Default::default() };
        let mut sys = system(0, 0, 1, 0);
        sys.adapters = vec![adapter(1000)];
        s.update(RawSample { at: t0, processes: vec![], system: sys.clone() });
        sys.adapters = vec![adapter(3000)];
        let snap = s.update(RawSample { at: t0 + Duration::from_secs(2), processes: vec![], system: sys });
        assert_eq!(snap.system.adapters[0].rx_bps, 1000.0);
        assert_eq!(snap.history.adapters[0].0.last(), Some(1000.0));
    }

    #[test]
    fn groups() {
        let mk = |name: &str, path: &str, win: bool| ProcessRow {
            pid: 1,
            parent_pid: 0,
            start_time: 0,
            name: name.into(),
            cpu_pct: 0.0,
            memory: 0,
            disk_bps: 0.0,
            net_bps: 0.0,
            suspended: false,
            efficiency: false,
            counters: Default::default(),
            gpu_engine: String::new(),
            cpu_time: 0,
            io_read_bytes: 0,
            io_write_bytes: 0,
            threads: 0,
            handles: 0,
            session_id: 1,
            base_priority: 8,
            info: Arc::new(StaticInfo { path: path.into(), ..Default::default() }),
            gpu_pct: 0.0,
            has_window: win,
            power: 0.0,
            power_trend: 0.0,
        };
        let w = r"C:\Windows";
        assert_eq!(group_of(&mk("notepad.exe", r"C:\Windows\System32\notepad.exe", true), w), ProcGroup::Apps);
        assert_eq!(group_of(&mk("svchost.exe", r"C:\WINDOWS\system32\svchost.exe", false), w), ProcGroup::Windows);
        assert_eq!(group_of(&mk("csrss.exe", "", false), w), ProcGroup::Windows);
        assert_eq!(group_of(&mk("x.exe", r"C:\WindowsApps\x.exe", false), w), ProcGroup::Background);
        assert_eq!(group_of(&mk("OneDrive.exe", r"C:\Users\a\OneDrive.exe", false), w), ProcGroup::Background);
    }

    #[test]
    fn gpu_engines() {
        let items: Vec<(String, f64)> = [
            ("pid_10_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 30.0),
            ("pid_10_luid_0x0_0x1_phys_0_eng_1_engtype_VideoDecode", 50.0),
            ("pid_20_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 40.0),
        ]
        .into_iter()
        .map(|(n, v)| (n.to_string(), v))
        .collect();
        let GpuUsage { total, per_pid: per, engines, engine_per_pid } = gpu_from_engines(&items);
        assert_eq!(engine_per_pid, vec![(10, "VideoDecode".to_string()), (20, "3D".to_string())]);
        assert_eq!(total, 70.0); // 3D: 30 + 40
        assert_eq!(per, vec![(10, 50.0), (20, 40.0)]);
        assert_eq!(engines, vec![("3D".to_string(), 70.0), ("VideoDecode".to_string(), 50.0)]);
    }

    #[test]
    fn priority_labels() {
        assert_eq!(priority_label(8), "Normal");
        assert_eq!(priority_label(13), "High");
        assert_eq!(priority_label(4), "Low");
    }

    #[test]
    fn formatting() {
        assert_eq!(format_bytes(512.0), "512 B");
        assert_eq!(format_bytes(1536.0), "1.5 KB");
        assert_eq!(format_bytes(300.0 * 1024.0 * 1024.0), "300 MB");
        assert_eq!(format_bits_per_sec(125_000.0), "1.0 Mbps");
        assert_eq!(format_uptime(Duration::from_secs(90_061)), "1:01:01:01");
        assert_eq!(format_filetime(116_444_736_000_000_000), "1970-01-01 00:00");
        assert_eq!(format_filetime(0), "");
    }
}
