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

/// Points kept for every Performance graph (one per sample: 60 s at the default 1 s interval).
pub const HISTORY_LEN: usize = 60;

/// Cumulative CPU time of one logical processor (or of all of them), in any fixed unit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub idle: u64,
    /// Idle + busy time.
    pub total: u64,
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
    pub threads: u32,
    pub handles: u32,
    pub session_id: u32,
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
}

/// System-wide counters for one sample.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawSystem {
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
    pub uptime: Duration,
}

#[derive(Clone, Debug)]
pub struct RawSample {
    pub at: Instant,
    pub processes: Vec<RawProcess>,
    pub system: RawSystem,
}

/// A process with its rates, ready to show.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessRow {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    pub cpu_pct: f32,
    pub memory: u64,
    /// Read + write bytes per second.
    pub disk_bps: f64,
    pub threads: u32,
    pub handles: u32,
    pub session_id: u32,
    pub base_priority: i32,
    pub info: Arc<StaticInfo>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemRates {
    pub cpu_pct: f32,
    pub cores_pct: Vec<f32>,
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
    pub uptime: Duration,
    pub process_count: usize,
    pub thread_count: u64,
    pub handle_count: u64,
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub id: u32,
    pub user: String,
    /// "Active", "Disconnected"...
    pub state: String,
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
    /// Memory in use, percent of total.
    pub mem_pct: Ring,
    pub disk_active_pct: Ring,
    /// Bytes per second.
    pub disk_bps: Ring,
    pub net_rx_bps: Ring,
    pub net_tx_bps: Ring,
    pub gpu_pct: Ring,
}

impl History {
    fn push(&mut self, s: &SystemRates) {
        self.cpu.push(s.cpu_pct);
        if self.cores.len() != s.cores_pct.len() {
            self.cores = vec![Ring::default(); s.cores_pct.len()];
        }
        for (ring, v) in self.cores.iter_mut().zip(&s.cores_pct) {
            ring.push(*v);
        }
        let mem_pct =
            if s.mem_total == 0 { 0.0 } else { (s.mem_used as f64 * 100.0 / s.mem_total as f64) as f32 };
        self.mem_pct.push(mem_pct);
        self.disk_active_pct.push(s.disk_active_pct.unwrap_or(0.0));
        self.disk_bps.push((s.disk_read_bps + s.disk_write_bps) as f32);
        self.net_rx_bps.push(s.net_rx_bps as f32);
        self.net_tx_bps.push(s.net_tx_bps as f32);
        self.gpu_pct.push(s.gpu_pct.unwrap_or(0.0));
    }
}

#[derive(Clone, Copy)]
struct PrevProc {
    cpu_time: u64,
    io: u64,
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

        let mut procs_now = HashMap::with_capacity(raw.processes.len());
        let mut thread_count = 0u64;
        let mut handle_count = 0u64;
        let processes: Vec<ProcessRow> = raw
            .processes
            .into_iter()
            .map(|p| {
                let key = (p.pid, p.start_time);
                let io = p.io_read_bytes.saturating_add(p.io_write_bytes);
                let (cpu_pct, disk_bps) = match (secs, self.prev_procs.get(&key)) {
                    (Some(secs), Some(prev)) => {
                        let cpu_100ns = p.cpu_time.saturating_sub(prev.cpu_time) as f64;
                        let cpu = cpu_100ns / (secs * 1e7 * ncpu) * 100.0;
                        (cpu.clamp(0.0, 100.0) as f32, io.saturating_sub(prev.io) as f64 / secs)
                    }
                    _ => (0.0, 0.0),
                };
                procs_now.insert(key, PrevProc { cpu_time: p.cpu_time, io });
                thread_count += u64::from(p.threads);
                handle_count += u64::from(p.handles);
                ProcessRow {
                    pid: p.pid,
                    parent_pid: p.parent_pid,
                    name: p.name,
                    cpu_pct,
                    memory: p.memory,
                    disk_bps,
                    threads: p.threads,
                    handles: p.handles,
                    session_id: p.session_id,
                    base_priority: p.base_priority,
                    info: p.info,
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
        let disk_from_procs: f64 = processes.iter().map(|p| p.disk_bps).sum();
        let system = SystemRates {
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
            uptime: sys.uptime,
            process_count: processes.len(),
            thread_count,
            handle_count,
        };
        // The very first sample has no rates yet; graphing its zeros would draw a false dip.
        if secs.is_some() {
            self.history.push(&system);
        }
        self.prev = Some((raw.at, raw.system));
        Snapshot { processes, system, history: self.history.clone(), services: None, sessions: None }
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
    Threads,
}

/// Indices of `rows` that match `filter` (case-insensitive name or exact PID), sorted by `key`.
/// Returns indices so the UI never clones rows.
pub fn sorted_view(rows: &[ProcessRow], filter: &str, key: SortKey, descending: bool) -> Vec<usize> {
    let filter = filter.trim().to_lowercase();
    let pid_filter: Option<u32> = filter.parse().ok();
    let mut idx: Vec<usize> = (0..rows.len())
        .filter(|&i| {
            filter.is_empty()
                || pid_filter == Some(rows[i].pid)
                || rows[i].name.to_lowercase().contains(&filter)
        })
        .collect();
    idx.sort_by(|&a, &b| {
        let (a, b) = (&rows[a], &rows[b]);
        let ord = match key {
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Pid => a.pid.cmp(&b.pid),
            SortKey::Cpu => a.cpu_pct.total_cmp(&b.cpu_pct),
            SortKey::Memory => a.memory.cmp(&b.memory),
            SortKey::Disk => a.disk_bps.total_cmp(&b.disk_bps),
            SortKey::Threads => a.threads.cmp(&b.threads),
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
    use super::*;

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
            cpu: CpuTimes { idle, total },
            cores: vec![CpuTimes { idle: idle / cores as u64, total: total / cores as u64 }; cores],
            mem_total: 1000,
            mem_available: 250,
            net_rx_bytes: rx,
            ..Default::default()
        }
    }

    #[test]
    fn cpu_usage_from_deltas() {
        let a = CpuTimes { idle: 100, total: 200 };
        let b = CpuTimes { idle: 125, total: 300 };
        assert_eq!(b.usage_since(&a), 75.0);
        assert_eq!(a.usage_since(&a), 0.0);
        // Counter going backwards (should not happen) must not panic or overflow.
        assert_eq!(a.usage_since(&b), 0.0);
    }

    #[test]
    fn process_cpu_and_disk_rates() {
        let t0 = Instant::now();
        let mut s = Sampler::new();
        let first = s.update(RawSample {
            at: t0,
            processes: vec![proc(4, 0, 0)],
            system: system(0, 0, 4, 0),
        });
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
                name: n.into(),
                cpu_pct: cpu,
                memory: 0,
                disk_bps: 0.0,
                threads: 0,
                handles: 0,
                session_id: 0,
                base_priority: 8,
                info: Arc::default(),
            })
            .collect();
        assert_eq!(sorted_view(&rows, "", SortKey::Cpu, true), vec![1, 0, 2]);
        assert_eq!(sorted_view(&rows, "CHROME", SortKey::Cpu, false), vec![2, 0]);
        assert_eq!(sorted_view(&rows, "3", SortKey::Name, false), vec![2]);
    }

    #[test]
    fn users_group_by_session() {
        let row = |pid, session, cpu, mem| ProcessRow {
            pid,
            parent_pid: 0,
            name: String::new(),
            cpu_pct: cpu,
            memory: mem,
            disk_bps: 0.0,
            threads: 1,
            handles: 1,
            session_id: session,
            base_priority: 8,
            info: Arc::new(StaticInfo { user: "svc".into(), ..Default::default() }),
        };
        let rows = [row(1, 0, 1.0, 10), row(2, 1, 2.0, 20), row(3, 1, 3.0, 30)];
        let sessions = [Session { id: 1, user: "PC\\phuoc".into(), state: "Active".into() }];
        let v = users_view(&rows, &sessions);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].session_id, v[0].processes, v[0].user.as_str()), (0, 1, "svc"));
        assert_eq!((v[1].processes, v[1].memory, v[1].cpu_pct), (2, 50, 5.0));
        assert_eq!(v[1].user, "PC\\phuoc");
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
    }
}
