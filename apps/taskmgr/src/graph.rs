//! Hand-drawn line + area graphs: a few hundred vertices per frame, no plotting crate.

use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};
use taskmgr_core::Ring;

pub fn area(ui: &mut Ui, size: Vec2, ring: &Ring, max: f32, color: Color32, grid: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    paint(ui, rect, &[(ring, color)], max, grid);
    resp
}

/// Utilization with its kernel-mode part drawn darker on top ("Show kernel times").
pub fn with_kernel(ui: &mut Ui, size: Vec2, ring: &Ring, kernel: &Ring, color: Color32, grid: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    paint(ui, rect, &[(ring, color), (kernel, color.gamma_multiply(0.6))], 100.0, grid);
    resp
}

/// Several series on one set of axes (network send + receive).
pub fn multi(ui: &mut Ui, size: Vec2, series: &[(&Ring, Color32)], max: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    paint(ui, rect, series, max, true);
    resp
}

fn paint(ui: &Ui, rect: Rect, series: &[(&Ring, Color32)], max: f32, grid: bool) {
    let painter = ui.painter_at(rect);
    let border = series.first().map_or(Color32::GRAY, |(_, c)| *c);
    painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
    if grid {
        let line = Stroke::new(1.0, border.gamma_multiply(0.18));
        for i in 1..10 {
            let x = rect.left() + rect.width() * i as f32 / 10.0;
            painter.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], line);
        }
        for i in 1..5 {
            let y = rect.top() + rect.height() * i as f32 / 5.0;
            painter.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], line);
        }
    }
    let max = if max > 0.0 { max } else { 1.0 };
    for (ring, color) in series {
        let cap = ring.capacity().max(2);
        let n = ring.len();
        // Newest point on the right edge; older points scroll left.
        let pts: Vec<Pos2> = ring
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let x = rect.right() - rect.width() * (n - 1 - i) as f32 / (cap - 1) as f32;
                let y = rect.bottom() - rect.height() * (v / max).clamp(0.0, 1.0);
                Pos2::new(x, y)
            })
            .collect();
        let fill = color.gamma_multiply(0.22);
        for w in pts.windows(2) {
            let quad = vec![w[0], w[1], Pos2::new(w[1].x, rect.bottom()), Pos2::new(w[0].x, rect.bottom())];
            painter.add(Shape::convex_polygon(quad, fill, Stroke::NONE));
        }
        if pts.len() >= 2 {
            painter.add(Shape::line(pts, Stroke::new(1.5, *color)));
        }
    }
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, border), egui::StrokeKind::Inside);
}

/// Hover readout for a graph drawn by the functions above: a thin line at the sample under the
/// pointer and a tooltip naming it ("12 seconds ago") with each series' value there.
/// `title` heads the tooltip (e.g. "CPU 3"); `fmt` turns a raw sample into text.
pub fn hover(ui: &Ui, resp: &egui::Response, title: &str, series: &[(&str, &Ring)], fmt: impl Fn(f32) -> String) {
    let Some(pos) = resp.hover_pos() else { return };
    let Some((_, first)) = series.first() else { return };
    let rect = resp.rect;
    let step = rect.width() / (first.capacity().max(2) - 1) as f32;
    let back = ((rect.right() - pos.x) / step).round().max(0.0) as usize;
    let at = |r: &Ring| r.len().checked_sub(back + 1).and_then(|i| r.iter().nth(i));
    if series.iter().all(|(_, r)| at(r).is_none()) {
        return;
    }
    let x = rect.right() - back as f32 * step;
    ui.painter_at(rect).vline(x, rect.y_range(), Stroke::new(1.0, ui.visuals().weak_text_color()));
    let mut lines = Vec::new();
    if !title.is_empty() {
        lines.push(title.to_string());
    }
    let ms = crate::INTERVAL_MS.load(std::sync::atomic::Ordering::Relaxed);
    lines.push(match back {
        0 => "Now".to_string(),
        _ if ms == 0 => format!("{back} samples ago"),
        _ => format!("{} seconds ago", (back as u64 * ms).div_ceil(1000)),
    });
    for (name, r) in series {
        if let Some(v) = at(r) {
            lines.push(if name.is_empty() { fmt(v) } else { format!("{name}: {}", fmt(v)) });
        }
    }
    resp.clone().on_hover_text_at_pointer(lines.join("\n"));
}
