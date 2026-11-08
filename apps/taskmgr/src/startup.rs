//! Startup apps tab: Run keys and Startup folders; enable / disable like the built-in tool.

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use std::collections::HashMap;

use taskmgr_core::{Snapshot, StartupItem, format_bytes, startup_impact};

use crate::Status;

pub struct State {
    items: Option<Vec<StartupItem>>,
    selected: Option<usize>,
    bios_ms: Option<u32>,
    status: Status,
    /// CPU time (100 ns) and disk I/O bytes per startup exe (lower-case path), from the first
    /// time a matching process that began soon after boot was seen; read again on Refresh.
    measured: HashMap<String, (u64, u64)>,
}

/// Processes that started within this long after boot count as started by sign-in.
const STARTUP_WINDOW_100NS: u64 = 5 * 60 * 10_000_000;

fn now_filetime() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| (d.as_nanos() / 100) as u64 + 116_444_736_000_000_000)
}

impl State {
    pub fn new(status: Status) -> Self {
        Self { items: None, selected: None, bios_ms: taskmgr_sys::last_bios_ms(), status, measured: HashMap::new() }
    }

    /// Records the startup cost of processes that sign-in started. Windows measures the boot itself;
    /// here the process's own totals stand in, so the earlier this runs after sign-in, the closer.
    fn measure(&mut self, snap: &Snapshot) {
        let boot = now_filetime().saturating_sub(snap.system.uptime.as_nanos() as u64 / 100);
        for p in &snap.processes {
            if p.info.path.is_empty() || p.start_time.saturating_sub(boot) > STARTUP_WINDOW_100NS {
                continue;
            }
            let e = self.measured.entry(p.info.path.to_lowercase()).or_insert((0, 0));
            e.0 = e.0.max(p.cpu_time);
            e.1 = e.1.max(p.io_read_bytes + p.io_write_bytes);
        }
    }

    pub fn show(&mut self, ui: &mut Ui, snap: &Snapshot) {
        if self.measured.is_empty() || self.items.is_none() {
            self.measure(snap);
        }
        // Registry reads take well under a millisecond: load on first view, reload on demand.
        let items = self.items.get_or_insert_with(taskmgr_sys::startup_items);
        let cost: Vec<Option<(u64, u64)>> = items
            .iter()
            .map(|it| self.measured.get(&taskmgr_sys::startup_exe(&it.command).to_lowercase()).copied())
            .collect();
        let mut toggle = None;
        let mut reload = false;
        ui.horizontal(|ui| {
            let sel = self.selected.and_then(|i| items.get(i));
            if ui.add_enabled(sel.is_some_and(|s| !s.enabled), egui::Button::new("Enable")).clicked() {
                toggle = self.selected.map(|i| (i, true));
            }
            if ui.add_enabled(sel.is_some_and(|s| s.enabled), egui::Button::new("Disable")).clicked() {
                toggle = self.selected.map(|i| (i, false));
            }
            if ui.button("Refresh").clicked() {
                reload = true;
            }
            ui.label(RichText::new(format!("{} startup apps", items.len())).weak());
            if let Some(ms) = self.bios_ms {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("Last BIOS time: {:.1} seconds", ms as f32 / 1000.0));
                });
            }
        });
        ui.add_space(4.0);
        if items.is_empty() {
            ui.label("No startup apps found.");
        }
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(200.0).clip(true))
            .column(Column::initial(160.0).clip(true))
            .column(Column::exact(80.0))
            .columns(Column::exact(100.0), 3)
            .column(Column::initial(300.0).clip(true))
            .column(Column::remainder().clip(true))
            .header(20.0, |mut h| {
                for t in [
                    "Name",
                    "Publisher",
                    "Status",
                    "Startup impact",
                    "CPU at startup",
                    "Disk I/O at startup",
                    "Command",
                    "Registered in",
                ] {
                    h.col(|ui| {
                        ui.label(RichText::new(t).strong());
                    });
                }
            })
            .body(|body| {
                body.rows(20.0, items.len(), |mut row| {
                    let i = row.index();
                    let it = &items[i];
                    row.set_selected(self.selected == Some(i));
                    row.col(|ui| {
                        ui.label(&it.name);
                    });
                    row.col(|ui| {
                        ui.label(&it.publisher);
                    });
                    row.col(|ui| {
                        ui.label(if it.enabled { "Enabled" } else { "Disabled" });
                    });
                    let c = cost[i];
                    row.col(|ui| {
                        ui.label(match c {
                            _ if !it.enabled => "None",
                            Some((cpu, io)) => startup_impact(cpu, io),
                            None => "Not measured",
                        })
                        .on_hover_text("From the app's own CPU and disk use since it started at sign-in");
                    });
                    row.col(|ui| {
                        ui.label(c.map_or(String::new(), |(cpu, _)| format!("{} ms", cpu / 10_000)));
                    });
                    row.col(|ui| {
                        ui.label(c.map_or(String::new(), |(_, io)| format_bytes(io as f64)));
                    });
                    row.col(|ui| {
                        ui.label(&it.command).on_hover_text(&it.command);
                    });
                    row.col(|ui| {
                        ui.label(&it.location);
                    });
                    let resp = row.response();
                    if resp.clicked() {
                        self.selected = Some(i);
                    }
                    resp.context_menu(|ui| {
                        let label = if it.enabled { "Disable" } else { "Enable" };
                        if ui.button(label).clicked() {
                            toggle = Some((i, !it.enabled));
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Open file location").clicked() {
                            let _ = taskmgr_sys::open_file_location(&taskmgr_sys::startup_exe(&it.command));
                            ui.close();
                        }
                        if ui.button("Search online").clicked() {
                            let _ = taskmgr_sys::search_online(&it.name);
                            ui.close();
                        }
                        if ui.button("Properties").clicked() {
                            let _ = taskmgr_sys::file_properties(&taskmgr_sys::startup_exe(&it.command));
                            ui.close();
                        }
                    });
                });
            });
        if let Some((i, on)) = toggle
            && let Some(it) = items.get(i)
        {
            match taskmgr_sys::set_startup_enabled(it, on) {
                Ok(()) => {
                    self.status.set(format!("{} {}", if on { "Enabled" } else { "Disabled" }, it.name));
                    reload = true;
                }
                Err(e) if !it.per_user => {
                    self.status.set(format!("{}: needs administrator ({e})", it.name));
                }
                Err(e) => self.status.set(format!("Could not change {}: {e}", it.name)),
            }
        }
        if reload {
            self.items = None;
            self.measured.clear();
        }
    }
}
