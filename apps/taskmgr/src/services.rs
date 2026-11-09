//! Services tab: list with state and PID; start / stop / restart run on a worker thread so a
//! slow service never freezes the window.

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use taskmgr_core::{Service, ServiceState, Snapshot};
use taskmgr_sys::ServiceAction;
use ui_common::{SORT_HEADER_HEIGHT, SortBy, cmp_text, sort_header};

use crate::Status;

/// The columns, in order; any of them sorts the list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Col {
    Name,
    Pid,
    Description,
    Status,
    Group,
}

impl Col {
    const ALL: [Col; 5] = [Col::Name, Col::Pid, Col::Description, Col::Status, Col::Group];

    pub fn key(self) -> &'static str {
        match self {
            Col::Name => "name",
            Col::Pid => "pid",
            Col::Description => "description",
            Col::Status => "status",
            Col::Group => "group",
        }
    }

    pub fn from_key(k: &str) -> Option<Col> {
        Col::ALL.into_iter().find(|c| c.key() == k)
    }

    fn title(self) -> &'static str {
        match self {
            Col::Name => "Name",
            Col::Pid => "PID",
            Col::Description => "Description",
            Col::Status => "Status",
            Col::Group => "Group",
        }
    }

    fn cmp(self, a: &Service, b: &Service) -> std::cmp::Ordering {
        match self {
            Col::Name => cmp_text(&a.name, &b.name),
            Col::Pid => a.pid.cmp(&b.pid),
            Col::Description => cmp_text(&a.display_name, &b.display_name),
            Col::Status => a.state.label().cmp(b.state.label()),
            Col::Group => cmp_text(&a.group, &b.group),
        }
    }
}

pub struct State {
    pub(crate) filter: String,
    selected: Option<String>,
    pub sort: SortBy<Col>,
    status: Status,
    ctx: egui::Context,
}

impl State {
    pub fn new(status: Status, ctx: egui::Context, sort: Option<SortBy<Col>>) -> Self {
        Self { filter: String::new(), selected: None, sort: sort.unwrap_or(SortBy::new(Col::Name, false)), status, ctx }
    }

    /// "Go to service(s)": show the services hosted by `pid`.
    pub fn show_pid(&mut self, _pid: u32) {
        self.selected = None;
    }

    pub fn commands(&mut self, ui: &mut Ui, snap: &Snapshot) {
        if ui_common::command_button(ui, ui_common::Icon::Services, "Open Services", true, "").clicked()
            && let Err(e) = taskmgr_sys::open_services_console()
        {
            self.status.set(format!("Could not open Services: {e}"));
        }
        ui_common::command_separator(ui);
        let services = snap.services.as_deref();
        let sel = self.selected.as_ref().and_then(|n| services?.iter().find(|s| &s.name == n));
        let stopped = sel.is_some_and(|s| s.state == ServiceState::Stopped);
        let running = sel.is_some_and(|s| s.state == ServiceState::Running);
        if ui_common::command_button(ui, ui_common::Icon::Refresh, "Restart", running, "").clicked()
            && let Some(s) = sel
        {
            self.run(s.name.clone(), ServiceAction::Restart);
        }
        if ui_common::command_button(ui, ui_common::Icon::Stop, "Stop", running, "").clicked()
            && let Some(s) = sel
        {
            self.run(s.name.clone(), ServiceAction::Stop);
        }
        if ui_common::command_button(ui, ui_common::Icon::Play, "Start", stopped, "").clicked()
            && let Some(s) = sel
        {
            self.run(s.name.clone(), ServiceAction::Start);
        }
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
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} services", services.len())).weak());
        });
        ui.add_space(4.0);

        let f = self.filter.trim().to_lowercase();
        let mut view: Vec<_> = services
            .iter()
            .filter(|s| {
                f.is_empty()
                    || s.name.to_lowercase().contains(&f)
                    || s.display_name.to_lowercase().contains(&f)
                    || s.pid.to_string() == f
                    || s.group.to_lowercase().contains(&f)
            })
            .collect();
        let sort = self.sort;
        view.sort_by(|a, b| sort.apply(sort.key.cmp(a, b)).then_with(|| cmp_text(&a.name, &b.name)));
        let cursor = self.selected.as_ref().and_then(|name| view.iter().position(|s| &s.name == name));
        let mut scroll_to = None;
        if let Some(new_i) = crate::keys::list_keys(ui, view.len(), cursor, |k| view[k].name.clone()) {
            self.selected = Some(view[new_i].name.clone());
            scroll_to = Some(new_i);
        }
        let mut action = None;
        let mut clicked = None;
        let mut table = TableBuilder::new(ui)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(200.0).clip(true))
            .column(Column::exact(70.0))
            .column(Column::remainder().clip(true))
            .column(Column::exact(90.0))
            .column(Column::initial(170.0).clip(true));
        if let Some(i) = scroll_to {
            table = table.scroll_to_row(i, Some(egui::Align::Center));
        }
        table
            .header(SORT_HEADER_HEIGHT, |mut h| {
                for c in Col::ALL {
                    h.col(|ui| {
                        if sort_header(ui, c.title(), self.sort.arrow(c)).clicked() {
                            clicked = Some(c);
                        }
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
        if let Some(c) = clicked {
            self.sort.click(c, false);
        }
        if let Some((name, a)) = action {
            self.run(name, a);
        }
    }
}
