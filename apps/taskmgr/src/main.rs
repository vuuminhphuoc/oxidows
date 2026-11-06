#![forbid(unsafe_code)]
// Release builds on Windows run without a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod details;
mod graph;
mod performance;
mod processes;
mod services;
mod startup;
mod users;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use taskmgr_core::{Extras, Sampler, Snapshot};

/// Shared with the sampler thread. 0 = paused.
static INTERVAL_MS: AtomicU64 = AtomicU64::new(1000);
/// The sampler reads services / sessions only while their tab is open.
static WANT_SERVICES: AtomicBool = AtomicBool::new(false);
static WANT_SESSIONS: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tab {
    Processes,
    Performance,
    Startup,
    Users,
    Details,
    Services,
}

impl Tab {
    const ALL: [Tab; 6] = [Tab::Processes, Tab::Performance, Tab::Startup, Tab::Users, Tab::Details, Tab::Services];

    fn label(self) -> &'static str {
        match self {
            Tab::Processes => "Processes",
            Tab::Performance => "Performance",
            Tab::Startup => "Startup apps",
            Tab::Users => "Users",
            Tab::Details => "Details",
            Tab::Services => "Services",
        }
    }

    fn from_env() -> Tab {
        let v = std::env::var("OXI_TASKMGR_TAB").unwrap_or_default().to_lowercase();
        Tab::ALL.into_iter().find(|t| t.label().to_lowercase().starts_with(&v) && !v.is_empty()).unwrap_or(Tab::Processes)
    }
}

/// Status line shared by every tab and by background actions (service start/stop).
#[derive(Clone, Default)]
pub struct Status(Arc<Mutex<Option<String>>>);

impl Status {
    pub fn set(&self, msg: impl Into<String>) {
        if let Ok(mut m) = self.0.lock() {
            *m = Some(msg.into());
        }
    }
    fn get(&self) -> Option<String> {
        self.0.lock().ok().and_then(|m| m.clone())
    }
}

struct App {
    rx: Receiver<Result<Arc<Snapshot>, String>>,
    snapshot: Arc<Snapshot>,
    error: Option<String>,
    tab: Tab,
    status: Status,
    elevated: bool,
    processes: processes::State,
    performance: performance::State,
    details: details::State,
    services: services::State,
    startup: startup::State,
    started: Instant,
    first_data_logged: bool,
}

/// Collects on its own thread so a slow OS call never blocks a frame. The UI only repaints when
/// a new snapshot arrives (or on input), which keeps idle CPU near zero.
fn spawn_sampler(ctx: egui::Context, tx: Sender<Result<Arc<Snapshot>, String>>) {
    thread::Builder::new()
        .name("sampler".into())
        .spawn(move || {
            let mut collector = match taskmgr_sys::Collector::new() {
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
                let extras = Extras {
                    services: WANT_SERVICES.load(Ordering::Relaxed),
                    sessions: WANT_SESSIONS.load(Ordering::Relaxed),
                };
                let msg = collector
                    .collect(extras)
                    .map(|(raw, extra)| {
                        let mut snap = sampler.update(raw);
                        snap.services = extra.services.map(Arc::new);
                        snap.sessions = extra.sessions.map(Arc::new);
                        Arc::new(snap)
                    })
                    .map_err(|e| e.to_string());
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
        let tab = Tab::from_env();
        WANT_SERVICES.store(tab == Tab::Services, Ordering::Relaxed);
        WANT_SESSIONS.store(tab == Tab::Users, Ordering::Relaxed);
        let (tx, rx) = channel();
        spawn_sampler(cc.egui_ctx.clone(), tx);
        let status = Status::default();
        Self {
            rx,
            snapshot: Arc::default(),
            error: None,
            tab,
            elevated: taskmgr_sys::is_elevated(),
            processes: processes::State::new(status.clone()),
            performance: performance::State::default(),
            details: details::State::new(status.clone()),
            services: services::State::new(status.clone(), cc.egui_ctx.clone()),
            startup: startup::State::new(status.clone()),
            status,
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
                        if std::env::var_os("OXI_TASKMGR_TIMING").is_some() {
                            eprintln!("first process list after {} ms", self.started.elapsed().as_millis());
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
                for t in Tab::ALL {
                    ui.selectable_value(&mut self.tab, t, t.label());
                }
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
                if !self.elevated
                    && ui
                        .button("Run as administrator")
                        .on_hover_text("Needed to end elevated processes, control services and edit machine-wide startup apps")
                        .clicked()
                {
                    match taskmgr_sys::restart_elevated() {
                        Ok(()) => std::process::exit(0),
                        Err(e) => self.status.set(e.to_string()),
                    }
                }
            });
            ui.add_space(2.0);
        });
        WANT_SERVICES.store(self.tab == Tab::Services, Ordering::Relaxed);
        WANT_SESSIONS.store(self.tab == Tab::Users, Ordering::Relaxed);

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
                if let Some(msg) = self.status.get() {
                    ui.separator();
                    ui.label(msg);
                }
            });
        });

        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Processes => self.processes.show(ui, &snap),
            Tab::Performance => self.performance.show(ui, &snap),
            Tab::Startup => self.startup.show(ui),
            Tab::Users => users::show(ui, &snap),
            Tab::Details => self.details.show(ui, &snap),
            Tab::Services => self.services.show(ui, &snap),
        });
    }
}

/// wgpu by default on Windows: it falls back to DirectX WARP (software) on machines without a usable
/// GPU driver (VMs, RDP, CI runners), where OpenGL is only the 1.1 GDI stub. `OXI_RENDERER=glow`
/// opts into OpenGL, which uses less memory where a real driver exists.
fn renderer() -> eframe::Renderer {
    match std::env::var("OXI_RENDERER").as_deref() {
        Ok("glow") => eframe::Renderer::Glow,
        Ok("wgpu") => eframe::Renderer::Wgpu,
        _ if cfg!(windows) => eframe::Renderer::Wgpu,
        _ => eframe::Renderer::Glow,
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        renderer: renderer(),
        viewport: egui::ViewportBuilder::default()
            .with_title("Task Manager (oxidows)")
            .with_inner_size([1000.0, 660.0])
            .with_min_inner_size([560.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native("oxi-taskmgr", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
