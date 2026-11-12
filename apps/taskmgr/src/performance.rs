//! Performance tab: a list of resources with sparklines on the left (CPU, memory, each disk,
//! each network adapter, GPU), the selected one large on the right with its figures underneath.

use egui::{Color32, RichText, Ui, Vec2};
use std::time::{Duration, Instant};

use taskmgr_core::{
    AdapterInfo, CpuInfo, DiskInfo, MemoryInfo, Ring, Snapshot, format_bits_per_sec, format_bytes, format_uptime,
};

use crate::graph;

const CPU: Color32 = Color32::from_rgb(17, 125, 187);
const MEM: Color32 = Color32::from_rgb(139, 18, 174);
const DISK: Color32 = Color32::from_rgb(77, 166, 12);
const DISK_WRITE: Color32 = Color32::from_rgb(40, 110, 8);
const NET: Color32 = Color32::from_rgb(167, 79, 1);
const NET_TX: Color32 = Color32::from_rgb(220, 140, 40);
const GPU: Color32 = Color32::from_rgb(10, 140, 120);

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum CpuViewMode {
    #[default]
    Overall,
    LogicalProcessors,
    NumaNodes,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum Res {
    #[default]
    Cpu,
    Memory,
    /// Index into `disks`; `usize::MAX` = the combined figure when PDH has no per-disk data.
    Disk(usize),
    Adapter(usize),
    /// Index into `gpus`; `usize::MAX` = the combined figure when GPUs cannot be told apart.
    Gpu(usize),
    /// Frames per second of the app drawing the most (not in Windows' Task Manager).
    Fps,
}

pub struct State {
    selected: Res,
    /// Only the resource list, without tabs or details (double-click to leave).
    pub summary: bool,
    /// Graph-only view (double-click or context menu to leave).
    pub graph_summary: bool,
    /// One graph per logical processor (saved as `cpu_logical`).
    pub per_core: bool,
    /// Draw kernel-mode time over the CPU graphs (saved as `cpu_kernel`).
    pub kernel: bool,
    /// Overall utilization / Logical processors / NUMA nodes.
    pub cpu_view: CpuViewMode,
    cpu: CpuInfo,
    /// Addresses of the shown adapter, re-read every few seconds.
    adapter: Option<(String, Instant, AdapterInfo)>,
    /// Model, capacity and type per disk number, read once.
    disk_info: std::collections::HashMap<u32, Option<DiskInfo>>,
    /// Speed and slots from SMBIOS, read once.
    memory: Option<MemoryInfo>,
    /// Temperatures by "disk N" / GPU LUID, re-read every two seconds.
    temps: std::collections::HashMap<String, (Instant, Option<f32>)>,
    fps: crate::fps::State,
    /// Selected engine per slot (up to 4) per GPU index.
    gpu_slots: std::collections::HashMap<usize, [String; 4]>,
    /// 60 s history rings per engine name, per GPU index.
    gpu_engine_history: std::collections::HashMap<usize, std::collections::HashMap<String, Ring>>,
    /// 60 s history rings for dedicated and shared memory: gpu_index -> (dedicated, shared).
    gpu_mem_history: std::collections::HashMap<usize, (Ring, Ring)>,
    last_uptime: Option<Duration>,
}

impl State {
    /// Starts with the CPU graph menu's "Logical processors" / "Show kernel times" as saved.
    pub fn with_cpu_view(per_core: bool, kernel: bool) -> Self {
        let mut s = Self { kernel, ..Self::default() };
        s.set_per_core(per_core);
        s
    }

    /// Applies the saved `cpu_logical` choice (also used by Settings > Reset).
    pub fn set_per_core(&mut self, per_core: bool) {
        self.per_core = per_core;
        self.cpu_view = if per_core { CpuViewMode::LogicalProcessors } else { CpuViewMode::Overall };
    }

    fn attach_graph_menu(&mut self, _ui: &mut Ui, resp: &egui::Response, summary_text: &str, is_cpu: bool) {
        let summary_clone = summary_text.to_string();
        let mut numa_count = self.cpu.numa_nodes.clone();
        numa_count.sort_unstable();
        numa_count.dedup();
        let has_numa = numa_count.len() > 1;

        resp.context_menu(|ui| {
            ui.checkbox(&mut self.graph_summary, "Graph summary view");
            if is_cpu {
                ui.menu_button("Change graph to", |ui| {
                    if ui.radio_value(&mut self.cpu_view, CpuViewMode::Overall, "Overall utilization").clicked() {
                        self.per_core = false;
                        ui.close();
                    }
                    if ui
                        .radio_value(&mut self.cpu_view, CpuViewMode::LogicalProcessors, "Logical processors")
                        .clicked()
                    {
                        self.per_core = true;
                        ui.close();
                    }
                    ui.add_enabled_ui(has_numa, |ui| {
                        if ui.radio_value(&mut self.cpu_view, CpuViewMode::NumaNodes, "NUMA nodes").clicked() {
                            self.per_core = false;
                            ui.close();
                        }
                    });
                });
                ui.checkbox(&mut self.kernel, "Show kernel times");
            }
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(summary_clone);
                ui.close();
            }
        });
    }

    /// Appends the newest per-engine and memory samples (once per snapshot) to the 60 s GPU rings.
    fn update_gpu_history(&mut self, snap: &Snapshot) {
        if self.last_uptime == Some(snap.system.uptime) {
            return;
        }
        self.last_uptime = Some(snap.system.uptime);
        let s = &snap.system;
        // Without per-adapter data the machine-wide figures live under one pseudo adapter.
        let mut push = |idx: usize, ded: Option<u64>, shared: Option<u64>, engines: &[(String, f32)]| {
            let (ded_ring, shared_ring) = self.gpu_mem_history.entry(idx).or_default();
            ded_ring.push(ded.unwrap_or(0) as f32);
            shared_ring.push(shared.unwrap_or(0) as f32);
            taskmgr_core::push_engine_rings(self.gpu_engine_history.entry(idx).or_default(), engines);
        };
        if s.gpus.is_empty() {
            push(usize::MAX, s.gpu_dedicated_used, s.gpu_shared_used, &s.gpu_engines);
        } else {
            for (i, g) in s.gpus.iter().enumerate() {
                push(i, g.dedicated_used, g.shared_used, &g.engines);
            }
        }
    }

    fn temp(&mut self, key: String, read: impl FnOnce() -> Option<f32>) -> Option<f32> {
        match self.temps.get(&key) {
            Some((at, t)) if at.elapsed() < Duration::from_secs(2) => *t,
            _ => {
                let t = read();
                self.temps.insert(key, (Instant::now(), t));
                t
            }
        }
    }
}

fn celsius(t: Option<f32>) -> String {
    t.map_or("-".into(), |t| format!("{t:.0} °C"))
}

impl Default for State {
    fn default() -> Self {
        // OXI_TASKMGR_RES picks the page shown first: memory, disk0, net0, gpu0... (used by smoke tests).
        let v = std::env::var("OXI_TASKMGR_RES").unwrap_or_default().to_lowercase();
        let n = |p: &str| v.strip_prefix(p).and_then(|r| r.parse::<usize>().ok()).unwrap_or(0);
        let selected = match v.as_str() {
            "memory" => Res::Memory,
            _ if v.starts_with("disk") => Res::Disk(n("disk")),
            _ if v.starts_with("net") => Res::Adapter(n("net")),
            _ if v.starts_with("gpu") => Res::Gpu(n("gpu")),
            "fps" => Res::Fps,
            _ => Res::Cpu,
        };
        Self {
            summary: std::env::var_os("OXI_TASKMGR_SUMMARY").is_some(),
            graph_summary: false,
            selected,
            per_core: false,
            kernel: false,
            cpu_view: CpuViewMode::Overall,
            cpu: taskmgr_sys::cpu_info(),
            adapter: None,
            disk_info: Default::default(),
            memory: None,
            temps: Default::default(),
            fps: Default::default(),
            gpu_slots: Default::default(),
            gpu_engine_history: Default::default(),
            gpu_mem_history: Default::default(),
            last_uptime: None,
        }
    }
}
fn pct(used: u64, total: u64) -> f32 {
    if total == 0 { 0.0 } else { (used as f64 * 100.0 / total as f64) as f32 }
}

fn opt_bytes(v: Option<u64>) -> String {
    v.map_or("-".into(), |b| format_bytes(b as f64))
}

fn cache(b: u64) -> String {
    if b == 0 { "-".into() } else { format_bytes(b as f64) }
}

struct Entry<'a> {
    res: Res,
    title: String,
    value: String,
    ring: &'a Ring,
    max: f32,
    color: Color32,
}

impl State {
    /// The resource entries with sparklines; in summary view they stretch to the window width.
    fn list(&mut self, ui: &mut Ui, entries: &[Entry]) {
        let spark =
            if self.summary { Vec2::new((ui.available_width() * 0.45).max(70.0), 44.0) } else { Vec2::new(70.0, 44.0) };
        let pal = ui_common::Palette::of(ui);
        for e in entries {
            let sel = self.selected == e.res && !self.summary;
            // Each item spans the whole column (as in Windows 11), so the full row is the click
            // target; its fill is painted behind the content once the hover state is known.
            let bg = ui.painter().add(egui::Shape::Noop);
            let r = egui::Frame::new()
                .inner_margin(6)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        graph::area(ui, spark, e.ring, e.max, e.color, false);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&e.title).strong());
                            ui.label(RichText::new(&e.value).small());
                        });
                    });
                })
                .response
                .interact(egui::Sense::click());
            if sel {
                ui.painter().set(bg, egui::Shape::rect_filled(r.rect, 4.0, pal.selected));
            } else {
                let t = ui_common::motion::fade(ui.ctx(), r.id, r.hovered());
                if t > 0.0 {
                    let fill = ui_common::motion::lerp_color(egui::Color32::TRANSPARENT, pal.hover, t);
                    ui.painter().set(bg, egui::Shape::rect_filled(r.rect, 4.0, fill));
                }
            }
            let r = r.on_hover_text(format!("{}\n{}", e.title, e.value));
            if r.clicked() {
                self.selected = e.res;
            }
            if r.double_clicked() {
                self.selected = e.res;
                self.summary = !self.summary;
            }
            r.context_menu(|ui| {
                if ui.checkbox(&mut self.summary, "Summary view").clicked() {
                    ui.close();
                }
            });
        }
    }

    pub fn show(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let s = &snap.system;
        let h = &snap.history;
        let net_max = |rx: &Ring, tx: &Ring| rx.max().max(tx.max()).max(1024.0);
        let disk_max = h.disk_bps.max().max(1024.0 * 1024.0);

        let mut entries = vec![
            Entry {
                res: Res::Cpu,
                title: "CPU".into(),
                value: match s.cpu_mhz {
                    Some(mhz) => format!("{:.0}%  {:.2} GHz", s.cpu_pct, mhz / 1000.0),
                    None => format!("{:.0}%", s.cpu_pct),
                },
                ring: &h.cpu,
                max: 100.0,
                color: CPU,
            },
            Entry {
                res: Res::Memory,
                title: "Memory".into(),
                value: format!(
                    "{}/{} ({:.0}%)",
                    format_bytes(s.mem_used as f64),
                    format_bytes(s.mem_total as f64),
                    pct(s.mem_used, s.mem_total)
                ),
                ring: &h.mem_pct,
                max: 100.0,
                color: MEM,
            },
        ];
        if s.disks.is_empty() {
            let (ring, max) =
                if s.disk_active_pct.is_some() { (&h.disk_active_pct, 100.0) } else { (&h.disk_bps, disk_max) };
            entries.push(Entry {
                res: Res::Disk(usize::MAX),
                title: "Disk".into(),
                value: match s.disk_active_pct {
                    Some(p) => format!("{p:.0}%"),
                    None => format!("{}/s", format_bytes(s.disk_read_bps + s.disk_write_bps)),
                },
                ring,
                max,
                color: DISK,
            });
        }
        for (i, d) in s.disks.iter().enumerate() {
            if let Some(ring) = h.disks.get(i) {
                entries.push(Entry {
                    res: Res::Disk(i),
                    title: disk_title(&d.name),
                    value: format!("{:.0}%", d.active_pct),
                    ring,
                    max: 100.0,
                    color: DISK,
                });
            }
        }
        if s.adapters.is_empty() {
            entries.push(Entry {
                res: Res::Adapter(usize::MAX),
                title: "Network".into(),
                value: format!("S: {}  R: {}", format_bits_per_sec(s.net_tx_bps), format_bits_per_sec(s.net_rx_bps)),
                ring: &h.net_rx_bps,
                max: net_max(&h.net_rx_bps, &h.net_tx_bps),
                color: NET,
            });
        }
        for (i, a) in s.adapters.iter().enumerate() {
            if let Some((rx, tx)) = h.adapters.get(i) {
                entries.push(Entry {
                    res: Res::Adapter(i),
                    title: format!("{} ({})", a.kind, a.name),
                    value: format!("S: {}  R: {}", format_bits_per_sec(a.tx_bps), format_bits_per_sec(a.rx_bps)),
                    ring: rx,
                    max: net_max(rx, tx),
                    color: NET,
                });
            }
        }
        if s.gpus.is_empty() {
            entries.push(Entry {
                res: Res::Gpu(usize::MAX),
                title: "GPU".into(),
                value: s.gpu_pct.map_or("n/a".to_string(), |g| format!("{g:.0}%")),
                ring: &h.gpu_pct,
                max: 100.0,
                color: GPU,
            });
        }
        let gpu_titles = taskmgr_core::gpu_titles(&s.gpus);
        for (i, g) in s.gpus.iter().enumerate() {
            if let Some(ring) = h.gpus.get(i) {
                let t = self.temp(g.luid.clone(), || taskmgr_sys::gpu_temperature(&g.luid));
                entries.push(Entry {
                    res: Res::Gpu(i),
                    title: gpu_titles[i].clone(),
                    value: match t {
                        Some(t) => format!("{}\n{:.0}% ({t:.0} °C)", g.name, g.util),
                        None => format!("{}\n{:.0}%", g.name, g.util),
                    },
                    ring,
                    max: 100.0,
                    color: GPU,
                });
            }
        }
        let (value, ring, max) = self.fps.entry(snap);
        entries.push(Entry { res: Res::Fps, title: "FPS".into(), value, ring, max, color: crate::fps::COLOR });
        if !entries.iter().any(|e| e.res == self.selected) {
            self.selected = Res::Cpu;
        }

        self.update_gpu_history(snap);
        if self.summary {
            egui::CentralPanel::default().show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.list(ui, &entries));
            });
            return;
        }
        if self.graph_summary {
            egui::CentralPanel::default().show(ui, |ui| {
                let avail_h = ui.available_height();
                egui::ScrollArea::vertical().show(ui, |ui| self.detail(ui, snap, avail_h, true));
            });
            return;
        }
        egui::Panel::left("perf-list").exact_size(250.0).resizable(false).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.list(ui, &entries));
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let avail_h = ui.available_height();
            egui::ScrollArea::vertical().show(ui, |ui| self.detail(ui, snap, avail_h, false));
        });
    }

    fn detail(&mut self, ui: &mut Ui, snap: &Snapshot, avail_h: f32, graph_only: bool) {
        let s = &snap.system;
        let h = &snap.history;
        let net_max = |rx: &Ring, tx: &Ring| rx.max().max(tx.max()).max(1024.0);
        let disk_max = h.disk_bps.max().max(1024.0 * 1024.0);
        {
            let w = ui.available_width() - 8.0;
            let big = if graph_only {
                Vec2::new(w, (avail_h - 40.0).max(180.0))
            } else {
                Vec2::new(w, (avail_h - 290.0).max(140.0))
            };
            let summary = match self.selected {
                Res::Cpu => self.cpu_view(ui, snap, w, big),
                Res::Fps => self.fps.view(ui, snap, big),
                Res::Memory => {
                    let m = self
                        .memory
                        .get_or_insert_with(|| {
                            let m = taskmgr_sys::memory_info();
                            if std::env::var_os("OXI_TASKMGR_TIMING").is_some() {
                                eprintln!("memory info: {m:?}");
                            }
                            m
                        })
                        .clone();
                    let total_str = format_bytes(s.mem_total as f64);
                    header(ui, "Memory", &total_str);
                    ui.label(RichText::new("Memory usage over 60 seconds").small().weak());
                    let g = graph::area_labeled(
                        ui,
                        big,
                        &h.mem_pct,
                        100.0,
                        MEM,
                        true,
                        Some(graph::AxisLabels::with_top("Memory usage", &total_str)),
                    );
                    graph::hover(ui, &g, "Memory", &[("In use", &h.mem_pct)], pct_text);
                    if g.double_clicked() {
                        self.graph_summary = !self.graph_summary;
                    }
                    let mut items = vec![
                        ("In use", format_bytes(s.mem_used as f64)),
                        ("Available", format_bytes(s.mem_total.saturating_sub(s.mem_used) as f64)),
                        (
                            "Committed",
                            format!("{} / {}", format_bytes(s.commit_used as f64), format_bytes(s.commit_limit as f64)),
                        ),
                        ("Cached", opt_bytes(s.mem_cached)),
                    ];
                    if let Some(comp) = s.mem_compressed {
                        items.push(("Compressed", format_bytes(comp as f64)));
                    }
                    items.extend([
                        ("Paged pool", opt_bytes(s.mem_paged_pool)),
                        ("Non-paged pool", opt_bytes(s.mem_nonpaged_pool)),
                        ("Hardware reserved", opt_bytes(s.mem_hw_reserved)),
                        ("Speed", if m.speed_mts == 0 { "-".into() } else { format!("{} MT/s", m.speed_mts) }),
                        (
                            "Slots used",
                            if m.slots_total == 0 {
                                "-".into()
                            } else {
                                format!("{} of {}", m.slots_used, m.slots_total)
                            },
                        ),
                        ("Form factor", if m.form_factor.is_empty() { "-".into() } else { m.form_factor }),
                    ]);
                    let copy_text = items.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n");
                    self.attach_graph_menu(ui, &g, &copy_text, false);
                    if graph_only {
                        return;
                    }
                    composition(ui, snap);
                    stats(ui, &items)
                }
                Res::Disk(i) => match s.disks.get(i) {
                    Some(d) => {
                        let mut parts = d.name.split_whitespace();
                        let number = parts.next().and_then(|n| n.parse::<u32>().ok());
                        let letters: Vec<String> = parts.map(|l| l.to_uppercase()).collect();
                        let info = number
                            .and_then(|n| {
                                self.disk_info
                                    .entry(n)
                                    .or_insert_with(|| {
                                        let i = taskmgr_sys::disk_info(n);
                                        if std::env::var_os("OXI_TASKMGR_TIMING").is_some() {
                                            eprintln!("disk info {n}: {i:?}");
                                        }
                                        i.ok()
                                    })
                                    .clone()
                            })
                            .unwrap_or_default();
                        header(ui, &disk_title(&d.name), &info.model);
                        ui.label(RichText::new("Active time over 60 seconds").small().weak());
                        let mut items = vec![
                            ("Active time", format!("{:.0}%", d.active_pct)),
                            ("Average response time", format!("{:.1} ms", d.response_ms)),
                            ("Read speed", format!("{}/s", format_bytes(d.read_bps))),
                            ("Write speed", format!("{}/s", format_bytes(d.write_bps))),
                            (
                                "Capacity",
                                if info.capacity == 0 { "-".into() } else { format_bytes(info.capacity as f64) },
                            ),
                            (
                                "Type",
                                match (info.kind.is_empty(), info.bus.is_empty()) {
                                    (true, true) => "-".into(),
                                    (false, true) => info.kind.clone(),
                                    (true, false) => info.bus.clone(),
                                    (false, false) => format!("{} ({})", info.kind, info.bus),
                                },
                            ),
                            (
                                "System disk",
                                if letters.contains(
                                    &std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()).to_uppercase(),
                                ) {
                                    "Yes"
                                } else {
                                    "No"
                                }
                                .to_string(),
                            ),
                            (
                                "Page file",
                                if letters.iter().any(|l| std::path::Path::new(&format!("{l}\\pagefile.sys")).exists())
                                {
                                    "Yes"
                                } else {
                                    "No"
                                }
                                .to_string(),
                            ),
                        ];
                        let temp =
                            number.and_then(|n| self.temp(format!("disk {n}"), || taskmgr_sys::disk_temperature(n)));
                        if temp.is_some() {
                            items.push(("Temperature", celsius(temp)));
                        }
                        let copy_text = items.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n");

                        if let Some(ring) = h.disks.get(i) {
                            let g = graph::area_labeled(
                                ui,
                                Vec2::new(big.x, big.y * 0.6),
                                ring,
                                100.0,
                                DISK,
                                true,
                                Some(graph::AxisLabels::with_top("% Active time", "100%")),
                            );
                            graph::hover(ui, &g, "", &[("Active time", ring)], pct_text);
                            if g.double_clicked() {
                                self.graph_summary = !self.graph_summary;
                            }
                            self.attach_graph_menu(ui, &g, &copy_text, false);
                        }
                        if let Some((r, w)) = h.disk_io.get(i) {
                            let max = r.max().max(w.max()).max(1024.0 * 1024.0);
                            let max_str = format!("{}/s", format_bytes(max as f64));
                            ui.label(RichText::new(format!("Disk transfer rate (max {max_str})")).small().weak());
                            let g = graph::multi_labeled(
                                ui,
                                Vec2::new(big.x, big.y * 0.4),
                                &[(r, DISK), (w, DISK_WRITE)],
                                max,
                                Some(graph::AxisLabels::with_top("Disk transfer rate", &max_str)),
                            );
                            graph::hover(ui, &g, "", &[("Read", r), ("Write", w)], bytes_per_sec);
                            if g.double_clicked() {
                                self.graph_summary = !self.graph_summary;
                            }
                            self.attach_graph_menu(ui, &g, &copy_text, false);
                        }
                        if graph_only {
                            return;
                        }
                        stats(ui, &items)
                    }
                    None => {
                        header(ui, "Disk", "");
                        let copy_text = format!(
                            "Active time: {}\nRead speed: {}/s\nWrite speed: {}/s",
                            s.disk_active_pct.map_or("n/a".into(), |p| format!("{p:.0}%")),
                            format_bytes(s.disk_read_bps),
                            format_bytes(s.disk_write_bps)
                        );
                        let g = graph::area_labeled(
                            ui,
                            big,
                            &h.disk_bps,
                            disk_max,
                            DISK,
                            true,
                            Some(graph::AxisLabels::PERCENT),
                        );
                        graph::hover(ui, &g, "", &[("Transfer rate", &h.disk_bps)], bytes_per_sec);
                        if g.double_clicked() {
                            self.graph_summary = !self.graph_summary;
                        }
                        self.attach_graph_menu(ui, &g, &copy_text, false);
                        if graph_only {
                            return;
                        }
                        stats(
                            ui,
                            &[
                                ("Active time", s.disk_active_pct.map_or("n/a".into(), |p| format!("{p:.0}%"))),
                                ("Read speed", format!("{}/s", format_bytes(s.disk_read_bps))),
                                ("Write speed", format!("{}/s", format_bytes(s.disk_write_bps))),
                            ],
                        )
                    }
                },
                Res::Adapter(i) => {
                    let alias = s.adapters.get(i).map(|a| a.name.clone()).unwrap_or_default();
                    let stale = self
                        .adapter
                        .as_ref()
                        .is_none_or(|(n, at, _)| *n != alias || at.elapsed() > Duration::from_secs(10));
                    if stale && !alias.is_empty() {
                        self.adapter = Some((alias.clone(), Instant::now(), taskmgr_sys::adapter_info(&alias)));
                    }
                    let (name, desc, rx, tx, link, rings) = match s.adapters.get(i) {
                        Some(a) => (
                            format!("{} ({})", a.kind, a.name),
                            a.description.clone(),
                            a.rx_bps,
                            a.tx_bps,
                            a.link_bps,
                            h.adapters.get(i).map(|(r, t)| (r, t)),
                        ),
                        None => (
                            "Network".into(),
                            String::new(),
                            s.net_rx_bps,
                            s.net_tx_bps,
                            0,
                            Some((&h.net_rx_bps, &h.net_tx_bps)),
                        ),
                    };
                    header(ui, &name, &desc);
                    let mut items = vec![
                        ("Send", format_bits_per_sec(tx)),
                        ("Receive", format_bits_per_sec(rx)),
                        ("Link speed", if link == 0 { "-".into() } else { format_bits_per_sec(link as f64 / 8.0) }),
                    ];
                    if let Some((n, _, info)) = &self.adapter
                        && *n == alias
                    {
                        if !info.connection_type.is_empty() {
                            items.push(("Connection type", info.connection_type.clone()));
                        }
                        if let Some(q) = info.signal_strength {
                            let bars = match q {
                                0 => "0 bars",
                                1..=25 => "1 bar",
                                26..=50 => "2 bars",
                                51..=75 => "3 bars",
                                _ => "4 bars",
                            };
                            items.push(("Signal strength", format!("{q}% ({bars})")));
                        }
                        if let Some(ssid) = &info.ssid {
                            items.push(("SSID", ssid.clone()));
                        }
                        if !info.dns_suffix.is_empty() {
                            items.push(("DNS name", info.dns_suffix.clone()));
                        }
                        if !info.ipv4.is_empty() {
                            items.push(("IPv4 address", info.ipv4.join(", ")));
                        }
                        if !info.ipv6.is_empty() {
                            items.push(("IPv6 address", info.ipv6.join(", ")));
                        }
                    }
                    let copy_text = items.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n");
                    if let Some((r, t)) = rings {
                        let max = net_max(r, t);
                        let max_str = format_bits_per_sec(max as f64);
                        ui.label(RichText::new(format!("Throughput, scale {max_str}")).small().weak());
                        let g = graph::multi_labeled(
                            ui,
                            big,
                            &[(r, NET), (t, NET_TX)],
                            max,
                            Some(graph::AxisLabels::with_top("Throughput", &max_str)),
                        );
                        graph::hover(ui, &g, "", &[("Send", t), ("Receive", r)], bits_per_sec);
                        if g.double_clicked() {
                            self.graph_summary = !self.graph_summary;
                        }
                        self.attach_graph_menu(ui, &g, &copy_text, false);
                    }
                    if graph_only {
                        return;
                    }
                    stats(ui, &items)
                }
                Res::Gpu(i) => {
                    let one = s.gpus.get(i);
                    let title = match one {
                        Some(_) => taskmgr_core::gpu_titles(&s.gpus)[i].clone(),
                        None => "GPU".into(),
                    };
                    header(ui, &title, one.map_or("", |g| g.name.as_str()));
                    let of_total = |used: Option<u64>, total: u64| match (used, total) {
                        (Some(u), t) if t > 0 => format!("{} / {}", format_bytes(u as f64), format_bytes(t as f64)),
                        (u, _) => opt_bytes(u),
                    };
                    let (util, ded, shared, engines, ded_total, shared_total) = match one {
                        Some(g) => (
                            format!("{:.0}%", g.util),
                            of_total(g.dedicated_used, g.dedicated_total),
                            of_total(g.shared_used, g.shared_total),
                            &g.engines,
                            g.dedicated_total,
                            g.shared_total,
                        ),
                        None => (
                            s.gpu_pct.map_or("n/a".into(), |g| format!("{g:.0}%")),
                            opt_bytes(s.gpu_dedicated_used),
                            opt_bytes(s.gpu_shared_used),
                            &s.gpu_engines,
                            0,
                            0,
                        ),
                    };
                    let mut items =
                        vec![("Utilization", util), ("Dedicated GPU memory", ded), ("Shared GPU memory", shared)];
                    if let Some(g) = one {
                        let t = self.temp(g.luid.clone(), || taskmgr_sys::gpu_temperature(&g.luid));
                        if t.is_some() {
                            items.push(("GPU temperature", celsius(t)));
                        }
                        for (k, v) in [
                            ("Driver version", &g.driver_version),
                            ("Driver date", &g.driver_date),
                            ("DirectX version", &g.directx),
                            ("Physical location", &g.location),
                        ] {
                            if !v.is_empty() {
                                items.push((k, v.clone()));
                            }
                        }
                    }
                    let engines_text: Vec<(String, String)> =
                        engines.iter().map(|(n, v)| (n.clone(), format!("{v:.0}%"))).collect();
                    items.extend(engines_text.iter().map(|(n, v)| (n.as_str(), v.clone())));
                    let copy_text = items.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n");

                    // Four engine graphs in 2x2 grid with dropdown ComboBox above each
                    let mut available_engines: Vec<String> = engines.iter().map(|(n, _)| n.clone()).collect();
                    if available_engines.is_empty() {
                        available_engines =
                            vec!["3D".into(), "Copy".into(), "Video Encode".into(), "Video Decode".into()];
                    }
                    let mut slots = self
                        .gpu_slots
                        .get(&i)
                        .cloned()
                        .unwrap_or_else(|| taskmgr_core::pick_default_gpu_engines(&available_engines));
                    for slot in slots.iter_mut() {
                        if !available_engines.contains(slot)
                            && let Some(first) = available_engines.first()
                        {
                            *slot = first.clone();
                        }
                    }

                    let col_w = (w - 8.0) / 2.0;
                    let cell_h = if graph_only { (avail_h - 120.0).max(180.0) / 2.0 } else { 105.0 };

                    egui::Grid::new(("gpu_engine_grid", i)).spacing([8.0, 8.0]).show(ui, |ui| {
                        for row in 0..2 {
                            for col in 0..2 {
                                let slot_idx = row * 2 + col;
                                ui.vertical(|ui| {
                                    ui.set_width(col_w);
                                    let current_engine = &mut slots[slot_idx];
                                    egui::ComboBox::from_id_salt(("gpu_eng_cb", i, slot_idx))
                                        .selected_text(current_engine.as_str())
                                        .width(col_w - 8.0)
                                        .show_ui(ui, |ui| {
                                            for eng in &available_engines {
                                                ui.selectable_value(current_engine, eng.clone(), eng);
                                            }
                                        });
                                    let ring = self
                                        .gpu_engine_history
                                        .get(&i)
                                        .and_then(|m| m.get(current_engine.as_str()))
                                        .cloned()
                                        .unwrap_or_default();
                                    let g = graph::area_labeled(
                                        ui,
                                        Vec2::new(col_w, cell_h),
                                        &ring,
                                        100.0,
                                        GPU,
                                        true,
                                        Some(graph::AxisLabels::PERCENT),
                                    );
                                    graph::hover(ui, &g, current_engine, &[("Utilization", &ring)], pct_text);
                                    if g.double_clicked() {
                                        self.graph_summary = !self.graph_summary;
                                    }
                                    self.attach_graph_menu(ui, &g, &copy_text, false);
                                });
                            }
                            ui.end_row();
                        }
                    });
                    self.gpu_slots.insert(i, slots);

                    // Memory graphs: Dedicated and Shared
                    ui.add_space(8.0);
                    let (ded_ring, shared_ring) = self.gpu_mem_history.get(&i).cloned().unwrap_or_default();
                    let ded_total_str = format_bytes(ded_total as f64);
                    ui.label(RichText::new("Dedicated GPU memory usage").small().weak());
                    let ded_max = if ded_total > 0 { ded_total as f32 } else { ded_ring.max().max(1.0) };
                    let g_ded = graph::area_labeled(
                        ui,
                        Vec2::new(w, if graph_only { 110.0 } else { 80.0 }),
                        &ded_ring,
                        ded_max,
                        GPU,
                        true,
                        Some(graph::AxisLabels::with_top("", &ded_total_str)),
                    );
                    graph::hover(ui, &g_ded, "Dedicated GPU memory", &[("In use", &ded_ring)], bytes_text);
                    if g_ded.double_clicked() {
                        self.graph_summary = !self.graph_summary;
                    }
                    self.attach_graph_menu(ui, &g_ded, &copy_text, false);

                    ui.add_space(6.0);
                    let shared_total_str = format_bytes(shared_total as f64);
                    ui.label(RichText::new("Shared GPU memory usage").small().weak());
                    let shared_max = if shared_total > 0 { shared_total as f32 } else { shared_ring.max().max(1.0) };
                    let g_shared = graph::area_labeled(
                        ui,
                        Vec2::new(w, if graph_only { 110.0 } else { 80.0 }),
                        &shared_ring,
                        shared_max,
                        GPU,
                        true,
                        Some(graph::AxisLabels::with_top("", &shared_total_str)),
                    );
                    graph::hover(ui, &g_shared, "Shared GPU memory", &[("In use", &shared_ring)], bytes_text);
                    if g_shared.double_clicked() {
                        self.graph_summary = !self.graph_summary;
                    }
                    self.attach_graph_menu(ui, &g_shared, &copy_text, false);

                    if graph_only {
                        return;
                    }
                    stats(ui, &items)
                }
            };
            if graph_only {
                return;
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Copy").on_hover_text("Copy these figures as text").clicked() {
                    ui.ctx().copy_text(summary);
                }
                if ui
                    .button("Summary view")
                    .on_hover_text("Show only the graphs; double-click one to come back")
                    .clicked()
                {
                    self.summary = true;
                }
                if ui.button("Open Resource Monitor").clicked() {
                    let _ = taskmgr_sys::run_new_task("resmon", false);
                }
            });
        }
    }

    fn cpu_view(&mut self, ui: &mut Ui, snap: &Snapshot, w: f32, big: Vec2) -> String {
        let s = &snap.system;
        let h = &snap.history;
        let c = &self.cpu;
        let items = vec![
            ("Utilization", format!("{:.0}%", s.cpu_pct)),
            ("Speed", s.cpu_mhz.map_or("-".into(), |m| format!("{:.2} GHz", m / 1000.0))),
            ("Processes", s.process_count.to_string()),
            ("Threads", s.thread_count.to_string()),
            ("Handles", s.handle_count.to_string()),
            ("Up time", format_uptime(s.uptime)),
            ("Base speed", if c.base_mhz == 0 { "-".into() } else { format!("{:.2} GHz", c.base_mhz as f32 / 1000.0) }),
            ("Sockets", if c.sockets == 0 { "-".into() } else { c.sockets.to_string() }),
            ("Cores", if c.cores == 0 { "-".into() } else { c.cores.to_string() }),
            ("Logical processors", c.logical.max(s.cores_pct.len() as u32).to_string()),
            (
                "Virtualization",
                c.virtualization.map_or("-".into(), |v| if v { "Enabled".into() } else { "Disabled".into() }),
            ),
            ("L1 cache", cache(c.l1_bytes)),
            ("L2 cache", cache(c.l2_bytes)),
            ("L3 cache", cache(c.l3_bytes)),
        ];
        let summary_text = items.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n");

        ui.horizontal(|ui| {
            header(ui, "CPU", &self.cpu.name);
        });
        ui.label(RichText::new("% Utilization over 60 seconds (right-click the graph to change it)").small().weak());

        let is_numa = self.cpu_view == CpuViewMode::NumaNodes;
        let is_logical = self.cpu_view == CpuViewMode::LogicalProcessors;

        if is_numa && !self.cpu.numa_nodes.is_empty() {
            let node_averages = taskmgr_core::group_by_numa_node(&s.cores_pct, &self.cpu.numa_nodes);
            let n = node_averages.len();
            let cols = (n as f32).sqrt().ceil() as usize;
            let rows = n.div_ceil(cols);
            let gap = 4.0;
            let cell =
                Vec2::new((w - gap * (cols - 1) as f32) / cols as f32, (big.y - gap * (rows - 1) as f32) / rows as f32);
            egui::Grid::new("numa_nodes").spacing([gap, gap]).show(ui, |ui| {
                for (i, &(node_id, _avg)) in node_averages.iter().enumerate() {
                    let title = format!("NUMA node {node_id}");
                    let node_cores: Vec<usize> = self
                        .cpu
                        .numa_nodes
                        .iter()
                        .enumerate()
                        .filter(|(_, nid)| **nid == node_id)
                        .map(|(c, _)| c)
                        .collect();
                    let mut node_ring = Ring::default();
                    if !node_cores.is_empty() && !h.cores.is_empty() {
                        let len = h.cores[0].len();
                        for step in 0..len {
                            let mut sum = 0.0;
                            let mut cnt = 0;
                            for &c in &node_cores {
                                if let Some(r) = h.cores.get(c)
                                    && let Some(v) = r.iter().nth(step)
                                {
                                    sum += v;
                                    cnt += 1;
                                }
                            }
                            if cnt > 0 {
                                node_ring.push(sum / cnt as f32);
                            }
                        }
                    }
                    let g = graph::area(ui, cell, &node_ring, 100.0, CPU, false);
                    graph::hover(ui, &g, &title, &[("Utilization", &node_ring)], pct_text);
                    if g.double_clicked() {
                        self.graph_summary = !self.graph_summary;
                    }
                    self.attach_graph_menu(ui, &g, &summary_text, true);
                    if (i + 1) % cols == 0 {
                        ui.end_row();
                    }
                }
            });
        } else if is_logical && !h.cores.is_empty() {
            let n = h.cores.len();
            let cols = (n as f32).sqrt().ceil() as usize;
            let rows = n.div_ceil(cols);
            let gap = 4.0;
            let cell =
                Vec2::new((w - gap * (cols - 1) as f32) / cols as f32, (big.y - gap * (rows - 1) as f32) / rows as f32);
            egui::Grid::new("cores").spacing([gap, gap]).show(ui, |ui| {
                for (i, ring) in h.cores.iter().enumerate() {
                    let title = format!("CPU {i}");
                    match h.cores_kernel.get(i).filter(|_| self.kernel) {
                        Some(k) => {
                            let g = graph::with_kernel(ui, cell, ring, k, CPU, false);
                            graph::hover(ui, &g, &title, &[("Utilization", ring), ("Kernel", k)], pct_text);
                            if g.double_clicked() {
                                self.graph_summary = !self.graph_summary;
                            }
                            self.attach_graph_menu(ui, &g, &summary_text, true);
                        }
                        None => {
                            let g = graph::area(ui, cell, ring, 100.0, CPU, false);
                            graph::hover(ui, &g, &title, &[("Utilization", ring)], pct_text);
                            if g.double_clicked() {
                                self.graph_summary = !self.graph_summary;
                            }
                            self.attach_graph_menu(ui, &g, &summary_text, true);
                        }
                    }
                    if (i + 1) % cols == 0 {
                        ui.end_row();
                    }
                }
            });
        } else if self.kernel {
            let g =
                graph::with_kernel_labeled(ui, big, &h.cpu, &h.cpu_kernel, CPU, true, Some(graph::AxisLabels::PERCENT));
            graph::hover(ui, &g, "", &[("Utilization", &h.cpu), ("Kernel", &h.cpu_kernel)], pct_text);
            if g.double_clicked() {
                self.graph_summary = !self.graph_summary;
            }
            self.attach_graph_menu(ui, &g, &summary_text, true);
        } else {
            let g = graph::area_labeled(ui, big, &h.cpu, 100.0, CPU, true, Some(graph::AxisLabels::PERCENT));
            graph::hover(ui, &g, "", &[("Utilization", &h.cpu)], pct_text);
            if g.double_clicked() {
                self.graph_summary = !self.graph_summary;
            }
            self.attach_graph_menu(ui, &g, &summary_text, true);
        }
        stats(ui, &items)
    }
}

fn pct_text(v: f32) -> String {
    format!("{v:.0}%")
}

fn bytes_text(v: f32) -> String {
    format_bytes(v as f64)
}

fn bytes_per_sec(v: f32) -> String {
    format!("{}/s", format_bytes(v as f64))
}

fn bits_per_sec(v: f32) -> String {
    format_bits_per_sec(v as f64)
}

pub fn header(ui: &mut Ui, title: &str, right: &str) {
    ui.horizontal(|ui| {
        ui.heading(title);
        if !right.is_empty() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(right).weak());
            });
        }
    });
}

/// The "Memory composition" bar: in use, modified/cached, free.
fn composition(ui: &mut Ui, snap: &Snapshot) {
    let s = &snap.system;
    if s.mem_total == 0 {
        return;
    }
    let total = s.mem_total as f32;
    let used = s.mem_used as f32 / total;
    let cached = s.mem_cached.unwrap_or(0) as f32 / total;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
    let w = rect.width();
    let used_r = egui::Rect::from_min_size(rect.min, Vec2::new(w * used, rect.height()));
    p.rect_filled(used_r, 0.0, MEM.gamma_multiply(0.6));
    if let Some(comp) = s.mem_compressed {
        let comp_w = (w * (comp as f32 / total)).min(used_r.width());
        let comp_r = egui::Rect::from_min_size(
            egui::Pos2::new(used_r.right() - comp_w, used_r.top()),
            Vec2::new(comp_w, rect.height()),
        );
        p.rect_filled(comp_r, 0.0, MEM.gamma_multiply(0.85));
    }
    let cached_w = (w * cached).min(w - used_r.width());
    let cached_r = egui::Rect::from_min_size(used_r.right_top(), Vec2::new(cached_w, rect.height()));
    p.rect_filled(cached_r, 0.0, MEM.gamma_multiply(0.25));
    p.rect_stroke(rect, 0.0, egui::Stroke::new(1.0, MEM), egui::StrokeKind::Inside);
    let comp_str = s.mem_compressed.map_or(String::new(), |c| format!(" (Compressed {})", format_bytes(c as f64)));
    resp.on_hover_text(format!(
        "In use {}{} · Cached {} · Free {}",
        format_bytes(s.mem_used as f64),
        comp_str,
        opt_bytes(s.mem_cached),
        format_bytes(s.mem_total.saturating_sub(s.mem_used).saturating_sub(s.mem_cached.unwrap_or(0)) as f64)
    ));
}

/// Figures grid; returns them as text for Copy.
pub fn stats(ui: &mut Ui, items: &[(&str, String)]) -> String {
    ui.add_space(8.0);
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

    if items.is_empty() {
        return String::new();
    }

    let primary_count = if items.len() > 2 { 2 } else { 1 };
    let (primary, params) = items.split_at(primary_count.min(items.len()));

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_min_width(140.0);
            for (k, v) in primary {
                ui.label(RichText::new(*k).small().weak());
                ui.label(RichText::new(v).size(24.0));
                ui.add_space(4.0);
            }
        });
        ui.add_space(16.0);
        egui::Grid::new("stats_params").min_col_width(120.0).spacing([20.0, 6.0]).show(ui, |ui| {
            for (i, (k, v)) in params.iter().enumerate() {
                ui.vertical(|ui| {
                    ui.label(RichText::new(*k).small().weak());
                    ui.label(RichText::new(v).size(15.0));
                });
                if (i + 1) % 2 == 0 {
                    ui.end_row();
                }
            }
        });
    });

    items.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n")
}

/// PDH names a disk `0 C: D:`; Task Manager shows `Disk 0 (C: D:)`, or `Disk 0` with no volumes.
fn disk_title(pdh: &str) -> String {
    match pdh.split_once(' ') {
        Some((n, vols)) if !vols.trim().is_empty() => format!("Disk {n} ({})", vols.trim()),
        _ => format!("Disk {}", pdh.trim()),
    }
}
