//! Performance tab: a list of resources with sparklines on the left, the selected one large on
//! the right with its figures underneath.

use egui::{Color32, RichText, Ui, Vec2};
use taskmgr_core::{Snapshot, format_bits_per_sec, format_bytes, format_uptime};

use crate::graph;

const CPU: Color32 = Color32::from_rgb(17, 125, 187);
const MEM: Color32 = Color32::from_rgb(139, 18, 174);
const DISK: Color32 = Color32::from_rgb(77, 166, 12);
const NET: Color32 = Color32::from_rgb(167, 79, 1);
const NET_TX: Color32 = Color32::from_rgb(220, 140, 40);
const GPU: Color32 = Color32::from_rgb(10, 140, 120);

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Res {
    #[default]
    Cpu,
    Memory,
    Disk,
    Network,
    Gpu,
}

#[derive(Default)]
pub struct State {
    selected: Res,
    per_core: bool,
}

fn pct(used: u64, total: u64) -> f32 {
    if total == 0 { 0.0 } else { (used as f64 * 100.0 / total as f64) as f32 }
}

impl State {
    pub fn show(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let s = &snap.system;
        let h = &snap.history;
        let net_max = h.net_rx_bps.max().max(h.net_tx_bps.max()).max(1024.0);
        let disk_max = h.disk_bps.max().max(1024.0 * 1024.0);

        egui::Panel::left("perf-list").exact_size(230.0).resizable(false).show(ui, |ui| {
            let mut entry = |ui: &mut Ui, res: Res, title: &str, value: String, ring: &taskmgr_core::Ring, max, color| {
                let sel = self.selected == res;
                let frame = egui::Frame::new()
                    .inner_margin(6)
                    .fill(if sel { ui.visuals().selection.bg_fill.gamma_multiply(0.35) } else { Color32::TRANSPARENT });
                let r = frame
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            graph::area(ui, Vec2::new(70.0, 44.0), ring, max, color, false);
                            ui.vertical(|ui| {
                                ui.label(RichText::new(title).strong());
                                ui.label(RichText::new(value).small());
                            });
                        });
                    })
                    .response
                    .interact(egui::Sense::click());
                if r.clicked() {
                    self.selected = res;
                }
            };
            entry(ui, Res::Cpu, "CPU", format!("{:.0}%", s.cpu_pct), &h.cpu, 100.0, CPU);
            entry(
                ui,
                Res::Memory,
                "Memory",
                format!(
                    "{}/{} ({:.0}%)",
                    format_bytes(s.mem_used as f64),
                    format_bytes(s.mem_total as f64),
                    pct(s.mem_used, s.mem_total)
                ),
                &h.mem_pct,
                100.0,
                MEM,
            );
            let disk_label = match s.disk_active_pct {
                Some(p) => format!("{p:.0}%"),
                None => format!("{}/s", format_bytes(s.disk_read_bps + s.disk_write_bps)),
            };
            let (disk_ring, disk_scale) =
                if s.disk_active_pct.is_some() { (&h.disk_active_pct, 100.0) } else { (&h.disk_bps, disk_max) };
            entry(ui, Res::Disk, "Disk", disk_label, disk_ring, disk_scale, DISK);
            entry(
                ui,
                Res::Network,
                "Network",
                format!("S: {}  R: {}", format_bits_per_sec(s.net_tx_bps), format_bits_per_sec(s.net_rx_bps)),
                &h.net_rx_bps,
                net_max,
                NET,
            );
            let gpu_label = s.gpu_pct.map_or("n/a".to_string(), |g| format!("{g:.0}%"));
            entry(ui, Res::Gpu, "GPU", gpu_label, &h.gpu_pct, 100.0, GPU);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let w = ui.available_width();
            let big = Vec2::new(w, (ui.available_height() - 150.0).max(120.0));
            match self.selected {
                Res::Cpu => {
                    ui.horizontal(|ui| {
                        ui.heading("CPU");
                        ui.checkbox(&mut self.per_core, "Show logical processors");
                    });
                    ui.label(RichText::new("% Utilization over 60 seconds").small().weak());
                    if self.per_core && !h.cores.is_empty() {
                        let n = h.cores.len();
                        let cols = (n as f32).sqrt().ceil() as usize;
                        let rows = n.div_ceil(cols);
                        let gap = 4.0;
                        let cell = Vec2::new(
                            (w - gap * (cols - 1) as f32) / cols as f32,
                            (big.y - gap * (rows - 1) as f32) / rows as f32,
                        );
                        egui::Grid::new("cores").spacing([gap, gap]).show(ui, |ui| {
                            for (i, ring) in h.cores.iter().enumerate() {
                                graph::area(ui, cell, ring, 100.0, CPU, false);
                                if (i + 1) % cols == 0 {
                                    ui.end_row();
                                }
                            }
                        });
                    } else {
                        graph::area(ui, big, &h.cpu, 100.0, CPU, true);
                    }
                    stats(ui, &[
                        ("Utilization", format!("{:.0}%", s.cpu_pct)),
                        ("Logical processors", s.cores_pct.len().to_string()),
                        ("Processes", s.process_count.to_string()),
                        ("Threads", s.thread_count.to_string()),
                        ("Handles", s.handle_count.to_string()),
                        ("Up time", format_uptime(s.uptime)),
                    ]);
                }
                Res::Memory => {
                    ui.heading("Memory");
                    ui.label(RichText::new("Memory usage over 60 seconds").small().weak());
                    graph::area(ui, big, &h.mem_pct, 100.0, MEM, true);
                    stats(ui, &[
                        ("In use", format_bytes(s.mem_used as f64)),
                        ("Available", format_bytes(s.mem_total.saturating_sub(s.mem_used) as f64)),
                        ("Total", format_bytes(s.mem_total as f64)),
                        (
                            "Committed",
                            format!("{} / {}", format_bytes(s.commit_used as f64), format_bytes(s.commit_limit as f64)),
                        ),
                    ]);
                }
                Res::Disk => {
                    ui.heading("Disk");
                    if s.disk_active_pct.is_some() {
                        ui.label(RichText::new("Active time over 60 seconds").small().weak());
                        graph::area(ui, big, &h.disk_active_pct, 100.0, DISK, true);
                    } else {
                        ui.label(RichText::new(format!("Transfer rate, scale {}/s", format_bytes(disk_max as f64))).small().weak());
                        graph::area(ui, big, &h.disk_bps, disk_max, DISK, true);
                    }
                    stats(ui, &[
                        ("Active time", s.disk_active_pct.map_or("n/a".into(), |p| format!("{p:.0}%"))),
                        ("Read speed", format!("{}/s", format_bytes(s.disk_read_bps))),
                        ("Write speed", format!("{}/s", format_bytes(s.disk_write_bps))),
                    ]);
                }
                Res::Network => {
                    ui.heading("Network");
                    ui.label(
                        RichText::new(format!("Throughput, scale {}", format_bits_per_sec(net_max as f64))).small().weak(),
                    );
                    graph::multi(ui, big, &[(&h.net_rx_bps, NET), (&h.net_tx_bps, NET_TX)], net_max);
                    stats(ui, &[
                        ("Receive", format_bits_per_sec(s.net_rx_bps)),
                        ("Send", format_bits_per_sec(s.net_tx_bps)),
                    ]);
                }
                Res::Gpu => {
                    ui.heading("GPU");
                    ui.label(RichText::new("3D engine utilization over 60 seconds").small().weak());
                    graph::area(ui, big, &h.gpu_pct, 100.0, GPU, true);
                    stats(ui, &[("Utilization", s.gpu_pct.map_or("n/a".into(), |g| format!("{g:.0}%")))]);
                }
            }
        });
    }
}

fn stats(ui: &mut Ui, items: &[(&str, String)]) {
    ui.add_space(8.0);
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
    egui::Grid::new("stats").min_col_width(110.0).num_columns(items.len().min(4)).spacing([28.0, 6.0]).show(ui, |ui| {
        for (i, (k, v)) in items.iter().enumerate() {
            ui.vertical(|ui| {
                ui.label(RichText::new(*k).small().weak());
                ui.label(RichText::new(v).size(18.0));
            });
            if (i + 1) % 4 == 0 {
                ui.end_row();
            }
        }
    });
}
