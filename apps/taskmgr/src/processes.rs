//! Processes tab: a virtualised table (only visible rows are laid out), sort by any column,
//! search by name or PID, End task from a button, the Delete key or the right-click menu.

use std::collections::{HashMap, HashSet};

use egui::{Color32, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use taskmgr_core::{
    ProcGroup, ProcessRow, Snapshot, SortKey, display_name, format_bits_per_sec, format_bytes, group_of, sorted_view,
};
use taskmgr_sys::PriorityClass;

use crate::{Dialog, Status};

pub struct State {
    filter: String,
    sort: SortKey,
    ascending: bool,
    /// Selected (pid, name): the name guards against ending a reused PID.
    selected: Option<(u32, String)>,
    group: bool,
    collapsed: [bool; 3],
    /// Apps whose child processes are shown.
    expanded: HashSet<u32>,
    /// Memory, Disk and Network as a share of the machine instead of absolute values.
    pub percent: bool,
    windir: String,
    status: Status,
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
pub fn end_tree(status: &Status, rows: &[ProcessRow], root: &ProcessRow) {
    let mut order = vec![root.pid];
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
    for pid in order.iter().rev() {
        if taskmgr_sys::end_process(*pid).is_err() {
            failed += 1;
        }
    }
    status.set(if failed == 0 {
        format!("Ended {} and {} child process(es)", root.name, order.len() - 1)
    } else {
        format!("Ended {} of {} processes in the {} tree", order.len() - failed, order.len(), root.name)
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
pub fn process_menu(ui: &mut Ui, status: &Status, rows: &[ProcessRow], p: &ProcessRow, on_details: bool) {
    ui.label(RichText::new(format!("{} ({})", p.name, p.pid)).weak());
    if p.has_window && ui.button("Switch to").clicked() {
        match taskmgr_sys::switch_to(p.pid) {
            Ok(()) => status.used(),
            Err(e) => status.set(format!("Could not switch to {}: {e}", p.name)),
        }
        ui.close();
    }
    if ui.button("End task").clicked() {
        end_task(status, rows, p.pid, &p.name);
        ui.close();
    }
    if p.name.eq_ignore_ascii_case("explorer.exe") && ui.button("Restart").clicked() {
        restart_explorer(status, rows, p);
        ui.close();
    }
    if ui.button("End process tree").clicked() {
        end_tree(status, rows, p);
        ui.close();
    }
    let mut eco = taskmgr_sys::efficiency_mode(p.pid).unwrap_or(false);
    if ui.checkbox(&mut eco, "Efficiency mode").changed() {
        status.set(match taskmgr_sys::set_efficiency_mode(p.pid, eco) {
            Ok(()) => format!("Efficiency mode {} for {}", if eco { "on" } else { "off" }, p.name),
            Err(e) => format!("Could not change efficiency mode of {}: {e}", p.name),
        });
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

impl State {
    pub fn new(status: Status, group: bool, percent: bool) -> Self {
        Self {
            percent,
            filter: String::new(),
            sort: SortKey::default(),
            ascending: false,
            selected: None,
            group,
            collapsed: [false; 3],
            expanded: HashSet::new(),
            windir: std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()),
            status,
        }
    }

    pub fn show(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let rows = &snap.processes;
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("name or PID").desired_width(220.0));
            let can_end = self.selected.is_some();
            if ui.add_enabled(can_end, egui::Button::new("End task")).clicked() {
                self.end_selected(rows);
            }
            ui.checkbox(&mut self.group, "Group by type");
        });
        if self.selected.is_some() && ui.input(|i| i.key_pressed(egui::Key::Delete)) {
            self.end_selected(rows);
        }
        ui.add_space(4.0);

        let view = sorted_view(rows, &self.filter, self.sort, !self.ascending);
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
        let row_h = 22.0;

        let mut clicked_sort = None;
        let mut toggle_group = None;
        let mut toggle_app = None;
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().at_least(200.0).clip(true))
            .column(Column::exact(76.0))
            .column(Column::exact(64.0))
            .column(Column::exact(64.0))
            .column(Column::exact(90.0))
            .column(Column::exact(90.0))
            .column(Column::exact(90.0))
            .column(Column::exact(60.0))
            .column(Column::initial(150.0).clip(true))
            .header(row_h, |mut h| {
                for (key, title) in [
                    (Some(SortKey::Name), "Name"),
                    (None, "Status"),
                    (Some(SortKey::Pid), "PID"),
                    (Some(SortKey::Cpu), "CPU"),
                    (Some(SortKey::Memory), "Memory"),
                    (Some(SortKey::Disk), "Disk"),
                    (Some(SortKey::Network), "Network"),
                    (Some(SortKey::Gpu), "GPU"),
                    (None, "Publisher"),
                ] {
                    h.col(|ui| match key {
                        Some(key) => {
                            let arrow = if self.sort == key { if self.ascending { " ^" } else { " v" } } else { "" };
                            if ui.button(RichText::new(format!("{title}{arrow}")).strong()).clicked() {
                                clicked_sort = Some(key);
                            }
                        }
                        None => {
                            ui.label(RichText::new(title).strong());
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(row_h, lines.len(), |mut row| {
                    let p: &ProcessRow = match lines[row.index()] {
                        Line::Header(g, n) => {
                            let gi = g as usize;
                            row.col(|ui| {
                                let arrow = if self.collapsed[gi] { ">" } else { "v" };
                                ui.label(RichText::new(format!("{arrow} {} ({n})", g.label())).strong());
                            });
                            for _ in 0..8 {
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
                                if ui.small_button(if open { "v" } else { ">" }).clicked() {
                                    toggle_app = Some(p.pid);
                                }
                                ui.label(format!("{} ({})", display_name(p), n + 1)).on_hover_text(&p.name);
                            }
                            Some(_) => {
                                ui.add_space(18.0);
                                ui.label(display_name(p)).on_hover_text(&p.name);
                            }
                            None => {
                                ui.label(display_name(p)).on_hover_text(&p.name);
                            }
                        }
                    });
                    row.col(|ui| {
                        if p.suspended {
                            ui.label(RichText::new("Suspended").weak());
                        } else if p.efficiency {
                            ui.label(RichText::new("Efficiency").color(Color32::from_rgb(60, 170, 90)))
                                .on_hover_text("Efficiency mode: this process runs at low priority and power");
                        }
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
                        ui.label(if percent {
                            format!("{:.1}%", p.memory as f32 * 100.0 / mem_total)
                        } else {
                            format_bytes(p.memory as f64)
                        });
                    });
                    row.col(|ui| {
                        ui.painter().rect_filled(ui.max_rect(), 0.0, heat(ui, (p.disk_bps / 10e6) as f32));
                        ui.label(if percent {
                            format!("{:.1}%", disk_pct(p.disk_bps))
                        } else {
                            format!("{}/s", format_bytes(p.disk_bps))
                        });
                    });
                    row.col(|ui| {
                        if net_on {
                            ui.painter().rect_filled(ui.max_rect(), 0.0, heat(ui, (p.net_bps * 8.0 / 10e6) as f32));
                            ui.label(if percent {
                                format!("{:.1}%", net_pct(p.net_bps))
                            } else {
                                format_bits_per_sec(p.net_bps)
                            });
                        } else {
                            ui.label(RichText::new("-").weak()).on_hover_text(NET_HINT);
                        }
                    });
                    row.col(|ui| {
                        ui.painter().rect_filled(ui.max_rect(), 0.0, heat(ui, p.gpu_pct / 100.0));
                        ui.label(format!("{:.0}%", p.gpu_pct));
                    });
                    row.col(|ui| {
                        ui.label(&p.info.publisher);
                    });
                    let resp = row.response();
                    if resp.clicked() {
                        self.selected = Some((p.pid, p.name.clone()));
                    }
                    // Actions on a grouped app act on its root process, not the totals row.
                    let target = match lines[row.index()] {
                        Line::App(i, _, _) => &rows[i],
                        _ => p,
                    };
                    resp.context_menu(|ui| process_menu(ui, &self.status, rows, target, false));
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
            if self.sort == key {
                self.ascending = !self.ascending;
            } else {
                self.sort = key;
                // Names read best A→Z; numbers biggest first.
                self.ascending = matches!(key, SortKey::Name | SortKey::Pid);
            }
        }
    }

    pub fn set_group(&mut self, on: bool) {
        self.group = on;
    }

    fn end_selected(&mut self, rows: &[ProcessRow]) {
        if let Some((pid, name)) = self.selected.take() {
            end_task(&self.status, rows, pid, &name);
        }
    }
}
