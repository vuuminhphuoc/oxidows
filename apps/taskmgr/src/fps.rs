//! The FPS page on the Performance tab: frames per second of the app drawing the most (or one the
//! user picks), its graph over 60 seconds, frame time, 1% low, and every app drawing frames.
//! Windows' own Task Manager has no such page (Xbox Game Bar has the counter); the figures come
//! from DirectX's present events, see `taskmgr_sys::fps`.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::{Color32, RichText, Ui, Vec2};
use taskmgr_core::{Fps, FpsApp, FpsStatus, Ring, Snapshot};

use crate::graph;

pub const COLOR: Color32 = Color32::from_rgb(200, 50, 110);

/// The sampler records frames only once the Performance tab has been opened: until then no
/// DirectX app pays for the events.
pub static WANT: AtomicBool = AtomicBool::new(false);

fn empty() -> &'static Ring {
    static R: OnceLock<Ring> = OnceLock::new();
    R.get_or_init(Ring::default)
}

#[derive(Default)]
pub struct State {
    /// The app the page follows; None follows the busiest windowed app.
    pinned: Option<u32>,
    status: Option<FpsStatus>,
}

/// A graph scale that reads well against common refresh rates, with headroom so a game capped
/// at 144 FPS draws below the top edge.
fn scale(max: f32) -> f32 {
    let want = max * 1.1;
    [30.0, 60.0, 90.0, 120.0, 165.0, 240.0, 360.0, 480.0]
        .into_iter()
        .find(|s| want <= *s)
        .unwrap_or_else(|| (want / 100.0).ceil() * 100.0)
}

fn fps_text(v: f32) -> String {
    format!("{v:.0} FPS")
}

impl State {
    /// Starts the recorder (once) and says whether it runs.
    pub fn status(&mut self) -> FpsStatus {
        WANT.store(true, Ordering::Relaxed);
        *self.status.get_or_insert_with(taskmgr_sys::fps::start)
    }

    /// The app shown: the pinned one while it still draws or has history, else the busiest.
    pub fn current<'a>(&mut self, fps: &'a Fps) -> Option<&'a FpsApp> {
        if let Some(pid) = self.pinned {
            match fps.apps.iter().find(|a| a.pid == pid) {
                Some(a) => return Some(a),
                None => self.pinned = None,
            }
        }
        fps.auto_pick(std::process::id())
    }

    /// The sidebar entry: (value text, graph ring, graph scale).
    pub fn entry<'a>(&mut self, snap: &'a Snapshot) -> (String, &'a Ring, f32) {
        let status = self.status();
        match self.current(&snap.fps) {
            Some(a) => (format!("{}\n{:.0} FPS", a.name, a.stats.fps), &a.history, scale(a.history.max())),
            None => (
                match status {
                    FpsStatus::NeedsAdmin => "Needs administrator".into(),
                    FpsStatus::Unavailable => "Not available".into(),
                    FpsStatus::Running => "No app drawing".into(),
                },
                empty(),
                60.0,
            ),
        }
    }

    /// The detail page; returns its figures as text for Copy.
    pub fn view(&mut self, ui: &mut Ui, snap: &Snapshot, big: Vec2) -> String {
        let status = self.status();
        let fps = &snap.fps;
        let app = self.current(fps);
        let right = app.map(|a| format!("{} (PID {})", a.name, a.pid)).unwrap_or_default();
        super::performance::header(ui, "FPS", &right);
        let ring = app.map_or(empty(), |a| &a.history);
        let max = scale(ring.max());
        ui.label(RichText::new(format!("Frames per second over 60 seconds, scale {max:.0} FPS")).small().weak());
        let g = graph::area(ui, big, ring, max, COLOR, true);
        g.on_hover_text_at_pointer(match app {
            Some(a) => format!("{}: {:.0} FPS", a.name, a.stats.fps),
            None => "No app drawing".into(),
        });

        let note = match status {
            FpsStatus::NeedsAdmin => Some(
                "Measuring FPS needs administrator rights: Windows only shares graphics events with elevated apps.",
            ),
            FpsStatus::Unavailable => Some("FPS is not available on this system."),
            FpsStatus::Running if fps.apps.is_empty() => {
                Some("No app is drawing frames with DirectX right now. Start a game or a video and its FPS shows here.")
            }
            FpsStatus::Running => None,
        };
        if let Some(n) = note {
            ui.add_space(8.0);
            ui.label(n);
            if status == FpsStatus::NeedsAdmin
                && ui.button(ui_common::Icon::Shield.with("Run as administrator")).clicked()
                && taskmgr_sys::restart_elevated().is_ok()
            {
                std::process::exit(0);
            }
        }

        let mut items: Vec<(&str, String)> = Vec::new();
        if let Some(a) = app {
            let s = a.stats;
            let (avg, best) = a.avg_max();
            items.push(("Frames per second", format!("{:.0}", s.fps)));
            items.push(("Frame time", if s.frame_ms > 0.0 { format!("{:.1} ms", s.frame_ms) } else { "-".into() }));
            items.push(("1% low", s.low_1pct.map_or("-".into(), fps_text)));
            items.push(("Average (60 s)", fps_text(avg)));
            items.push(("Highest (60 s)", fps_text(best)));
            items.push(("Graphics API", a.api.label().into()));
            items.push(("Process", a.name.clone()));
            items.push(("PID", a.pid.to_string()));
        }
        let text = if items.is_empty() { String::new() } else { super::performance::stats(ui, &items) };

        if !fps.apps.is_empty() {
            ui.add_space(10.0);
            ui.label(RichText::new("Apps drawing frames (click one to follow it)").small().weak());
            let current = app.map(|a| a.pid);
            egui::Grid::new("fps-apps").spacing([24.0, 4.0]).show(ui, |ui| {
                if ui
                    .selectable_label(self.pinned.is_none(), "Automatic")
                    .on_hover_text("Follow the windowed app drawing the most frames")
                    .clicked()
                {
                    self.pinned = None;
                }
                ui.label(RichText::new("PID").small().weak());
                ui.label(RichText::new("FPS").small().weak());
                ui.label(RichText::new("Frame time").small().weak());
                ui.label(RichText::new("1% low").small().weak());
                ui.end_row();
                for a in &fps.apps {
                    let name = if a.name.is_empty() { format!("PID {}", a.pid) } else { a.name.clone() };
                    if ui.selectable_label(current == Some(a.pid) && self.pinned.is_some(), name).clicked() {
                        self.pinned = Some(a.pid);
                    }
                    ui.label(a.pid.to_string());
                    ui.label(format!("{:.0}", a.stats.fps));
                    ui.label(if a.stats.frame_ms > 0.0 { format!("{:.1} ms", a.stats.frame_ms) } else { "-".into() });
                    ui.label(a.stats.low_1pct.map_or("-".into(), |v| format!("{v:.0}")));
                    ui.end_row();
                }
            });
        }
        text
    }
}
