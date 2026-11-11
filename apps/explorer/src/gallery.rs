//! The Gallery: photos and videos from collection folders, newest first,
//! laid out in justified rows with month headings and a timeline scrubber.

use std::collections::HashSet;
use std::path::PathBuf;

use eframe::egui::{self, Rect, Sense, Stroke, Ui, pos2, vec2};
use explorer_core::{format_month_year, justified_layout, timeline_hit, timeline_positions};

use crate::view::{Action, drop_hover};
use crate::{App, IconState};

/// Draws the Gallery photo view.
pub fn show(app: &mut App, ui: &mut Ui, action: &mut Option<Action>, shell_menus: bool) {
    let n = app.view.len();
    if n == 0 {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new("No pictures or videos in this collection.").weak());
        });
        return;
    }

    let avail = ui.available_size();
    let scrubber_w = 32.0;
    let container_w = (avail.x - scrubber_w - 12.0).max(120.0);
    let gap = 4.0;
    let target_h = app.gallery_row_height.clamp(80.0, 400.0);

    // Ctrl+wheel scales row height
    let ctrl_held = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
    if ctrl_held {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        if dy.abs() > 0.0 {
            app.gallery_row_height = (app.gallery_row_height + dy * 0.5).clamp(80.0, 400.0);
        }
    }

    // Collect aspect ratios from loaded thumbnails, defaulting to 4:3
    let mut aspect_ratios = Vec::with_capacity(n);
    for &idx in &app.view {
        let e = &app.entries[idx];
        let p = PathBuf::from(&e.name);
        let ar = match app.thumbs.get(&p) {
            Some(IconState::Ready(tex)) => {
                let sz = tex.size_vec2();
                if sz.y > 0.0 { sz.x / sz.y } else { 1.33 }
            }
            _ => 1.33,
        };
        aspect_ratios.push(ar);
    }

    // Group items into months
    let mut month_groups: Vec<(String, usize, usize)> = Vec::new();
    let mut cur_month = String::new();
    let mut group_start = 0;
    for (i, &idx) in app.view.iter().enumerate() {
        let m = format_month_year(app.entries[idx].modified, app.utc_offset);
        if i == 0 {
            cur_month = m;
            group_start = 0;
        } else if m != cur_month {
            month_groups.push((cur_month, group_start, i - group_start));
            cur_month = m;
            group_start = i;
        }
    }
    if !app.view.is_empty() {
        month_groups.push((cur_month, group_start, n - group_start));
    }

    // Build justified rows for each month
    struct MonthLayout {
        label: String,
        item_start: usize,
        rows: Vec<Vec<(usize, f32)>>,
        row_heights: Vec<f32>,
    }

    let mut layouts = Vec::with_capacity(month_groups.len());
    for (label, start, len) in &month_groups {
        let ratios = &aspect_ratios[*start..*start + len];
        let rows = justified_layout(ratios, container_w, target_h, gap);
        let mut row_heights = Vec::with_capacity(rows.len());
        for (r_idx, r) in rows.iter().enumerate() {
            let is_last = r_idx + 1 == rows.len();
            let sum_r: f32 = r.iter().map(|&(i, _)| ratios[i].max(0.1)).sum();
            let k = r.len() as f32;
            let natural_w = sum_r * target_h + (k - 1.0) * gap;
            if is_last && natural_w < container_w {
                row_heights.push(target_h);
            } else {
                let avail_w = (container_w - (k - 1.0) * gap).max(1.0);
                row_heights.push((avail_w / sum_r).max(40.0));
            }
        }
        layouts.push(MonthLayout { label: label.clone(), item_start: *start, rows, row_heights });
    }

    // Scrubber setup on right edge
    let whole_rect = ui.available_rect_before_wrap();
    let scrubber_rect = Rect::from_min_max(
        pos2(whole_rect.right() - scrubber_w, whole_rect.top()),
        pos2(whole_rect.right(), whole_rect.bottom()),
    );

    let month_counts: Vec<(&str, usize)> = month_groups.iter().map(|(m, _, c)| (m.as_str(), *c)).collect();
    let segs = timeline_positions(&month_counts, scrubber_rect.height());

    let mut scroll_target_row: Option<usize> = None;
    if let Some(target_pos) = app.scroll_to.take() {
        scroll_target_row = Some(target_pos);
    }

    let dragging = app.drag.is_some();
    let mut drag_start: Option<String> = None;
    let mut hover_target: Option<PathBuf> = None;
    let mut band_select: Option<(HashSet<String>, bool)> = None;
    let sel_fill = ui.visuals().selection.bg_fill.gamma_multiply(0.6);
    let hover_fill = ui.visuals().widgets.hovered.weak_bg_fill;

    let mut sticky_month = month_groups.first().map(|(m, _, _)| m.clone()).unwrap_or_default();

    let mut item_rects: Vec<(usize, Rect)> = Vec::new();
    let scroll_area = egui::ScrollArea::vertical().auto_shrink(false);
    scroll_area.show_viewport(ui, |ui, viewport| {
        let origin = ui.max_rect().min;
        let full = Rect::from_min_size(origin, vec2(container_w, viewport.height()));
        let bg = ui.interact(full, ui.id().with("gal-bg"), Sense::click_and_drag());
        if bg.clicked() {
            *action = Some(Action::ClearSelection);
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
                Stroke::new(1.0, ui.visuals().selection.bg_fill),
                egui::StrokeKind::Inside,
            );
            let names: HashSet<String> = item_rects
                .iter()
                .filter(|(_, r)| r.shrink(4.0).intersects(band))
                .map(|&(p, _)| app.entries[app.view[p]].name.clone())
                .collect();
            let add = ui.input(|i| i.modifiers.command);
            band_select = Some((names, add));
            ui.scroll_to_rect(Rect::from_center_size(now, egui::vec2(1.0, 1.0)), None);
        }
        if bg.drag_stopped() {
            ui.data_mut(|d| d.remove::<egui::Vec2>(band_id));
        }

        let mut cur_y = origin.y + 4.0;

        for m_layout in &layouts {
            let heading_h = 28.0;
            if cur_y <= viewport.min.y + 10.0 {
                sticky_month = m_layout.label.clone();
            }
            let heading_rect = Rect::from_min_size(pos2(origin.x + 4.0, cur_y), vec2(container_w, heading_h));
            if heading_rect.bottom() >= viewport.min.y && heading_rect.top() <= viewport.max.y {
                ui.painter().text(
                    pos2(heading_rect.left() + 4.0, heading_rect.top() + 6.0),
                    egui::Align2::LEFT_TOP,
                    &m_layout.label,
                    egui::FontId::proportional(16.0),
                    ui.visuals().text_color(),
                );
            }
            cur_y += heading_h;

            for (r_idx, row) in m_layout.rows.iter().enumerate() {
                let h = m_layout.row_heights[r_idx];
                let row_top = cur_y;
                let row_bottom = cur_y + h;

                let row_visible = row_bottom >= viewport.min.y && row_top <= viewport.max.y;
                let mut cur_x = origin.x + 4.0;

                for &(item_rel_idx, item_w) in row {
                    let global_pos = m_layout.item_start + item_rel_idx;
                    let item_rect = Rect::from_min_size(pos2(cur_x, cur_y), vec2(item_w, h));

                    if scroll_target_row == Some(global_pos) {
                        ui.scroll_to_rect(item_rect, None);
                    }

                    item_rects.push((global_pos, item_rect));
                    if row_visible {
                        let e = &app.entries[app.view[global_pos]];
                        let path = PathBuf::from(&e.name);
                        let resp =
                            ui.interact(item_rect, ui.id().with(("gal_item", global_pos)), Sense::click_and_drag());
                        let resp =
                            if app.drag.is_some() { resp } else { crate::view::with_info_tip(resp, e, app.utc_offset) };

                        if resp.drag_started() {
                            drag_start = Some(e.name.clone());
                        }

                        let is_selected = app.selected.contains(&e.name);
                        if is_selected {
                            ui.painter().rect_filled(item_rect, 4.0, sel_fill);
                        } else if resp.hovered() {
                            ui.painter().rect_filled(item_rect, 4.0, hover_fill);
                        }

                        // Thumbnail image
                        let cut = app.is_cut(&e.name);
                        let tint = if cut { egui::Color32::WHITE.gamma_multiply(0.5) } else { egui::Color32::WHITE };
                        let thumb = app.thumbs.get(&path);
                        match thumb {
                            Some(IconState::Ready(tex)) => {
                                ui.painter().image(
                                    tex.id(),
                                    item_rect.shrink(2.0),
                                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                    tint,
                                );
                            }
                            _ => {
                                app.thumb_wanted.push(path.clone());
                                ui.painter().rect_filled(item_rect.shrink(2.0), 2.0, ui.visuals().faint_bg_color);
                            }
                        }

                        // Selection interaction
                        if resp.double_clicked() {
                            *action = Some(Action::Open(global_pos));
                        } else if resp.clicked() {
                            *action = Some(Action::Click(global_pos, resp.ctx.input(|i| i.modifiers)));
                        } else if resp.secondary_clicked() {
                            let shift = ui.input(|i| i.modifiers.shift);
                            *action = Some(if shell_menus && shift {
                                Action::ShellMenu(Some(global_pos))
                            } else {
                                Action::ContextSelect(global_pos)
                            });
                        }

                        resp.context_menu(|ui| {
                            let info = crate::menus::ItemInfo::new(app, e, global_pos, shell_menus);
                            crate::menus::item(ui, &info, action);
                        });

                        if e.is_dir && !app.is_dragged(&e.name) {
                            drop_hover(&resp, dragging, &path, &mut hover_target);
                        }
                    }

                    cur_x += item_w + gap;
                }
                cur_y += h + gap;
            }
            cur_y += 12.0;
        }

        // Sticky month floating header
        if !sticky_month.is_empty() {
            let galley = ui.painter().layout_no_wrap(
                sticky_month.clone(),
                egui::FontId::proportional(15.0),
                ui.visuals().text_color(),
            );
            let pill_rect = Rect::from_min_size(
                pos2(origin.x + 8.0, viewport.min.y + 6.0),
                vec2(galley.size().x + 16.0, galley.size().y + 8.0),
            );
            ui.painter().rect_filled(pill_rect, 6.0, ui.visuals().window_fill().gamma_multiply(0.92));
            ui.painter().rect_stroke(
                pill_rect,
                6.0,
                Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                egui::StrokeKind::Inside,
            );
            ui.painter().galley(pos2(pill_rect.left() + 8.0, pill_rect.top() + 4.0), galley, ui.visuals().text_color());
        }
        ui.set_min_height(cur_y - origin.y);
    });

    // Draw timeline scrubber on right
    let scrubber_resp = ui.interact(scrubber_rect, ui.id().with("scrubber"), Sense::click_and_drag());
    let painter = ui.painter_at(scrubber_rect);
    painter.rect_filled(scrubber_rect, 0.0, ui.visuals().faint_bg_color);
    painter.line_segment(
        [pos2(scrubber_rect.left(), scrubber_rect.top()), pos2(scrubber_rect.left(), scrubber_rect.bottom())],
        Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );

    for seg in &segs {
        let tick_y = scrubber_rect.top() + seg.y_start;
        painter.line_segment(
            [pos2(scrubber_rect.left() + 2.0, tick_y), pos2(scrubber_rect.left() + 8.0, tick_y)],
            Stroke::new(1.0, ui.visuals().weak_text_color()),
        );
    }

    if (scrubber_resp.hovered() || scrubber_resp.dragged())
        && let Some(pos) = scrubber_resp.interact_pointer_pos()
    {
        let rel_y = pos.y - scrubber_rect.top();
        if let Some(seg) = timeline_hit(&segs, rel_y) {
            // Show floating label beside scrubber
            let label_pos = pos2(scrubber_rect.left() - 8.0, pos.y);
            let galley = ui.painter().layout_no_wrap(
                seg.label.clone(),
                egui::FontId::proportional(13.0),
                ui.visuals().text_color(),
            );
            let badge_rect = Rect::from_min_size(
                pos2(label_pos.x - galley.size().x - 12.0, label_pos.y - galley.size().y / 2.0 - 4.0),
                vec2(galley.size().x + 12.0, galley.size().y + 8.0),
            );
            ui.painter().rect_filled(badge_rect, 4.0, ui.visuals().window_fill());
            ui.painter().rect_stroke(
                badge_rect,
                4.0,
                Stroke::new(1.0, ui.visuals().window_stroke().color),
                egui::StrokeKind::Inside,
            );
            ui.painter().galley(
                pos2(badge_rect.left() + 6.0, badge_rect.top() + 4.0),
                galley,
                ui.visuals().text_color(),
            );

            if scrubber_resp.clicked() || scrubber_resp.dragged() {
                app.scroll_to = Some(seg.item_index);
            }
        }
    }

    // Drag start
    if let Some(n) = drag_start {
        app.drag_source(&n);
    }

    if let Some((names, add)) = band_select {
        if add {
            app.selected.extend(names);
        } else {
            app.selected = names;
        }
    }

    app.drop_target = hover_target;
}
