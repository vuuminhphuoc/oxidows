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
const NET: Color32 = Color32::from_rgb(167, 79, 1);
const NET_TX: Color32 = Color32::from_rgb(220, 140, 40);
const GPU: Color32 = Color32::from_rgb(10, 140, 120);

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
}

pub struct State {
    selected: Res,
    /// Only the resource list, without tabs or details (double-click to leave).
    pub summary: bool,
    per_core: bool,
    /// Draw kernel-mode time over the CPU graphs.
    kernel: bool,
    cpu: CpuInfo,
    /// Addresses of the shown adapter, re-read every few seconds.
    adapter: Option<(String, Instant, AdapterInfo)>,
    /// Model, capacity and type per disk number, read once.
    disk_info: std::collections::HashMap<u32, Option<DiskInfo>>,
    /// Speed and slots from SMBIOS, read once.
    memory: Option<MemoryInfo>,
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
            _ => Res::Cpu,
        };
        Self {
            summary: std::env::var_os("OXI_TASKMGR_SUMMARY").is_some(),
            selected,
            per_core: false,
            kernel: false,
            cpu: taskmgr_sys::cpu_info(),
            adapter: None,
            disk_info: Default::default(),
            memory: None,
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
        for e in entries {
            let sel = self.selected == e.res;
            let frame = egui::Frame::new().inner_margin(6).fill(if sel && !self.summary {
                ui.visuals().selection.bg_fill.gamma_multiply(0.35)
            } else {
                Color32::TRANSPARENT
            });
            let r = frame
                .show(ui, |ui| {
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
                    title: format!("Disk {}", d.name),
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
        for (i, g) in s.gpus.iter().enumerate() {
            if let Some(ring) = h.gpus.get(i) {
                entries.push(Entry {
                    res: Res::Gpu(i),
                    title: format!("GPU {i}"),
                    value: format!("{}\n{:.0}%", g.name, g.util),
                    ring,
                    max: 100.0,
                    color: GPU,
                });
            }
        }
        if !entries.iter().any(|e| e.res == self.selected) {
            self.selected = Res::Cpu;
        }

        if self.summary {
            egui::CentralPanel::default().show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.list(ui, &entries));
            });
            return;
        }
        egui::Panel::left("perf-list").exact_size(250.0).resizable(false).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.list(ui, &entries));
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let avail_h = ui.available_height();
            egui::ScrollArea::vertical().show(ui, |ui| self.detail(ui, snap, avail_h));
        });
    }

    fn detail(&mut self, ui: &mut Ui, snap: &Snapshot, avail_h: f32) {
        let s = &snap.system;
        let h = &snap.history;
        let net_max = |rx: &Ring, tx: &Ring| rx.max().max(tx.max()).max(1024.0);
        let disk_max = h.disk_bps.max().max(1024.0 * 1024.0);
        {
            let w = ui.available_width() - 8.0;
            let big = Vec2::new(w, (avail_h - 290.0).max(140.0));
            let summary = match self.selected {
                Res::Cpu => self.cpu_view(ui, snap, w, big),
                Res::Memory => {
                    let m = self.memory.get_or_insert_with(taskmgr_sys::memory_info).clone();
                    header(ui, "Memory", &format_bytes(s.mem_total as f64));
                    ui.label(RichText::new("Memory usage over 60 seconds").small().weak());
                    graph::area(ui, big, &h.mem_pct, 100.0, MEM, true);
                    composition(ui, snap);
                    stats(
                        ui,
                        &[
                            ("In use", format_bytes(s.mem_used as f64)),
                            ("Available", format_bytes(s.mem_total.saturating_sub(s.mem_used) as f64)),
                            (
                                "Committed",
                                format!(
                                    "{} / {}",
                                    format_bytes(s.commit_used as f64),
                                    format_bytes(s.commit_limit as f64)
                                ),
                            ),
                            ("Cached", opt_bytes(s.mem_cached)),
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
                        ],
                    )
                }
                Res::Disk(i) => match s.disks.get(i) {
                    Some(d) => {
                        // PDH names a disk "0 C: D:": the number, then its volumes.
                        let mut parts = d.name.split_whitespace();
                        let number = parts.next().and_then(|n| n.parse::<u32>().ok());
                        let letters: Vec<String> = parts.map(|l| l.to_uppercase()).collect();
                        let info = number
                            .and_then(|n| {
                                self.disk_info.entry(n).or_insert_with(|| taskmgr_sys::disk_info(n).ok()).clone()
                            })
                            .unwrap_or_default();
                        header(ui, &format!("Disk {}", d.name), &info.model);
                        ui.label(RichText::new("Active time over 60 seconds").small().weak());
                        if let Some(ring) = h.disks.get(i) {
                            graph::area(ui, big, ring, 100.0, DISK, true);
                        }
                        let sys = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()).to_uppercase();
                        let is_sys = letters.contains(&sys);
                        let pagefile =
                            letters.iter().any(|l| std::path::Path::new(&format!("{l}\\pagefile.sys")).exists());
                        let yes_no = |b: bool| if b { "Yes" } else { "No" }.to_string();
                        stats(
                            ui,
                            &[
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
                                ("System disk", yes_no(is_sys)),
                                ("Page file", yes_no(pagefile)),
                            ],
                        )
                    }
                    None => {
                        header(ui, "Disk", "");
                        graph::area(ui, big, &h.disk_bps, disk_max, DISK, true);
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
                    if let Some((r, t)) = rings {
                        let max = net_max(r, t);
                        ui.label(
                            RichText::new(format!("Throughput, scale {}", format_bits_per_sec(max as f64)))
                                .small()
                                .weak(),
                        );
                        graph::multi(ui, big, &[(r, NET), (t, NET_TX)], max);
                    }
                    let mut items = vec![
                        ("Send", format_bits_per_sec(tx)),
                        ("Receive", format_bits_per_sec(rx)),
                        ("Link speed", if link == 0 { "-".into() } else { format_bits_per_sec(link as f64 / 8.0) }),
                    ];
                    if let Some((n, _, info)) = &self.adapter
                        && *n == alias
                    {
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
                    stats(ui, &items)
                }
                Res::Gpu(i) => {
                    let one = s.gpus.get(i);
                    let ring = h.gpus.get(i).unwrap_or(&h.gpu_pct);
                    let title = if one.is_some() { format!("GPU {i}") } else { "GPU".into() };
                    header(ui, &title, one.map_or("", |g| g.name.as_str()));
                    ui.label(RichText::new("Busiest engine over 60 seconds").small().weak());
                    graph::area(ui, big, ring, 100.0, GPU, true);
                    let of_total = |used: Option<u64>, total: u64| match (used, total) {
                        (Some(u), t) if t > 0 => format!("{} / {}", format_bytes(u as f64), format_bytes(t as f64)),
                        (u, _) => opt_bytes(u),
                    };
                    let (util, ded, shared, engines) = match one {
                        Some(g) => (
                            format!("{:.0}%", g.util),
                            of_total(g.dedicated_used, g.dedicated_total),
                            of_total(g.shared_used, g.shared_total),
                            &g.engines,
                        ),
                        None => (
                            s.gpu_pct.map_or("n/a".into(), |g| format!("{g:.0}%")),
                            opt_bytes(s.gpu_dedicated_used),
                            opt_bytes(s.gpu_shared_used),
                            &s.gpu_engines,
                        ),
                    };
                    let mut items =
                        vec![("Utilization", util), ("Dedicated GPU memory", ded), ("Shared GPU memory", shared)];
                    if let Some(g) = one {
                        for (k, v) in [("Driver version", &g.driver_version), ("DirectX version", &g.directx)] {
                            if !v.is_empty() {
                                items.push((k, v.clone()));
                            }
                        }
                    }
                    let engines: Vec<(String, String)> =
                        engines.iter().map(|(n, v)| (n.clone(), format!("{v:.0}%"))).collect();
                    items.extend(engines.iter().map(|(n, v)| (n.as_str(), v.clone())));
                    stats(ui, &items)
                }
            };
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
        ui.horizontal(|ui| {
            header(ui, "CPU", &self.cpu.name);
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("% Utilization over 60 seconds").small().weak());
            ui.checkbox(&mut self.per_core, "Logical processors");
            ui.checkbox(&mut self.kernel, "Show kernel times");
        });
        if self.per_core && !h.cores.is_empty() {
            let n = h.cores.len();
            let cols = (n as f32).sqrt().ceil() as usize;
            let rows = n.div_ceil(cols);
            let gap = 4.0;
            let cell =
                Vec2::new((w - gap * (cols - 1) as f32) / cols as f32, (big.y - gap * (rows - 1) as f32) / rows as f32);
            egui::Grid::new("cores").spacing([gap, gap]).show(ui, |ui| {
                for (i, ring) in h.cores.iter().enumerate() {
                    match h.cores_kernel.get(i).filter(|_| self.kernel) {
                        Some(k) => graph::with_kernel(ui, cell, ring, k, CPU, false),
                        None => graph::area(ui, cell, ring, 100.0, CPU, false),
                    };
                    if (i + 1) % cols == 0 {
                        ui.end_row();
                    }
                }
            });
        } else if self.kernel {
            graph::with_kernel(ui, big, &h.cpu, &h.cpu_kernel, CPU, true);
        } else {
            graph::area(ui, big, &h.cpu, 100.0, CPU, true);
        }
        let c = &self.cpu;
        stats(
            ui,
            &[
                ("Utilization", format!("{:.0}%", s.cpu_pct)),
                ("Speed", s.cpu_mhz.map_or("-".into(), |m| format!("{:.2} GHz", m / 1000.0))),
                ("Processes", s.process_count.to_string()),
                ("Threads", s.thread_count.to_string()),
                ("Handles", s.handle_count.to_string()),
                ("Up time", format_uptime(s.uptime)),
                (
                    "Base speed",
                    if c.base_mhz == 0 { "-".into() } else { format!("{:.2} GHz", c.base_mhz as f32 / 1000.0) },
                ),
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
            ],
        )
    }
}

fn header(ui: &mut Ui, title: &str, right: &str) {
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
    let cached_w = (w * cached).min(w - used_r.width());
    let cached_r = egui::Rect::from_min_size(used_r.right_top(), Vec2::new(cached_w, rect.height()));
    p.rect_filled(cached_r, 0.0, MEM.gamma_multiply(0.25));
    p.rect_stroke(rect, 0.0, egui::Stroke::new(1.0, MEM), egui::StrokeKind::Inside);
    resp.on_hover_text(format!(
        "In use {} · Cached {} · Free {}",
        format_bytes(s.mem_used as f64),
        opt_bytes(s.mem_cached),
        format_bytes(s.mem_total.saturating_sub(s.mem_used).saturating_sub(s.mem_cached.unwrap_or(0)) as f64)
    ));
}

/// Figures grid; returns them as text for Copy.
fn stats(ui: &mut Ui, items: &[(&str, String)]) -> String {
    ui.add_space(8.0);
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
    egui::Grid::new("stats").min_col_width(120.0).spacing([24.0, 6.0]).show(ui, |ui| {
        for (i, (k, v)) in items.iter().enumerate() {
            ui.vertical(|ui| {
                ui.label(RichText::new(*k).small().weak());
                ui.label(RichText::new(v).size(17.0));
            });
            if (i + 1) % 4 == 0 {
                ui.end_row();
            }
        }
    });
    items.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n")
}
