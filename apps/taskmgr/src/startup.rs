//! Startup apps tab: Run keys and Startup folders; enable / disable like the built-in tool.

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use taskmgr_core::StartupItem;

use crate::Status;

pub struct State {
    items: Option<Vec<StartupItem>>,
    selected: Option<usize>,
    bios_ms: Option<u32>,
    status: Status,
}

impl State {
    pub fn new(status: Status) -> Self {
        Self { items: None, selected: None, bios_ms: taskmgr_sys::last_bios_ms(), status }
    }

    pub fn show(&mut self, ui: &mut Ui) {
        // Registry reads take well under a millisecond: load on first view, reload on demand.
        let items = self.items.get_or_insert_with(taskmgr_sys::startup_items);
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
            .column(Column::initial(300.0).clip(true))
            .column(Column::remainder().clip(true))
            .header(20.0, |mut h| {
                for t in ["Name", "Publisher", "Status", "Command", "Registered in"] {
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
        }
    }
}
