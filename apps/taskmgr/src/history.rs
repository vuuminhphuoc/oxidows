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

use crate::details::format_cpu_time;

enum Load {
    Idle,
    Loading,
    Done(AppHistory),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Name,
    Cpu,
    Network,
    Upload,
    Download,
    Metered,
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
    filter: String,
    sort: Sort,
    descending: bool,
    baseline: Option<Baseline>,
    message: Option<String>,
}

impl State {
    pub fn new(ctx: egui::Context) -> Self {
        Self {
            load: Arc::new(Mutex::new(Load::Idle)),
            ctx,
            filter: String::new(),
            sort: Sort::Cpu,
            descending: true,
            baseline: Baseline::load(),
            message: None,
        }
    }

    fn start(&self) {
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

    pub fn show(&mut self, ui: &mut Ui, elevated: bool) {
        if !elevated {
            ui.add_space(8.0);
            ui.label("App history comes from Windows' resource usage database, which only administrators can read.");
            ui.label("Use \"Run as administrator\" at the top to see it.");
            return;
        }
        let idle = matches!(self.load.lock().as_deref(), Ok(Load::Idle));
        if idle {
            self.start();
        }
        let guard = self.load.lock();
        let Ok(load) = guard.as_deref() else { return };
        let mut refresh = false;
        let mut delete = None;
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
                    refresh = ui.button("Refresh").clicked();
                    if ui.button("Delete usage history").on_hover_text("Start counting from now").clicked() {
                        delete = Some(h.apps.clone());
                    }
                });
                if let Some(m) = &self.message {
                    ui.label(RichText::new(m).weak());
                }
                ui.horizontal(|ui| {
                    ui.label("Search:");
                    ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("app name").desired_width(220.0));
                });
                ui.add_space(4.0);
                let apps = self.view(&h.apps);
                let mut clicked = None;
                TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::remainder().at_least(220.0).clip(true))
                    .columns(Column::exact(110.0), 5)
                    .header(22.0, |mut hd| {
                        for (k, t) in [
                            (Sort::Name, "Name"),
                            (Sort::Cpu, "CPU time"),
                            (Sort::Network, "Network"),
                            (Sort::Metered, "Metered network"),
                            (Sort::Upload, "Uploads"),
                            (Sort::Download, "Downloads"),
                        ] {
                            hd.col(|ui| {
                                let arrow = if self.sort == k { if self.descending { " v" } else { " ^" } } else { "" };
                                if ui.button(RichText::new(format!("{t}{arrow}")).strong()).clicked() {
                                    clicked = Some(k);
                                }
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(22.0, apps.len(), |mut row| {
                            let a = &apps[row.index()];
                            row.col(|ui| {
                                ui.label(&a.name);
                            });
                            row.col(|ui| {
                                ui.label(format_cpu_time(a.cpu_time_100ns));
                            });
                            row.col(|ui| {
                                ui.label(format_bytes((a.bytes_sent + a.bytes_received) as f64));
                            });
                            row.col(|ui| {
                                ui.label(format_bytes(a.metered_bytes as f64));
                            });
                            row.col(|ui| {
                                ui.label(format_bytes(a.bytes_sent as f64));
                            });
                            row.col(|ui| {
                                ui.label(format_bytes(a.bytes_received as f64));
                            });
                        });
                    });
                if let Some(k) = clicked {
                    if self.sort == k {
                        self.descending = !self.descending;
                    } else {
                        self.sort = k;
                        self.descending = k != Sort::Name;
                    }
                }
            }
        }
        drop(guard);
        if let Some(apps) = delete {
            let b = Baseline {
                at: now_ole(),
                apps: apps
                    .iter()
                    .map(|a| (a.name.clone(), (a.cpu_cycles, a.bytes_sent, a.bytes_received, a.metered_bytes)))
                    .collect(),
            };
            let saved = baseline_path().map(|p| {
                p.parent().map(std::fs::create_dir_all);
                std::fs::write(p, b.to_text())
            });
            self.message = Some(match saved {
                Some(Ok(())) => "Usage history cleared. Counting starts now.".into(),
                Some(Err(e)) => format!("Could not save: {e}"),
                None => "No settings folder".into(),
            });
            self.baseline = Some(b);
        }
        if refresh {
            self.start();
        }
    }

    /// Apps after the baseline is subtracted, filtered and sorted; apps with no use are dropped.
    fn view(&self, apps: &[AppUsage]) -> Vec<AppUsage> {
        let f = self.filter.trim().to_lowercase();
        let mut v: Vec<AppUsage> = apps
            .iter()
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
            let o = match self.sort {
                Sort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                Sort::Cpu => a.cpu_cycles.cmp(&b.cpu_cycles),
                Sort::Network => (a.bytes_sent + a.bytes_received).cmp(&(b.bytes_sent + b.bytes_received)),
                Sort::Upload => a.bytes_sent.cmp(&b.bytes_sent),
                Sort::Download => a.bytes_received.cmp(&b.bytes_received),
                Sort::Metered => a.metered_bytes.cmp(&b.metered_bytes),
            };
            if self.descending { o.reverse() } else { o }
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
}
