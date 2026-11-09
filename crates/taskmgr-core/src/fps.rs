//! Frames per second of the apps that draw with DirectX, from the present calls `taskmgr-sys`
//! records. The original Task Manager has no FPS figure (Xbox Game Bar has one); this module turns
//! the raw present times into the numbers and the 60-second history the Performance tab shows.

use std::collections::HashMap;

use crate::{HISTORY_LEN, ProcessRow, Ring};

/// Which runtime the frames came through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FpsApi {
    /// Direct3D 10, 11 and 12 (and Vulkan / OpenGL drivers that present through DXGI).
    #[default]
    Dxgi,
    D3d9,
}

impl FpsApi {
    pub fn label(self) -> &'static str {
        match self {
            FpsApi::Dxgi => "DirectX (DXGI)",
            FpsApi::D3d9 => "Direct3D 9",
        }
    }
}

/// Present times of one process's busiest swap chain, in seconds on one clock, oldest first, at
/// most [`WINDOW_SECS`] old, with `now` read from the same clock.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawFps {
    pub pid: u32,
    pub api: FpsApi,
    pub times: Vec<f64>,
    pub now: f64,
}

/// How long the recorder keeps present times (the 1% low looks this far back).
pub const WINDOW_SECS: f64 = 10.0;

/// Whether the FPS figures can be read at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FpsStatus {
    /// Not Windows, or the graphics event trace could not start.
    #[default]
    Unavailable,
    /// Windows only lets elevated apps read graphics events.
    NeedsAdmin,
    Running,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameStats {
    /// Frames presented in the last second.
    pub fps: f32,
    /// Average time between those frames, ms.
    pub frame_ms: f32,
    /// FPS of the slowest 1% of frames over the last [`WINDOW_SECS`] (the 99th percentile frame
    /// time), None until there are enough frames for it to mean something.
    pub low_1pct: Option<f32>,
}

/// Figures from present times (seconds, oldest first) as of `now`.
pub fn frame_stats(times: &[f64], now: f64) -> FrameStats {
    let recent: Vec<f64> = times.iter().copied().filter(|t| *t > now - 1.0 && *t <= now).collect();
    let fps = recent.len() as f32;
    let frame_ms = if recent.len() >= 2 {
        ((recent[recent.len() - 1] - recent[0]) * 1000.0 / (recent.len() - 1) as f64) as f32
    } else {
        0.0
    };
    let mut gaps: Vec<f64> =
        times.windows(2).filter(|w| w[1] > now - WINDOW_SECS).map(|w| w[1] - w[0]).filter(|g| *g > 0.0).collect();
    let low_1pct = if gaps.len() >= 20 && fps > 0.0 {
        gaps.sort_by(|a, b| b.total_cmp(a));
        let worst = gaps[gaps.len() / 100];
        Some((1.0 / worst) as f32)
    } else {
        None
    };
    FrameStats { fps, frame_ms, low_1pct }
}

/// One app that drew frames in the last minute.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FpsApp {
    pub pid: u32,
    pub name: String,
    /// Has a visible top-level window (games and players do; helpers usually not).
    pub has_window: bool,
    pub api: FpsApi,
    pub stats: FrameStats,
    /// FPS once per sample, for the graph.
    pub history: Ring,
}

impl FpsApp {
    /// Average and best over the history.
    pub fn avg_max(&self) -> (f32, f32) {
        let n = self.history.len().max(1) as f32;
        (self.history.iter().sum::<f32>() / n, self.history.max())
    }
}

/// What the Performance tab's FPS page shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fps {
    pub status: FpsStatus,
    /// Busiest first.
    pub apps: Vec<FpsApp>,
}

impl Fps {
    /// The app the page follows on its own: the windowed app drawing the most frames, leaving out
    /// this Task Manager and the desktop compositor.
    pub fn auto_pick(&self, me: u32) -> Option<&FpsApp> {
        let skip = |a: &&FpsApp| a.pid != me && !a.name.eq_ignore_ascii_case("dwm.exe");
        let drawing = |a: &&FpsApp| a.stats.fps > 0.0;
        self.apps
            .iter()
            .filter(skip)
            .filter(drawing)
            .find(|a| a.has_window)
            .or_else(|| self.apps.iter().filter(skip).find(drawing))
            .or_else(|| self.apps.iter().find(|a| a.pid != me))
    }
}

/// Keeps a 60-sample FPS history per app between samples.
#[derive(Default)]
pub struct FpsTracker {
    rings: HashMap<u32, (Ring, FpsApi)>,
}

impl FpsTracker {
    /// One sample: `raw` from the recorder, names and windows from this tick's processes.
    pub fn update(&mut self, status: FpsStatus, raw: &[RawFps], processes: &[ProcessRow]) -> Fps {
        let mut now: HashMap<u32, (FrameStats, FpsApi)> = HashMap::with_capacity(raw.len());
        for r in raw {
            now.insert(r.pid, (frame_stats(&r.times, r.now), r.api));
        }
        let alive: HashMap<u32, &ProcessRow> = processes.iter().map(|p| (p.pid, p)).collect();
        // Apps that stopped drawing keep their graph for a minute, then drop off.
        self.rings.retain(|pid, (ring, _)| {
            alive.contains_key(pid) && (now.contains_key(pid) || ring.len() < HISTORY_LEN || ring.max() > 0.0)
        });
        for (pid, (_, api)) in &now {
            if alive.contains_key(pid) {
                self.rings.entry(*pid).or_insert_with(|| (Ring::default(), *api)).1 = *api;
            }
        }
        let mut apps: Vec<FpsApp> = self
            .rings
            .iter_mut()
            .map(|(pid, (ring, api))| {
                let stats = now.get(pid).map(|(s, _)| *s).unwrap_or_default();
                ring.push(stats.fps);
                let p = alive.get(pid);
                FpsApp {
                    pid: *pid,
                    name: p.map(|p| p.name.clone()).unwrap_or_default(),
                    has_window: p.is_some_and(|p| p.has_window),
                    api: *api,
                    stats,
                    history: ring.clone(),
                }
            })
            .collect();
        apps.sort_by(|a, b| b.stats.fps.total_cmp(&a.stats.fps).then(a.name.cmp(&b.name)).then(a.pid.cmp(&b.pid)));
        Fps { status, apps }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steady(fps: f64, secs: f64, now: f64) -> Vec<f64> {
        let n = (fps * secs) as usize;
        (0..n).map(|i| now - secs + (i as f64 + 1.0) / fps).collect()
    }

    #[test]
    fn steady_sixty() {
        let t = steady(60.0, 5.0, 100.0);
        let s = frame_stats(&t, 100.0);
        assert!((s.fps - 60.0).abs() <= 1.0, "{s:?}");
        assert!((s.frame_ms - 16.67).abs() < 0.1, "{s:?}");
        assert!((s.low_1pct.unwrap() - 60.0).abs() < 0.5, "{s:?}");
    }

    #[test]
    fn hitch_lowers_one_percent_low() {
        let mut t = steady(100.0, 5.0, 100.0);
        // Six 100 ms stalls in five seconds (500 frames, so over 1% of them): the 1% low falls
        // to 10 FPS while the average barely moves.
        for k in [50, 120, 200, 280, 360, 440] {
            for x in &mut t[k..] {
                *x += 0.09;
            }
        }
        let t: Vec<f64> = t.into_iter().map(|x| x - 0.54).collect();
        let s = frame_stats(&t, 100.0);
        assert!(s.low_1pct.unwrap() < 15.0, "{s:?}");
        assert!(s.fps > 90.0, "{s:?}");
    }

    #[test]
    fn idle_app_reads_zero() {
        let s = frame_stats(&[1.0, 2.0], 50.0);
        assert_eq!(s.fps, 0.0);
        assert_eq!(s.low_1pct, None);
        assert_eq!(frame_stats(&[], 1.0), FrameStats::default());
    }

    fn proc(pid: u32, name: &str, window: bool) -> ProcessRow {
        ProcessRow {
            pid,
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
            session_id: 0,
            base_priority: 8,
            info: Default::default(),
            gpu_pct: 0.0,
            has_window: window,
            power: 0.0,
            power_trend: 0.0,
        }
    }

    #[test]
    fn tracker_orders_and_picks_windowed_app() {
        let procs = [proc(1, "me.exe", true), proc(2, "helper.exe", false), proc(3, "game.exe", true)];
        let raw = [
            RawFps { pid: 1, api: FpsApi::Dxgi, times: steady(2.0, 3.0, 10.0), now: 10.0 },
            RawFps { pid: 2, api: FpsApi::Dxgi, times: steady(144.0, 3.0, 10.0), now: 10.0 },
            RawFps { pid: 3, api: FpsApi::D3d9, times: steady(60.0, 3.0, 10.0), now: 10.0 },
        ];
        let mut t = FpsTracker::default();
        let f = t.update(FpsStatus::Running, &raw, &procs);
        assert_eq!(f.apps.iter().map(|a| a.pid).collect::<Vec<_>>(), [2, 3, 1]);
        assert_eq!(f.auto_pick(1).map(|a| a.pid), Some(3));
        assert_eq!(f.apps[1].api, FpsApi::D3d9);
        assert_eq!(f.apps[1].history.len(), 1);

        // The game stops drawing: it stays listed at 0 FPS while its minute of history lasts.
        let f = t.update(FpsStatus::Running, &raw[..2], &procs);
        let game = f.apps.iter().find(|a| a.pid == 3).unwrap();
        assert_eq!(game.stats.fps, 0.0);
        assert_eq!(game.history.len(), 2);
        assert_eq!(f.auto_pick(1).map(|a| a.pid), Some(2));

        // It exits: gone.
        let f = t.update(FpsStatus::Running, &raw[..2], &procs[..2]);
        assert!(f.apps.iter().all(|a| a.pid != 3));
    }
}
