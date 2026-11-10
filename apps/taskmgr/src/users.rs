//! Users tab: CPU, memory and disk per signed-in session; click a user to see their processes;
//! Disconnect and Sign off (both ask first).

use std::collections::HashSet;

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use taskmgr_core::{
    ColumnTotals, ProcessRow, Snapshot, UserUsage, display_name, format_bits_per_sec, format_bytes, users_view,
};
use ui_common::{SORT_HEADER_TOTAL_HEIGHT, SortBy, cmp_text, sort_header_total};

use crate::icons::Icons;
use crate::{ConfirmAction, Dialog, Status};

/// The columns, in order; any of them sorts the users, and the processes under an expanded user
/// where the column applies to them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Col {
    User,
    Id,
    Session,
    ClientName,
    Status,
    Processes,
    Cpu,
    Memory,
    Disk,
    Network,
    Gpu,
}

impl Col {
    pub const ALL: [Col; 11] = [
        Col::User,
        Col::Id,
        Col::Session,
        Col::ClientName,
        Col::Status,
        Col::Processes,
        Col::Cpu,
        Col::Memory,
        Col::Disk,
        Col::Network,
        Col::Gpu,
    ];

    pub const DEFAULT: [Col; 9] =
        [Col::User, Col::Id, Col::Status, Col::Processes, Col::Cpu, Col::Memory, Col::Disk, Col::Network, Col::Gpu];

    pub fn key(self) -> &'static str {
        match self {
            Col::User => "user",
            Col::Id => "id",
            Col::Session => "session",
            Col::ClientName => "client_name",
            Col::Status => "status",
            Col::Processes => "processes",
            Col::Cpu => "cpu",
            Col::Memory => "memory",
            Col::Disk => "disk",
            Col::Network => "network",
            Col::Gpu => "gpu",
        }
    }

    pub fn from_key(k: &str) -> Option<Col> {
        Col::ALL.into_iter().find(|c| c.key() == k)
    }

    pub fn title(self) -> &'static str {
        match self {
            Col::User => "User",
            Col::Id => "ID",
            Col::Session => "Session",
            Col::ClientName => "Client name",
            Col::Status => "Status",
            Col::Processes => "Processes",
            Col::Cpu => "CPU",
            Col::Memory => "Memory",
            Col::Disk => "Disk",
            Col::Network => "Network",
            Col::Gpu => "GPU",
        }
    }

    fn column(self) -> Column {
        match self {
            Col::User => Column::initial(240.0).clip(true),
            Col::Id => Column::exact(60.0),
            Col::Session => Column::initial(90.0).clip(true),
            Col::ClientName => Column::initial(110.0).clip(true),
            Col::Status => Column::exact(100.0),
            Col::Processes => Column::exact(80.0),
            Col::Cpu => Column::exact(70.0),
            Col::Memory => Column::exact(130.0),
            Col::Disk => Column::exact(90.0),
            Col::Network => Column::exact(90.0),
            Col::Gpu => Column::exact(70.0),
        }
    }

    fn descending_first(self) -> bool {
        matches!(self, Col::Processes | Col::Cpu | Col::Memory | Col::Disk | Col::Network | Col::Gpu)
    }

    fn cmp_users(self, a: &UserLine, b: &UserLine) -> std::cmp::Ordering {
        match self {
            Col::User => cmp_text(&a.display_user, &b.display_user),
            Col::Id => a.u.session_id.cmp(&b.u.session_id),
            Col::Session => cmp_text(&a.u.session_name, &b.u.session_name),
            Col::ClientName => cmp_text(&a.u.client_name, &b.u.client_name),
            Col::Status => cmp_text(&a.u.state, &b.u.state),
            Col::Processes => a.u.processes.cmp(&b.u.processes),
            Col::Cpu => a.u.cpu_pct.total_cmp(&b.u.cpu_pct),
            Col::Memory => a.u.memory.cmp(&b.u.memory),
            Col::Disk => a.disk.total_cmp(&b.disk),
            Col::Network => a.net.total_cmp(&b.net),
            Col::Gpu => a.gpu.total_cmp(&b.gpu),
        }
    }

    /// Processes have no session status or count: those columns order them by CPU, biggest first.
    fn cmp_procs(self, a: &ProcessRow, b: &ProcessRow) -> Option<std::cmp::Ordering> {
        Some(match self {
            Col::User => cmp_text(display_name(a), display_name(b)),
            Col::Id => a.pid.cmp(&b.pid),
            Col::Cpu => a.cpu_pct.total_cmp(&b.cpu_pct),
            Col::Memory => a.memory.cmp(&b.memory),
            Col::Disk => a.disk_bps.total_cmp(&b.disk_bps),
            Col::Network => a.net_bps.total_cmp(&b.net_bps),
            Col::Gpu => a.gpu_pct.total_cmp(&b.gpu_pct),
            Col::Session | Col::ClientName | Col::Status | Col::Processes => return None,
        })
    }
}

pub fn parse_columns(v: &str) -> Vec<Col> {
    if v.trim().is_empty() {
        return Col::DEFAULT.to_vec();
    }
    let mut cols: Vec<Col> = v.split(',').filter_map(|k| Col::from_key(k.trim())).collect();
    if !cols.contains(&Col::User) {
        cols.insert(0, Col::User);
    }
    cols
}

pub fn columns_text(cols: &[Col]) -> String {
    cols.iter().map(|c| c.key()).collect::<Vec<_>>().join(",")
}

/// A user with the disk and network totals of their processes.
struct UserLine {
    u: UserUsage,
    disk: f64,
    net: f64,
    gpu: f32,
    display_user: String,
}

fn net_cell(ui: &mut Ui, on: bool, bps: f64) {
    if on {
        ui.label(format_bits_per_sec(bps));
    } else {
        ui.label(RichText::new("-").weak()).on_hover_text(crate::processes::NET_HINT);
    }
}

pub struct State {
    pub(crate) filter: String,
    expanded: HashSet<u32>,
    selected: Option<u32>,
    pub sort: SortBy<Col>,
    pub columns: Vec<Col>,
    pub show_full_name: bool,
    status: Status,
    icons: Icons,
}

enum Line {
    User(usize),
    Proc(usize),
}

impl State {
    pub fn new(
        status: Status,
        icons: Icons,
        columns: Vec<Col>,
        sort: Option<SortBy<Col>>,
        show_full_name: bool,
    ) -> Self {
        Self {
            filter: String::new(),
            expanded: HashSet::new(),
            selected: None,
            sort: sort.unwrap_or(SortBy::new(Col::User, false)),
            columns,
            show_full_name,
            status,
            icons,
        }
    }

    pub fn commands(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let sessions = snap.sessions.as_deref().map(Vec::as_slice).unwrap_or(&[]);
        let users = users_view(&snap.processes, sessions);
        let sel = self.selected.and_then(|id| users.iter().find(|u| u.session_id == id));
        let can = sel.is_some_and(|u| !u.state.is_empty());

        if ui_common::command_button(ui, ui_common::Icon::Close, "Sign off", can, "").clicked()
            && let Some(u) = sel
        {
            self.status.open(Dialog::Confirm {
                text: format!("Sign off {}? Any unsaved data in their apps may be lost.", u.user),
                action: ConfirmAction::SignOff { session: u.session_id, user: u.user.clone() },
            });
        }
        if ui_common::command_button(ui, ui_common::Icon::DisconnectDrive, "Disconnect", can, "").clicked()
            && let Some(u) = sel
        {
            self.status.open(Dialog::Confirm {
                text: format!("Disconnect {}? Their apps keep running.", u.user),
                action: ConfirmAction::Disconnect { session: u.session_id, user: u.user.clone() },
            });
        }
        if ui_common::command_button(
            ui,
            ui_common::Icon::Users,
            "Manage user accounts",
            true,
            "Open user accounts in Windows Settings",
        )
        .clicked()
        {
            let _ = taskmgr_sys::open_url("ms-settings:otherusers");
        }
    }

    pub fn show(&mut self, ui: &mut Ui, snap: &Snapshot) -> bool {
        let before = self.columns.clone();
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
                let gpu = mine().map(|p| p.gpu_pct).sum();
                let display_user = if self.show_full_name {
                    u.user.clone()
                } else {
                    u.user.rsplit_once('\\').map_or(u.user.as_str(), |(_, short)| short).to_string()
                };
                UserLine { u, disk, net, gpu, display_user }
            })
            .collect();
        users.sort_by(|a, b| sort.apply(sort.key.cmp_users(a, b)).then(a.u.session_id.cmp(&b.u.session_id)));
        let f = self.filter.trim().to_lowercase();
        if !f.is_empty() {
            users.retain(|l| l.display_user.to_lowercase().contains(&f) || l.u.user.to_lowercase().contains(&f));
        }

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
        let totals = ColumnTotals::of(&snap.system);
        let mut toggle = None;
        let mut clicked = None;
        let selectable: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter_map(|(idx, line)| match line {
                Line::User(..) => Some(idx),
                _ => None,
            })
            .collect();
        let cursor = self.selected.and_then(|id| {
            selectable.iter().position(|&line_idx| match lines[line_idx] {
                Line::User(i) => users[i].u.session_id == id,
                _ => false,
            })
        });
        let mut scroll_to = None;
        if let Some(new_k) = crate::keys::list_keys(ui, selectable.len(), cursor, |k| match lines[selectable[k]] {
            Line::User(i) => {
                let name = &users[i].display_user;
                if name.is_empty() { "(system)".to_string() } else { name.clone() }
            }
            _ => String::new(),
        }) {
            let line_idx = selectable[new_k];
            if let Line::User(i) = lines[line_idx] {
                self.selected = Some(users[i].u.session_id);
            }
            scroll_to = Some(line_idx);
        }
        let cols = self.columns.clone();
        let mut table = TableBuilder::new(ui)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        for (idx, &c) in cols.iter().enumerate() {
            table = table.column(if idx + 1 == cols.len() { Column::remainder().clip(true) } else { c.column() });
        }
        if let Some(idx) = scroll_to {
            table = table.scroll_to_row(idx, Some(egui::Align::Center));
        }
        table
            .header(SORT_HEADER_TOTAL_HEIGHT, |mut h| {
                for &c in &cols {
                    let total = match c {
                        Col::Cpu => Some(totals.cpu),
                        Col::Memory => Some(totals.memory),
                        Col::Disk => Some(totals.disk),
                        Col::Network => Some(totals.network),
                        Col::Gpu => Some(totals.gpu),
                        _ => None,
                    }
                    .map(ColumnTotals::label);
                    h.col(|ui| {
                        let resp = sort_header_total(ui, c.title(), total.as_deref(), self.sort.arrow(c));
                        if resp.clicked() {
                            clicked = Some(c);
                        }
                        resp.context_menu(|ui| {
                            ui_common::column_chooser_menu(
                                ui,
                                &Col::ALL,
                                &mut self.columns,
                                Some(("User", Col::User)),
                                |c| c.title(),
                            );
                        });
                    });
                }
            })
            .body(|body| {
                body.rows(22.0, lines.len(), |mut row| match lines[row.index()] {
                    Line::User(i) => {
                        let UserLine { u, disk, net, gpu, display_user } = &users[i];
                        let (disk, net, gpu) = (*disk, *net, *gpu);
                        row.set_selected(self.selected == Some(u.session_id));
                        let open = self.expanded.contains(&u.session_id);
                        for &c in &cols {
                            row.col(|ui| match c {
                                Col::User => {
                                    let icon =
                                        if open { ui_common::Icon::ChevronDown } else { ui_common::Icon::ChevronRight };
                                    ui.label(RichText::new(icon.glyph()).font(ui_common::Icon::font(12.0)));
                                    let name = if display_user.is_empty() { "(system)" } else { display_user };
                                    ui.label(RichText::new(name).strong());
                                }
                                Col::Id => {
                                    ui.label(u.session_id.to_string());
                                }
                                Col::Session => {
                                    ui.label(&u.session_name);
                                }
                                Col::ClientName => {
                                    ui.label(&u.client_name);
                                }
                                Col::Status => {
                                    ui.label(&u.state);
                                }
                                Col::Processes => {
                                    ui.label(u.processes.to_string());
                                }
                                Col::Cpu => {
                                    ui.label(format!("{:.1}%", u.cpu_pct.min(100.0)));
                                }
                                Col::Memory => {
                                    ui.label(format!(
                                        "{} ({:.0}%)",
                                        format_bytes(u.memory as f64),
                                        u.memory as f64 * 100.0 / mem_total
                                    ));
                                }
                                Col::Disk => {
                                    ui.label(format!("{}/s", format_bytes(disk)));
                                }
                                Col::Network => net_cell(ui, net_on, net),
                                Col::Gpu => {
                                    ui.label(format!("{:.1}%", gpu.min(100.0)));
                                }
                            });
                        }
                        if row.response().clicked() {
                            self.selected = Some(u.session_id);
                            toggle = Some(u.session_id);
                        }
                    }
                    Line::Proc(i) => {
                        let p = &snap.processes[i];
                        for &c in &cols {
                            row.col(|ui| match c {
                                Col::User => {
                                    ui.add_space(18.0);
                                    self.icons.show(ui, p);
                                    ui.label(display_name(p));
                                }
                                Col::Id => {
                                    ui.label(RichText::new(p.pid.to_string()).weak());
                                }
                                Col::Session | Col::ClientName | Col::Status | Col::Processes => {}
                                Col::Cpu => {
                                    ui.label(format!("{:.1}%", p.cpu_pct));
                                }
                                Col::Memory => {
                                    ui.label(format_bytes(p.memory as f64));
                                }
                                Col::Disk => {
                                    ui.label(format!("{}/s", format_bytes(p.disk_bps)));
                                }
                                Col::Network => net_cell(ui, net_on, p.net_bps),
                                Col::Gpu => {
                                    ui.label(format!("{:.1}%", p.gpu_pct));
                                }
                            });
                        }
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
        if !self.columns.contains(&self.sort.key) {
            self.sort = SortBy::new(Col::User, false);
        }
        self.columns != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_round_trip() {
        let cols = vec![Col::User, Col::Id, Col::Gpu];
        assert_eq!(parse_columns(&columns_text(&cols)), cols);
        assert_eq!(parse_columns(""), Col::DEFAULT.to_vec());
    }
}
