//! Processes tab: a virtualised table (only visible rows are laid out), sort by any column,
//! search by name or PID, End task from a button, the Delete key or the right-click menu.

use std::collections::{HashMap, HashSet};

use egui::{Color32, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use taskmgr_core::{
    ColumnTotals, ProcGroup, ProcessRow, Snapshot, SortKey, display_name, format_bits_per_sec, format_bytes, group_of,
    is_critical_process, sorted_view, type_label,
};
use taskmgr_sys::PriorityClass;
use ui_common::{SORT_HEADER_TOTAL_HEIGHT, SortBy, sort_header_total};

use crate::icons::Icons;
use crate::{ConfirmAction, Dialog, Status};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PCol {
    Type,
    Status,
    Publisher,
    Pid,
    ProcessName,
    CommandLine,
    Cpu,
    Memory,
    Disk,
    Network,
    Gpu,
    GpuEngine,
    Power,
    PowerTrend,
}

impl PCol {
    pub const ALL: [PCol; 14] = [
        PCol::Type,
        PCol::Status,
        PCol::Publisher,
        PCol::Pid,
        PCol::ProcessName,
        PCol::CommandLine,
        PCol::Cpu,
        PCol::Memory,
        PCol::Disk,
        PCol::Network,
        PCol::Gpu,
        PCol::GpuEngine,
        PCol::Power,
        PCol::PowerTrend,
    ];

    pub const DEFAULT: [PCol; 8] =
        [PCol::Status, PCol::Cpu, PCol::Memory, PCol::Disk, PCol::Network, PCol::Gpu, PCol::Power, PCol::PowerTrend];

    pub fn key(self) -> &'static str {
        match self {
            PCol::Type => "type",
            PCol::Status => "status",
            PCol::Publisher => "publisher",
            PCol::Pid => "pid",
            PCol::ProcessName => "processname",
            PCol::CommandLine => "commandline",
            PCol::Cpu => "cpu",
            PCol::Memory => "memory",
            PCol::Disk => "disk",
            PCol::Network => "network",
            PCol::Gpu => "gpu",
            PCol::GpuEngine => "gpuengine",
            PCol::Power => "power",
            PCol::PowerTrend => "powertrend",
        }
    }

    pub fn from_key(k: &str) -> Option<PCol> {
        match k {
            "process_name" => Some(PCol::ProcessName),
            "command_line" => Some(PCol::CommandLine),
            "gpu_engine" => Some(PCol::GpuEngine),
            _ => PCol::ALL.into_iter().find(|c| c.key() == k),
        }
    }

    pub fn header(self) -> &'static str {
        match self {
            PCol::Type => "Type",
            PCol::Status => "Status",
            PCol::Publisher => "Publisher",
            PCol::Pid => "PID",
            PCol::ProcessName => "Process name",
            PCol::CommandLine => "Command line",
            PCol::Cpu => "CPU",
            PCol::Memory => "Memory",
            PCol::Disk => "Disk",
            PCol::Network => "Network",
            PCol::Gpu => "GPU",
            PCol::GpuEngine => "GPU engine",
            PCol::Power => "Power usage",
            PCol::PowerTrend => "Power usage trend",
        }
    }

    pub fn sort_key(self) -> SortKey {
        match self {
            PCol::Type => SortKey::Type,
            PCol::Status => SortKey::Status,
            PCol::Publisher => SortKey::Publisher,
            PCol::Pid => SortKey::Pid,
            PCol::ProcessName => SortKey::ProcessName,
            PCol::CommandLine => SortKey::CommandLine,
            PCol::Cpu => SortKey::Cpu,
            PCol::Memory => SortKey::Memory,
            PCol::Disk => SortKey::Disk,
            PCol::Network => SortKey::Network,
            PCol::Gpu => SortKey::Gpu,
            PCol::GpuEngine => SortKey::GpuEngine,
            PCol::Power => SortKey::Power,
            PCol::PowerTrend => SortKey::PowerTrend,
        }
    }

    fn column(self) -> Column {
        match self {
            PCol::Status => Column::exact(76.0),
            PCol::Pid => Column::exact(64.0),
            PCol::Cpu => Column::exact(64.0),
            PCol::Memory | PCol::Disk | PCol::Network => Column::exact(90.0),
            PCol::Gpu => Column::exact(60.0),
            PCol::Power | PCol::PowerTrend => Column::exact(96.0),
            PCol::Type => Column::initial(120.0).clip(true),
            PCol::ProcessName => Column::initial(150.0).clip(true),
            PCol::CommandLine => Column::initial(260.0).clip(true),
            PCol::GpuEngine => Column::initial(100.0).clip(true),
            PCol::Publisher => Column::initial(150.0).clip(true),
        }
    }
}

/// Missing setting = Windows' default set; "name" alone = every optional column hidden.
pub fn parse_columns(v: &str) -> Vec<PCol> {
    if v.trim().is_empty() {
        return PCol::DEFAULT.to_vec();
    }
    PCol::ALL.into_iter().filter(|c| v.split(',').any(|k| PCol::from_key(k.trim()) == Some(*c))).collect()
}

/// Name is always shown, so it leads the list and keeps an empty choice distinct from "not set".
pub fn columns_text(cols: &[PCol]) -> String {
    std::iter::once("name").chain(cols.iter().map(|c| c.key())).collect::<Vec<_>>().join(",")
}

pub struct State {
    pub(crate) filter: String,
    pub sort: SortBy<SortKey>,
    /// Selected (pid, name): the name guards against ending a reused PID.
    pub(crate) selected: Option<(u32, String)>,
    pub(crate) group: bool,
    collapsed: [bool; 3],
    /// Apps whose child processes are shown.
    expanded: HashSet<u32>,
    /// Memory, Disk and Network as a share of the machine instead of absolute values.
    pub percent: bool,
    windir: String,
    status: Status,
    icons: Icons,
    pub columns: Vec<PCol>,
    pub confirm_efficiency: bool,
}

enum Line {
    Header(ProcGroup, usize),
    Row(usize),
    /// An app with its child processes folded in: root index, totals, child count.
    App(usize, Box<ProcessRow>, usize),
    Child(usize),
}

/// Indexes of the processes `root` started, directly or not (a child starts after its parent,
/// which rules out reused PIDs). Children that are apps themselves stay separate.
fn descendants(rows: &[ProcessRow], kids: &HashMap<u32, Vec<usize>>, root: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(i) = stack.pop() {
        let p = &rows[i];
        for &c in kids.get(&p.pid).map_or(&[][..], Vec::as_slice) {
            let child = &rows[c];
            if c != i && child.start_time >= p.start_time && !child.has_window && !out.contains(&c) {
                out.push(c);
                stack.push(c);
            }
        }
    }
    out
}

/// The root's row with its children's usage added, as Task Manager shows a grouped app.
fn app_total(rows: &[ProcessRow], root: usize, children: &[usize]) -> ProcessRow {
    let mut t = rows[root].clone();
    for &c in children {
        let r = &rows[c];
        t.cpu_pct += r.cpu_pct;
        t.memory += r.memory;
        t.disk_bps += r.disk_bps;
        t.net_bps += r.net_bps;
        t.gpu_pct = t.gpu_pct.max(r.gpu_pct);
        t.power += r.power;
        t.power_trend += r.power_trend;
    }
    t
}

/// Ends `pid` only if it still belongs to the program the user picked.
pub fn end_task(status: &Status, rows: &[ProcessRow], pid: u32, name: &str) {
    if !rows.iter().any(|r| r.pid == pid && r.name == name) {
        status.set(format!("{name} ({pid}) has already exited"));
        return;
    }
    status.set(match taskmgr_sys::end_process(pid) {
        Ok(()) => format!("Ended {name} ({pid})"),
        Err(e) => format!("Could not end {name} ({pid}): {e}"),
    });
}

/// Ends a process and everything it started (children first), like "End process tree".
pub fn end_tree(status: &Status, rows: &[ProcessRow], pid: u32, name: &str) {
    // The confirmation can stay open a while; the PID may be gone or reused by then.
    if !rows.iter().any(|r| r.pid == pid && r.name == name) {
        status.set(format!("{name} ({pid}) has already exited"));
        return;
    }
    let mut order = vec![pid];
    let mut i = 0;
    while i < order.len() {
        let parent = order[i];
        // A child must have started after its parent, or the parent PID was reused.
        let parent_start = rows.iter().find(|r| r.pid == parent).map_or(0, |r| r.start_time);
        order.extend(
            rows.iter()
                .filter(|r| r.parent_pid == parent && r.pid != parent && r.start_time >= parent_start)
                .map(|r| r.pid),
        );
        i += 1;
    }
    let mut failed = 0;
    for p in order.iter().rev() {
        if taskmgr_sys::end_process(*p).is_err() {
            failed += 1;
        }
    }
    status.set(if failed == 0 {
        format!("Ended {name} and {} child process(es)", order.len() - 1)
    } else {
        format!("Ended {} of {} processes in the {name} tree", order.len() - failed, order.len())
    });
}

/// "Restart" for Windows Explorer: end it, then start a fresh shell once it is gone.
fn restart_explorer(status: &Status, rows: &[ProcessRow], p: &ProcessRow) {
    if !rows.iter().any(|r| r.pid == p.pid && r.name == p.name) {
        status.set("Windows Explorer has already exited");
        return;
    }
    if let Err(e) = taskmgr_sys::end_process(p.pid) {
        status.set(format!("Could not end Windows Explorer: {e}"));
        return;
    }
    let status = status.clone();
    std::thread::spawn(move || {
        // Windows relaunches the shell itself when AutoRestartShell is on; start one only if it did not,
        // so no extra File Explorer window opens.
        std::thread::sleep(std::time::Duration::from_millis(2000));
        if taskmgr_sys::is_running("explorer.exe") {
            status.set("Windows Explorer restarted");
            return;
        }
        status.set(match taskmgr_sys::run_new_task("explorer.exe", false) {
            Ok(()) => "Windows Explorer restarted".to_string(),
            Err(e) => format!("Could not start Windows Explorer: {e}"),
        });
    });
}

/// The right-click menu shared by Processes and Details.
pub fn process_menu(
    ui: &mut Ui,
    status: &Status,
    rows: &[ProcessRow],
    p: &ProcessRow,
    on_details: bool,
    confirm_efficiency: bool,
) {
    ui.label(RichText::new(format!("{} ({})", p.name, p.pid)).weak());
    if p.has_window && ui.button("Switch to").clicked() {
        match taskmgr_sys::switch_to(p.pid) {
            Ok(()) => status.used(),
            Err(e) => status.set(format!("Could not switch to {}: {e}", p.name)),
        }
        ui.close();
    }
    if ui.button("End task").clicked() {
        if is_critical_process(&p.name) {
            status.open(Dialog::Confirm {
                text: format!(
                    "Do you want to end the system process {}?\n\nEnding this process will cause Windows to become unusable or shut down, causing you to lose any unsaved data.",
                    p.name
                ),
                action: ConfirmAction::CriticalEndTask { pid: p.pid, name: p.name.clone() },
            });
        } else {
            end_task(status, rows, p.pid, &p.name);
        }
        ui.close();
    }
    if p.name.eq_ignore_ascii_case("explorer.exe") && ui.button("Restart").clicked() {
        restart_explorer(status, rows, p);
        ui.close();
    }
    if ui.button("End process tree").clicked() {
        status.open(Dialog::Confirm {
            text: format!(
                "Do you want to end the process tree of {}?\n\nIf an open program is associated with this process, it will close and you will lose any unsaved data. If you end a system process, it might result in system instability. Are you sure you want to continue?",
                p.name
            ),
            action: ConfirmAction::EndTree { pid: p.pid, name: p.name.clone() },
        });
        ui.close();
    }
    let mut eco = taskmgr_sys::efficiency_mode(p.pid).unwrap_or(false);
    if ui.checkbox(&mut eco, "Efficiency mode").changed() {
        if eco {
            if confirm_efficiency {
                status.open(Dialog::Confirm {
                    text: format!(
                        "Turn on Efficiency mode for {}?\n\nEfficiency mode will lower process priority and improve power efficiency. It might cause instability for some processes.",
                        p.name
                    ),
                    action: ConfirmAction::EfficiencyMode { pid: p.pid, name: p.name.clone() },
                });
            } else {
                status.set(match taskmgr_sys::set_efficiency_mode(p.pid, true) {
                    Ok(()) => format!("Efficiency mode on for {}", p.name),
                    Err(e) => format!("Could not change efficiency mode of {}: {e}", p.name),
                });
            }
        } else {
            status.set(match taskmgr_sys::set_efficiency_mode(p.pid, false) {
                Ok(()) => format!("Efficiency mode off for {}", p.name),
                Err(e) => format!("Could not change efficiency mode of {}: {e}", p.name),
            });
        }
        ui.close();
    }
    ui.separator();
    ui.menu_button("Set priority", |ui| {
        for c in PriorityClass::ALL {
            if ui.button(c.label()).clicked() {
                status.set(match taskmgr_sys::set_priority(p.pid, c) {
                    Ok(()) => format!("{} priority set to {}", p.name, c.label()),
                    Err(e) => format!("Could not change priority of {}: {e}", p.name),
                });
                ui.close();
            }
        }
    });
    if ui.button("Set affinity...").clicked() {
        match taskmgr_sys::affinity(p.pid) {
            Ok((mask, system)) => status.open(Dialog::Affinity { pid: p.pid, name: p.name.clone(), mask, system }),
            Err(e) => status.set(format!("Could not read affinity of {}: {e}", p.name)),
        }
        ui.close();
    }
    if ui.button("Create memory dump file").clicked() {
        let path = std::env::temp_dir().join(format!("{}.DMP", p.name.trim_end_matches(".exe")));
        let (pid, name, status2) = (p.pid, p.name.clone(), status.clone());
        status.set(format!("Writing dump of {name}..."));
        std::thread::spawn(move || {
            status2.set(match taskmgr_sys::create_dump(pid, &path) {
                Ok(()) => format!("Dump of {name} written to {}", path.display()),
                Err(e) => format!("Could not dump {name}: {e}"),
            });
        });
        ui.close();
    }
    if on_details {
        if ui.button("Analyze wait chain").clicked() {
            match taskmgr_sys::wait_chain(p.pid) {
                Ok(chains) => {
                    let mut names: Vec<(u32, String)> = Vec::new();
                    for b in chains.iter().flat_map(|c| &c.blocking) {
                        if !names.iter().any(|(q, _)| q == b) {
                            let n = rows.iter().find(|r| r.pid == *b).map_or("process".to_string(), |r| r.name.clone());
                            names.push((*b, n));
                        }
                    }
                    status.open(Dialog::WaitChain { pid: p.pid, name: p.name.clone(), chains, names });
                }
                Err(e) => status.set(format!("Could not analyze {}: {e}", p.name)),
            }
            ui.close();
        }
        if let Some(mut on) = taskmgr_sys::uac_virtualization(p.pid)
            && ui.checkbox(&mut on, "UAC virtualization").changed()
        {
            status.set(match taskmgr_sys::set_uac_virtualization(p.pid, on) {
                Ok(()) => format!("UAC virtualization {} for {}", if on { "on" } else { "off" }, p.name),
                Err(e) => format!("Could not change UAC virtualization of {}: {e}", p.name),
            });
            ui.close();
        }
    }
    ui.separator();
    if !on_details && ui.button("Go to details").clicked() {
        status.goto_details(p.pid);
        ui.close();
    }
    if p.name.eq_ignore_ascii_case("svchost.exe") && ui.button("Go to service(s)").clicked() {
        status.goto_services(p.pid);
        ui.close();
    }
    if ui.add_enabled(!p.info.path.is_empty(), egui::Button::new("Open file location")).clicked() {
        if let Err(e) = taskmgr_sys::open_file_location(&p.info.path) {
            status.set(format!("Could not open Explorer: {e}"));
        }
        ui.close();
    }
    if ui.add_enabled(!p.info.path.is_empty(), egui::Button::new("Properties")).clicked() {
        if let Err(e) = taskmgr_sys::file_properties(&p.info.path) {
            status.set(format!("Could not open Properties: {e}"));
        }
        ui.close();
    }
    if ui.button("Search online").clicked() {
        if let Err(e) = taskmgr_sys::search_online(&p.name) {
            status.set(format!("Could not open the browser: {e}"));
        }
        ui.close();
    }
    if ui.add_enabled(!p.info.command_line.is_empty(), egui::Button::new("Copy command line")).clicked() {
        ui.ctx().copy_text(p.info.command_line.clone());
        ui.close();
    }
}

/// Shown where per-process network data needs elevation.
pub const NET_HINT: &str = "Per-process network needs administrator: use Run as administrator";

/// Task Manager style heat: transparent at 0, amber at the top of the scale.
fn heat(ui: &Ui, fraction: f32) -> Color32 {
    let base = if ui.visuals().dark_mode { Color32::from_rgb(180, 120, 0) } else { Color32::from_rgb(255, 200, 40) };
    base.gamma_multiply((0.08 + fraction.clamp(0.0, 1.0) * 0.7).min(0.78))
}

fn header_menu(ui: &mut Ui, columns: &mut Vec<PCol>) {
    ui_common::MenuItem::new("Name").checked(true).enabled(false).ui(ui);
    for c in PCol::ALL {
        let mut on = columns.contains(&c);
        if ui_common::menu_check(ui, None, c.header(), &mut on).clicked() {
            if on {
                if !columns.contains(&c) {
                    columns.push(c);
                    columns.sort_by_key(|x| PCol::ALL.iter().position(|a| a == x).unwrap_or(0));
                }
            } else {
                columns.retain(|x| *x != c);
            }
        }
    }
}

impl State {
    pub fn new(
        status: Status,
        icons: Icons,
        group: bool,
        percent: bool,
        columns: Vec<PCol>,
        sort: Option<SortBy<SortKey>>,
        confirm_efficiency: bool,
    ) -> Self {
        Self {
            percent,
            filter: String::new(),
            sort: sort.unwrap_or(SortBy::new(SortKey::default(), true)),
            selected: None,
            group,
            collapsed: [false; 3],
            expanded: HashSet::new(),
            windir: std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()),
            status,
            icons,
            columns,
            confirm_efficiency,
        }
    }

    pub fn show(&mut self, ui: &mut Ui, snap: &Snapshot) -> bool {
        let before_cols = self.columns.clone();
        let rows = &snap.processes;
        if self.selected.is_some()
            && ui.input(|i| i.key_pressed(egui::Key::Delete))
            && ui.ctx().memory(|m| m.focused().is_none())
        {
            self.end_selected(rows);
        }
        ui.add_space(4.0);

        let view = sorted_view(rows, &self.filter, self.sort.key, self.sort.descending);
        let lines: Vec<Line> = if self.group {
            let mut kids: HashMap<u32, Vec<usize>> = HashMap::new();
            for (i, r) in rows.iter().enumerate() {
                kids.entry(r.parent_pid).or_default().push(i);
            }
            // Apps claim their child processes, so those leave the other groups.
            let apps: Vec<(usize, Vec<usize>)> = view
                .iter()
                .copied()
                .filter(|&i| group_of(&rows[i], &self.windir) == ProcGroup::Apps)
                .map(|i| (i, descendants(rows, &kids, i)))
                .collect();
            let claimed: HashSet<usize> = apps.iter().flat_map(|(_, c)| c.iter().copied()).collect();
            let mut lines = Vec::with_capacity(view.len() + 3);
            if !apps.is_empty() {
                lines.push(Line::Header(ProcGroup::Apps, apps.len()));
                if !self.collapsed[0] {
                    for (i, children) in &apps {
                        let n = children.len();
                        lines.push(Line::App(*i, Box::new(app_total(rows, *i, children)), n));
                        if self.expanded.contains(&rows[*i].pid) {
                            lines.push(Line::Child(*i));
                            lines.extend(children.iter().copied().map(Line::Child));
                        }
                    }
                }
            }
            for (gi, g) in [(1, ProcGroup::Background), (2, ProcGroup::Windows)] {
                let members: Vec<usize> = view
                    .iter()
                    .copied()
                    .filter(|&i| !claimed.contains(&i) && group_of(&rows[i], &self.windir) == g)
                    .collect();
                if members.is_empty() {
                    continue;
                }
                lines.push(Line::Header(g, members.len()));
                if !self.collapsed[gi] {
                    lines.extend(members.into_iter().map(Line::Row));
                }
            }
            lines
        } else {
            view.into_iter().map(Line::Row).collect()
        };

        // Selectable rows: non-header rows in `lines`
        let selectable: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter_map(|(idx, line)| match line {
                Line::Header(..) => None,
                _ => Some(idx),
            })
            .collect();

        let cursor = self.selected.as_ref().and_then(|(pid, _)| {
            selectable.iter().position(|&line_idx| match &lines[line_idx] {
                Line::Row(i) | Line::Child(i) => rows[*i].pid == *pid,
                Line::App(root, _, _) => rows[*root].pid == *pid,
                Line::Header(..) => false,
            })
        });

        let line_name = |k: usize| -> String {
            let line_idx = selectable[k];
            match &lines[line_idx] {
                Line::Row(i) | Line::Child(i) => display_name(&rows[*i]).to_string(),
                Line::App(root, _, _) => display_name(&rows[*root]).to_string(),
                Line::Header(..) => String::new(),
            }
        };

        let mut scroll_to_line = None;
        if let Some(new_k) = crate::keys::list_keys(ui, selectable.len(), cursor, line_name) {
            let line_idx = selectable[new_k];
            let p = match &lines[line_idx] {
                Line::Row(i) | Line::Child(i) => &rows[*i],
                Line::App(root, _, _) => &rows[*root],
                Line::Header(..) => unreachable!(),
            };
            self.selected = Some((p.pid, p.name.clone()));
            scroll_to_line = Some(line_idx);
        }

        // Enter expands/collapses an app row in Processes
        if ui.input(|i| i.key_pressed(egui::Key::Enter))
            && ui.ctx().memory(|m| m.focused().is_none())
            && let Some((pid, _)) = &self.selected
            && lines.iter().any(|line| matches!(line, Line::App(root, _, n) if rows[*root].pid == *pid && *n > 0))
            && !self.expanded.remove(pid)
        {
            self.expanded.insert(*pid);
        }

        let mem_total = snap.system.mem_total.max(1) as f32;
        let net_on = snap.system.per_process_net;
        let sys = &snap.system;
        let disk_total = sys.disk_read_bps + sys.disk_write_bps;
        // A process's share of disk activity, scaled to how busy the disks are (as Task Manager does).
        let disk_pct = |bps: f64| {
            if disk_total <= 0.0 { 0.0 } else { bps / disk_total * sys.disk_active_pct.unwrap_or(100.0) as f64 }
        };
        let link_bps: f64 = sys.adapters.iter().map(|a| a.link_bps as f64).sum();
        let net_pct = |bps: f64| if link_bps <= 0.0 { 0.0 } else { bps * 8.0 * 100.0 / link_bps };
        let percent = self.percent;
        let totals = ColumnTotals::of(sys);
        let row_h = 22.0;

        let mut clicked_sort = None;
        let mut toggle_group = None;
        let mut toggle_app = None;
        let cols = self.columns.clone();
        let mut table = TableBuilder::new(ui)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().at_least(200.0).clip(true));
        for &col in &cols {
            table = table.column(col.column());
        }
        if let Some(line_idx) = scroll_to_line {
            table = table.scroll_to_row(line_idx, Some(egui::Align::Center));
        }
        table
            .header(SORT_HEADER_TOTAL_HEIGHT, |mut h| {
                h.col(|ui| {
                    let resp = sort_header_total(ui, "Name", None, self.sort.arrow(SortKey::Name));
                    if resp.clicked() {
                        clicked_sort = Some(SortKey::Name);
                    }
                    resp.context_menu(|ui| {
                        header_menu(ui, &mut self.columns);
                    });
                });
                for &col in &cols {
                    let total = match col {
                        PCol::Cpu => Some(totals.cpu),
                        PCol::Memory => Some(totals.memory),
                        PCol::Disk => Some(totals.disk),
                        PCol::Network => Some(totals.network),
                        PCol::Gpu => Some(totals.gpu),
                        _ => None,
                    }
                    .map(ColumnTotals::label);
                    h.col(|ui| {
                        let resp =
                            sort_header_total(ui, col.header(), total.as_deref(), self.sort.arrow(col.sort_key()));
                        if resp.clicked() {
                            clicked_sort = Some(col.sort_key());
                        }
                        resp.context_menu(|ui| {
                            header_menu(ui, &mut self.columns);
                        });
                    });
                }
            })
            .body(|body| {
                body.rows(row_h, lines.len(), |mut row| {
                    let p: &ProcessRow = match lines[row.index()] {
                        Line::Header(g, n) => {
                            let gi = g as usize;
                            row.col(|ui| {
                                let icon = if self.collapsed[gi] {
                                    ui_common::Icon::ChevronRight
                                } else {
                                    ui_common::Icon::ChevronDown
                                };
                                ui.label(RichText::new(icon.glyph()).font(ui_common::Icon::font(11.0)));
                                ui.label(RichText::new(format!("{} ({n})", g.label())).strong());
                            });
                            for _ in 0..cols.len() {
                                row.col(|_| {});
                            }
                            if row.response().clicked() {
                                toggle_group = Some(gi);
                            }
                            return;
                        }
                        Line::Row(i) | Line::Child(i) => &rows[i],
                        Line::App(_, ref total, _) => total,
                    };
                    let (indent, fold) = match lines[row.index()] {
                        Line::App(_, _, n) => (0.0, Some(n)),
                        Line::Child(_) => (34.0, None),
                        _ => (if self.group { 14.0 } else { 0.0 }, None),
                    };
                    let is_sel = self.selected.as_ref().is_some_and(|(pid, _)| *pid == p.pid);
                    row.set_selected(is_sel);
                    row.col(|ui| {
                        ui.add_space(indent);
                        match fold {
                            Some(n) if n > 0 => {
                                let open = self.expanded.contains(&p.pid);
                                let icon =
                                    if open { ui_common::Icon::ChevronDown } else { ui_common::Icon::ChevronRight };
                                if ui
                                    .small_button(RichText::new(icon.glyph()).font(ui_common::Icon::font(10.0)))
                                    .clicked()
                                {
                                    toggle_app = Some(p.pid);
                                }
                                self.icons.show(ui, p);
                                ui.label(format!("{} ({})", display_name(p), n + 1)).on_hover_text(&p.name);
                            }
                            Some(_) => {
                                ui.add_space(18.0);
                                self.icons.show(ui, p);
                                ui.label(display_name(p)).on_hover_text(&p.name);
                            }
                            None => {
                                self.icons.show(ui, p);
                                ui.label(display_name(p)).on_hover_text(&p.name);
                            }
                        }
                    });
                    for &col in &cols {
                        row.col(|ui| match col {
                            PCol::Type => {
                                ui.label(type_label(p, &self.windir));
                            }
                            PCol::Status => {
                                ui.horizontal(|ui| {
                                    if p.efficiency {
                                        ui.label(
                                            RichText::new(ui_common::Icon::Leaf.glyph())
                                                .font(ui_common::Icon::font(12.0))
                                                .color(Color32::from_rgb(60, 170, 90)),
                                        )
                                        .on_hover_text("Efficiency mode");
                                    }
                                    if p.suspended {
                                        ui.label(
                                            RichText::new(ui_common::Icon::Pause.glyph())
                                                .font(ui_common::Icon::font(12.0))
                                                .weak(),
                                        )
                                        .on_hover_text("Suspended");
                                    }
                                });
                            }
                            PCol::Publisher => {
                                ui.label(&p.info.publisher);
                            }
                            PCol::Pid => {
                                ui.label(p.pid.to_string());
                            }
                            PCol::ProcessName => {
                                ui.label(&p.name);
                            }
                            PCol::CommandLine => {
                                ui.label(&p.info.command_line).on_hover_text(&p.info.command_line);
                            }
                            PCol::Cpu => {
                                ui.painter().rect_filled(ui.max_rect(), 0.0, heat(ui, p.cpu_pct / 100.0));
                                ui.label(format!("{:.1}%", p.cpu_pct));
                            }
                            PCol::Memory => {
                                ui.painter().rect_filled(
                                    ui.max_rect(),
                                    0.0,
                                    heat(ui, p.memory as f32 / mem_total * 8.0),
                                );
                                ui.label(if percent {
                                    format!("{:.1}%", p.memory as f32 * 100.0 / mem_total)
                                } else {
                                    format_bytes(p.memory as f64)
                                });
                            }
                            PCol::Disk => {
                                ui.painter().rect_filled(ui.max_rect(), 0.0, heat(ui, (p.disk_bps / 10e6) as f32));
                                ui.label(if percent {
                                    format!("{:.1}%", disk_pct(p.disk_bps))
                                } else {
                                    format!("{}/s", format_bytes(p.disk_bps))
                                });
                            }
                            PCol::Network => {
                                if net_on {
                                    ui.painter().rect_filled(
                                        ui.max_rect(),
                                        0.0,
                                        heat(ui, (p.net_bps * 8.0 / 10e6) as f32),
                                    );
                                    ui.label(if percent {
                                        format!("{:.1}%", net_pct(p.net_bps))
                                    } else {
                                        format_bits_per_sec(p.net_bps)
                                    });
                                } else {
                                    ui.label(RichText::new("-").weak()).on_hover_text(NET_HINT);
                                }
                            }
                            PCol::Gpu => {
                                ui.painter().rect_filled(ui.max_rect(), 0.0, heat(ui, p.gpu_pct / 100.0));
                                ui.label(format!("{:.0}%", p.gpu_pct));
                            }
                            PCol::GpuEngine => {
                                ui.label(&p.gpu_engine);
                            }
                            PCol::Power => {
                                ui.painter().rect_filled(ui.max_rect(), 0.0, heat(ui, p.power / 25.0));
                                ui.label(taskmgr_core::power_label(p.power))
                                    .on_hover_text("Estimated from CPU, GPU, disk and network use");
                            }
                            PCol::PowerTrend => {
                                ui.painter().rect_filled(ui.max_rect(), 0.0, heat(ui, p.power_trend / 25.0));
                                ui.label(taskmgr_core::power_label(p.power_trend))
                                    .on_hover_text("Estimated from CPU, GPU, disk and network use");
                            }
                        });
                    }
                    let resp = row.response();
                    if resp.clicked() {
                        self.selected = Some((p.pid, p.name.clone()));
                    }
                    // Actions on a grouped app act on its root process, not the totals row.
                    let target = match lines[row.index()] {
                        Line::App(i, _, _) => &rows[i],
                        _ => p,
                    };
                    resp.context_menu(|ui| {
                        process_menu(ui, &self.status, rows, target, false, self.confirm_efficiency)
                    });
                });
            });
        if let Some(pid) = toggle_app
            && !self.expanded.remove(&pid)
        {
            self.expanded.insert(pid);
        }
        if let Some(gi) = toggle_group {
            self.collapsed[gi] = !self.collapsed[gi];
        }

        if let Some(key) = clicked_sort {
            self.sort.click(key, key.descending_first());
        }
        before_cols != self.columns
    }

    pub fn commands(&mut self, ui: &mut Ui, snap: &Snapshot) {
        ui_common::command_menu(ui, ui_common::Icon::View, "View", "", |ui| {
            ui_common::menu_check(ui, None, "Group by type", &mut self.group);
        });
        ui_common::command_separator(ui);
        let can_action = self.selected.is_some();
        if ui_common::command_button(ui, ui_common::Icon::Leaf, "Efficiency mode", can_action, "").clicked()
            && let Some((pid, name)) = &self.selected
        {
            let eco = !taskmgr_sys::efficiency_mode(*pid).unwrap_or(false);
            if eco {
                if self.confirm_efficiency {
                    self.status.open(Dialog::Confirm {
                        text: format!(
                            "Turn on Efficiency mode for {name}?\n\nEfficiency mode will lower process priority and improve power efficiency. It might cause instability for some processes."
                        ),
                        action: ConfirmAction::EfficiencyMode { pid: *pid, name: name.clone() },
                    });
                } else {
                    self.status.set(match taskmgr_sys::set_efficiency_mode(*pid, true) {
                        Ok(()) => format!("Efficiency mode on for {name}"),
                        Err(e) => format!("Could not change efficiency mode of {name}: {e}"),
                    });
                }
            } else {
                self.status.set(match taskmgr_sys::set_efficiency_mode(*pid, false) {
                    Ok(()) => format!("Efficiency mode off for {name}"),
                    Err(e) => format!("Could not change efficiency mode of {name}: {e}"),
                });
            }
        }
        if ui_common::command_button(ui, ui_common::Icon::Close, "End task", can_action, "").clicked() {
            self.end_selected(&snap.processes);
        }
    }

    pub fn set_group(&mut self, on: bool) {
        self.group = on;
    }

    pub fn end_selected(&mut self, rows: &[ProcessRow]) {
        if let Some((pid, name)) = self.selected.take() {
            if is_critical_process(&name) {
                self.status.open(Dialog::Confirm {
                    text: format!(
                        "Do you want to end the system process {name}?\n\nEnding this process will cause Windows to become unusable or shut down, causing you to lose any unsaved data."
                    ),
                    action: ConfirmAction::CriticalEndTask { pid, name },
                });
            } else {
                end_task(&self.status, rows, pid, &name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcol_keys_and_round_trip() {
        for col in PCol::ALL {
            assert_eq!(PCol::from_key(col.key()), Some(col));
        }
        let text = columns_text(&PCol::DEFAULT);
        let parsed = parse_columns(&text);
        assert_eq!(parsed, PCol::DEFAULT.to_vec());
        assert_eq!(parse_columns(""), PCol::DEFAULT.to_vec());
        assert_eq!(parse_columns("invalid,junk"), Vec::<PCol>::new());
        assert_eq!(parse_columns(&columns_text(&[])), Vec::<PCol>::new());
        assert_eq!(parse_columns("cpu,type,cpu"), vec![PCol::Type, PCol::Cpu]);
        assert_eq!(parse_columns("type,process_name,cpu"), vec![PCol::Type, PCol::ProcessName, PCol::Cpu]);
    }
}
