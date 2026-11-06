//! Users tab: CPU and memory per signed-in session.

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use taskmgr_core::{Snapshot, format_bytes, users_view};

pub fn show(ui: &mut Ui, snap: &Snapshot) {
    let sessions = snap.sessions.as_deref().map(Vec::as_slice).unwrap_or(&[]);
    let mut users = users_view(&snap.processes, sessions);
    // Session 0 (services) is not a person; Windows hides it here too, unless it is all we have.
    if users.len() > 1 {
        users.retain(|u| u.session_id != 0 || !u.state.is_empty());
    }
    let mem_total = snap.system.mem_total.max(1) as f64;
    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(240.0).clip(true))
        .column(Column::exact(70.0))
        .column(Column::exact(100.0))
        .column(Column::exact(90.0))
        .column(Column::exact(80.0))
        .column(Column::remainder())
        .header(22.0, |mut h| {
            for t in ["User", "Session", "Status", "Processes", "CPU", "Memory"] {
                h.col(|ui| {
                    ui.label(RichText::new(t).strong());
                });
            }
        })
        .body(|body| {
            body.rows(24.0, users.len(), |mut row| {
                let u = &users[row.index()];
                row.col(|ui| {
                    ui.label(if u.user.is_empty() { "(system)" } else { &u.user });
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
                    ui.label(format!("{} ({:.0}%)", format_bytes(u.memory as f64), u.memory as f64 * 100.0 / mem_total));
                });
            });
        });
}
