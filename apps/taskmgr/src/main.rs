#![forbid(unsafe_code)]
// Release builds on Windows run without a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod details;
mod graph;
mod history;
mod performance;
mod processes;
mod services;
mod settings;
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
    AppHistory,
    Startup,
    Users,
    Details,
    Services,
    Settings,
}

impl Tab {
    const ALL: [Tab; 8] = [
        Tab::Processes,
        Tab::Performance,
        Tab::AppHistory,
        Tab::Startup,
        Tab::Users,
        Tab::Details,
        Tab::Services,
        Tab::Settings,
    ];

    fn label(self) -> &'static str {
        match self {
            Tab::Processes => "Processes",
            Tab::Performance => "Performance",
            Tab::AppHistory => "App history",
            Tab::Startup => "Startup apps",
            Tab::Users => "Users",
            Tab::Details => "Details",
            Tab::Services => "Services",
            Tab::Settings => "Settings",
        }
    }

    fn icon(self) -> ui_common::Icon {
        use ui_common::Icon;
        match self {
            Tab::Processes => Icon::Processes,
            Tab::Performance => Icon::Performance,
            Tab::AppHistory => Icon::AppHistory,
            Tab::Startup => Icon::Startup,
            Tab::Users => Icon::Users,
            Tab::Details => Icon::Details,
            Tab::Services => Icon::Services,
            Tab::Settings => Icon::Settings,
        }
    }

    fn find(v: &str) -> Option<Tab> {
        let v = v.to_lowercase();
        Tab::ALL.into_iter().find(|t| !v.is_empty() && t.label().to_lowercase().starts_with(&v))
    }

    /// `OXI_TASKMGR_TAB` wins over the saved default start page.
    fn initial(default_tab: &str) -> Tab {
        std::env::var("OXI_TASKMGR_TAB")
            .ok()
            .and_then(|v| Tab::find(&v))
            .or_else(|| Tab::find(default_tab))
            .unwrap_or(Tab::Processes)
    }
}

/// A modal-ish window opened from a menu.
pub enum Dialog {
    RunTask {
        command: String,
        admin: bool,
    },
    Affinity {
        pid: u32,
        name: String,
        mask: u64,
        system: u64,
    },
    Confirm {
        text: String,
        action: ConfirmAction,
    },
    /// Result of "Analyze wait chain"; `names` labels the blocking processes.
    WaitChain {
        pid: u32,
        name: String,
        chains: Vec<taskmgr_core::WaitChain>,
        names: Vec<(u32, String)>,
    },
}

/// Actions that lose someone's work, so they ask first.
pub enum ConfirmAction {
    SignOff { session: u32, user: String },
    Disconnect { session: u32, user: String },
}

/// Shared by every tab and by background actions: the status line, an open dialog, and a
/// request to jump to a process on the Details tab.
#[derive(Clone, Default)]
pub struct Status {
    msg: Arc<Mutex<Option<String>>>,
    dialog: Arc<Mutex<Option<Dialog>>>,
    goto_details: Arc<Mutex<Option<u32>>>,
    goto_services: Arc<Mutex<Option<u32>>>,
    /// "Switch to" brought another window forward ("Minimize on use" acts on it).
    used: Arc<AtomicBool>,
}

impl Status {
    pub fn set(&self, msg: impl Into<String>) {
        if let Ok(mut m) = self.msg.lock() {
            *m = Some(msg.into());
        }
    }
    fn get(&self) -> Option<String> {
        self.msg.lock().ok().and_then(|m| m.clone())
    }
    pub fn open(&self, d: Dialog) {
        if let Ok(mut m) = self.dialog.lock() {
            *m = Some(d);
        }
    }
    pub fn used(&self) {
        self.used.store(true, Ordering::Relaxed);
    }
    pub fn goto_services(&self, pid: u32) {
        if let Ok(mut m) = self.goto_services.lock() {
            *m = Some(pid);
        }
    }
    pub fn goto_details(&self, pid: u32) {
        if let Ok(mut m) = self.goto_details.lock() {
            *m = Some(pid);
        }
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
    history: history::State,
    details: details::State,
    services: services::State,
    startup: startup::State,
    users: users::State,
    settings: settings::Settings,
    /// Whether Ctrl+Shift+Esc opens this exe; None where that cannot be changed.
    replaces_taskmgr: Option<bool>,
    tray: bool,
    /// Navigation pane expanded (icons and labels) or collapsed to icons.
    nav_open: bool,
    started: Instant,
    first_data_logged: bool,
    /// OXI_TASKMGR_TIMING: frames drawn since `frames_since`.
    frames: u32,
    frame_cpu: f32,
    frames_since: Instant,
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
            let timing = std::env::var_os("OXI_TASKMGR_TIMING").is_some();
            let (mut n, mut spent, mut since) = (0u32, Duration::ZERO, Instant::now());
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
                let t0 = Instant::now();
                let msg = collector
                    .collect(extras)
                    .map(|(raw, extra)| {
                        let mut snap = sampler.update(raw);
                        snap.services = extra.services.map(Arc::new);
                        snap.sessions = extra.sessions.map(Arc::new);
                        Arc::new(snap)
                    })
                    .map_err(|e| e.to_string());
                if timing {
                    n += 1;
                    spent += t0.elapsed();
                    if since.elapsed() >= Duration::from_secs(5) {
                        eprintln!("sampler: {n} samples, {:.1} ms each", spent.as_secs_f64() * 1000.0 / f64::from(n));
                        if let Ok(s) = &msg {
                            let busy: Vec<String> = s
                                .processes
                                .iter()
                                .filter(|p| p.net_bps > 0.0)
                                .take(5)
                                .map(|p| format!("{} {:.0} B/s", p.name, p.net_bps))
                                .collect();
                            let suspended = s.processes.iter().filter(|p| p.suspended).count();
                            let gpus: Vec<String> = s
                                .system
                                .gpus
                                .iter()
                                .map(|g| {
                                    format!(
                                        "{} {:.0}% driver {} {} dx {} at {}",
                                        g.name, g.util, g.driver_version, g.driver_date, g.directx, g.location
                                    )
                                })
                                .collect();
                            eprintln!("gpus: [{}]", gpus.join(", "));
                            if let Some(a) = s.system.adapters.first() {
                                eprintln!("adapter {}: {:?}", a.name, taskmgr_sys::adapter_info(&a.name));
                            }
                            eprintln!(
                                "per-process network: {}, active: [{}]; suspended processes: {suspended}",
                                s.system.per_process_net,
                                busy.join(", ")
                            );
                        }
                        (n, spent, since) = (0, Duration::ZERO, Instant::now());
                    }
                }
                if let Ok(s) = &msg {
                    let sys = &s.system;
                    let mem = if sys.mem_total == 0 { 0.0 } else { sys.mem_used as f64 * 100.0 / sys.mem_total as f64 };
                    taskmgr_sys::tray::update(
                        sys.cpu_pct,
                        &format!("Task Manager (oxidows)\nCPU: {:.0}%  Memory: {mem:.0}%", sys.cpu_pct),
                    );
                }
                if tx.send(msg).is_err() {
                    return; // window closed
                }
                // A zero-delay request makes egui paint two frames; a tiny delay paints one, which
                // halves idle CPU where rendering is expensive (software rendering in VMs and RDP).
                ctx.request_repaint_after(Duration::from_millis(1));
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
        if std::env::var_os("OXI_TASKMGR_TIMING").is_some() {
            match &cc.wgpu_render_state {
                Some(rs) => {
                    let i = rs.adapter.get_info();
                    eprintln!("renderer: wgpu {:?} {} ({:?}, driver {})", i.backend, i.name, i.device_type, i.driver);
                    for a in &rs.available_adapters {
                        let i = a.get_info();
                        eprintln!("  available: {:?} {} ({:?})", i.backend, i.name, i.device_type);
                    }
                }
                None => eprintln!("renderer: glow"),
            }
        }
        let settings = settings::Settings::load();
        ui_common::setup(&cc.egui_ctx, settings.theme);
        if settings.always_on_top {
            settings.apply_on_top(&cc.egui_ctx);
        }
        let interval = std::env::var("OXI_TASKMGR_INTERVAL_MS").ok().and_then(|v| v.parse().ok());
        INTERVAL_MS.store(interval.unwrap_or(settings.interval_ms), Ordering::Relaxed);
        let tab = Tab::initial(&settings.default_tab);
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
            processes: processes::State::new(status.clone(), settings.group_by_type, settings.values_pct),
            performance: performance::State::default(),
            history: history::State::new(cc.egui_ctx.clone()),
            details: details::State::new(status.clone(), details::parse_columns(&settings.details_columns)),
            services: services::State::new(status.clone(), cc.egui_ctx.clone()),
            startup: startup::State::new(status.clone()),
            users: users::State::new(status.clone()),
            settings,
            replaces_taskmgr: cfg!(windows).then(taskmgr_sys::replaces_taskmgr),
            tray: false,
            nav_open: true,
            status,
            started: Instant::now(),
            first_data_logged: false,
            frames: 0,
            frame_cpu: 0.0,
            frames_since: Instant::now(),
        }
    }

    fn save_settings(&self) {
        if let Err(e) = self.settings.save() {
            self.status.set(format!("Could not save settings: {e}"));
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

impl App {
    fn dialogs(&mut self, ctx: &egui::Context) {
        let Ok(mut slot) = self.status.dialog.lock() else {
            return;
        };
        let mut close = false;
        match slot.as_mut() {
            None => {}
            Some(Dialog::RunTask { command, admin }) => {
                egui::Window::new("Create new task").collapsible(false).resizable(false).show(ctx, |ui| {
                    ui.label("Type the name of a program, folder, document or Internet resource.");
                    let r = ui.add(egui::TextEdit::singleline(command).desired_width(360.0).hint_text("e.g. notepad"));
                    r.request_focus();
                    ui.checkbox(admin, "Create this task with administrative privileges");
                    ui.horizontal(|ui| {
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if (ui.button("OK").clicked() || enter) && !command.trim().is_empty() {
                            match taskmgr_sys::run_new_task(command, *admin) {
                                Ok(()) => self.status.set(format!("Started {command}")),
                                Err(e) => self.status.set(format!("Could not start {command}: {e}")),
                            }
                            close = true;
                        }
                        if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            close = true;
                        }
                    });
                });
            }
            Some(Dialog::Affinity { pid, name, mask, system }) => {
                egui::Window::new("Processor affinity").collapsible(false).resizable(false).show(ctx, |ui| {
                    ui.label(format!("Which processors are allowed to run {name} ({pid})?"));
                    let cpus: Vec<u32> = (0..64).filter(|i| *system & (1u64 << i) != 0).collect();
                    let mut all = *mask & *system == *system;
                    if ui.checkbox(&mut all, "<All Processors>").changed() {
                        *mask = if all { *system } else { 0 };
                    }
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        egui::Grid::new("cpus").show(ui, |ui| {
                            for (n, i) in cpus.iter().enumerate() {
                                let mut on = *mask & (1u64 << i) != 0;
                                if ui.checkbox(&mut on, format!("CPU {i}")).changed() {
                                    *mask ^= 1u64 << i;
                                }
                                if n % 4 == 3 {
                                    ui.end_row();
                                }
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        if ui.add_enabled(*mask != 0, egui::Button::new("OK")).clicked() {
                            match taskmgr_sys::set_affinity(*pid, *mask) {
                                Ok(()) => self.status.set(format!("Affinity of {name} set")),
                                Err(e) => self.status.set(format!("Could not set affinity of {name}: {e}")),
                            }
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            }
            Some(Dialog::Confirm { text, action }) => {
                egui::Window::new("Task Manager").collapsible(false).resizable(false).show(ctx, |ui| {
                    ui.label(text.as_str());
                    ui.horizontal(|ui| {
                        if ui.button("Yes").clicked() {
                            let msg = match action {
                                ConfirmAction::SignOff { session, user } => match taskmgr_sys::logoff_session(*session)
                                {
                                    Ok(()) => format!("Signing out {user}"),
                                    Err(e) => format!("Could not sign out {user}: {e}"),
                                },
                                ConfirmAction::Disconnect { session, user } => {
                                    match taskmgr_sys::disconnect_session(*session) {
                                        Ok(()) => format!("Disconnected {user}"),
                                        Err(e) => format!("Could not disconnect {user}: {e}"),
                                    }
                                }
                            };
                            self.status.set(msg);
                            close = true;
                        }
                        if ui.button("No").clicked() {
                            close = true;
                        }
                    });
                });
            }
            Some(Dialog::WaitChain { pid, name, chains, names }) => {
                egui::Window::new("Analyze wait chain").collapsible(false).resizable(true).show(ctx, |ui| {
                    if chains.is_empty() {
                        ui.label(format!(
                            "{name} ({pid}) is running normally: none of its threads are waiting on another process."
                        ));
                    } else {
                        ui.label(format!("{name} ({pid}) has {} blocked thread(s).", chains.len()));
                        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                            for c in chains.iter() {
                                let cycle = if c.cycle { "  (deadlock)" } else { "" };
                                ui.label(format!("Thread {}: waits on {}{cycle}", c.thread, c.steps.join(" -> ")));
                            }
                        });
                        if !names.is_empty() {
                            ui.separator();
                            ui.label("Ending one of these processes may let it continue:");
                            for (p, n) in names.iter() {
                                ui.horizontal(|ui| {
                                    ui.label(format!("{n} ({p})"));
                                    if ui.button("End process").clicked() {
                                        self.status.set(match taskmgr_sys::end_process(*p) {
                                            Ok(()) => format!("Ended {n} ({p})"),
                                            Err(e) => format!("Could not end {n} ({p}): {e}"),
                                        });
                                    }
                                });
                            }
                        }
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            }
        }
        if close {
            *slot = None;
        }
    }
}

impl App {
    /// The navigation pane, as in Windows 11's Task Manager: a menu button that collapses it to
    /// icons, the pages, and Settings at the bottom.
    fn nav(&mut self, ui: &mut egui::Ui) {
        // Narrow windows get the icon strip, as Task Manager does.
        let open = self.nav_open && ui.ctx().content_rect().width() >= 720.0;
        let width = if open { 200.0 } else { 52.0 };
        egui::Panel::left("nav").resizable(false).exact_size(width).show(ui, |ui| {
            ui.add_space(4.0);
            let menu = ui_common::Icon::Menu;
            if ui_common::tool_button(ui, menu, true, if open { "Collapse navigation" } else { "Expand navigation" })
                .clicked()
            {
                self.nav_open = !open;
            }
            ui.add_space(4.0);
            let mut item = |ui: &mut egui::Ui, t: Tab| {
                let label = if open { t.label() } else { "" };
                let r = ui_common::nav_item(ui, Some(t.icon()), label, self.tab == t);
                let r = if open { r } else { r.on_hover_text(t.label()) };
                if r.clicked() {
                    self.tab = t;
                }
            };
            for t in Tab::ALL.into_iter().filter(|t| *t != Tab::Settings) {
                item(ui, t);
            }
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                ui.add_space(6.0);
                item(ui, Tab::Settings);
            });
        });
    }

    /// The page title on the left, commands on the right.
    fn header(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("header").show_separator_line(false).show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui_common::page_title(ui, self.tab.label());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut ms = INTERVAL_MS.load(Ordering::Relaxed);
                    let label = match ms {
                        0 => "Paused",
                        500 => "0.5 s",
                        1000 => "1 s",
                        _ => "2 s",
                    };
                    egui::ComboBox::from_id_salt("interval")
                        .selected_text(ui_common::Icon::Stopwatch.with(label))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut ms, 500, "High (0.5 s)");
                            ui.selectable_value(&mut ms, 1000, "Normal (1 s)");
                            ui.selectable_value(&mut ms, 2000, "Low (2 s)");
                            ui.selectable_value(&mut ms, 0, "Paused");
                        })
                        .response
                        .on_hover_text("Real time update speed");
                    if ms != INTERVAL_MS.load(Ordering::Relaxed) {
                        INTERVAL_MS.store(ms, Ordering::Relaxed);
                        self.settings.interval_ms = ms;
                        self.save_settings();
                    }
                    if !self.elevated
                        && ui
                            .button(ui_common::Icon::Shield.with("Run as administrator"))
                            .on_hover_text(
                                "Needed to end elevated processes, control services and edit machine-wide startup apps",
                            )
                            .clicked()
                    {
                        match taskmgr_sys::restart_elevated() {
                            Ok(()) => std::process::exit(0),
                            Err(e) => self.status.set(e.to_string()),
                        }
                    }
                    if ui.button(ui_common::Icon::NewWindow.with("Run new task")).clicked() {
                        self.status.open(Dialog::RunTask { command: String::new(), admin: false });
                    }
                });
            });
            ui.add_space(4.0);
        });
    }
}

impl eframe::App for App {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        taskmgr_sys::shutdown();
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.drain();
        if !self.tray
            && let Ok(h) = raw_window_handle::HasWindowHandle::window_handle(frame)
        {
            self.tray = true; // one attempt
            if let raw_window_handle::RawWindowHandle::Win32(w) = h.as_raw() {
                taskmgr_sys::tray::install(w.hwnd.get());
            }
        }
        if self.status.used.swap(false, Ordering::Relaxed) && self.settings.minimize_on_use {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        if self.settings.hide_when_minimized && ui.ctx().input(|i| i.viewport().minimized == Some(true)) {
            taskmgr_sys::tray::hide_window();
        }
        if self.first_data_logged && std::env::var_os("OXI_TASKMGR_TIMING").is_some() {
            self.frames += 1;
            self.frame_cpu += frame.info().cpu_usage.unwrap_or(0.0);
            if self.frames_since.elapsed() >= Duration::from_secs(5) {
                eprintln!(
                    "ui: {} frames in {:.1} s, {:.1} ms app time each",
                    self.frames,
                    self.frames_since.elapsed().as_secs_f64(),
                    self.frame_cpu * 1000.0 / self.frames as f32
                );
                (self.frames, self.frame_cpu, self.frames_since) = (0, 0.0, Instant::now());
            }
        }
        let snap = self.snapshot.clone();
        // Ctrl+Tab / Ctrl+Shift+Tab move between pages, as in Task Manager.
        let (next, prev) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::CTRL, egui::Key::Tab),
                i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::Tab),
            )
        });
        if next || prev {
            let n = Tab::ALL.len();
            let i = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
            self.tab = Tab::ALL[if next { (i + 1) % n } else { (i + n - 1) % n }];
        }

        // Summary view: the Performance list alone, as in Task Manager.
        let summary = self.tab == Tab::Performance && self.performance.summary;
        if !summary {
            self.nav(ui);
            self.header(ui);
        }
        if let Some(pid) = self.status.goto_details.lock().ok().and_then(|mut g| g.take()) {
            self.tab = Tab::Details;
            self.details.select(pid, &snap);
        }
        if let Some(pid) = self.status.goto_services.lock().ok().and_then(|mut g| g.take()) {
            self.tab = Tab::Services;
            self.services.show_pid(pid);
        }
        self.dialogs(ui.ctx());
        WANT_SERVICES.store(self.tab == Tab::Services, Ordering::Relaxed);
        WANT_SESSIONS.store(self.tab == Tab::Users, Ordering::Relaxed);

        if !summary {
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
        }

        let content = ui_common::content_frame(ui.style());
        egui::CentralPanel::default().frame(content).show(ui, |ui| match self.tab {
            Tab::Processes => self.processes.show(ui, &snap),
            Tab::Performance => self.performance.show(ui, &snap),
            Tab::AppHistory => self.history.show(ui, self.elevated),
            Tab::Startup => self.startup.show(ui),
            Tab::Users => self.users.show(ui, &snap),
            Tab::Details => {
                if self.details.show(ui, &snap) {
                    self.settings.details_columns = details::columns_text(&self.details.columns);
                    self.save_settings();
                }
            }
            Tab::Services => self.services.show(ui, &snap),
            Tab::Settings => {
                let tabs: Vec<&str> = Tab::ALL.iter().filter(|t| **t != Tab::Settings).map(|t| t.label()).collect();
                let before = self.settings.clone();
                if settings::show(ui, &mut self.settings, &tabs, &self.status, &mut self.replaces_taskmgr) {
                    let s = &self.settings;
                    if s.theme != before.theme {
                        s.apply_theme(ui.ctx());
                    }
                    if s.always_on_top != before.always_on_top {
                        s.apply_on_top(ui.ctx());
                    }
                    INTERVAL_MS.store(s.interval_ms, Ordering::Relaxed);
                    self.processes.set_group(s.group_by_type);
                    self.processes.percent = s.values_pct;
                    self.save_settings();
                }
            }
        });
    }
}

fn main() -> eframe::Result {
    // OpenGL where there is a GPU driver, wgpu (WARP) otherwise; see `ui_common::choose_renderer`.
    let (renderer, auto) = ui_common::choose_renderer();
    let r = run(renderer);
    // Only OpenGL start-up failures: an error from the app itself would fail the same way again.
    let gl_failed =
        matches!(r, Err(eframe::Error::Glutin(_) | eframe::Error::NoGlutinConfigs(..) | eframe::Error::OpenGL(_)));
    if gl_failed && auto && renderer == ui_common::Renderer::Glow {
        eprintln!("OpenGL failed ({r:?}); starting again with wgpu");
        if ui_common::relaunch_with(ui_common::Renderer::Wgpu).is_ok() {
            return Ok(());
        }
    }
    r
}

fn run(renderer: ui_common::Renderer) -> eframe::Result {
    let mut options = eframe::NativeOptions {
        renderer: match renderer {
            ui_common::Renderer::Glow => eframe::Renderer::Glow,
            ui_common::Renderer::Wgpu => eframe::Renderer::Wgpu,
        },
        viewport: egui::ViewportBuilder::default()
            .with_title("Task Manager (oxidows)")
            .with_icon(Arc::new(
                eframe::icon_data::from_png_bytes(ui_common::app_icon::TASKMGR_PNG).expect("embedded icon is a PNG"),
            ))
            .with_inner_size([1000.0, 660.0])
            .with_min_inner_size([560.0, 360.0]),
        ..Default::default()
    };
    // OXI_PRESENT=immediate|mailbox|fifo|novsync: tuning knob for software rendering (WARP).
    let present = match std::env::var("OXI_PRESENT").as_deref() {
        Ok("immediate") => Some(eframe::wgpu::PresentMode::Immediate),
        Ok("mailbox") => Some(eframe::wgpu::PresentMode::Mailbox),
        Ok("fifo") => Some(eframe::wgpu::PresentMode::Fifo),
        Ok("novsync") => Some(eframe::wgpu::PresentMode::AutoNoVsync),
        _ => None,
    };
    if let Some(mode) = present {
        options.wgpu_options.surface.present_mode = mode;
    }
    eframe::run_native("oxi-taskmgr", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
