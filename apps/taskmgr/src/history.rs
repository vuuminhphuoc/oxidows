//! App history: CPU time and network per app, from Windows' resource usage database (SRUM).
//! Reading it needs administrator and a few seconds, so it runs on a worker thread.
//! "Delete usage history" works like Task Manager's: it starts counting from now, by saving the
//! current totals as a baseline that is subtracted afterwards (Windows' own data is untouched).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use taskmgr_core::{AppHistory, AppUsage, format_bytes, format_ole_date};
use ui_common::{SORT_HEADER_HEIGHT, SortBy, cmp_text, sort_header};

use crate::details::format_cpu_time;

enum Load {
    Idle,
    Loading,
    Done(AppHistory),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Col {
    Name,
    Cpu,
    Network,
    Metered,
    Upload,
    Download,
}

pub type Sort = Col;

impl Col {
    pub const ALL: [Col; 6] = [Col::Name, Col::Cpu, Col::Network, Col::Metered, Col::Upload, Col::Download];
    pub const DEFAULT: [Col; 6] = [Col::Name, Col::Cpu, Col::Network, Col::Metered, Col::Upload, Col::Download];

    pub fn key(self) -> &'static str {
        match self {
            Col::Name => "name",
            Col::Cpu => "cpu",
            Col::Network => "network",
            Col::Metered => "metered",
            Col::Upload => "upload",
            Col::Download => "download",
        }
    }

    pub fn from_key(k: &str) -> Option<Col> {
        Col::ALL.into_iter().find(|s| s.key() == k)
    }

    pub fn title(self) -> &'static str {
        match self {
            Col::Name => "Name",
            Col::Cpu => "CPU time",
            Col::Network => "Network",
            Col::Metered => "Metered network",
            Col::Upload => "Uploads",
            Col::Download => "Downloads",
        }
    }

    fn column(self) -> Column {
        match self {
            Col::Name => Column::remainder().at_least(220.0).clip(true),
            _ => Column::exact(110.0),
        }
    }
}

pub fn parse_columns(v: &str) -> Vec<Col> {
    if v.trim().is_empty() {
        return Col::DEFAULT.to_vec();
    }
    let mut cols: Vec<Col> = v.split(',').filter_map(|k| Col::from_key(k.trim())).collect();
    if !cols.contains(&Col::Name) {
        cols.insert(0, Col::Name);
    }
    cols
}

pub fn columns_text(cols: &[Col]) -> String {
    cols.iter().map(|c| c.key()).collect::<Vec<_>>().join(",")
}

#[derive(Default, Clone)]
struct Baseline {
    /// OLE date the baseline was taken.
    at: f64,
    /// cpu cycles, bytes sent, bytes received, metered bytes.
    apps: HashMap<String, (u64, u64, u64, u64)>,
}

fn baseline_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("oxidows").join("app-history-baseline.tsv"))
}

impl Baseline {
    fn parse(text: &str) -> Baseline {
        let mut b = Baseline::default();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            match f.as_slice() {
                ["at", d] => b.at = d.parse().unwrap_or(0.0),
                [name, c, s, r, rest @ ..] => {
                    let n = |v: &str| v.parse().unwrap_or(0);
                    let m = rest.first().map_or(0, |v| n(v));
                    b.apps.insert((*name).to_string(), (n(c), n(s), n(r), m));
                }
                _ => {}
            }
        }
        b
    }

    fn to_text(&self) -> String {
        let mut out = format!("at\t{}\n", self.at);
        for (name, (c, s, r, m)) in &self.apps {
            out.push_str(&format!("{}\t{c}\t{s}\t{r}\t{m}\n", name.replace(['\t', '\n'], " ")));
        }
        out
    }

    fn load() -> Option<Baseline> {
        Some(Baseline::parse(&std::fs::read_to_string(baseline_path()?).ok()?))
    }
}

/// Today as an OLE automation date.
fn now_ole() -> f64 {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    secs / 86_400.0 + 25_569.0
}

pub struct State {
    load: Arc<Mutex<Load>>,
    ctx: egui::Context,
    pub(crate) filter: String,
    pub sort: SortBy<Col>,
    pub columns: Vec<Col>,
    pub show_all_processes: bool,
    baseline: Option<Baseline>,
    message: Option<String>,
    /// The app row clicked last (hover and selection, as on the other pages).
    selected: Option<String>,
}

impl State {
    pub fn new(ctx: egui::Context, columns: Vec<Col>, sort: Option<SortBy<Col>>, show_all_processes: bool) -> Self {
        Self {
            load: Arc::new(Mutex::new(Load::Idle)),
            ctx,
            filter: String::new(),
            sort: sort.unwrap_or(SortBy::new(Col::Cpu, true)),
            columns,
            show_all_processes,
            selected: None,
            baseline: Baseline::load(),
            message: None,
        }
    }
    pub fn start(&self) {
        if let Ok(mut l) = self.load.lock() {
            *l = Load::Loading;
        }
        let (load, ctx) = (self.load.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let t0 = std::time::Instant::now();
            let r = taskmgr_sys::app_history();
            if std::env::var_os("OXI_TASKMGR_TIMING").is_some() {
                match &r {
                    Ok(h) => {
                        let mut top = h.apps.clone();
                        top.sort_by_key(|a| std::cmp::Reverse(a.cpu_cycles));
                        let top: Vec<String> = top
                            .iter()
                            .take(4)
                            .map(|a| {
                                format!(
                                    "{} cpu {} net {}",
                                    a.name,
                                    a.cpu_time_100ns / 10_000_000,
                                    a.bytes_sent + a.bytes_received
                                )
                            })
                            .collect();
                        eprintln!(
                            "app history: {} apps since {:?} in {} ms: {}",
                            h.apps.len(),
                            h.since_ole_date.map(format_ole_date),
                            t0.elapsed().as_millis(),
                            top.join("; ")
                        );
                    }
                    Err(e) => eprintln!("app history failed after {} ms: {e}", t0.elapsed().as_millis()),
                }
            }
            if let Ok(mut l) = load.lock() {
                *l = match r {
                    Ok(h) => Load::Done(h),
                    Err(e) => Load::Failed(e.to_string()),
                };
            }
            ctx.request_repaint();
        });
    }

    pub fn delete_history(&mut self) {
        let apps = if let Ok(guard) = self.load.lock() {
            if let Load::Done(h) = &*guard { Some(h.apps.clone()) } else { None }
        } else {
            None
        };
        if let Some(apps) = apps {
            let b = Baseline {
                at: now_ole(),
                apps: apps
                    .iter()
                    .map(|a| (a.name.clone(), (a.cpu_cycles, a.bytes_sent, a.bytes_received, a.metered_bytes)))
                    .collect(),
            };
            let saved = baseline_path().map(|p| {
                if let Some(parent) = p.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::write(p, b.to_text())
            });
            self.message = Some(match saved {
                Some(Ok(())) => "Usage history cleared. Counting starts now.".into(),
                Some(Err(e)) => format!("Could not save: {e}"),
                None => "No settings folder".into(),
            });
            self.baseline = Some(b);
        }
    }

    pub fn commands(&mut self, ui: &mut Ui) {
        if ui_common::command_button(
            ui,
            ui_common::Icon::Delete,
            "Delete usage history",
            true,
            "Start counting from now",
        )
        .clicked()
        {
            self.delete_history();
        }
    }

    pub fn show(&mut self, ui: &mut Ui, elevated: bool) -> bool {
        let before = self.columns.clone();
        if !elevated {
            ui.add_space(8.0);
            ui.label("App history comes from Windows' resource usage database, which only administrators can read.");
            ui.label("Use \"Run as administrator\" at the top to see it.");
            return false;
        }
        let idle = matches!(self.load.lock().as_deref(), Ok(Load::Idle));
        if idle {
            self.start();
        }
        let guard = self.load.lock();
        let Ok(load) = guard.as_deref() else {
            return false;
        };
        let mut refresh = false;
        match load {
            Load::Idle | Load::Loading => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Reading usage history (a shadow copy of SRUDB.dat)...");
                });
            }
            Load::Failed(e) => {
                ui.colored_label(ui.visuals().error_fg_color, format!("Could not read App history: {e}"));
                refresh = ui.button("Try again").clicked();
            }
            Load::Done(h) => {
                let base = self.baseline.as_ref();
                let since = match (base, h.since_ole_date) {
                    (Some(b), _) => Some(b.at),
                    (None, d) => d,
                };
                ui.horizontal(|ui| {
                    ui.label(match since {
                        Some(d) => {
                            format!("Resource usage since {} for current user and all accounts.", format_ole_date(d))
                        }
                        None => "No usage recorded yet.".to_string(),
                    });
                });
                if let Some(m) = &self.message {
                    ui.label(RichText::new(m).weak());
                }
                ui.add_space(4.0);
                let apps = self.view(&h.apps);
                let mut clicked = None;
                let mut pick = None;
                let cursor = self.selected.as_ref().and_then(|name| apps.iter().position(|a| &a.name == name));
                let mut scroll_to = None;
                if let Some(new_i) = crate::keys::list_keys(ui, apps.len(), cursor, |k| apps[k].name.clone()) {
                    self.selected = Some(apps[new_i].name.clone());
                    scroll_to = Some(new_i);
                }
                let cols = self.columns.clone();
                let ctx = ui.ctx().clone();
                let pointer_pos = ui.input(|i| i.pointer.hover_pos());
                let mut table = TableBuilder::new(ui)
                    .resizable(true)
                    .sense(egui::Sense::click())
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
                for (idx, &c) in cols.iter().enumerate() {
                    table = table.column(if idx == 0 {
                        Column::remainder().at_least(220.0).clip(true)
                    } else {
                        c.column()
                    });
                }
                if let Some(i) = scroll_to {
                    table = table.scroll_to_row(i, Some(egui::Align::Center));
                }
                table
                    .header(SORT_HEADER_HEIGHT, |mut hd| {
                        for &c in &cols {
                            hd.col(|ui| {
                                let resp = sort_header(ui, c.title(), self.sort.arrow(c));
                                if resp.clicked() {
                                    clicked = Some(c);
                                }
                                resp.context_menu(|ui| {
                                    ui_common::column_chooser_menu(
                                        ui,
                                        &Col::ALL,
                                        &mut self.columns,
                                        Some(("Name", Col::Name)),
                                        |c| c.title(),
                                    );
                                });
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(22.0, apps.len(), |mut row| {
                            let a = &apps[row.index()];
                            let is_sel = self.selected.as_deref() == Some(a.name.as_str());
                            row.set_selected(is_sel);
                            let _ = pointer_pos;
                            for &c in &cols {
                                row.col(|ui| match c {
                                    Col::Name => {
                                        ui.label(&a.name);
                                    }
                                    Col::Cpu => {
                                        ui.label(format_cpu_time(a.cpu_time_100ns));
                                    }
                                    Col::Network => {
                                        ui.label(format_bytes((a.bytes_sent + a.bytes_received) as f64));
                                    }
                                    Col::Metered => {
                                        ui.label(format_bytes(a.metered_bytes as f64));
                                    }
                                    Col::Upload => {
                                        ui.label(format_bytes(a.bytes_sent as f64));
                                    }
                                    Col::Download => {
                                        ui.label(format_bytes(a.bytes_received as f64));
                                    }
                                });
                            }
                            let resp = row.response();
                            if resp.clicked() {
                                pick = Some(a.name.clone());
                            }
                            if resp.hovered() {
                                ctx.request_repaint();
                            }
                        });
                    });
                if pick.is_some() {
                    self.selected = pick;
                }
                if let Some(k) = clicked {
                    self.sort.click(k, k != Sort::Name);
                }
            }
        }
        drop(guard);
        if refresh {
            self.start();
        }
        if !self.columns.contains(&self.sort.key) {
            self.sort = SortBy::new(Col::Cpu, true);
        }
        self.columns != before
    }

    /// Apps after the baseline is subtracted, filtered and sorted; apps with no use are dropped.
    fn view(&self, apps: &[AppUsage]) -> Vec<AppUsage> {
        let f = self.filter.trim().to_lowercase();
        let mut v: Vec<AppUsage> = apps
            .iter()
            .filter(|a| self.show_all_processes || a.packaged)
            .map(|a| {
                let mut a = a.clone();
                if let Some((c, s, r, m)) = self.baseline.as_ref().and_then(|b| b.apps.get(&a.name)) {
                    let ratio = if a.cpu_cycles == 0 { 0.0 } else { a.cpu_time_100ns as f64 / a.cpu_cycles as f64 };
                    a.cpu_cycles = a.cpu_cycles.saturating_sub(*c);
                    a.cpu_time_100ns = (a.cpu_cycles as f64 * ratio) as u64;
                    a.bytes_sent = a.bytes_sent.saturating_sub(*s);
                    a.bytes_received = a.bytes_received.saturating_sub(*r);
                    a.metered_bytes = a.metered_bytes.saturating_sub(*m);
                }
                a
            })
            .filter(|a| a.cpu_cycles + a.bytes_sent + a.bytes_received > 0)
            .filter(|a| f.is_empty() || a.name.to_lowercase().contains(&f))
            .collect();
        v.sort_by(|a, b| {
            let o = match self.sort.key {
                Sort::Name => cmp_text(&a.name, &b.name),
                Sort::Cpu => a.cpu_cycles.cmp(&b.cpu_cycles),
                Sort::Network => (a.bytes_sent + a.bytes_received).cmp(&(b.bytes_sent + b.bytes_received)),
                Sort::Upload => a.bytes_sent.cmp(&b.bytes_sent),
                Sort::Download => a.bytes_received.cmp(&b.bytes_received),
                Sort::Metered => a.metered_bytes.cmp(&b.metered_bytes),
            };
            self.sort.apply(o).then_with(|| cmp_text(&a.name, &b.name))
        });
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_round_trip() {
        let mut b = Baseline { at: 46_000.5, ..Default::default() };
        b.apps.insert("chrome.exe".into(), (10, 20, 30, 5));
        let p = Baseline::parse(&b.to_text());
        assert_eq!(p.at, 46_000.5);
        assert_eq!(p.apps.get("chrome.exe"), Some(&(10, 20, 30, 5)));
        assert_eq!(Baseline::parse("x\t1\t2\t3\n").apps.get("x"), Some(&(1, 2, 3, 0)));
    }

    #[test]
    fn columns_round_trip() {
        let cols = vec![Col::Name, Col::Cpu, Col::Metered];
        assert_eq!(parse_columns(&columns_text(&cols)), cols);
        assert_eq!(parse_columns(""), Col::DEFAULT.to_vec());
    }
}
