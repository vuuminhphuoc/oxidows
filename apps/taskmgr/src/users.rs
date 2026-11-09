//! Users tab: CPU, memory and disk per signed-in session; click a user to see their processes;
//! Disconnect and Sign off (both ask first).

use std::collections::HashSet;

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use taskmgr_core::{ProcessRow, Snapshot, UserUsage, display_name, format_bits_per_sec, format_bytes, users_view};
use ui_common::{SORT_HEADER_HEIGHT, SortBy, cmp_text, sort_header};

use crate::{ConfirmAction, Dialog, Status};

/// The columns, in order; any of them sorts the users, and the processes under an expanded user
/// where the column applies to them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Col {
    User,
    Session,
    Status,
    Processes,
    Cpu,
    Memory,
    Disk,
    Network,
}

impl Col {
    const ALL: [Col; 8] =
        [Col::User, Col::Session, Col::Status, Col::Processes, Col::Cpu, Col::Memory, Col::Disk, Col::Network];

    pub fn key(self) -> &'static str {
        match self {
            Col::User => "user",
            Col::Session => "session",
            Col::Status => "status",
            Col::Processes => "processes",
            Col::Cpu => "cpu",
            Col::Memory => "memory",
            Col::Disk => "disk",
            Col::Network => "network",
        }
    }

    pub fn from_key(k: &str) -> Option<Col> {
        Col::ALL.into_iter().find(|c| c.key() == k)
    }

    fn title(self) -> &'static str {
        match self {
            Col::User => "User",
            Col::Session => "Session",
            Col::Status => "Status",
            Col::Processes => "Processes",
            Col::Cpu => "CPU",
            Col::Memory => "Memory",
            Col::Disk => "Disk",
            Col::Network => "Network",
        }
    }

    fn descending_first(self) -> bool {
        matches!(self, Col::Processes | Col::Cpu | Col::Memory | Col::Disk | Col::Network)
    }

    fn cmp_users(self, a: &UserLine, b: &UserLine) -> std::cmp::Ordering {
        match self {
            Col::User => cmp_text(&a.u.user, &b.u.user),
            Col::Session => a.u.session_id.cmp(&b.u.session_id),
            Col::Status => cmp_text(&a.u.state, &b.u.state),
            Col::Processes => a.u.processes.cmp(&b.u.processes),
            Col::Cpu => a.u.cpu_pct.total_cmp(&b.u.cpu_pct),
            Col::Memory => a.u.memory.cmp(&b.u.memory),
            Col::Disk => a.disk.total_cmp(&b.disk),
            Col::Network => a.net.total_cmp(&b.net),
        }
    }

    /// Processes have no session status or count: those columns order them by CPU, biggest first.
    fn cmp_procs(self, a: &ProcessRow, b: &ProcessRow) -> Option<std::cmp::Ordering> {
        Some(match self {
            Col::User => cmp_text(display_name(a), display_name(b)),
            Col::Session => a.pid.cmp(&b.pid),
            Col::Cpu => a.cpu_pct.total_cmp(&b.cpu_pct),
            Col::Memory => a.memory.cmp(&b.memory),
            Col::Disk => a.disk_bps.total_cmp(&b.disk_bps),
            Col::Network => a.net_bps.total_cmp(&b.net_bps),
            Col::Status | Col::Processes => return None,
        })
    }
}

/// A user with the disk and network totals of their processes.
struct UserLine {
    u: UserUsage,
    disk: f64,
    net: f64,
}

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
    pub sort: SortBy<Col>,
    status: Status,
}

enum Line {
    User(usize),
    Proc(usize),
}

impl State {
    pub fn new(status: Status, sort: Option<SortBy<Col>>) -> Self {
        Self { expanded: HashSet::new(), selected: None, sort: sort.unwrap_or(SortBy::new(Col::User, false)), status }
    }

    pub fn show(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let sessions = snap.sessions.as_deref().map(Vec::as_slice).unwrap_or(&[]);
        let mut users = users_view(&snap.processes, sessions);
        // Session 0 (services) is not a person; Windows hides it here too, unless it is all we have.
        if users.len() > 1 {
            users.retain(|u| u.session_id != 0 || !u.state.is_empty());
        }
        let sort = self.sort;
        let mut users: Vec<UserLine> = users
            .into_iter()
            .map(|u| {
                let mine = || snap.processes.iter().filter(|p| p.session_id == u.session_id);
                let disk = mine().map(|p| p.disk_bps).sum();
                let net = mine().map(|p| p.net_bps).sum();
                UserLine { u, disk, net }
            })
            .collect();
        users.sort_by(|a, b| sort.apply(sort.key.cmp_users(a, b)).then(a.u.session_id.cmp(&b.u.session_id)));
        let sel = self.selected.and_then(|id| users.iter().map(|l| &l.u).find(|u| u.session_id == id));
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
        for (i, l) in users.iter().enumerate() {
            lines.push(Line::User(i));
            if self.expanded.contains(&l.u.session_id) {
                let mut procs: Vec<usize> =
                    (0..snap.processes.len()).filter(|&i| snap.processes[i].session_id == l.u.session_id).collect();
                procs.sort_by(|&a, &b| {
                    let (a, b) = (&snap.processes[a], &snap.processes[b]);
                    match sort.key.cmp_procs(a, b) {
                        Some(o) => sort.apply(o),
                        None => b.cpu_pct.total_cmp(&a.cpu_pct),
                    }
                    .then(a.pid.cmp(&b.pid))
                });
                lines.extend(procs.into_iter().map(Line::Proc));
            }
        }
        let mem_total = snap.system.mem_total.max(1) as f64;
        let net_on = snap.system.per_process_net;
        let mut toggle = None;
        let mut clicked = None;
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
                body.rows(22.0, lines.len(), |mut row| match lines[row.index()] {
                    Line::User(i) => {
                        let UserLine { u, disk, net } = &users[i];
                        let (disk, net) = (*disk, *net);
                        row.set_selected(self.selected == Some(u.session_id));
                        let open = self.expanded.contains(&u.session_id);
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
        if let Some(c) = clicked {
            self.sort.click(c, c.descending_first());
        }
        if let Some(id) = toggle
            && !self.expanded.remove(&id)
        {
            self.expanded.insert(id);
        }
    }
}
