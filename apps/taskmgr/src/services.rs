//! Services tab: list with state and PID; start / stop / restart run on a worker thread so a
//! slow service never freezes the window.

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use taskmgr_core::{ServiceState, Snapshot};
use taskmgr_sys::ServiceAction;

use crate::Status;

pub struct State {
    filter: String,
    selected: Option<String>,
    status: Status,
    ctx: egui::Context,
}

impl State {
    pub fn new(status: Status, ctx: egui::Context) -> Self {
        Self { filter: String::new(), selected: None, status, ctx }
    }

    /// "Go to service(s)": show the services hosted by `pid`.
    pub fn show_pid(&mut self, pid: u32) {
        self.filter = pid.to_string();
        self.selected = None;
    }

    fn run(&self, name: String, action: ServiceAction) {
        let status = self.status.clone();
        let ctx = self.ctx.clone();
        let verb = match action {
            ServiceAction::Start => "Starting",
            ServiceAction::Stop => "Stopping",
            ServiceAction::Restart => "Restarting",
        };
        status.set(format!("{verb} {name}..."));
        std::thread::spawn(move || {
            let msg = match taskmgr_sys::service_control(&name, action) {
                Ok(()) => format!("{verb} {name}: request accepted"),
                Err(e) => format!("{verb} {name} failed: {e}"),
            };
            status.set(msg);
            ctx.request_repaint();
        });
    }

    pub fn show(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let Some(services) = snap.services.as_ref() else {
            ui.label("Reading services...");
            return;
        };
        let sel = self.selected.as_ref().and_then(|n| services.iter().find(|s| &s.name == n));
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter).hint_text("name, description or PID").desired_width(240.0),
            );
            let stopped = sel.is_some_and(|s| s.state == ServiceState::Stopped);
            let running = sel.is_some_and(|s| s.state == ServiceState::Running);
            if ui.add_enabled(stopped, egui::Button::new("Start")).clicked()
                && let Some(s) = sel
            {
                self.run(s.name.clone(), ServiceAction::Start);
            }
            if ui.add_enabled(running, egui::Button::new("Stop")).clicked()
                && let Some(s) = sel
            {
                self.run(s.name.clone(), ServiceAction::Stop);
            }
            if ui.add_enabled(running, egui::Button::new("Restart")).clicked()
                && let Some(s) = sel
            {
                self.run(s.name.clone(), ServiceAction::Restart);
            }
            if ui.button("Open Services").clicked()
                && let Err(e) = taskmgr_sys::open_services_console()
            {
                self.status.set(format!("Could not open Services: {e}"));
            }
            ui.label(RichText::new(format!("{} services", services.len())).weak());
        });
        ui.add_space(4.0);

        let f = self.filter.trim().to_lowercase();
        let view: Vec<_> = services
            .iter()
            .filter(|s| {
                f.is_empty()
                    || s.name.to_lowercase().contains(&f)
                    || s.display_name.to_lowercase().contains(&f)
                    || s.pid.to_string() == f
                    || s.group.to_lowercase().contains(&f)
            })
            .collect();
        let mut action = None;
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(200.0).clip(true))
            .column(Column::exact(70.0))
            .column(Column::remainder().clip(true))
            .column(Column::exact(90.0))
            .column(Column::initial(170.0).clip(true))
            .header(20.0, |mut h| {
                for t in ["Name", "PID", "Description", "Status", "Group"] {
                    h.col(|ui| {
                        ui.label(RichText::new(t).strong());
                    });
                }
            })
            .body(|body| {
                body.rows(20.0, view.len(), |mut row| {
                    let s = view[row.index()];
                    row.set_selected(self.selected.as_deref() == Some(s.name.as_str()));
                    row.col(|ui| {
                        ui.label(&s.name);
                    });
                    row.col(|ui| {
                        ui.label(if s.pid == 0 { String::new() } else { s.pid.to_string() });
                    });
                    row.col(|ui| {
                        ui.label(&s.display_name);
                    });
                    row.col(|ui| {
                        ui.label(s.state.label());
                    });
                    row.col(|ui| {
                        ui.label(&s.group);
                    });
                    let resp = row.response();
                    if resp.clicked() {
                        self.selected = Some(s.name.clone());
                    }
                    resp.context_menu(|ui| {
                        for (label, a, ok) in [
                            ("Start", ServiceAction::Start, s.state == ServiceState::Stopped),
                            ("Stop", ServiceAction::Stop, s.state == ServiceState::Running),
                            ("Restart", ServiceAction::Restart, s.state == ServiceState::Running),
                        ] {
                            if ui.add_enabled(ok, egui::Button::new(label)).clicked() {
                                action = Some((s.name.clone(), a));
                                ui.close();
                            }
                        }
                        ui.separator();
                        if ui.add_enabled(s.pid != 0, egui::Button::new("Go to details")).clicked() {
                            self.status.goto_details(s.pid);
                            ui.close();
                        }
                        if ui.button("Search online").clicked() {
                            let _ = taskmgr_sys::search_online(&s.name);
                            ui.close();
                        }
                    });
                });
            });
        if let Some((name, a)) = action {
            self.run(name, a);
        }
    }
}
