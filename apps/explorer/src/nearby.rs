//! Nearby sharing: send and receive files with phones and PCs that run the free LocalSend app,
//! over the shared Wi-Fi or a Wi-Fi Direct network this PC opens. The protocol lives in the
//! `oxi-share` crate; this is the window, the incoming-request prompt and their settings.
//!
//! Only one Explorer process shares at a time (each window is a process): the first takes a lock
//! file, and the others show that sharing runs elsewhere and take over when it closes.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Color32, Layout, RichText, Ui};
use explorer_core::format_bytes;
use oxi_share::{Device, DeviceType, Direction, Share, Snapshot, State};
use ui_common::{Icon, Palette};

use crate::App;

#[derive(Default)]
pub struct Nearby {
    /// The window is shown.
    pub open: bool,
    share: Option<Share>,
    /// Held while this process shares.
    lock: Option<File>,
    /// Another Explorer window is sharing.
    elsewhere: bool,
    last_try: Option<Instant>,
    error: Option<String>,
    snap: Snapshot,
    hotspot: Option<explorer_sys::Hotspot>,
    hotspot_error: Option<String>,
    /// Items picked to send (toolbar Share), waiting for a device.
    pub queue: Vec<PathBuf>,
    /// Device whose "send text" box is open, and the text.
    text_for: Option<(String, String)>,
    probe_ip: String,
    firewall_ok: Option<bool>,
    editing_alias: Option<String>,
    /// Request already brought to the user's attention.
    attention_for: u64,
    /// On for this run without changing the setting (`OXI_EXPLORER_NEARBY=1`, for screenshots).
    pub force_on: bool,
    /// When the device list was last refreshed while the window was open.
    refreshed: Option<Instant>,
    focus_text: bool,
}

fn computer_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Windows PC".into())
}

impl Nearby {
    pub fn new() -> Self {
        let screenshot = std::env::var_os("OXI_EXPLORER_NEARBY").is_some();
        Self { open: screenshot, force_on: screenshot, ..Default::default() }
    }

    pub fn alias(app: &App) -> String {
        let a = app.persist.settings.share.alias.trim();
        if a.is_empty() { computer_name() } else { a.to_string() }
    }

    pub fn receive_dir(app: &App) -> PathBuf {
        app.persist.settings.share.dir.clone().unwrap_or_else(|| {
            explorer_sys::known_folders()
                .into_iter()
                .find(|(n, _)| *n == "Downloads")
                .map(|(_, p)| p)
                .or_else(|| std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from("."))
        })
    }

    fn stop(&mut self) {
        self.share = None;
        self.hotspot = None;
        self.lock = None;
        self.snap = Snapshot::default();
    }
}

/// Starts or stops the service to match the setting. Cheap when nothing changes.
fn sync(app: &mut App, ctx: &egui::Context) {
    let on = app.persist.settings.share.on || app.nearby.force_on;
    if !on {
        if app.nearby.share.is_some() || app.nearby.lock.is_some() {
            app.nearby.stop();
        }
        app.nearby.elsewhere = false;
        return;
    }
    if app.nearby.share.is_some() {
        return;
    }
    // Not started yet, or another window holds the lock: try again every few seconds.
    if app.nearby.last_try.is_some_and(|t| t.elapsed() < Duration::from_secs(3)) {
        ctx.request_repaint_after(Duration::from_secs(3));
        return;
    }
    app.nearby.last_try = Some(Instant::now());
    let Some(dir) = crate::settings::config_dir().map(|d| d.join("share")) else {
        app.nearby.error = Some("No settings folder".into());
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let lock = match File::options().create(true).truncate(false).write(true).open(dir.join("lock")) {
        Ok(f) => f,
        Err(e) => {
            app.nearby.error = Some(e.to_string());
            return;
        }
    };
    if lock.try_lock().is_err() {
        app.nearby.elsewhere = true;
        ctx.request_repaint_after(Duration::from_secs(3));
        return;
    }
    app.nearby.elsewhere = false;
    let cfg = oxi_share::Config {
        alias: Nearby::alias(app),
        device_model: Some(if cfg!(windows) { "Windows" } else { "PC" }.into()),
        port: oxi_share::DEFAULT_PORT,
        identity_dir: dir,
        receive_dir: Nearby::receive_dir(app),
        quick_save: app.persist.settings.share.quick_save,
        multicast: true,
    };
    let c = ctx.clone();
    match Share::start(cfg, Arc::new(move || c.request_repaint())) {
        Ok(s) => {
            app.nearby.share = Some(s);
            app.nearby.lock = Some(lock);
            app.nearby.error = None;
            app.nearby.firewall_ok = Some(explorer_sys::firewall_rule_exists());
        }
        Err(e) => app.nearby.error = Some(e.to_string()),
    }
}

/// Every frame: keeps the service in step, prompts for incoming transfers, draws the window.
pub fn show(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    sync(app, &ctx);
    if let Some(s) = &app.nearby.share {
        app.nearby.snap = s.snapshot();
    }
    incoming_prompt(app, &ctx);
    // Look for devices when the window opens, and now and then while it stays open; devices
    // that went away drop off the list.
    if !app.nearby.open {
        app.nearby.refreshed = None;
    } else if let Some(s) = &app.nearby.share {
        let due = app.nearby.refreshed.is_none_or(|t| t.elapsed() > Duration::from_secs(60));
        if due {
            app.nearby.refreshed = Some(Instant::now());
            s.refresh();
        }
        ctx.request_repaint_after(Duration::from_secs(60));
    }
    if app.nearby.open {
        let mut open = true;
        egui::Window::new("Nearby sharing")
            .open(&mut open)
            .default_width(380.0)
            .default_pos(ctx.content_rect().right_top() + egui::vec2(-400.0, 80.0))
            .resizable(true)
            .collapsible(false)
            .show(&ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(ctx.content_rect().height() - 140.0)
                    .show(ui, |ui| window(app, ui));
            });
        app.nearby.open = open;
    }
    // Progress bars move while something is in flight.
    if app.nearby.snap.transfers.iter().any(|t| !t.state.finished()) {
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

fn device_icon(t: DeviceType) -> Icon {
    match t {
        DeviceType::Mobile => Icon::Phone,
        DeviceType::Web | DeviceType::Server | DeviceType::Headless => Icon::Network,
        DeviceType::Desktop => Icon::ThisPc,
    }
}

fn incoming_prompt(app: &mut App, ctx: &egui::Context) {
    let Some(inc) = app.nearby.snap.incoming.clone() else { return };
    if app.nearby.attention_for != inc.id {
        app.nearby.attention_for = inc.id;
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
    }
    let mut answer: Option<bool> = None;
    egui::Modal::new(egui::Id::new("nearby-incoming")).show(ctx, |ui| {
        ui.set_width(420.0);
        let who = match &inc.device_model {
            Some(m) if !m.is_empty() => format!("{} ({m})", inc.alias),
            _ => inc.alias.clone(),
        };
        ui.horizontal(|ui| {
            ui.label(device_icon(inc.device_type).sized(28.0));
            ui.vertical(|ui| {
                ui.label(RichText::new(who).strong());
                ui.label(RichText::new(inc.ip.to_string()).weak());
            });
        });
        ui.add_space(8.0);
        if let Some(text) = &inc.message {
            ui.label("sent you a message:");
            ui.add_space(4.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                    ui.add(egui::Label::new(text.as_str()).selectable(true).wrap());
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Copy").clicked() {
                    ui.ctx().copy_text(text.clone());
                    answer = Some(true);
                }
                if is_link(text) && ui.button("Open link").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(text.trim()));
                    answer = Some(true);
                }
                if ui.button("Close").clicked() {
                    answer = Some(true);
                }
            });
            return;
        }
        let n = inc.files.len();
        ui.label(if n == 1 {
            format!("wants to send you a file ({})", format_bytes(inc.total_size()))
        } else {
            format!("wants to send you {n} files ({})", format_bytes(inc.total_size()))
        });
        ui.add_space(4.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                ui.set_width(ui.available_width());
                for f in inc.files.iter().take(200) {
                    ui.horizontal(|ui| {
                        ui.label(&f.file_name);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new(format_bytes(f.size)).weak());
                        });
                    });
                }
                if n > 200 {
                    ui.label(RichText::new(format!("and {} more", n - 200)).weak());
                }
            });
        });
        ui.add_space(4.0);
        ui.label(RichText::new(format!("Saved to {}", app.nearby.snap.receive_dir.display())).weak());
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(RichText::new("Accept").strong()).clicked() {
                answer = Some(true);
            }
            if ui.button("Decline").clicked() {
                answer = Some(false);
            }
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            answer = Some(false);
        }
    });
    if let (Some(a), Some(s)) = (answer, &app.nearby.share) {
        s.respond(inc.id, a);
        app.nearby.snap.incoming = None;
        if a && inc.message.is_none() {
            app.nearby.open = true;
        }
    }
}

fn is_link(t: &str) -> bool {
    let t = t.trim();
    (t.starts_with("https://") || t.starts_with("http://")) && !t.contains(char::is_whitespace)
}

fn window(app: &mut App, ui: &mut Ui) {
    let mut on = app.persist.settings.share.on || app.nearby.force_on;
    ui.horizontal(|ui| {
        if ui.checkbox(&mut on, RichText::new("Share with nearby devices").strong()).changed() {
            app.persist.settings.share.on = on;
            app.nearby.force_on = false;
            app.nearby.last_try = None;
        }
    });
    if !on {
        ui.add_space(4.0);
        ui.label(
            "Send and receive files with phones and computers that have the free LocalSend app \
             (Android, iPhone, Mac, Linux, Windows). They find this PC on the same Wi-Fi, or over \
             a Wi-Fi Direct connection when there is none.",
        );
        ui.add_space(4.0);
        ui.hyperlink_to("Get LocalSend for your phone", "https://localsend.org/download");
        return;
    }
    if app.nearby.elsewhere {
        ui.label("Sharing is running in another Explorer window.");
        return;
    }
    if let Some(e) = &app.nearby.error {
        ui.colored_label(Palette::of(ui).danger, format!("Could not start: {e}"));
        return;
    }
    let Some(share) = &app.nearby.share else {
        ui.spinner();
        return;
    };
    let snap = app.nearby.snap.clone();

    // Who we are and where we listen.
    ui.horizontal_wrapped(|ui| match app.nearby.editing_alias.as_mut() {
        Some(text) => {
            let r = ui.add(egui::TextEdit::singleline(text).desired_width(180.0));
            r.request_focus();
            if r.lost_focus() {
                let t = text.trim().to_string();
                app.nearby.editing_alias = None;
                if !t.is_empty() {
                    share.set_alias(t.clone());
                    app.persist.settings.share.alias = t;
                }
            }
        }
        None => {
            ui.label("Visible as");
            if ui.link(RichText::new(&snap.alias).strong()).on_hover_text("Rename").clicked() {
                app.nearby.editing_alias = Some(snap.alias.clone());
            }
        }
    });
    match &snap.listening {
        Some(Ok(port)) => {
            let ips: Vec<String> = oxi_share::local_ipv4().iter().map(|(_, ip, _)| ip.to_string()).collect();
            let text = if *port == oxi_share::DEFAULT_PORT {
                ips.join(", ")
            } else {
                format!("{} (port {port})", ips.join(", "))
            };
            ui.label(RichText::new(text).weak()).on_hover_text("This PC's addresses on the local networks");
        }
        Some(Err(e)) => {
            ui.colored_label(Palette::of(ui).danger, format!("Cannot receive: {e}"));
        }
        None => {}
    }
    if app.nearby.firewall_ok == Some(false) {
        ui.horizontal_wrapped(|ui| {
            ui.label(Icon::Warning.sized(14.0).color(Palette::of(ui).warning));
            ui.label(RichText::new("Windows Firewall may block phones on Wi-Fi Direct and public networks.").weak());
            if ui.button("Allow").on_hover_text("Adds an inbound rule for this app (asks for admin rights)").clicked() {
                match explorer_sys::allow_in_firewall() {
                    Ok(()) => app.nearby.firewall_ok = None,
                    Err(e) => app.status = Some(format!("Firewall: {e}")),
                }
            }
        });
    }
    if app.nearby.firewall_ok.is_none() && ui.ctx().cumulative_frame_nr().is_multiple_of(120) {
        app.nearby.firewall_ok = Some(explorer_sys::firewall_rule_exists());
    }
    ui.separator();

    // What to send.
    let selection = app.selected_paths();
    let items: Vec<PathBuf> = if app.nearby.queue.is_empty() { selection } else { app.nearby.queue.clone() };
    ui.horizontal_wrapped(|ui| {
        match items.as_slice() {
            [] => {
                ui.label(RichText::new("Select files in the folder, then pick a device. Or drag them onto it.").weak())
            }
            [one] => ui.label(format!("Send {}", name_of(one))),
            many => ui.label(format!("Send {} items", many.len())),
        };
        if !app.nearby.queue.is_empty() && ui_common::tool_button(ui, Icon::Close, true, "Clear").clicked() {
            app.nearby.queue.clear();
        }
    });
    ui.add_space(4.0);

    // Devices.
    ui.horizontal(|ui| {
        ui.label(RichText::new("Devices").strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui_common::tool_button(ui, Icon::Refresh, true, "Look again").clicked() {
                share.refresh();
            }
            if snap.scanning {
                ui.spinner();
            }
        });
    });
    if snap.devices.is_empty() {
        ui.label(
            RichText::new(
                "No devices yet. Open LocalSend on the phone, on the same Wi-Fi as this PC, \
                 or connect it with Wi-Fi Direct below.",
            )
            .weak(),
        );
    }
    let dragging = app.drag.as_ref().map(|d| d.paths.clone());
    let released = ui.input(|i| !i.pointer.primary_down());
    let mut send_to: Option<(Device, Vec<PathBuf>)> = None;
    let mut text_send: Option<(Device, String)> = None;
    for d in &snap.devices {
        let row = egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(device_icon(d.device_type).sized(22.0));
                ui.vertical(|ui| {
                    ui.label(RichText::new(&d.alias).strong());
                    let model = d.device_model.clone().unwrap_or_default();
                    ui.label(RichText::new(format!("{model}  {}", d.ip)).weak().small());
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui_common::tool_button(ui, Icon::Message, true, "Send text").clicked() {
                        app.nearby.text_for = Some((d.fingerprint.clone(), String::new()));
                        app.nearby.focus_text = true;
                    }
                    let tip = if items.is_empty() { "Select files first" } else { "Send the files" };
                    if ui
                        .add_enabled(!items.is_empty(), egui::Button::new(Icon::Send.with("Send")))
                        .on_hover_text(tip)
                        .clicked()
                    {
                        send_to = Some((d.clone(), items.clone()));
                    }
                });
            });
            if let Some((fp, text)) = app.nearby.text_for.as_mut()
                && *fp == d.fingerprint
            {
                ui.horizontal(|ui| {
                    let r = ui.add(egui::TextEdit::singleline(text).hint_text("Message or link").desired_width(220.0));
                    if std::mem::take(&mut app.nearby.focus_text) {
                        r.request_focus();
                    }
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (ui.button("Send").clicked() || enter) && !text.trim().is_empty() {
                        text_send = Some((d.clone(), text.clone()));
                    }
                });
            }
        });
        // Files dragged from the list and dropped on a device are sent to it.
        if let Some(paths) = &dragging
            && row.response.contains_pointer()
        {
            ui.painter().rect_stroke(
                row.response.rect,
                4.0,
                egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
                egui::StrokeKind::Inside,
            );
            if released {
                send_to = Some((d.clone(), paths.clone()));
                app.drag = None;
            }
        }
    }
    ui.collapsing("Add a device by address", |ui| {
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut app.nearby.probe_ip).hint_text("192.168.1.20").desired_width(140.0));
            if ui.button("Add").clicked()
                && let Ok(ip) = app.nearby.probe_ip.trim().parse()
            {
                share.probe(ip, oxi_share::DEFAULT_PORT);
            }
        });
    });
    if let Some((d, paths)) = send_to {
        share.send(&d, paths);
        app.nearby.queue.clear();
    }
    if let Some((d, text)) = text_send {
        share.send_text(&d, text);
        app.nearby.text_for = None;
    }
    ui.separator();
    hotspot_section(app, ui);
    ui.separator();
    transfers(app, ui, &snap);
    ui.separator();
    settings_section(app, ui, &snap);
}

fn name_of(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// SSID of the Wi-Fi Direct group: DIRECT- like every Wi-Fi Direct group; at most 32 bytes.
fn ssid_for(alias: &str) -> String {
    let clean: String = alias.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    let mut s = format!("DIRECT-oxi-{}", if clean.is_empty() { "PC" } else { &clean });
    s.truncate(32);
    s
}

/// The text of a Wi-Fi QR code (the format phone cameras join networks from).
fn wifi_qr_text(ssid: &str, pass: &str) -> String {
    let esc = |s: &str| {
        s.chars().fold(String::new(), |mut o, c| {
            if matches!(c, '\\' | ';' | ',' | ':' | '"') {
                o.push('\\');
            }
            o.push(c);
            o
        })
    };
    format!("WIFI:T:WPA;S:{};P:{};;", esc(ssid), esc(pass))
}

fn hotspot_section(app: &mut App, ui: &mut Ui) {
    egui::CollapsingHeader::new(Icon::Wifi.with("No shared Wi-Fi? Connect with Wi-Fi Direct"))
        .default_open(app.nearby.hotspot.is_some())
        .show(ui, |ui| {
            let status = app.nearby.hotspot.as_ref().map(|h| h.status());
            match status {
                None => {
                    ui.label(
                        "This PC opens its own Wi-Fi network. The phone joins it by scanning a code, \
                         and LocalSend finds this PC on it. The PC keeps its normal Wi-Fi.",
                    );
                    if let Some(e) = &app.nearby.hotspot_error {
                        ui.colored_label(Palette::of(ui).danger, e);
                    }
                    if ui.button("Start Wi-Fi Direct").clicked() {
                        if app.persist.settings.share.hotspot_pass.len() < 8 {
                            app.persist.settings.share.hotspot_pass = oxi_share::random_password(8);
                        }
                        let ssid = ssid_for(&Nearby::alias(app));
                        match explorer_sys::Hotspot::start(&ssid, &app.persist.settings.share.hotspot_pass) {
                            Ok(h) => {
                                app.nearby.hotspot = Some(h);
                                app.nearby.hotspot_error = None;
                            }
                            Err(e) => app.nearby.hotspot_error = Some(e.to_string()),
                        }
                    }
                }
                Some(explorer_sys::HotspotStatus::Failed(e)) => {
                    app.nearby.hotspot = None;
                    app.nearby.hotspot_error = Some(format!("Wi-Fi Direct could not start: {e}"));
                }
                Some(s) => {
                    let ssid = ssid_for(&Nearby::alias(app));
                    let pass = app.persist.settings.share.hotspot_pass.clone();
                    if s == explorer_sys::HotspotStatus::Started {
                        ui.label(
                            "On the phone: scan this code with the camera, join the network, then open LocalSend.",
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            qr(ui, &wifi_qr_text(&ssid, &pass), 168.0);
                            ui.vertical(|ui| {
                                ui.label(RichText::new("Network").weak());
                                ui.add(egui::Label::new(RichText::new(&ssid).strong()).selectable(true));
                                ui.label(RichText::new("Password").weak());
                                ui.add(egui::Label::new(RichText::new(&pass).strong().monospace()).selectable(true));
                            });
                        });
                    } else {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Starting Wi-Fi Direct…");
                        });
                        ui.ctx().request_repaint_after(Duration::from_millis(300));
                    }
                    if ui.button("Stop Wi-Fi Direct").clicked() {
                        app.nearby.hotspot = None;
                    }
                }
            }
        });
}

/// Draws a QR code, dark on white whatever the theme (cameras need the contrast).
fn qr(ui: &mut Ui, text: &str, size: f32) {
    let Ok(code) = qrcode::QrCode::new(text.as_bytes()) else { return };
    let w = code.width();
    let colors = code.to_colors();
    let quiet = 2;
    let cells = w + 2 * quiet;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 4.0, Color32::WHITE);
    let cell = size / cells as f32;
    for (i, c) in colors.iter().enumerate() {
        if *c == qrcode::Color::Dark {
            let (x, y) = ((i % w + quiet) as f32, (i / w + quiet) as f32);
            let min = rect.min + egui::vec2(x * cell, y * cell);
            p.rect_filled(egui::Rect::from_min_size(min, egui::vec2(cell + 0.5, cell + 0.5)), 0.0, Color32::BLACK);
        }
    }
}

fn transfers(app: &mut App, ui: &mut Ui, snap: &Snapshot) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Transfers").strong());
        if snap.transfers.iter().any(|t| t.state.finished())
            && let Some(s) = &app.nearby.share
        {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Clear").clicked() {
                    s.clear_finished();
                }
            });
        }
    });
    if snap.transfers.is_empty() {
        ui.label(RichText::new("Nothing sent or received yet.").weak());
    }
    let mut open_folder: Option<PathBuf> = None;
    for t in snap.transfers.iter().rev() {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let (arrow, verb) = match t.direction {
                Direction::Receive => (Icon::Downloads, "From"),
                Direction::Send => (Icon::Send, "To"),
            };
            let what = match (&t.message, t.files.as_slice()) {
                (Some(m), _) => format!("\u{201C}{}\u{201D}", m.chars().take(60).collect::<String>()),
                (None, [one]) => name_of(Path::new(&one.name)),
                (None, many) => format!("{} files", many.len()),
            };
            ui.horizontal(|ui| {
                ui.label(arrow.with(format!("{verb} {}", t.peer)));
                ui.label(RichText::new(what).weak());
            });
            let (total, done) = (t.total(), t.done());
            match &t.state {
                State::Waiting => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Waiting for the other device to accept…");
                    });
                }
                State::Running => {
                    let f = if total == 0 { 0.0 } else { done as f32 / total as f32 };
                    let secs = t.started.elapsed().as_secs_f64().max(0.001);
                    let speed = (done as f64 / secs) as u64;
                    ui.add(egui::ProgressBar::new(f).text(format!(
                        "{} of {}  ({}/s)",
                        format_bytes(done),
                        format_bytes(total),
                        format_bytes(speed)
                    )));
                }
                State::Done => {
                    let took = t.finished.map(|f| f.duration_since(t.started)).unwrap_or_default();
                    let mut text = if t.message.is_some() {
                        "Delivered".to_string()
                    } else {
                        format!("Done, {}", format_bytes(total))
                    };
                    if took > Duration::from_secs(1) && total > 0 {
                        text += &format!(" in {:.0} s", took.as_secs_f64());
                    }
                    ui.label(RichText::new(text).color(Palette::of(ui).success));
                }
                State::Declined => {
                    ui.label(RichText::new("Declined").weak());
                }
                State::Cancelled => {
                    ui.label(RichText::new("Cancelled").weak());
                }
                State::Failed(e) => {
                    ui.colored_label(Palette::of(ui).danger, e);
                }
            }
            ui.horizontal(|ui| {
                if !t.state.finished()
                    && let Some(s) = &app.nearby.share
                    && ui.small_button("Cancel").clicked()
                {
                    s.cancel(t.id);
                }
                if t.direction == Direction::Receive {
                    if let Some(m) = &t.message
                        && ui.small_button("Copy").clicked()
                    {
                        ui.ctx().copy_text(m.clone());
                    }
                    let first = t.files.iter().find_map(|f| f.path.clone());
                    if let Some(p) = first
                        && t.state == State::Done
                        && ui.small_button("Show in folder").clicked()
                    {
                        open_folder = Some(p);
                    }
                }
            });
        });
    }
    if let Some(p) = open_folder {
        // The top folder the transfer created in the receive folder, or the file's folder.
        let root = Nearby::receive_dir(app);
        let top = p.strip_prefix(&root).ok().and_then(|r| r.components().next()).map(|c| root.join(c));
        let (dir, select) = match top {
            Some(t) if t.is_dir() => (root, t.file_name().map(|n| n.to_string_lossy().into_owned())),
            _ => {
                (p.parent().map(PathBuf::from).unwrap_or(root), p.file_name().map(|n| n.to_string_lossy().into_owned()))
            }
        };
        app.navigate(dir);
        app.pending_select = select;
    }
}

fn settings_section(app: &mut App, ui: &mut Ui, snap: &Snapshot) {
    ui.collapsing("Settings", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label("Save received files to");
            if ui.link(snap.receive_dir.display().to_string()).on_hover_text("Open").clicked() {
                app.navigate(snap.receive_dir.clone());
            }
        });
        ui.horizontal(|ui| {
            let here = app.cur.clone();
            let real_folder = here.is_dir();
            if ui
                .add_enabled(real_folder, egui::Button::new("Use the current folder"))
                .on_hover_text(here.display().to_string())
                .clicked()
            {
                app.persist.settings.share.dir = Some(here.clone());
                if let Some(s) = &app.nearby.share {
                    s.set_receive_dir(here);
                }
            }
            if app.persist.settings.share.dir.is_some() && ui.button("Use Downloads").clicked() {
                app.persist.settings.share.dir = None;
                let d = Nearby::receive_dir(app);
                if let Some(s) = &app.nearby.share {
                    s.set_receive_dir(d);
                }
            }
        });
        let mut quick = app.persist.settings.share.quick_save;
        if ui
            .checkbox(&mut quick, "Accept transfers without asking")
            .on_hover_text("Anyone on the network running LocalSend could then put files on this PC")
            .changed()
        {
            app.persist.settings.share.quick_save = quick;
            if let Some(s) = &app.nearby.share {
                s.set_quick_save(quick);
            }
        }
        ui.label(RichText::new(format!("Device id {}…", snap.fingerprint.get(..16).unwrap_or(""))).weak().small());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wifi_code_escapes_and_ssid_fits() {
        assert_eq!(wifi_qr_text("DIRECT-oxi-PC", "a;b\\c"), "WIFI:T:WPA;S:DIRECT-oxi-PC;P:a\\;b\\\\c;;");
        assert_eq!(ssid_for("Phước's very long computer name here"), "DIRECT-oxi-Phcsverylongcomputern");
        assert!(ssid_for("Phước's very long computer name here").len() <= 32);
        assert_eq!(ssid_for("Máy"), "DIRECT-oxi-My");
        assert_eq!(ssid_for("ữ"), "DIRECT-oxi-PC");
    }
}
