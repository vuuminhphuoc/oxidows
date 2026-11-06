//! Users tab: CPU, memory and disk per signed-in session; click a user to see their processes;
//! Disconnect and Sign off (both ask first).

use std::collections::HashSet;

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use taskmgr_core::{Snapshot, display_name, format_bits_per_sec, format_bytes, users_view};

use crate::{ConfirmAction, Dialog, Status};

fn net_cell(ui: &mut Ui, on: bool, bps: f64) {
    if on {
        ui.label(format_bits_per_sec(bps));
    } else {
        ui.label(RichText::new("-").weak()).on_hover_text(crate::processes::NET_HINT);
    }
}

pub struct State {
    expanded: HashSet<u32>,
    selected: Option<u32>,
    status: Status,
}

enum Line {
    User(usize),
    Proc(usize),
}

impl State {
    pub fn new(status: Status) -> Self {
        Self { expanded: HashSet::new(), selected: None, status }
    }

    pub fn show(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let sessions = snap.sessions.as_deref().map(Vec::as_slice).unwrap_or(&[]);
        let mut users = users_view(&snap.processes, sessions);
        // Session 0 (services) is not a person; Windows hides it here too, unless it is all we have.
        if users.len() > 1 {
            users.retain(|u| u.session_id != 0 || !u.state.is_empty());
        }
        let sel = self.selected.and_then(|id| users.iter().find(|u| u.session_id == id));
        ui.horizontal(|ui| {
            let can = sel.is_some_and(|u| !u.state.is_empty());
            if ui.add_enabled(can, egui::Button::new("Disconnect")).clicked()
                && let Some(u) = sel
            {
                self.status.open(Dialog::Confirm {
                    text: format!("Disconnect {}? Their apps keep running.", u.user),
                    action: ConfirmAction::Disconnect { session: u.session_id, user: u.user.clone() },
                });
            }
            if ui.add_enabled(can, egui::Button::new("Sign off")).clicked()
                && let Some(u) = sel
            {
                self.status.open(Dialog::Confirm {
                    text: format!("Sign off {}? Any unsaved data in their apps may be lost.", u.user),
                    action: ConfirmAction::SignOff { session: u.session_id, user: u.user.clone() },
                });
            }
        });
        ui.add_space(4.0);

        let mut lines = Vec::new();
        for (i, u) in users.iter().enumerate() {
            lines.push(Line::User(i));
            if self.expanded.contains(&u.session_id) {
                let mut procs: Vec<usize> =
                    (0..snap.processes.len()).filter(|&i| snap.processes[i].session_id == u.session_id).collect();
                procs.sort_by(|&a, &b| snap.processes[b].cpu_pct.total_cmp(&snap.processes[a].cpu_pct));
                lines.extend(procs.into_iter().map(Line::Proc));
            }
        }
        let mem_total = snap.system.mem_total.max(1) as f64;
        let net_on = snap.system.per_process_net;
        let mut toggle = None;
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(260.0).clip(true))
            .column(Column::exact(70.0))
            .column(Column::exact(100.0))
            .column(Column::exact(80.0))
            .column(Column::exact(70.0))
            .column(Column::exact(130.0))
            .column(Column::exact(90.0))
            .column(Column::remainder())
            .header(22.0, |mut h| {
                for t in ["User", "Session", "Status", "Processes", "CPU", "Memory", "Disk", "Network"] {
                    h.col(|ui| {
                        ui.label(RichText::new(t).strong());
                    });
                }
            })
            .body(|body| {
                body.rows(22.0, lines.len(), |mut row| match lines[row.index()] {
                    Line::User(i) => {
                        let u = &users[i];
                        row.set_selected(self.selected == Some(u.session_id));
                        let open = self.expanded.contains(&u.session_id);
                        let mine = || snap.processes.iter().filter(|p| p.session_id == u.session_id);
                        let disk: f64 = mine().map(|p| p.disk_bps).sum();
                        let net: f64 = mine().map(|p| p.net_bps).sum();
                        row.col(|ui| {
                            let name = if u.user.is_empty() { "(system)" } else { &u.user };
                            ui.label(RichText::new(format!("{} {name}", if open { "v" } else { ">" })).strong());
                        });
                        row.col(|ui| {
                            ui.label(u.session_id.to_string());
                        });
                        row.col(|ui| {
                            ui.label(&u.state);
                        });
                        row.col(|ui| {
                            ui.label(u.processes.to_string());
                        });
                        row.col(|ui| {
                            ui.label(format!("{:.1}%", u.cpu_pct.min(100.0)));
                        });
                        row.col(|ui| {
                            ui.label(format!(
                                "{} ({:.0}%)",
                                format_bytes(u.memory as f64),
                                u.memory as f64 * 100.0 / mem_total
                            ));
                        });
                        row.col(|ui| {
                            ui.label(format!("{}/s", format_bytes(disk)));
                        });
                        row.col(|ui| net_cell(ui, net_on, net));
                        if row.response().clicked() {
                            self.selected = Some(u.session_id);
                            toggle = Some(u.session_id);
                        }
                    }
                    Line::Proc(i) => {
                        let p = &snap.processes[i];
                        row.col(|ui| {
                            ui.add_space(18.0);
                            ui.label(display_name(p));
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(p.pid.to_string()).weak());
                        });
                        row.col(|_| {});
                        row.col(|_| {});
                        row.col(|ui| {
                            ui.label(format!("{:.1}%", p.cpu_pct));
                        });
                        row.col(|ui| {
                            ui.label(format_bytes(p.memory as f64));
                        });
                        row.col(|ui| {
                            ui.label(format!("{}/s", format_bytes(p.disk_bps)));
                        });
                        row.col(|ui| net_cell(ui, net_on, p.net_bps));
                    }
                });
            });
        if let Some(id) = toggle
            && !self.expanded.remove(&id)
        {
            self.expanded.insert(id);
        }
    }
}
