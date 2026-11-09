//! Every view except Details: Extra large / Large / Medium icons (image above the name),
//! Small icons and List (icon left of the name; List fills columns top to bottom and scrolls
//! sideways), Tiles (icon with name, type and size) and Content (one full-width row per item
//! with name and type on the left, date and size on the right). Virtualised, with rubber-band
//! (drag a rectangle) selection on the background.

use std::collections::HashSet;
use std::path::PathBuf;

use eframe::egui::{self, Align, Rect, Sense, Ui};
use explorer_core::{format_size_kb, icon_for};

use crate::view::{Action, drop_hover};
use crate::{App, IconKey, IconState, ViewMode};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    /// Picture on top, name below (centred).
    Stacked,
    /// Icon on the left, one line of name.
    Inline,
    /// Icon on the left, name, type and size.
    Tile,
    /// A full-width row: name and type, then date modified and size.
    Content,
}

struct Spec {
    tile: egui::Vec2,
    img: f32,
    style: Style,
    /// Items flow down columns (List) instead of across rows.
    column_major: bool,
}

fn spec(mode: ViewMode) -> Spec {
    let s = |w: f32, h: f32, img: f32, style, column_major| Spec { tile: egui::vec2(w, h), img, style, column_major };
    match mode {
        ViewMode::ExtraLarge => s(272.0, 292.0, 256.0, Style::Stacked, false),
        ViewMode::Large => s(112.0, 124.0, 96.0, Style::Stacked, false),
        ViewMode::Medium => s(84.0, 88.0, 48.0, Style::Stacked, false),
        ViewMode::Small => s(220.0, 22.0, 16.0, Style::Inline, false),
        ViewMode::List => s(240.0, 22.0, 16.0, Style::Inline, true),
        ViewMode::Tiles | ViewMode::Details => s(260.0, 58.0, 48.0, Style::Tile, false),
        // Width is the pane's, set when drawing.
        ViewMode::Content => s(0.0, 62.0, 48.0, Style::Content, false),
    }
}

/// How the items of an icon view are laid out, for the arrow keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridNav {
    /// Items in a row (in List: in a column).
    pub per_line: usize,
    /// List: items flow down columns.
    pub column_major: bool,
    /// Each group's first position and item count (one group when not grouped).
    pub groups: Vec<(usize, usize)>,
}

/// An arrow key in an icon view moves to the item drawn next to `pos` in that direction, as in
/// Explorer: Left/Right along the row, Up/Down to the row above or below (into the next or the
/// previous group too). `None` when there is nothing that way.
pub fn arrow_step(nav: &GridNav, pos: usize, dir: egui::Key) -> Option<usize> {
    use egui::Key;
    let per = nav.per_line.max(1);
    let n: usize = nav.groups.iter().map(|g| g.1).sum();
    if n == 0 || pos >= n {
        return None;
    }
    // In List the columns are the lines: Up/Down move by one, Left/Right by a column.
    let (along, across_back, across_fwd) = if nav.column_major {
        (matches!(dir, Key::ArrowUp | Key::ArrowDown), Key::ArrowLeft, Key::ArrowRight)
    } else {
        (matches!(dir, Key::ArrowLeft | Key::ArrowRight), Key::ArrowUp, Key::ArrowDown)
    };
    if along {
        let back = matches!(dir, Key::ArrowLeft | Key::ArrowUp);
        return if back { pos.checked_sub(1) } else { Some(pos + 1).filter(|&p| p < n) };
    }
    if nav.column_major {
        return match dir {
            d if d == across_back => pos.checked_sub(per),
            d if d == across_fwd => Some((pos + per).min(n - 1)).filter(|&p| p != pos),
            _ => None,
        };
    }
    let k = nav.groups.iter().rposition(|g| g.0 <= pos)?;
    let (start, len) = nav.groups[k];
    let (row, col) = ((pos - start) / per, (pos - start) % per);
    let rows = |len: usize| len.div_ceil(per);
    match dir {
        d if d == across_fwd => {
            if row + 1 < rows(len) {
                Some((start + (row + 1) * per + col).min(start + len - 1))
            } else {
                let &(s2, l2) = nav.groups[k + 1..].iter().find(|g| g.1 > 0)?;
                Some(s2 + col.min(l2 - 1))
            }
        }
        d if d == across_back => {
            if row > 0 {
                Some(pos - per)
            } else {
                let &(s2, l2) = nav.groups[..k].iter().rev().find(|g| g.1 > 0)?;
                Some((s2 + (rows(l2) - 1) * per + col).min(s2 + l2 - 1))
            }
        }
        _ => None,
    }
}

/// Height of a group heading in the icon views.
const HEADING: f32 = 32.0;

/// Thumbnail size the view asks the shell for: the size the picture is drawn at, in pixels of
/// this display. The shell scales it smoothly; scaling a bigger one down here would leave jagged
/// edges, and a smaller one up would blur it (at 125 % or 150 % display scaling).
pub fn thumb_px(mode: ViewMode, pixels_per_point: f32) -> u32 {
    (spec(mode).img * pixels_per_point.max(0.5)).round().clamp(16.0, 1024.0) as u32
}

/// Whether the view draws shell thumbnails (otherwise small system icons).
fn uses_thumbs(mode: ViewMode) -> bool {
    matches!(mode, ViewMode::ExtraLarge | ViewMode::Large | ViewMode::Medium | ViewMode::Tiles | ViewMode::Content)
}

pub fn show(
    app: &mut App,
    ui: &mut Ui,
    action: &mut Option<Action>,
    wanted_icons: &mut Vec<IconKey>,
    shell_menus: bool,
    in_bin: bool,
) {
    let mut sp = spec(app.view_mode);
    let thumbs = uses_thumbs(app.view_mode) && !in_bin;
    let n = app.view.len();
    let avail = ui.available_size();
    if sp.style == Style::Content {
        sp.tile.x = (avail.x - 14.0).max(320.0);
    }
    // Compact view: less padding around every item.
    if app.compact {
        sp.tile.y -= if sp.style == Style::Inline { 4.0 } else { 8.0 };
        if sp.style == Style::Stacked {
            sp.tile.x -= 8.0;
        }
    }
    // Grid shape: `major` lines (rows, or List's columns) of `per_line` items.
    let per_line = if sp.column_major {
        ((avail.y / sp.tile.y).floor() as usize).max(1)
    } else {
        ((avail.x / sp.tile.x).floor() as usize).max(1)
    };
    let lines = n.div_ceil(per_line);
    // Groups (row-major views only): each starts on a new line under a heading.
    // (first position, count, top y)
    let grouped = !app.groups.is_empty() && !sp.column_major;
    let head = if grouped { HEADING } else { 0.0 };
    let mut glayout: Vec<(usize, usize, f32)> = Vec::new();
    let mut total_y = 0.0;
    if grouped {
        for g in &app.groups {
            glayout.push((g.start, g.len, total_y));
            total_y += head + g.len.div_ceil(per_line) as f32 * sp.tile.y;
        }
    } else {
        glayout.push((0, n, 0.0));
        total_y = lines as f32 * sp.tile.y;
    }
    // Offset of an item from the content origin.
    let offset = |pos: usize| -> egui::Vec2 {
        if sp.column_major {
            return egui::vec2((pos / per_line) as f32 * sp.tile.x, (pos % per_line) as f32 * sp.tile.y);
        }
        let k = glayout.partition_point(|g| g.0 <= pos).saturating_sub(1);
        let (start, _, y) = glayout[k];
        let j = pos - start;
        egui::vec2((j % per_line) as f32 * sp.tile.x, y + head + (j / per_line) as f32 * sp.tile.y)
    };
    app.grid_nav = Some(GridNav {
        per_line,
        column_major: sp.column_major,
        groups: if grouped { glayout.iter().map(|g| (g.0, g.1)).collect() } else { vec![(0, n)] },
    });
    let content = if sp.column_major {
        egui::vec2(lines as f32 * sp.tile.x, per_line as f32 * sp.tile.y)
    } else {
        egui::vec2(per_line as f32 * sp.tile.x, total_y)
    };

    let mut scroll = if sp.column_major { egui::ScrollArea::horizontal() } else { egui::ScrollArea::vertical() }
        .auto_shrink([false, false]);
    if let Some(pos) = app.scroll_to.take().filter(|&p| p < n) {
        let o = offset(pos);
        scroll = if sp.column_major {
            scroll.horizontal_scroll_offset((o.x - avail.x / 2.0).max(0.0))
        } else {
            scroll.vertical_scroll_offset((o.y - avail.y / 2.0).max(0.0))
        };
    }

    let dragging = app.drag.is_some();
    let mut drag_start: Option<String> = None;
    let mut hover_target: Option<PathBuf> = None;
    let mut band_select: Option<(HashSet<String>, bool)> = None;
    let weak = ui.visuals().weak_text_color();
    let text = ui.visuals().text_color();
    let sel_fill = ui.visuals().selection.bg_fill.gamma_multiply(0.6);
    let hover_fill = ui.visuals().widgets.hovered.weak_bg_fill;
    let folder = ui_common::Palette::of(ui).folder;

    scroll.show_viewport(ui, |ui, viewport| {
        let origin = ui.max_rect().min;
        let full = Rect::from_min_size(origin, content.max(viewport.size()));
        ui.set_min_size(full.size());
        let tile_rect = |pos: usize| Rect::from_min_size(origin + offset(pos), sp.tile);

        // Background first, so items (added later) win the pointer: click clears the
        // selection, right-click is the folder menu, drag draws the selection rectangle.
        let bg = ui.interact(full, ui.id().with("grid-bg"), Sense::click_and_drag());
        if bg.clicked() {
            *action = Some(Action::ClearSelection);
        }
        if bg.secondary_clicked() && shell_menus {
            *action = Some(Action::ShellMenu(None));
        }
        let band_id = ui.id().with("band");
        if bg.drag_started()
            && let Some(p) = bg.interact_pointer_pos()
        {
            ui.data_mut(|d| d.insert_temp(band_id, p - origin));
        }
        if bg.dragged()
            && let (Some(start), Some(now)) =
                (ui.data(|d| d.get_temp::<egui::Vec2>(band_id)), bg.interact_pointer_pos())
        {
            let band = Rect::from_two_pos(origin + start, now);
            ui.painter().rect(
                band,
                0.0,
                ui.visuals().selection.bg_fill.gamma_multiply(0.25),
                egui::Stroke::new(1.0, ui.visuals().selection.bg_fill),
                egui::StrokeKind::Inside,
            );
            let names = (0..n)
                .filter(|&p| tile_rect(p).shrink(4.0).intersects(band))
                .map(|p| app.entries[app.view[p]].name.clone())
                .collect();
            let add = ui.input(|i| i.modifiers.command);
            band_select = Some((names, add));
            // Scroll when the band reaches the edge.
            ui.scroll_to_rect(Rect::from_center_size(now, egui::vec2(1.0, 1.0)), None);
        }
        if bg.drag_stopped() {
            ui.data_mut(|d| d.remove::<egui::Vec2>(band_id));
        }

        // Only the items (and group headings) in view.
        let mut visible: Vec<usize> = Vec::new();
        if sp.column_major {
            let first = (viewport.min.x / sp.tile.x).floor().max(0.0) as usize;
            let last = (viewport.max.x / sp.tile.x).ceil() as usize;
            visible.extend((first * per_line).min(n)..((last + 1) * per_line).min(n));
        } else {
            for (k, &(start, len, y)) in glayout.iter().enumerate() {
                let rows = len.div_ceil(per_line) as f32;
                let (top, bottom) = (y, y + head + rows * sp.tile.y);
                if bottom < viewport.min.y || top > viewport.max.y {
                    continue;
                }
                if grouped {
                    let g = &app.groups[k];
                    let at = origin + egui::vec2(8.0, y + 6.0);
                    let galley = ui.painter().layout_no_wrap(
                        format!("{}  ({})", g.label, g.len),
                        egui::FontId::proportional(14.0),
                        ui.visuals().hyperlink_color,
                    );
                    let w = galley.size().x;
                    ui.painter().galley(at, galley, text);
                    let ly = origin.y + y + 6.0 + 9.0;
                    ui.painter().hline(
                        (origin.x + 16.0 + w)..=(origin.x + full.width() - 8.0),
                        ly,
                        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                    );
                }
                let first = ((viewport.min.y - y - head) / sp.tile.y).floor().max(0.0) as usize;
                let last = ((viewport.max.y - y - head) / sp.tile.y).ceil().max(0.0) as usize;
                visible.extend(
                    (start + first * per_line).min(start + len)..(start + (last + 1) * per_line).min(start + len),
                );
            }
        }
        for pos in visible {
            let rect = tile_rect(pos);
            let resp = ui.interact(rect, ui.id().with(("item", pos)), Sense::click_and_drag());
            let e = &app.entries[app.view[pos]];
            if resp.drag_started() && !in_bin {
                drag_start = Some(e.name.clone());
            }
            if app.selected.contains(&e.name) {
                ui.painter().rect_filled(rect.shrink(2.0), 4.0, sel_fill);
            } else if resp.hovered() {
                ui.painter().rect_filled(rect.shrink(2.0), 4.0, hover_fill);
            }
            let path = app.child(&e.name);
            if e.is_dir && !app.is_dragged(&e.name) {
                drop_hover(&resp, dragging, &path, &mut hover_target);
            }

            let img_rect = match sp.style {
                Style::Stacked => Rect::from_center_size(
                    egui::pos2(rect.center().x, rect.top() + 6.0 + sp.img / 2.0),
                    egui::vec2(sp.img, sp.img),
                ),
                Style::Inline | Style::Tile | Style::Content => Rect::from_center_size(
                    egui::pos2(rect.left() + 6.0 + sp.img / 2.0, rect.center().y),
                    egui::vec2(sp.img, sp.img),
                ),
            };
            // A shell thumbnail (or the shell's large icon for anything without one), else
            // the small system icon, else a glyph.
            let thumb = if thumbs { app.thumbs.get(&path) } else { None };
            let painted = match thumb {
                Some(IconState::Ready(tex)) => {
                    // Texture pixels to points: drawn at most at its own size on this display.
                    let s = tex.size_vec2() / ui.ctx().pixels_per_point();
                    let k = (sp.img / s.x.max(s.y)).min(1.0);
                    let r = Rect::from_center_size(img_rect.center(), s * k);
                    ui.painter().image(
                        tex.id(),
                        r,
                        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                    true
                }
                None if thumbs => {
                    app.thumb_wanted.push(path.clone());
                    false
                }
                _ => false,
            };
            if !painted {
                let key = IconKey::for_entry(e, &app.cur);
                match app.icons.get(&key) {
                    Some(IconState::Ready(tex)) => {
                        let side = sp.img.min(48.0);
                        let r = Rect::from_center_size(img_rect.center(), egui::vec2(side, side));
                        ui.painter().image(
                            tex.id(),
                            r,
                            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                    state => {
                        if state.is_none() {
                            wanted_icons.push(key);
                        }
                        let color = if e.is_dir { folder } else { text };
                        ui.painter().text(
                            img_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            icon_for(e),
                            egui::FontId::proportional((sp.img * 0.45).max(13.0)),
                            color,
                        );
                    }
                }
            }

            let name_color = if e.hidden || e.system { weak } else { text };
            let name = if app.show_extensions || e.is_dir {
                e.display().to_string()
            } else {
                let d = e.display();
                d.rfind('.').filter(|&i| i > 0).map_or(d, |i| &d[..i]).to_string()
            };
            match sp.style {
                Style::Stacked => {
                    let mut job = egui::text::LayoutJob::simple(
                        name,
                        egui::FontId::proportional(12.5),
                        name_color,
                        sp.tile.x - 8.0,
                    );
                    job.wrap.max_rows = 2;
                    job.halign = Align::Center;
                    let galley = ui.fonts_mut(|f| f.layout_job(job));
                    ui.painter().galley(egui::pos2(rect.center().x, img_rect.bottom() + 4.0), galley, text);
                }
                Style::Inline => {
                    let x = img_rect.right() + 6.0;
                    let mut job =
                        egui::text::LayoutJob::simple_singleline(name, egui::FontId::proportional(13.0), name_color);
                    job.wrap.max_width = rect.right() - x - 4.0;
                    job.wrap.max_rows = 1;
                    job.wrap.break_anywhere = true;
                    let galley = ui.fonts_mut(|f| f.layout_job(job));
                    let y = rect.center().y - galley.size().y / 2.0;
                    ui.painter().galley(egui::pos2(x, y), galley, text);
                }
                Style::Tile => {
                    let x = img_rect.right() + 8.0;
                    let width = rect.right() - x - 4.0;
                    let second = e.note.clone().unwrap_or_else(|| e.type_name());
                    let third = if e.is_dir { String::new() } else { format_size_kb(e.size) };
                    for (i, (t, c)) in [(name, name_color), (second, weak), (third, weak)].into_iter().enumerate() {
                        let mut job = egui::text::LayoutJob::simple_singleline(t, egui::FontId::proportional(12.5), c);
                        job.wrap.max_width = width;
                        job.wrap.max_rows = 1;
                        job.wrap.break_anywhere = true;
                        let galley = ui.fonts_mut(|f| f.layout_job(job));
                        ui.painter().galley(egui::pos2(x, rect.top() + 6.0 + i as f32 * 15.0), galley, text);
                    }
                }
                Style::Content => {
                    let x = img_rect.right() + 10.0;
                    let right = rect.left() + rect.width() * 0.58;
                    let line = |t: String, c, size, at: egui::Pos2, max: f32| {
                        let mut job = egui::text::LayoutJob::simple_singleline(t, egui::FontId::proportional(size), c);
                        job.wrap.max_width = max;
                        job.wrap.max_rows = 1;
                        job.wrap.break_anywhere = true;
                        let galley = ui.fonts_mut(|f| f.layout_job(job));
                        ui.painter().galley(at, galley, text);
                    };
                    let (y1, y2) = (rect.top() + 12.0, rect.top() + 32.0);
                    line(name, name_color, 14.0, egui::pos2(x, y1), right - x - 12.0);
                    let kind = e.note.clone().unwrap_or_else(|| e.type_name());
                    line(format!("Type: {kind}"), weak, 12.5, egui::pos2(x, y2), right - x - 12.0);
                    let width = rect.right() - right - 8.0;
                    let when = explorer_core::format_time(e.modified, app.utc_offset);
                    line(format!("Date modified: {when}"), weak, 12.5, egui::pos2(right, y1), width);
                    if !e.is_dir {
                        line(format!("Size: {}", format_size_kb(e.size)), weak, 12.5, egui::pos2(right, y2), width);
                    }
                    ui.painter().hline(
                        rect.x_range(),
                        rect.bottom() - 0.5,
                        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                    );
                }
            }

            // Item check boxes: on hover or when selected, top-left of the item.
            let mut ticked = false;
            if app.item_checkboxes && (resp.hovered() || app.selected.contains(&e.name)) {
                let mut on = app.selected.contains(&e.name);
                let r = Rect::from_min_size(rect.min + egui::vec2(4.0, 4.0), egui::vec2(18.0, 18.0));
                ticked = ui.put(r, egui::Checkbox::without_text(&mut on)).changed();
            }
            if ticked {
                *action = Some(Action::Click(pos, egui::Modifiers::COMMAND | egui::Modifiers::CTRL));
            } else if resp.middle_clicked() && e.is_dir {
                *action = Some(Action::OpenInNewTab(pos));
            } else if resp.double_clicked() {
                *action = Some(Action::Open(pos));
            } else if resp.clicked() {
                *action = Some(Action::Click(pos, resp.ctx.input(|i| i.modifiers)));
            } else if resp.secondary_clicked() {
                *action = Some(if shell_menus { Action::ShellMenu(Some(pos)) } else { Action::ContextSelect(pos) });
            }
        }
    });

    if let Some((names, add)) = band_select {
        // Ctrl adds to the selection the band started with.
        let base_id = egui::Id::new("band-base");
        let base: HashSet<String> = if add {
            ui.data_mut(|d| d.get_temp_mut_or_insert_with(base_id, || app.selected.clone()).clone())
        } else {
            HashSet::new()
        };
        app.selected = base.union(&names).cloned().collect();
    } else {
        ui.data_mut(|d| d.remove::<HashSet<String>>(egui::Id::new("band-base")));
    }
    if let Some(n) = drag_start {
        app.drag_source(&n);
    }
    if let Some(t) = hover_target {
        app.drop_target = Some(t);
    }
}

#[cfg(test)]
mod tests {
    use super::{GridNav, arrow_step};
    use eframe::egui::Key::{ArrowDown as D, ArrowLeft as L, ArrowRight as R, ArrowUp as U};

    #[test]
    fn arrows_in_rows() {
        // 10 items, 4 per row: 0 1 2 3 / 4 5 6 7 / 8 9
        let nav = GridNav { per_line: 4, column_major: false, groups: vec![(0, 10)] };
        assert_eq!(arrow_step(&nav, 0, R), Some(1));
        assert_eq!(arrow_step(&nav, 3, R), Some(4));
        assert_eq!(arrow_step(&nav, 0, L), None);
        assert_eq!(arrow_step(&nav, 9, R), None);
        assert_eq!(arrow_step(&nav, 1, D), Some(5));
        assert_eq!(arrow_step(&nav, 7, D), Some(9));
        assert_eq!(arrow_step(&nav, 9, D), None);
        assert_eq!(arrow_step(&nav, 6, U), Some(2));
        assert_eq!(arrow_step(&nav, 2, U), None);
    }

    #[test]
    fn arrows_across_groups() {
        // Group A: 0 1 2 / 3 4 ; group B: 5 6 7 / 8
        let nav = GridNav { per_line: 3, column_major: false, groups: vec![(0, 5), (5, 4)] };
        assert_eq!(arrow_step(&nav, 4, D), Some(6));
        assert_eq!(arrow_step(&nav, 2, D), Some(4));
        assert_eq!(arrow_step(&nav, 7, U), Some(4));
        assert_eq!(arrow_step(&nav, 5, U), Some(3));
        assert_eq!(arrow_step(&nav, 8, U), Some(5));
    }

    #[test]
    fn arrows_in_list_columns() {
        // List, 3 per column: 0 3 6 / 1 4 7 / 2 5
        let nav = GridNav { per_line: 3, column_major: true, groups: vec![(0, 8)] };
        assert_eq!(arrow_step(&nav, 1, D), Some(2));
        assert_eq!(arrow_step(&nav, 1, R), Some(4));
        assert_eq!(arrow_step(&nav, 5, R), Some(7));
        assert_eq!(arrow_step(&nav, 7, R), None);
        assert_eq!(arrow_step(&nav, 4, L), Some(1));
        assert_eq!(arrow_step(&nav, 1, L), None);
    }
}
