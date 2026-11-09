//! Startup apps tab: Run keys and Startup folders; enable / disable like the built-in tool.

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use std::collections::HashMap;

use taskmgr_core::{Snapshot, StartupItem, format_bytes, startup_impact};
use ui_common::{SORT_HEADER_HEIGHT, SortBy, cmp_text, sort_header};

use crate::Status;

/// The columns, in order; any of them sorts the list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Col {
    Name,
    Publisher,
    Status,
    Impact,
    Cpu,
    Disk,
    Command,
    Location,
}

impl Col {
    const ALL: [Col; 8] =
        [Col::Name, Col::Publisher, Col::Status, Col::Impact, Col::Cpu, Col::Disk, Col::Command, Col::Location];

    pub fn key(self) -> &'static str {
        match self {
            Col::Name => "name",
            Col::Publisher => "publisher",
            Col::Status => "status",
            Col::Impact => "impact",
            Col::Cpu => "cpu",
            Col::Disk => "disk",
            Col::Command => "command",
            Col::Location => "location",
        }
    }

    pub fn from_key(k: &str) -> Option<Col> {
        Col::ALL.into_iter().find(|c| c.key() == k)
    }

    fn title(self) -> &'static str {
        match self {
            Col::Name => "Name",
            Col::Publisher => "Publisher",
            Col::Status => "Status",
            Col::Impact => "Startup impact",
            Col::Cpu => "CPU at startup",
            Col::Disk => "Disk I/O at startup",
            Col::Command => "Command",
            Col::Location => "Registered in",
        }
    }

    /// Text columns sort A to Z first; impact and costs biggest first.
    fn descending_first(self) -> bool {
        matches!(self, Col::Impact | Col::Cpu | Col::Disk)
    }
}

/// "Startup impact" as shown: disabled apps have none, unmeasured ones say so.
fn impact(it: &StartupItem, cost: Option<(u64, u64)>) -> &'static str {
    match cost {
        _ if !it.enabled => "None",
        Some((cpu, io)) => startup_impact(cpu, io),
        None => "Not measured",
    }
}

fn impact_rank(label: &str) -> i8 {
    match label {
        "High" => 3,
        "Medium" => 2,
        "Low" => 1,
        "None" => 0,
        _ => -1,
    }
}

/// Indices of `items` in the order of `sort`, name breaking ties.
fn sorted(items: &[StartupItem], cost: &[Option<(u64, u64)>], sort: SortBy<Col>) -> Vec<usize> {
    let mut v: Vec<usize> = (0..items.len()).collect();
    v.sort_by(|&i, &j| {
        let (a, b) = (&items[i], &items[j]);
        let o = match sort.key {
            Col::Name => cmp_text(&a.name, &b.name),
            Col::Publisher => cmp_text(&a.publisher, &b.publisher),
            Col::Status => a.enabled.cmp(&b.enabled),
            Col::Impact => impact_rank(impact(a, cost[i])).cmp(&impact_rank(impact(b, cost[j]))),
            Col::Cpu => cost[i].map(|c| c.0).cmp(&cost[j].map(|c| c.0)),
            Col::Disk => cost[i].map(|c| c.1).cmp(&cost[j].map(|c| c.1)),
            Col::Command => cmp_text(&a.command, &b.command),
            Col::Location => cmp_text(&a.location, &b.location),
        };
        sort.apply(o).then_with(|| cmp_text(&a.name, &b.name))
    });
    v
}

pub struct State {
    pub(crate) filter: String,
    items: Option<Vec<StartupItem>>,
    pub sort: SortBy<Col>,
    selected: Option<usize>,
    bios_ms: Option<u32>,
    status: Status,
    /// CPU time (100 ns) and disk I/O bytes per startup exe (lower-case path), from the first
    /// time a matching process that began soon after boot was seen; read again on Refresh.
    measured: HashMap<String, (u64, u64)>,
}

/// Processes that started within this long after boot count as started by sign-in.
const STARTUP_WINDOW_100NS: u64 = 5 * 60 * 10_000_000;

fn now_filetime() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| (d.as_nanos() / 100) as u64 + 116_444_736_000_000_000)
}

impl State {
    pub fn new(status: Status, sort: Option<SortBy<Col>>) -> Self {
        Self {
            filter: String::new(),
            items: None,
            sort: sort.unwrap_or(SortBy::new(Col::Name, false)),
            selected: None,
            bios_ms: taskmgr_sys::last_bios_ms(),
            status,
            measured: HashMap::new(),
        }
    }

    pub fn reload(&mut self) {
        self.items = None;
        self.measured.clear();
    }

    pub fn set_selected_enabled(&mut self, on: bool) {
        let Some(i) = self.selected else { return };
        let Some(items) = self.items.as_ref() else { return };
        let Some(it) = items.get(i) else { return };
        match taskmgr_sys::set_startup_enabled(it, on) {
            Ok(()) => {
                self.status.set(format!("{} {}", if on { "Enabled" } else { "Disabled" }, it.name));
                self.reload();
            }
            Err(e) if !it.per_user => {
                self.status.set(format!("{}: needs administrator ({e})", it.name));
            }
            Err(e) => self.status.set(format!("Could not change {}: {e}", it.name)),
        }
    }

    pub fn commands(&mut self, ui: &mut Ui, snap: &Snapshot) {
        if self.measured.is_empty() || self.items.is_none() {
            self.measure(snap);
        }
        let items = self.items.get_or_insert_with(taskmgr_sys::startup_items);
        let sel = self.selected.and_then(|i| items.get(i));
        let can_enable = sel.is_some_and(|s| !s.enabled);
        let can_disable = sel.is_some_and(|s| s.enabled);

        if ui_common::command_button(ui, ui_common::Icon::Close, "Disable", can_disable, "").clicked() {
            self.set_selected_enabled(false);
        }
        if ui_common::command_button(ui, ui_common::Icon::CheckMark, "Enable", can_enable, "").clicked() {
            self.set_selected_enabled(true);
        }
    }

    /// Records the startup cost of processes that sign-in started. Windows measures the boot itself;
    /// here the process's own totals stand in, so the earlier this runs after sign-in, the closer.
    fn measure(&mut self, snap: &Snapshot) {
        let boot = now_filetime().saturating_sub(snap.system.uptime.as_nanos() as u64 / 100);
        for p in &snap.processes {
            if p.info.path.is_empty() || p.start_time.saturating_sub(boot) > STARTUP_WINDOW_100NS {
                continue;
            }
            let e = self.measured.entry(p.info.path.to_lowercase()).or_insert((0, 0));
            e.0 = e.0.max(p.cpu_time);
            e.1 = e.1.max(p.io_read_bytes + p.io_write_bytes);
        }
    }

    pub fn show(&mut self, ui: &mut Ui, snap: &Snapshot) {
        if self.measured.is_empty() || self.items.is_none() {
            self.measure(snap);
        }
        // Registry reads take well under a millisecond: load on first view, reload on demand.
        let items = self.items.get_or_insert_with(taskmgr_sys::startup_items);
        let cost: Vec<Option<(u64, u64)>> = items
            .iter()
            .map(|it| self.measured.get(&taskmgr_sys::startup_exe(&it.command).to_lowercase()).copied())
            .collect();
        let view = sorted(items, &cost, self.sort);
        let f = self.filter.trim().to_lowercase();
        let view: Vec<usize> = view
            .into_iter()
            .filter(|&i| {
                let it = &items[i];
                f.is_empty() || it.name.to_lowercase().contains(&f) || it.publisher.to_lowercase().contains(&f)
            })
            .collect();
        let mut toggle = None;
        let mut reload = false;
        let mut clicked = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} startup apps", items.len())).weak());
            if let Some(ms) = self.bios_ms {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("Last BIOS time: {:.1} seconds", ms as f32 / 1000.0));
                });
            }
        });
        ui.add_space(4.0);
        if items.is_empty() {
            ui.label("No startup apps found.");
        }
        let cursor = self.selected.and_then(|sel| view.iter().position(|&i| i == sel));
        let mut scroll_to = None;
        if let Some(new_k) = crate::keys::list_keys(ui, view.len(), cursor, |k| items[view[k]].name.clone()) {
            self.selected = Some(view[new_k]);
            scroll_to = Some(new_k);
        }
        let mut table = TableBuilder::new(ui)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(200.0).clip(true))
            .column(Column::initial(160.0).clip(true))
            .column(Column::exact(80.0))
            .columns(Column::exact(100.0), 3)
            .column(Column::initial(300.0).clip(true))
            .column(Column::remainder().clip(true));
        if let Some(pos) = scroll_to {
            table = table.scroll_to_row(pos, Some(egui::Align::Center));
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
                    let i = view[row.index()];
                    let it = &items[i];
                    row.set_selected(self.selected == Some(i));
                    row.col(|ui| {
                        ui.label(&it.name);
                    });
                    row.col(|ui| {
                        ui.label(&it.publisher);
                    });
                    row.col(|ui| {
                        ui.label(if it.enabled { "Enabled" } else { "Disabled" });
                    });
                    let c = cost[i];
                    row.col(|ui| {
                        ui.label(impact(it, c))
                            .on_hover_text("From the app's own CPU and disk use since it started at sign-in");
                    });
                    row.col(|ui| {
                        ui.label(c.map_or(String::new(), |(cpu, _)| format!("{} ms", cpu / 10_000)));
                    });
                    row.col(|ui| {
                        ui.label(c.map_or(String::new(), |(_, io)| format_bytes(io as f64)));
                    });
                    row.col(|ui| {
                        ui.label(&it.command).on_hover_text(&it.command);
                    });
                    row.col(|ui| {
                        ui.label(&it.location);
                    });
                    let resp = row.response();
                    if resp.clicked() {
                        self.selected = Some(i);
                    }
                    resp.context_menu(|ui| {
                        let label = if it.enabled { "Disable" } else { "Enable" };
                        if ui.button(label).clicked() {
                            toggle = Some((i, !it.enabled));
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Open file location").clicked() {
                            let _ = taskmgr_sys::open_file_location(&taskmgr_sys::startup_exe(&it.command));
                            ui.close();
                        }
                        if ui.button("Search online").clicked() {
                            let _ = taskmgr_sys::search_online(&it.name);
                            ui.close();
                        }
                        if ui.button("Properties").clicked() {
                            let _ = taskmgr_sys::file_properties(&taskmgr_sys::startup_exe(&it.command));
                            ui.close();
                        }
                    });
                });
            });
        if let Some(c) = clicked {
            self.sort.click(c, c.descending_first());
        }
        if let Some((i, on)) = toggle
            && let Some(it) = items.get(i)
        {
            match taskmgr_sys::set_startup_enabled(it, on) {
                Ok(()) => {
                    self.status.set(format!("{} {}", if on { "Enabled" } else { "Disabled" }, it.name));
                    reload = true;
                }
                Err(e) if !it.per_user => {
                    self.status.set(format!("{}: needs administrator ({e})", it.name));
                }
                Err(e) => self.status.set(format!("Could not change {}: {e}", it.name)),
            }
        }
        if reload {
            self.items = None;
            self.measured.clear();
        }
    }
}
