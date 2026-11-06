//! Development-only reader over `/proc`. Not tuned: the product target is Windows.

use std::fs;
use std::io;
use std::time::{Duration, Instant};

use tm_core::{CpuTimes, RawProcess, RawSample, RawSystem};

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
            threads: num(17) as u32,
            handles,
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
            let times = CpuTimes { idle, total: v.iter().take(8).sum() };
            if label == "cpu" {
                sys.cpu = times;
            } else {
                sys.cores.push(times);
            }
        }
        for line in fs::read_to_string("/proc/meminfo")?.lines() {
            let mut it = line.split_whitespace();
            let (Some(k), Some(v)) = (it.next(), it.next()) else { continue };
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
                let Some((iface, rest)) = line.split_once(':') else { continue };
                if iface.trim() == "lo" {
                    continue;
                }
                let v: Vec<u64> = rest.split_whitespace().map(|x| x.parse().unwrap_or(0)).collect();
                sys.net_rx_bytes += v.first().copied().unwrap_or(0);
                sys.net_tx_bytes += v.get(8).copied().unwrap_or(0);
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
