//! Details tab: every process, with columns the user picks (right-click the header, or the
//! "Select columns" button), sortable by any of them.

use std::cmp::Ordering;

use crate::Status;
use crate::icons::Icons;
use crate::processes::{NET_HINT, end_task, process_menu};
use egui::{RichText, Stroke, Ui, pos2};
use egui_extras::{Column, TableBuilder};
use taskmgr_core::{ProcessRow, Snapshot, format_bits_per_sec, format_bytes, priority_label};
use ui_common::{SORT_HEADER_HEIGHT, SortBy, sort_header_drag};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Col {
    Name,
    Pid,
    Status,
    User,
    Session,
    Cpu,
    CpuTime,
    Memory,
    Threads,
    Handles,
    Priority,
    IoRead,
    IoWrite,
    Network,
    Gpu,
    Description,
    Publisher,
    Path,
    CommandLine,
    ParentPid,
    WorkingSet,
    PeakWorkingSet,
    Commit,
    PagedPool,
    NonPagedPool,
    PageFaults,
    IoReads,
    IoWrites,
    IoOther,
    IoOtherBytes,
    GpuEngine,
    Elevated,
    UacVirtualization,
    Platform,
    GdiObjects,
    UserObjects,
    Architecture,
    PowerThrottling,
    PackageName,
}

impl Col {
    pub const ALL: [Col; 39] = [
        Col::Name,
        Col::Pid,
        Col::Status,
        Col::User,
        Col::Session,
        Col::Cpu,
        Col::CpuTime,
        Col::Memory,
        Col::Threads,
        Col::Handles,
        Col::Priority,
        Col::IoRead,
        Col::IoWrite,
        Col::Network,
        Col::Gpu,
        Col::Description,
        Col::Publisher,
        Col::Path,
        Col::CommandLine,
        Col::ParentPid,
        Col::WorkingSet,
        Col::PeakWorkingSet,
        Col::Commit,
        Col::PagedPool,
        Col::NonPagedPool,
        Col::PageFaults,
        Col::IoReads,
        Col::IoWrites,
        Col::IoOther,
        Col::IoOtherBytes,
        Col::GpuEngine,
        Col::Elevated,
        Col::UacVirtualization,
        Col::Platform,
        Col::GdiObjects,
        Col::UserObjects,
        Col::Architecture,
        Col::PowerThrottling,
        Col::PackageName,
    ];

    /// What a fresh install shows, as in Task Manager plus Path.
    pub const DEFAULT: [Col; 10] = [
        Col::Name,
        Col::Pid,
        Col::Status,
        Col::User,
        Col::Cpu,
        Col::Memory,
        Col::Threads,
        Col::Handles,
        Col::Priority,
        Col::Path,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Col::Name => "Name",
            Col::Pid => "PID",
            Col::Status => "Status",
            Col::User => "User name",
            Col::Session => "Session ID",
            Col::Cpu => "CPU",
            Col::CpuTime => "CPU time",
            Col::Memory => "Memory (private working set)",
            Col::Threads => "Threads",
            Col::Handles => "Handles",
            Col::Priority => "Base priority",
            Col::IoRead => "I/O read bytes",
            Col::IoWrite => "I/O write bytes",
            Col::Network => "Network",
            Col::Gpu => "GPU",
            Col::Description => "Description",
            Col::Publisher => "Company",
            Col::Path => "Image path name",
            Col::CommandLine => "Command line",
            Col::ParentPid => "Parent PID",
            Col::WorkingSet => "Working set (memory)",
            Col::PeakWorkingSet => "Peak working set (memory)",
            Col::Commit => "Commit size",
            Col::PagedPool => "Paged pool",
            Col::NonPagedPool => "NP pool",
            Col::PageFaults => "Page faults",
            Col::IoReads => "I/O reads",
            Col::IoWrites => "I/O writes",
            Col::IoOther => "I/O other",
            Col::IoOtherBytes => "I/O other bytes",
            Col::GpuEngine => "GPU engine",
            Col::Elevated => "Elevated",
            Col::UacVirtualization => "UAC virtualization",
            Col::Platform => "Platform",
            Col::GdiObjects => "GDI objects",
            Col::UserObjects => "User objects",
            Col::Architecture => "Architecture",
            Col::PowerThrottling => "Power throttling",
            Col::PackageName => "Package name",
        }
    }

    /// Short key used in the settings file.
    pub fn key(self) -> &'static str {
        match self {
            Col::Name => "name",
            Col::Pid => "pid",
            Col::Status => "status",
            Col::User => "user",
            Col::Session => "session",
            Col::Cpu => "cpu",
            Col::CpuTime => "cputime",
            Col::Memory => "memory",
            Col::Threads => "threads",
            Col::Handles => "handles",
            Col::Priority => "priority",
            Col::IoRead => "ioread",
            Col::IoWrite => "iowrite",
            Col::Network => "network",
            Col::Gpu => "gpu",
            Col::Description => "description",
            Col::Publisher => "company",
            Col::Path => "path",
            Col::CommandLine => "cmdline",
            Col::ParentPid => "ppid",
            Col::WorkingSet => "workingset",
            Col::PeakWorkingSet => "peakws",
            Col::Commit => "commit",
            Col::PagedPool => "pagedpool",
            Col::NonPagedPool => "nppool",
            Col::PageFaults => "pagefaults",
            Col::IoReads => "ioreads",
            Col::IoWrites => "iowrites",
            Col::IoOther => "ioother",
            Col::IoOtherBytes => "iootherbytes",
            Col::GpuEngine => "gpuengine",
            Col::Elevated => "elevated",
            Col::UacVirtualization => "uacvirt",
            Col::Platform => "platform",
            Col::GdiObjects => "gdi_objects",
            Col::UserObjects => "user_objects",
            Col::Architecture => "architecture",
            Col::PowerThrottling => "power_throttling",
            Col::PackageName => "package_name",
        }
    }

    pub fn from_key(k: &str) -> Option<Col> {
        Col::ALL.into_iter().find(|c| c.key() == k)
    }

    fn header(self) -> &'static str {
        match self {
            Col::Memory => "Memory",
            c => c.title(),
        }
    }

    fn column(self) -> Column {
        match self {
            Col::Name => Column::initial(170.0).clip(true),
            Col::User | Col::Description | Col::Publisher | Col::GpuEngine | Col::UacVirtualization => {
                Column::initial(150.0).clip(true)
            }
            Col::Path | Col::CommandLine => Column::initial(260.0).clip(true),
            Col::Memory | Col::IoRead | Col::IoWrite | Col::Network | Col::CpuTime => Column::initial(90.0),
            Col::Priority | Col::Status => Column::initial(90.0),
            Col::PowerThrottling => Column::initial(110.0),
            Col::PackageName => Column::initial(180.0).clip(true),
            Col::GdiObjects | Col::UserObjects | Col::Architecture => Column::initial(80.0),
            _ => Column::initial(64.0),
        }
    }

    /// Numbers read best biggest first; text A to Z.
    pub fn descending_first(self) -> bool {
        matches!(
            self,
            Col::Cpu
                | Col::CpuTime
                | Col::Memory
                | Col::Threads
                | Col::Handles
                | Col::IoRead
                | Col::IoWrite
                | Col::Network
                | Col::Gpu
                | Col::WorkingSet
                | Col::PeakWorkingSet
                | Col::Commit
                | Col::PagedPool
                | Col::NonPagedPool
                | Col::PageFaults
                | Col::IoReads
                | Col::IoWrites
                | Col::IoOther
                | Col::IoOtherBytes
                | Col::GdiObjects
                | Col::UserObjects
        )
    }

    fn cmp(self, a: &ProcessRow, b: &ProcessRow) -> Ordering {
        let text = |x: &str, y: &str| x.to_lowercase().cmp(&y.to_lowercase());
        match self {
            Col::Name => text(&a.name, &b.name),
            Col::Pid => a.pid.cmp(&b.pid),
            Col::Status => a.suspended.cmp(&b.suspended),
            Col::User => text(&a.info.user, &b.info.user),
            Col::Session => a.session_id.cmp(&b.session_id),
            Col::Cpu => a.cpu_pct.total_cmp(&b.cpu_pct),
            Col::CpuTime => a.cpu_time.cmp(&b.cpu_time),
            Col::Memory => a.memory.cmp(&b.memory),
            Col::Threads => a.threads.cmp(&b.threads),
            Col::Handles => a.handles.cmp(&b.handles),
            Col::Priority => a.base_priority.cmp(&b.base_priority),
            Col::IoRead => a.io_read_bytes.cmp(&b.io_read_bytes),
            Col::IoWrite => a.io_write_bytes.cmp(&b.io_write_bytes),
            Col::Network => a.net_bps.total_cmp(&b.net_bps),
            Col::Gpu => a.gpu_pct.total_cmp(&b.gpu_pct),
            Col::Description => text(&a.info.description, &b.info.description),
            Col::Publisher => text(&a.info.publisher, &b.info.publisher),
            Col::Path => text(&a.info.path, &b.info.path),
            Col::CommandLine => text(&a.info.command_line, &b.info.command_line),
            Col::ParentPid => a.parent_pid.cmp(&b.parent_pid),
            Col::WorkingSet => a.counters.working_set.cmp(&b.counters.working_set),
            Col::PeakWorkingSet => a.counters.peak_working_set.cmp(&b.counters.peak_working_set),
            Col::Commit => a.counters.commit.cmp(&b.counters.commit),
            Col::PagedPool => a.counters.paged_pool.cmp(&b.counters.paged_pool),
            Col::NonPagedPool => a.counters.nonpaged_pool.cmp(&b.counters.nonpaged_pool),
            Col::PageFaults => a.counters.page_faults.cmp(&b.counters.page_faults),
            Col::IoReads => a.counters.read_ops.cmp(&b.counters.read_ops),
            Col::IoWrites => a.counters.write_ops.cmp(&b.counters.write_ops),
            Col::IoOther => a.counters.other_ops.cmp(&b.counters.other_ops),
            Col::IoOtherBytes => a.counters.other_bytes.cmp(&b.counters.other_bytes),
            Col::GpuEngine => a.gpu_engine.cmp(&b.gpu_engine),
            Col::Elevated => a.info.elevated.cmp(&b.info.elevated),
            Col::UacVirtualization => a.info.uac_virtualization.cmp(&b.info.uac_virtualization),
            Col::Platform => a.info.platform.cmp(&b.info.platform),
            Col::GdiObjects => a.info.gdi_objects.cmp(&b.info.gdi_objects),
            Col::UserObjects => a.info.user_objects.cmp(&b.info.user_objects),
            Col::Architecture => text(&a.info.architecture, &b.info.architecture),
            Col::PowerThrottling => text(&a.info.power_throttling, &b.info.power_throttling),
            Col::PackageName => text(&a.info.package_name, &b.info.package_name),
        }
    }

    fn cell(self, ui: &mut Ui, p: &ProcessRow, net_on: bool, icons: &Icons) {
        let dash = |s: &str| if s.is_empty() { "-".to_string() } else { s.to_string() };
        let text = match self {
            Col::Name => {
                icons.show(ui, p);
                p.name.clone()
            }
            Col::Pid => p.pid.to_string(),
            Col::Status => (if p.suspended {
                "Suspended"
            } else if p.efficiency {
                "Running (efficiency mode)"
            } else {
                "Running"
            })
            .to_string(),
            Col::User => dash(&p.info.user),
            Col::Session => p.session_id.to_string(),
            Col::Cpu => format!("{:.0}", p.cpu_pct),
            Col::CpuTime => format_cpu_time(p.cpu_time),
            Col::Memory => format_bytes(p.memory as f64),
            Col::Threads => p.threads.to_string(),
            Col::Handles => p.handles.to_string(),
            Col::Priority => priority_label(p.base_priority).to_string(),
            Col::IoRead => format_bytes(p.io_read_bytes as f64),
            Col::IoWrite => format_bytes(p.io_write_bytes as f64),
            Col::Network if !net_on => {
                ui.label(RichText::new("-").weak()).on_hover_text(NET_HINT);
                return;
            }
            Col::Network => format_bits_per_sec(p.net_bps),
            Col::Gpu => format!("{:.0}", p.gpu_pct),
            Col::Description => dash(&p.info.description),
            Col::Publisher => dash(&p.info.publisher),
            Col::Path => {
                ui.label(dash(&p.info.path)).on_hover_text(dash(&p.info.command_line));
                return;
            }
            Col::CommandLine => {
                ui.label(dash(&p.info.command_line)).on_hover_text(dash(&p.info.command_line));
                return;
            }
            Col::ParentPid => p.parent_pid.to_string(),
            Col::WorkingSet => format_bytes(p.counters.working_set as f64),
            Col::PeakWorkingSet => format_bytes(p.counters.peak_working_set as f64),
            Col::Commit => format_bytes(p.counters.commit as f64),
            Col::PagedPool => format_bytes(p.counters.paged_pool as f64),
            Col::NonPagedPool => format_bytes(p.counters.nonpaged_pool as f64),
            Col::PageFaults => p.counters.page_faults.to_string(),
            Col::IoReads => p.counters.read_ops.to_string(),
            Col::IoWrites => p.counters.write_ops.to_string(),
            Col::IoOther => p.counters.other_ops.to_string(),
            Col::IoOtherBytes => format_bytes(p.counters.other_bytes as f64),
            Col::GpuEngine => dash(&p.gpu_engine),
            Col::Elevated => (match p.info.elevated {
                Some(true) => "Yes",
                Some(false) => "No",
                None => "-",
            })
            .to_string(),
            Col::UacVirtualization => dash(&p.info.uac_virtualization),
            Col::Platform => dash(&p.info.platform),
            Col::GdiObjects => p.info.gdi_objects.map_or("-".to_string(), |v| v.to_string()),
            Col::UserObjects => p.info.user_objects.map_or("-".to_string(), |v| v.to_string()),
            Col::Architecture => dash(&p.info.architecture),
            Col::PowerThrottling => dash(&p.info.power_throttling),
            Col::PackageName => dash(&p.info.package_name),
        };
        ui.label(text);
    }
}

/// CPU time as Task Manager shows it: h:mm:ss.
pub fn format_cpu_time(t_100ns: u64) -> String {
    let s = t_100ns / 10_000_000;
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

pub fn parse_columns(v: &str) -> Vec<Col> {
    let mut cols: Vec<Col> = v.split(',').filter_map(|k| Col::from_key(k.trim())).collect();
    cols.dedup();
    if !cols.contains(&Col::Name) {
        cols.insert(0, Col::Name);
    }
    cols
}

pub fn columns_text(cols: &[Col]) -> String {
    cols.iter().map(|c| c.key()).collect::<Vec<_>>().join(",")
}

/// Moves a column from index `from` to index `to`, shifting intervening columns.
pub fn move_column<T>(cols: &mut Vec<T>, from: usize, to: usize) {
    if from >= cols.len() || to >= cols.len() || from == to {
        return;
    }
    let item = cols.remove(from);
    cols.insert(to, item);
}

pub struct State {
    pub(crate) filter: String,
    pub sort: SortBy<Col>,
    selected: Option<(u32, String)>,
    scroll_to: Option<u32>,
    status: Status,
    icons: Icons,
    pub columns: Vec<Col>,
    picking: bool,
    drag_col: Option<usize>,
    pub confirm_efficiency: bool,
}

impl State {
    pub fn new(
        status: Status,
        icons: Icons,
        columns: Vec<Col>,
        sort: Option<SortBy<Col>>,
        confirm_efficiency: bool,
    ) -> Self {
        Self {
            filter: String::new(),
            sort: sort.unwrap_or(SortBy::new(Col::Name, false)),
            selected: None,
            scroll_to: None,
            status,
            icons,
            columns,
            picking: false,
            drag_col: None,
            confirm_efficiency,
        }
    }

    /// Selects `pid` and clears the search so the row is visible.
    pub fn select(&mut self, pid: u32, snap: &Snapshot) {
        if let Some(p) = snap.processes.iter().find(|p| p.pid == pid) {
            self.selected = Some((pid, p.name.clone()));
            self.scroll_to = Some(pid);
        }
    }

    pub fn commands(&mut self, ui: &mut Ui, snap: &Snapshot) {
        if ui_common::command_button(ui, ui_common::Icon::Settings, "Select columns", true, "").clicked() {
            self.picking = true;
        }
        let can_end = self.selected.is_some();
        if ui_common::command_button(ui, ui_common::Icon::Close, "End task", can_end, "").clicked() {
            self.end_selected(&snap.processes);
        }
    }

    pub fn end_selected(&mut self, rows: &[ProcessRow]) {
        if let Some((pid, name)) = self.selected.take() {
            if taskmgr_core::is_critical_process(&name) {
                self.status.open(crate::Dialog::Confirm {
                    text: format!(
                        "Do you want to end the system process {name}?\n\nEnding this process will cause Windows to become unusable or shut down, causing you to lose any unsaved data."
                    ),
                    action: crate::ConfirmAction::CriticalEndTask { pid, name },
                });
            } else {
                end_task(&self.status, rows, pid, &name);
            }
        }
    }

    /// Returns true when the visible columns changed (the caller saves them).
    pub fn show(&mut self, ui: &mut Ui, snap: &Snapshot) -> bool {
        let rows = &snap.processes;
        let before = self.columns.clone();
        self.column_picker(ui.ctx());

        if self.selected.is_some()
            && ui.input(|i| i.key_pressed(egui::Key::Delete))
            && ui.ctx().memory(|m| m.focused().is_none())
        {
            self.end_selected(rows);
        }
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
        let sort = self.sort;
        view.sort_by(|a, b| sort.apply(sort.key.cmp(a, b)).then(a.pid.cmp(&b.pid)));

        let cursor = self.selected.as_ref().and_then(|(pid, _)| view.iter().position(|p| p.pid == *pid));
        if let Some(new_i) = crate::keys::list_keys(ui, view.len(), cursor, |k| view[k].name.clone()) {
            let p = view[new_i];
            self.selected = Some((p.pid, p.name.clone()));
            self.scroll_to = Some(p.pid);
        }
        let net_on = snap.system.per_process_net;
        let cols = self.columns.clone();
        let row_h = 20.0;
        let mut clicked = None;
        let mut table = TableBuilder::new(ui);
        if let Some(pid) = self.scroll_to.take()
            && let Some(i) = view.iter().position(|p| p.pid == pid)
        {
            table = table.scroll_to_row(i, Some(egui::Align::Center));
        }
        table = table
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        for (i, c) in cols.iter().enumerate() {
            table = table.column(if i + 1 == cols.len() { Column::remainder().clip(true) } else { c.column() });
        }
        let mut header_rects: Vec<(usize, egui::Rect)> = Vec::new();
        table
            .header(SORT_HEADER_HEIGHT, |mut h| {
                for (col_idx, &c) in cols.iter().enumerate() {
                    h.col(|ui| {
                        let resp = sort_header_drag(ui, c.header(), self.sort.arrow(c));
                        header_rects.push((col_idx, resp.rect));
                        if resp.drag_started() {
                            self.drag_col = Some(col_idx);
                        }
                        if resp.clicked() {
                            clicked = Some(c);
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Select columns").clicked() {
                                self.picking = true;
                                ui.close();
                            }
                            if c != Col::Name && ui.button(format!("Hide \"{}\"", c.header())).clicked() {
                                self.columns.retain(|x| *x != c);
                                ui.close();
                            }
                        });
                    });
                }
            })
            .body(|body| {
                body.rows(row_h, view.len(), |mut row| {
                    let p = view[row.index()];
                    row.set_selected(self.selected.as_ref().is_some_and(|(pid, _)| *pid == p.pid));
                    for &c in &cols {
                        row.col(|ui| c.cell(ui, p, net_on, &self.icons));
                    }
                    let resp = row.response();
                    if resp.clicked() {
                        self.selected = Some((p.pid, p.name.clone()));
                    }
                    resp.context_menu(|ui| process_menu(ui, &self.status, rows, p, true, self.confirm_efficiency));
                });
            });
        if let Some(k) = clicked {
            self.sort.click(k, k.descending_first());
        }
        if let Some(from_idx) = self.drag_col {
            let mouse_pos = ui.input(|i| i.pointer.latest_pos());
            if let Some(pos) = mouse_pos {
                let mut dest = from_idx;
                let mut drop_x = None;
                for (idx, rect) in &header_rects {
                    if pos.x < rect.center().x {
                        dest = if from_idx < *idx { *idx - 1 } else { *idx };
                        drop_x = Some(rect.left());
                        break;
                    } else {
                        dest = if from_idx > *idx { *idx + 1 } else { *idx };
                        drop_x = Some(rect.right());
                    }
                }
                if dest != from_idx
                    && let Some(x) = drop_x
                {
                    let p = ui_common::Palette::of(ui);
                    let header_top = header_rects.first().map_or(0.0, |r| r.1.top());
                    let header_bottom = header_rects.first().map_or(0.0, |r| r.1.bottom());
                    let painter = ui.painter();
                    painter.line_segment([pos2(x, header_top), pos2(x, header_bottom)], Stroke::new(2.5, p.accent));
                }
                if ui.input(|i| i.pointer.any_released()) {
                    self.drag_col = None;
                    if dest != from_idx {
                        move_column(&mut self.columns, from_idx, dest);
                    }
                }
            } else if ui.input(|i| i.pointer.any_released()) {
                self.drag_col = None;
            }
        }
        if !self.columns.contains(&self.sort.key) {
            self.sort = SortBy::new(Col::Name, false);
        }
        self.columns != before
    }

    fn column_picker(&mut self, ctx: &egui::Context) {
        if !self.picking {
            return;
        }
        let mut open = true;
        egui::Window::new("Select columns").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label("Choose the columns that appear in the table.");
            ui.add_space(4.0);
            egui::Grid::new("cols").num_columns(2).show(ui, |ui| {
                for (i, c) in Col::ALL.into_iter().enumerate() {
                    let mut on = self.columns.contains(&c);
                    let resp = ui.add_enabled(c != Col::Name, egui::Checkbox::new(&mut on, c.title()));
                    if resp.changed() {
                        if on {
                            // Keep the order of Col::ALL.
                            let at = self
                                .columns
                                .iter()
                                .position(|x| Col::ALL.iter().position(|a| a == x) > Some(i))
                                .unwrap_or(self.columns.len());
                            self.columns.insert(at, c);
                        } else {
                            self.columns.retain(|x| *x != c);
                        }
                    }
                    if i % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Reset").clicked() {
                    self.columns = Col::DEFAULT.to_vec();
                }
                if ui.button("Close").clicked() {
                    self.picking = false;
                }
            });
        });
        if !open {
            self.picking = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_round_trip() {
        let cols = vec![Col::Name, Col::Pid, Col::CommandLine];
        assert_eq!(parse_columns(&columns_text(&cols)), cols);
        assert_eq!(parse_columns("pid,bogus"), vec![Col::Name, Col::Pid]);
        assert_eq!(format_cpu_time(36_610_000_000), "1:01:01");
    }

    #[test]
    fn move_column_reorders() {
        let mut cols = vec!["A", "B", "C", "D"];
        move_column(&mut cols, 0, 2);
        assert_eq!(cols, vec!["B", "C", "A", "D"]);

        move_column(&mut cols, 2, 0);
        assert_eq!(cols, vec!["A", "B", "C", "D"]);

        move_column(&mut cols, 1, 3);
        assert_eq!(cols, vec!["A", "C", "D", "B"]);

        // Same index does nothing
        move_column(&mut cols, 1, 1);
        assert_eq!(cols, vec!["A", "C", "D", "B"]);

        // Out of bounds does nothing
        move_column(&mut cols, 10, 1);
        assert_eq!(cols, vec!["A", "C", "D", "B"]);
        move_column(&mut cols, 1, 10);
        assert_eq!(cols, vec!["A", "C", "D", "B"]);
    }
}
