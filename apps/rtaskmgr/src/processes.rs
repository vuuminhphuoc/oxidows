//! Processes tab: a virtualised table (only visible rows are laid out), sort by any column,
//! search by name or PID, End task from a button, the Delete key or the right-click menu.

use egui::{Color32, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use tm_core::{ProcessRow, SortKey, Snapshot, format_bytes, sorted_view};

#[derive(Default)]
pub struct State {
    filter: String,
    sort: SortKey,
    ascending: bool,
    /// Selected (pid, name): the name guards against ending a reused PID.
    selected: Option<(u32, String)>,
    pub message: Option<String>,
}

/// Task Manager style heat: transparent at 0, amber at the top of the scale.
fn heat(ui: &Ui, fraction: f32) -> Color32 {
    let base = if ui.visuals().dark_mode { Color32::from_rgb(180, 120, 0) } else { Color32::from_rgb(255, 200, 40) };
    base.gamma_multiply((0.08 + fraction.clamp(0.0, 1.0) * 0.7).min(0.78))
}

impl State {
    pub fn show(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let rows = &snap.processes;
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("name or PID").desired_width(220.0));
            let can_end = self.selected.is_some();
            if ui.add_enabled(can_end, egui::Button::new("End task")).clicked() {
                self.end_selected(rows);
            }
        });
        if self.selected.is_some() && ui.input(|i| i.key_pressed(egui::Key::Delete)) {
            self.end_selected(rows);
        }
        ui.add_space(4.0);

        let view = sorted_view(rows, &self.filter, self.sort, !self.ascending);
        let mem_total = snap.system.mem_total.max(1) as f32;
        let row_h = 22.0;

        let mut clicked_sort = None;
        let mut end_now = None;
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().at_least(180.0).clip(true))
            .column(Column::exact(70.0))
            .column(Column::exact(70.0))
            .column(Column::exact(100.0))
            .column(Column::exact(100.0))
            .column(Column::exact(70.0))
            .header(row_h, |mut h| {
                for (key, title) in [
                    (SortKey::Name, "Name"),
                    (SortKey::Pid, "PID"),
                    (SortKey::Cpu, "CPU"),
                    (SortKey::Memory, "Memory"),
                    (SortKey::Disk, "Disk"),
                    (SortKey::Threads, "Threads"),
                ] {
                    h.col(|ui| {
                        let arrow = if self.sort == key { if self.ascending { " ^" } else { " v" } } else { "" };
                        if ui.button(RichText::new(format!("{title}{arrow}")).strong()).clicked() {
                            clicked_sort = Some(key);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(row_h, view.len(), |mut row| {
                    let p: &ProcessRow = &rows[view[row.index()]];
                    let is_sel = self.selected.as_ref().is_some_and(|(pid, _)| *pid == p.pid);
                    row.set_selected(is_sel);
                    row.col(|ui| {
                        ui.label(&p.name);
                    });
                    row.col(|ui| {
                        ui.label(p.pid.to_string());
                    });
                    row.col(|ui| {
                        ui.painter().rect_filled(ui.max_rect(), 0.0, heat(ui, p.cpu_pct / 100.0));
                        ui.label(format!("{:.1}%", p.cpu_pct));
                    });
                    row.col(|ui| {
                        ui.painter().rect_filled(ui.max_rect(), 0.0, heat(ui, p.memory as f32 / mem_total * 8.0));
                        ui.label(format_bytes(p.memory as f64));
                    });
                    row.col(|ui| {
                        ui.label(format!("{}/s", format_bytes(p.disk_bps)));
                    });
                    row.col(|ui| {
                        ui.label(p.threads.to_string());
                    });
                    let resp = row.response();
                    if resp.clicked() {
                        self.selected = Some((p.pid, p.name.clone()));
                    }
                    resp.context_menu(|ui| {
                        ui.label(RichText::new(format!("{} ({})", p.name, p.pid)).weak());
                        if ui.button("End task").clicked() {
                            end_now = Some((p.pid, p.name.clone()));
                            ui.close();
                        }
                    });
                });
            });

        if let Some(key) = clicked_sort {
            if self.sort == key {
                self.ascending = !self.ascending;
            } else {
                self.sort = key;
                // Names read best A→Z; numbers biggest first.
                self.ascending = matches!(key, SortKey::Name | SortKey::Pid);
            }
        }
        if let Some(target) = end_now {
            self.selected = Some(target);
            self.end_selected(rows);
        }
    }

    fn end_selected(&mut self, rows: &[ProcessRow]) {
        let Some((pid, name)) = self.selected.take() else { return };
        // Only end it if that PID still belongs to the same program.
        if !rows.iter().any(|r| r.pid == pid && r.name == name) {
            self.message = Some(format!("{name} ({pid}) has already exited"));
            return;
        }
        self.message = Some(match tm_sys::end_process(pid) {
            Ok(()) => format!("Ended {name} ({pid})"),
            Err(e) => format!("Could not end {name} ({pid}): {e}"),
        });
    }
}
