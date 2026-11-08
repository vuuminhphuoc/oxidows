//! Development-only reader over `/proc`. Not tuned: the product target is Windows.

use std::fs;
use std::io;
use std::time::{Duration, Instant};

use taskmgr_core::{CpuTimes, RawProcess, RawSample, RawSystem};

pub struct Collector {
    ticks_per_sec: u64,
    page_size: u64,
}

impl Collector {
    pub fn new() -> io::Result<Self> {
        // USER_HZ is 100 on every mainstream Linux; page size 4 KiB on x86-64.
        Ok(Self { ticks_per_sec: 100, page_size: 4096 })
    }

    pub fn collect(&mut self) -> io::Result<RawSample> {
        let at = Instant::now();
        let mut processes = Vec::new();
        for entry in fs::read_dir("/proc")? {
            let Ok(entry) = entry else { continue };
            let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            if let Some(p) = self.read_process(pid) {
                processes.push(p);
            }
        }
        Ok(RawSample { at, processes, system: self.read_system()? })
    }

    fn read_process(&self, pid: u32) -> Option<RawProcess> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let open = stat.find('(')?;
        let close = stat.rfind(')')?;
        let name = stat[open + 1..close].to_string();
        let f: Vec<&str> = stat[close + 2..].split_whitespace().collect();
        // Fields after "comm": state(0) ppid(1) ... utime(11) stime(12) ... num_threads(17) starttime(19) rss(21)
        let num = |i: usize| f.get(i).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        let to_100ns = 10_000_000 / self.ticks_per_sec;
        let (mut read, mut write) = (0, 0);
        if let Ok(io) = fs::read_to_string(format!("/proc/{pid}/io")) {
            for line in io.lines() {
                if let Some(v) = line.strip_prefix("read_bytes: ") {
                    read = v.trim().parse().unwrap_or(0);
                } else if let Some(v) = line.strip_prefix("write_bytes: ") {
                    write = v.trim().parse().unwrap_or(0);
                }
            }
        }
        let handles = fs::read_dir(format!("/proc/{pid}/fd")).map(|d| d.count() as u32).unwrap_or(0);
        Some(RawProcess {
            pid,
            parent_pid: num(1) as u32,
            start_time: num(19),
            name,
            cpu_time: (num(11) + num(12)) * to_100ns,
            memory: num(21) * self.page_size,
            io_read_bytes: read,
            io_write_bytes: write,
            net_bytes: 0,
            suspended: false,
            efficiency: false,
            counters: Default::default(),
            threads: num(17) as u32,
            handles,
            session_id: num(3) as u32,
            has_window: false,
            // Map nice (-20..19) onto the Windows scale around Normal = 8.
            base_priority: 8 - (f.get(16).and_then(|v| v.parse::<i32>().ok()).unwrap_or(0) / 3),
            info: Default::default(),
        })
    }

    fn read_system(&self) -> io::Result<RawSystem> {
        let mut sys = RawSystem::default();
        for line in fs::read_to_string("/proc/stat")?.lines() {
            let mut it = line.split_whitespace();
            let Some(label) = it.next() else { continue };
            if !label.starts_with("cpu") {
                continue;
            }
            let v: Vec<u64> = it.map(|x| x.parse().unwrap_or(0)).collect();
            let idle = v.get(3).copied().unwrap_or(0) + v.get(4).copied().unwrap_or(0);
            // system + irq + softirq
            let kernel = [2, 5, 6].iter().map(|&i| v.get(i).copied().unwrap_or(0)).sum();
            let times = CpuTimes { idle, total: v.iter().take(8).sum(), kernel };
            if label == "cpu" {
                sys.cpu = times;
            } else {
                sys.cores.push(times);
            }
        }
        for line in fs::read_to_string("/proc/meminfo")?.lines() {
            let mut it = line.split_whitespace();
            let (Some(k), Some(v)) = (it.next(), it.next()) else {
                continue;
            };
            let bytes = v.parse::<u64>().unwrap_or(0) * 1024;
            match k {
                "MemTotal:" => sys.mem_total = bytes,
                "MemAvailable:" => sys.mem_available = bytes,
                "Committed_AS:" => sys.commit_used = bytes,
                "CommitLimit:" => sys.commit_limit = bytes,
                _ => {}
            }
        }
        if let Ok(dev) = fs::read_to_string("/proc/net/dev") {
            for line in dev.lines().skip(2) {
                let Some((iface, rest)) = line.split_once(':') else {
                    continue;
                };
                if iface.trim() == "lo" {
                    continue;
                }
                let v: Vec<u64> = rest.split_whitespace().map(|x| x.parse().unwrap_or(0)).collect();
                sys.net_rx_bytes += v.first().copied().unwrap_or(0);
                sys.net_tx_bytes += v.get(8).copied().unwrap_or(0);
                sys.adapters.push(taskmgr_core::RawAdapter {
                    name: iface.trim().to_string(),
                    kind: if iface.trim().starts_with('w') { "Wi-Fi" } else { "Ethernet" }.into(),
                    rx_bytes: v.first().copied().unwrap_or(0),
                    tx_bytes: v.get(8).copied().unwrap_or(0),
                    ..Default::default()
                });
            }
        }
        if let Ok(up) = fs::read_to_string("/proc/uptime") {
            let secs: f64 = up.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            sys.uptime = Duration::from_secs_f64(secs);
        }
        Ok(sys)
    }
}

pub fn end_process(pid: u32) -> io::Result<()> {
    let status = std::process::Command::new("kill").args(["-9", &pid.to_string()]).status()?;
    if status.success() { Ok(()) } else { Err(io::Error::other(format!("kill exited with {status}"))) }
}

// ---- Extras (development stand-ins for the Windows features) ----

use taskmgr_core::{Service, Session, StartupItem, StaticInfo};

use crate::{PriorityClass, ServiceAction};

pub fn static_info(pid: u32) -> StaticInfo {
    let path = fs::read_link(format!("/proc/{pid}/exe")).map(|p| p.display().to_string()).unwrap_or_default();
    let command_line = fs::read(format!("/proc/{pid}/cmdline"))
        .map(|b| String::from_utf8_lossy(&b).split('\0').filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    let uid = fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("Uid:").map(|v| v.split_whitespace().next().unwrap_or("").to_string()))
        })
        .unwrap_or_default();
    let user = fs::read_to_string("/etc/passwd")
        .ok()
        .and_then(|p| {
            p.lines().find_map(|l| {
                let f: Vec<&str> = l.split(':').collect();
                (f.get(2) == Some(&uid.as_str())).then(|| f[0].to_string())
            })
        })
        .unwrap_or(uid);
    StaticInfo { path, command_line, user, ..Default::default() }
}

pub fn set_priority(pid: u32, class: PriorityClass) -> io::Result<()> {
    let nice = match class {
        PriorityClass::Idle => 19,
        PriorityClass::BelowNormal => 10,
        PriorityClass::Normal => 0,
        PriorityClass::AboveNormal => -5,
        PriorityClass::High => -10,
        PriorityClass::Realtime => -20,
    };
    let st = std::process::Command::new("renice").args(["-n", &nice.to_string(), "-p", &pid.to_string()]).status()?;
    if st.success() { Ok(()) } else { Err(io::Error::other(format!("renice exited with {st}"))) }
}

pub fn list_services() -> io::Result<Vec<Service>> {
    Ok(Vec::new())
}

pub fn service_control(_name: &str, _action: ServiceAction) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "services are Windows-only"))
}

pub fn list_sessions() -> io::Result<Vec<Session>> {
    Ok(Vec::new())
}

pub fn startup_items() -> Vec<StartupItem> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let dir = std::path::Path::new(&home).join(".config/autostart");
    let Ok(rd) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    rd.flatten()
        .filter_map(|e| {
            let text = fs::read_to_string(e.path()).ok()?;
            let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(k)).unwrap_or("").to_string();
            Some(StartupItem {
                name: get("Name="),
                command: get("Exec="),
                location: dir.display().to_string(),
                enabled: get("Hidden=") != "true",
                per_user: true,
                publisher: String::new(),
            })
        })
        .collect()
}

pub fn set_startup_enabled(_item: &StartupItem, _enabled: bool) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "not implemented on Linux"))
}

pub fn is_elevated() -> bool {
    fs::read_to_string("/proc/self/status")
        .map(|s| s.lines().any(|l| l.starts_with("Uid:") && l.split_whitespace().nth(1) == Some("0")))
        .unwrap_or(false)
}

pub fn restart_elevated() -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "run with sudo instead"))
}

fn unsupported<T>() -> io::Result<T> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "Windows-only"))
}

pub fn affinity(_pid: u32) -> io::Result<(u64, u64)> {
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(64);
    let all = if n == 64 { u64::MAX } else { (1u64 << n) - 1 };
    Ok((all, all))
}
pub fn set_affinity(_pid: u32, _mask: u64) -> io::Result<()> {
    unsupported()
}
pub fn efficiency_mode(_pid: u32) -> io::Result<bool> {
    Ok(false)
}
pub fn set_efficiency_mode(_pid: u32, _on: bool) -> io::Result<()> {
    unsupported()
}
pub fn create_dump(_pid: u32, _path: &std::path::Path) -> io::Result<()> {
    unsupported()
}
pub fn run_new_task(command: &str, _admin: bool) -> io::Result<()> {
    let (file, params) = crate::split_command(command);
    std::process::Command::new(file).args(params.split_whitespace()).spawn().map(|_| ())
}
pub fn open_url(url: &str) -> io::Result<()> {
    std::process::Command::new("xdg-open").arg(url).spawn().map(|_| ())
}

pub fn cpu_info() -> taskmgr_core::CpuInfo {
    let text = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let field = |k: &str| {
        text.lines().find_map(|l| l.split_once(':').filter(|(a, _)| a.trim() == k).map(|(_, v)| v.trim().to_string()))
    };
    let logical = text.lines().filter(|l| l.starts_with("processor")).count() as u32;
    taskmgr_core::CpuInfo {
        name: field("model name").unwrap_or_default(),
        base_mhz: field("cpu MHz").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0) as u32,
        sockets: 1,
        cores: field("cpu cores").and_then(|v| v.parse().ok()).unwrap_or(logical),
        logical,
        virtualization: Some(text.contains(" vmx") || text.contains(" svm")),
        ..Default::default()
    }
}

pub fn disconnect_session(_id: u32) -> io::Result<()> {
    unsupported()
}
pub fn logoff_session(_id: u32) -> io::Result<()> {
    unsupported()
}
pub fn last_bios_ms() -> Option<u32> {
    None
}

pub fn taskmgr_replacement() -> Option<String> {
    None
}
pub fn set_taskmgr_replacement(_exe: Option<&str>) -> io::Result<()> {
    unsupported()
}
pub fn is_running(name: &str) -> bool {
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| c.trim().eq_ignore_ascii_case(name)))
}
pub fn shutdown() {}
pub fn adapter_info(_name: &str) -> taskmgr_core::AdapterInfo {
    taskmgr_core::AdapterInfo::default()
}
pub mod tray {
    pub fn install(_hwnd: isize) -> bool {
        false
    }
    pub fn update(_cpu_pct: f32, _tip: &str) {}
    pub fn hide_window() {}
}
pub fn app_history() -> io::Result<taskmgr_core::AppHistory> {
    unsupported()
}
pub fn switch_to(_pid: u32) -> io::Result<()> {
    unsupported()
}
pub fn file_properties(_path: &str) -> io::Result<()> {
    unsupported()
}
pub fn disk_info(_number: u32) -> io::Result<taskmgr_core::DiskInfo> {
    unsupported()
}
pub fn memory_info() -> taskmgr_core::MemoryInfo {
    taskmgr_core::MemoryInfo::default()
}

pub fn wait_chain(_pid: u32) -> io::Result<Vec<taskmgr_core::WaitChain>> {
    unsupported()
}
pub fn uac_virtualization(_pid: u32) -> Option<bool> {
    None
}
pub fn set_uac_virtualization(_pid: u32, _on: bool) -> io::Result<()> {
    unsupported()
}
pub fn disk_temperature(_number: u32) -> Option<f32> {
    None
}
pub fn gpu_temperature(_luid: &str) -> Option<f32> {
    None
}
