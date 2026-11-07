//! Widgets drawn the same way in every app: navigation rows, toolbar buttons, page titles.

use egui::{Button, Response, Sense, Ui, Vec2, vec2};

use crate::{Icon, Palette, theme};

/// Height of a navigation row (Task Manager's pages, Explorer's sidebar).
pub const NAV_HEIGHT: f32 = 30.0;
/// Size of an icon-only toolbar button.
pub const TOOL_SIZE: Vec2 = vec2(32.0, 28.0);

/// A navigation row as Windows 11 draws one: icon and label, a soft fill on hover, and on the
/// selected row a fill plus a short accent bar at the left edge. Takes the full width; with
/// `label` empty only the icon shows (a collapsed navigation pane) and `tooltip` names it.
pub fn nav_item(ui: &mut Ui, icon: Option<Icon>, label: &str, selected: bool) -> Response {
    nav_item_sense(ui, icon, label, selected, Sense::click())
}

pub fn nav_item_sense(ui: &mut Ui, icon: Option<Icon>, label: &str, selected: bool, sense: Sense) -> Response {
    let width = ui.available_width().max(NAV_HEIGHT);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, NAV_HEIGHT), sense);
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let p = Palette::of(ui);
    let painter = ui.painter();
    let fill = if selected {
        Some(p.hover)
    } else if resp.hovered() {
        Some(theme::mix(p.chrome, p.hover, 0.6))
    } else {
        None
    };
    let body = rect.shrink2(vec2(0.0, 1.0));
    if let Some(f) = fill {
        painter.rect_filled(body, 4.0, f);
    }
    if selected {
        let bar = egui::Rect::from_center_size(egui::pos2(body.left() + 1.5, body.center().y), vec2(3.0, 16.0));
        painter.rect_filled(bar, 1.5, p.accent);
    }
    let text = if ui.is_enabled() { p.text } else { p.text_secondary };
    let mut x = rect.left() + 12.0;
    if let Some(i) = icon {
        let g = ui.painter().layout_no_wrap(i.glyph(), Icon::font(16.0), text);
        painter.galley(egui::pos2(x, rect.center().y - g.size().y / 2.0), g, text);
        x += 16.0 + 12.0;
    }
    if !label.is_empty() {
        let font = egui::FontId::proportional(theme::BODY);
        let mut job = egui::text::LayoutJob::simple_singleline(label.to_string(), font, text);
        job.wrap = egui::text::TextWrapping::truncate_at_width((rect.right() - x - 4.0).max(0.0));
        let g = ui.painter().layout_job(job);
        painter.galley(egui::pos2(x, rect.center().y - g.size().y / 2.0), g, text);
    }
    resp
}

/// An icon-only toolbar button: no frame until hovered, as in Windows 11's command bars.
pub fn tool_button_widget<'a>(icon: Icon) -> Button<'a> {
    Button::new(icon.sized(16.0)).frame_when_inactive(false).min_size(TOOL_SIZE)
}

pub fn tool_button(ui: &mut Ui, icon: Icon, enabled: bool, tooltip: &str) -> Response {
    let r = ui.add_enabled(enabled, tool_button_widget(icon));
    if tooltip.is_empty() { r } else { r.on_hover_text(tooltip) }
}

/// A toolbar button that opens a menu.
pub fn tool_menu<R>(ui: &mut Ui, icon: Icon, tooltip: &str, add: impl FnOnce(&mut Ui) -> R) -> Response {
    let (r, _) = egui::containers::menu::MenuButton::from_button(tool_button_widget(icon)).ui(ui, add);
    if tooltip.is_empty() { r } else { r.on_hover_text(tooltip) }
}

/// The title at the top of a page ("Processes", "Settings"), Segoe UI Semibold 20 px.
pub fn page_title(ui: &mut Ui, title: &str) -> Response {
    ui.label(egui::RichText::new(title).text_style(egui::TextStyle::Heading))
}

/// A secondary-text label, e.g. a hint or a unit.
pub fn secondary(ui: &mut Ui, text: impl Into<String>) -> Response {
    let c = Palette::of(ui).text_secondary;
    ui.label(egui::RichText::new(text.into()).color(c))
}

/// The frame of an app's main content (lists, tables, pages): the lighter surface Windows 11
/// draws content on, beside the navigation and toolbars on the window background.
pub fn content_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::central_panel(style).fill(Palette::get(style.visuals.dark_mode).content)
}
