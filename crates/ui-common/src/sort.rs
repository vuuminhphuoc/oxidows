//! Sortable table columns as Windows 11 draws them: click a header to sort by that column, click
//! it again to flip the order, and a small chevron above the title marks the sorted column
//! (pointing up for ascending, down for descending).

use std::cmp::Ordering;

use egui::{Response, Sense, Stroke, Ui, pos2, vec2};

use crate::{Palette, theme};

/// Height of a sortable header row: room for the chevron above the title.
pub const SORT_HEADER_HEIGHT: f32 = 28.0;

/// Height of a header row whose columns show a total above the title (Processes, Users).
pub const SORT_HEADER_TOTAL_HEIGHT: f32 = 46.0;

/// The column a table is sorted by and in which direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortBy<K> {
    pub key: K,
    pub descending: bool,
}

impl<K: Copy + PartialEq> SortBy<K> {
    pub const fn new(key: K, descending: bool) -> Self {
        Self { key, descending }
    }

    /// A click on `key`'s header: the sorted column flips its order; another column becomes the
    /// sorted one, in its natural first order (`descending_first`: numbers biggest first).
    pub fn click(&mut self, key: K, descending_first: bool) {
        if self.key == key {
            self.descending = !self.descending;
        } else {
            *self = Self::new(key, descending_first);
        }
    }

    /// `Some(descending)` when the table is sorted by `key`, for [`sort_header`].
    pub fn arrow(&self, key: K) -> Option<bool> {
        (self.key == key).then_some(self.descending)
    }

    /// `o` (an ascending comparison) in the table's direction.
    pub fn apply(&self, o: Ordering) -> Ordering {
        if self.descending { o.reverse() } else { o }
    }

    /// `name:asc` or `name:desc`, for a settings file.
    pub fn to_text(&self, name: impl Fn(K) -> &'static str) -> String {
        format!("{}:{}", name(self.key), if self.descending { "desc" } else { "asc" })
    }

    /// Reads [`Self::to_text`]; `None` for an unknown column.
    pub fn parse(text: &str, from_name: impl Fn(&str) -> Option<K>) -> Option<Self> {
        let (name, dir) = text.trim().split_once(':').unwrap_or((text.trim(), "asc"));
        Some(Self::new(from_name(name.trim())?, dir.trim() == "desc"))
    }
}

/// Case-insensitive text order, as Windows sorts names.
pub fn cmp_text(a: &str, b: &str) -> Ordering {
    a.chars().flat_map(char::to_lowercase).cmp(b.chars().flat_map(char::to_lowercase))
}

/// A table header cell that sorts: fills the cell, highlights on hover, and with `sorted` set
/// (`Some(descending)`) draws the chevron above the title. Click it to sort.
pub fn sort_header(ui: &mut Ui, title: &str, sorted: Option<bool>) -> Response {
    sort_header_total(ui, title, None, sorted)
}

/// Like [`sort_header`], but responds to dragging as well as clicking.
pub fn sort_header_drag(ui: &mut Ui, title: &str, sorted: Option<bool>) -> Response {
    sort_header_total_opts(ui, title, None, sorted, Sense::click_and_drag())
}

/// [`sort_header`] with the column's total ("37%") in a larger font above the title, as Task
/// Manager heads its CPU, Memory, Disk, Network and GPU columns. Use a row
/// [`SORT_HEADER_TOTAL_HEIGHT`] high.
pub fn sort_header_total(ui: &mut Ui, title: &str, total: Option<&str>, sorted: Option<bool>) -> Response {
    sort_header_total_opts(ui, title, total, sorted, Sense::click())
}

pub fn sort_header_total_opts(
    ui: &mut Ui,
    title: &str,
    total: Option<&str>,
    sorted: Option<bool>,
    sense: Sense,
) -> Response {
    let size = vec2(ui.available_width().max(1.0), ui.available_height().max(1.0));
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let p = Palette::of(ui);
    let painter = ui.painter_at(rect);
    if resp.hovered() {
        painter.rect_filled(rect, 2.0, theme::mix(p.content, p.hover, 0.7));
    }
    let color = if sorted.is_some() { p.text } else { p.text_secondary };
    if let Some(descending) = sorted {
        let (cx, top) = (rect.center().x, rect.top() + 3.0);
        let (y0, y1) = if descending { (top, top + 3.5) } else { (top + 3.5, top) };
        painter.line(vec![pos2(cx - 4.0, y0), pos2(cx, y1), pos2(cx + 4.0, y0)], Stroke::new(1.2, p.text_secondary));
    }
    let width = (rect.width() - 8.0).max(0.0);
    let line = |text: &str, size: f32, color| {
        let mut job =
            egui::text::LayoutJob::simple_singleline(text.to_string(), egui::FontId::proportional(size), color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(width);
        ui.painter().layout_job(job)
    };
    let g = line(title, theme::BODY, color);
    let y = rect.bottom() - g.size().y - 3.0;
    if let Some(total) = total {
        let t = line(total, theme::BODY + 4.0, p.text);
        painter.galley(pos2(rect.left() + 4.0, y - t.size().y + 2.0), t, p.text);
    }
    painter.galley(pos2(rect.left() + 4.0, y), g, color);
    // Windows keeps the arrow pointer over headers.
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, PartialEq, Debug)]
    enum K {
        Name,
        Cpu,
    }

    fn name(k: K) -> &'static str {
        match k {
            K::Name => "name",
            K::Cpu => "cpu",
        }
    }

    fn from(s: &str) -> Option<K> {
        [K::Name, K::Cpu].into_iter().find(|k| name(*k) == s)
    }

    #[test]
    fn click_flips_then_switches() {
        let mut s = SortBy::new(K::Name, false);
        s.click(K::Name, false);
        assert_eq!(s, SortBy::new(K::Name, true));
        s.click(K::Cpu, true);
        assert_eq!(s, SortBy::new(K::Cpu, true));
        s.click(K::Cpu, true);
        assert_eq!(s, SortBy::new(K::Cpu, false));
        assert_eq!(s.arrow(K::Cpu), Some(false));
        assert_eq!(s.arrow(K::Name), None);
    }

    #[test]
    fn text_round_trip() {
        let s = SortBy::new(K::Cpu, true);
        assert_eq!(s.to_text(name), "cpu:desc");
        assert_eq!(SortBy::parse(&s.to_text(name), from), Some(s));
        assert_eq!(SortBy::parse("name", from), Some(SortBy::new(K::Name, false)));
        assert_eq!(SortBy::parse("bogus:desc", from), None);
        assert_eq!(cmp_text("apple", "Banana"), Ordering::Less);
    }
}
