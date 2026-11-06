#![forbid(unsafe_code)]
// Release builds on Windows run without a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod graph;
mod performance;
mod processes;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use tm_core::{Sampler, Snapshot};

/// Shared with the sampler thread. 0 = paused.
static INTERVAL_MS: AtomicU64 = AtomicU64::new(1000);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Processes,
    Performance,
}

struct App {
    rx: Receiver<Result<Arc<Snapshot>, String>>,
    snapshot: Arc<Snapshot>,
    error: Option<String>,
    tab: Tab,
    processes: processes::State,
    performance: performance::State,
    started: Instant,
    first_data_logged: bool,
}

/// Collects on its own thread so a slow OS call never blocks a frame. The UI only repaints when
/// a new snapshot arrives (or on input), which keeps idle CPU near zero.
fn spawn_sampler(ctx: egui::Context, tx: Sender<Result<Arc<Snapshot>, String>>) {
    thread::Builder::new()
        .name("sampler".into())
        .spawn(move || {
            let mut collector = match tm_sys::Collector::new() {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(Err(e.to_string()));
                    return;
                }
            };
            let mut sampler = Sampler::new();
            let mut first = true;
            loop {
                let ms = INTERVAL_MS.load(Ordering::Relaxed);
                if ms == 0 && !first {
                    thread::sleep(Duration::from_millis(200));
                    continue;
                }
                let msg = collector.collect().map(|raw| Arc::new(sampler.update(raw))).map_err(|e| e.to_string());
                if tx.send(msg).is_err() {
                    return; // window closed
                }
                ctx.request_repaint();
                if first {
                    // A second sample right away gives real CPU % on screen within ~250 ms.
                    first = false;
                    thread::sleep(Duration::from_millis(250));
                } else {
                    thread::sleep(Duration::from_millis(ms.max(250)));
                }
            }
        })
        .expect("spawn sampler thread");
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = channel();
        spawn_sampler(cc.egui_ctx.clone(), tx);
        Self {
            rx,
            snapshot: Arc::default(),
            error: None,
            // RTASKMGR_TAB=performance opens on the Performance tab (handy for scripted screenshots).
            tab: if std::env::var("RTASKMGR_TAB").is_ok_and(|v| v.eq_ignore_ascii_case("performance")) {
                Tab::Performance
            } else {
                Tab::Processes
            },
            processes: processes::State::default(),
            performance: performance::State::default(),
            started: Instant::now(),
            first_data_logged: false,
        }
    }

    fn drain(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Ok(s) => {
                    self.snapshot = s;
                    self.error = None;
                    if !self.first_data_logged && !self.snapshot.processes.is_empty() {
                        self.first_data_logged = true;
                        if std::env::var_os("RTASKMGR_TIMING").is_some() {
                            eprintln!("first process list after {:?}", self.started.elapsed());
                        }
                    }
                }
                Err(e) => self.error = Some(e),
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain();
        let snap = self.snapshot.clone();

        egui::Panel::top("tabs").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Processes, "Processes");
                ui.selectable_value(&mut self.tab, Tab::Performance, "Performance");
                ui.separator();
                let mut ms = INTERVAL_MS.load(Ordering::Relaxed);
                let label = match ms {
                    0 => "Paused",
                    500 => "Update: 0.5 s",
                    1000 => "Update: 1 s",
                    _ => "Update: 2 s",
                };
                egui::ComboBox::from_id_salt("interval").selected_text(label).show_ui(ui, |ui| {
                    ui.selectable_value(&mut ms, 500, "0.5 s");
                    ui.selectable_value(&mut ms, 1000, "1 s");
                    ui.selectable_value(&mut ms, 2000, "2 s");
                    ui.selectable_value(&mut ms, 0, "Paused");
                });
                INTERVAL_MS.store(ms, Ordering::Relaxed);
            });
            ui.add_space(2.0);
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                let s = &snap.system;
                ui.label(format!(
                    "Processes: {}   CPU: {:.0}%   Memory: {:.0}%",
                    s.process_count,
                    s.cpu_pct,
                    if s.mem_total == 0 { 0.0 } else { s.mem_used as f64 * 100.0 / s.mem_total as f64 }
                ));
                if let Some(e) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                if let Some(msg) = &self.processes.message {
                    ui.separator();
                    ui.label(msg);
                }
            });
        });

        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Processes => self.processes.show(ui, &snap),
            Tab::Performance => self.performance.show(ui, &snap),
        });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Task Manager (Rust)")
            .with_inner_size([960.0, 640.0])
            .with_min_inner_size([560.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native("rtaskmgr", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
