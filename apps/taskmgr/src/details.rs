//! Details tab: every process with user, session, priority, handles and full path.

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use taskmgr_core::{ProcessRow, Snapshot, format_bytes, priority_label};

use crate::Status;
use crate::processes::{end_task, process_menu};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Name,
    Pid,
    User,
    Cpu,
    Memory,
    Handles,
}

pub struct State {
    filter: String,
    sort: Sort,
    descending: bool,
    selected: Option<(u32, String)>,
    status: Status,
}

impl State {
    pub fn new(status: Status) -> Self {
        Self { filter: String::new(), sort: Sort::Name, descending: false, selected: None, status }
    }

    pub fn show(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let rows = &snap.processes;
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("name, PID, user or path").desired_width(260.0));
            if ui.add_enabled(self.selected.is_some(), egui::Button::new("End task")).clicked()
                && let Some((pid, name)) = self.selected.take()
            {
                end_task(&self.status, rows, pid, &name);
            }
        });
        ui.add_space(4.0);

        let f = self.filter.trim().to_lowercase();
        let mut view: Vec<&ProcessRow> = rows
            .iter()
            .filter(|p| {
                f.is_empty()
                    || p.pid.to_string() == f
                    || p.name.to_lowercase().contains(&f)
                    || p.info.user.to_lowercase().contains(&f)
                    || p.info.path.to_lowercase().contains(&f)
            })
            .collect();
        view.sort_by(|a, b| {
            let o = match self.sort {
                Sort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                Sort::Pid => a.pid.cmp(&b.pid),
                Sort::User => a.info.user.cmp(&b.info.user),
                Sort::Cpu => a.cpu_pct.total_cmp(&b.cpu_pct),
                Sort::Memory => a.memory.cmp(&b.memory),
                Sort::Handles => a.handles.cmp(&b.handles),
            };
            let o = if self.descending { o.reverse() } else { o };
            o.then(a.pid.cmp(&b.pid))
        });

        let row_h = 20.0;
        let mut clicked = None;
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(170.0).clip(true))
            .column(Column::exact(60.0))
            .column(Column::initial(150.0).clip(true))
            .column(Column::exact(55.0))
            .column(Column::exact(55.0))
            .column(Column::exact(85.0))
            .column(Column::exact(60.0))
            .column(Column::exact(60.0))
            .column(Column::exact(95.0))
            .column(Column::remainder().clip(true))
            .header(row_h, |mut h| {
                let cols: [(Option<Sort>, &str); 10] = [
                    (Some(Sort::Name), "Name"),
                    (Some(Sort::Pid), "PID"),
                    (Some(Sort::User), "User name"),
                    (None, "Session"),
                    (Some(Sort::Cpu), "CPU"),
                    (Some(Sort::Memory), "Memory"),
                    (None, "Threads"),
                    (Some(Sort::Handles), "Handles"),
                    (None, "Priority"),
                    (None, "Path"),
                ];
                for (key, title) in cols {
                    h.col(|ui| match key {
                        Some(k) => {
                            let arrow = if self.sort == k { if self.descending { " v" } else { " ^" } } else { "" };
                            if ui.button(RichText::new(format!("{title}{arrow}")).strong()).clicked() {
                                clicked = Some(k);
                            }
                        }
                        None => {
                            ui.label(RichText::new(title).strong());
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(row_h, view.len(), |mut row| {
                    let p = view[row.index()];
                    row.set_selected(self.selected.as_ref().is_some_and(|(pid, _)| *pid == p.pid));
                    let dash = |s: &str| if s.is_empty() { "-".to_string() } else { s.to_string() };
                    row.col(|ui| {
                        ui.label(&p.name);
                    });
                    row.col(|ui| {
                        ui.label(p.pid.to_string());
                    });
                    row.col(|ui| {
                        ui.label(dash(&p.info.user));
                    });
                    row.col(|ui| {
                        ui.label(p.session_id.to_string());
                    });
                    row.col(|ui| {
                        ui.label(format!("{:.0}", p.cpu_pct));
                    });
                    row.col(|ui| {
                        ui.label(format_bytes(p.memory as f64));
                    });
                    row.col(|ui| {
                        ui.label(p.threads.to_string());
                    });
                    row.col(|ui| {
                        ui.label(p.handles.to_string());
                    });
                    row.col(|ui| {
                        ui.label(priority_label(p.base_priority));
                    });
                    row.col(|ui| {
                        ui.label(dash(&p.info.path)).on_hover_text(dash(&p.info.command_line));
                    });
                    let resp = row.response();
                    if resp.clicked() {
                        self.selected = Some((p.pid, p.name.clone()));
                    }
                    resp.context_menu(|ui| process_menu(ui, &self.status, rows, p));
                });
            });
        if let Some(k) = clicked {
            if self.sort == k {
                self.descending = !self.descending;
            } else {
                self.sort = k;
                self.descending = matches!(k, Sort::Cpu | Sort::Memory | Sort::Handles);
            }
        }
    }
}
