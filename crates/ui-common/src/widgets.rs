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
    nav_item_opt(ui, icon, label, selected, Sense::click(), true)
}

pub fn nav_item_sense(ui: &mut Ui, icon: Option<Icon>, label: &str, selected: bool, sense: Sense) -> Response {
    nav_item_opt(ui, icon, label, selected, sense, true)
}

/// A navigation row with control over whether the static selection indicator bar is drawn.
pub fn nav_item_opt(
    ui: &mut Ui,
    icon: Option<Icon>,
    label: &str,
    selected: bool,
    sense: Sense,
    draw_indicator: bool,
) -> Response {
    let width = ui.available_width().max(NAV_HEIGHT);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, NAV_HEIGHT), sense);
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let p = Palette::of(ui);
    let painter = ui.painter();
    let body = rect.shrink2(vec2(0.0, 1.0));
    if selected {
        painter.rect_filled(body, 4.0, p.hover);
    } else {
        let t = crate::motion::fade(ui.ctx(), resp.id, resp.hovered());
        if t > 0.0 {
            let hover_color = theme::mix(p.chrome, p.hover, 0.6);
            painter.rect_filled(body, 4.0, crate::motion::lerp_color(egui::Color32::TRANSPARENT, hover_color, t));
        }
    }
    if selected && draw_indicator {
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

/// Height of a command-bar button (Explorer's New, Sort, View; Task Manager's page commands).
pub const COMMAND_HEIGHT: f32 = 32.0;

/// A command-bar button as Windows 11 draws one: icon, then a label when there is one, no frame
/// until hovered, a darker fill while pressed. `tooltip` names it and its shortcut.
pub fn command_button(ui: &mut Ui, icon: Icon, label: &str, enabled: bool, tooltip: &str) -> Response {
    let text = icon.with_sized(label, 16.0, theme::BODY);
    let bg_slot = ui.painter().add(egui::Shape::Noop);
    let b = Button::new(text)
        .frame_when_inactive(false)
        .fill(egui::Color32::TRANSPARENT)
        .min_size(vec2(COMMAND_HEIGHT, COMMAND_HEIGHT));
    let r = ui.add_enabled(enabled, b);
    if ui.is_rect_visible(r.rect) {
        let p = Palette::of(ui);
        if r.is_pointer_button_down_on() {
            ui.painter().set(bg_slot, egui::epaint::RectShape::filled(r.rect, 4.0, theme::mix(p.hover, p.text, 0.08)));
        } else {
            let t = crate::motion::fade(ui.ctx(), r.id, r.hovered() && enabled);
            if t > 0.0 {
                let fill = crate::motion::lerp_color(egui::Color32::TRANSPARENT, p.hover, t);
                ui.painter().set(bg_slot, egui::epaint::RectShape::filled(r.rect, 4.0, fill));
            }
        }
    }
    if tooltip.is_empty() { r } else { r.on_hover_text(tooltip) }
}

/// A command-bar button with a chevron that opens a menu (New, Sort, View).
pub fn command_menu<R>(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    tooltip: &str,
    add: impl FnOnce(&mut Ui) -> R,
) -> Response {
    let mut job = egui::text::LayoutJob::default();
    let fmt = |font_id| egui::TextFormat {
        font_id,
        color: egui::Color32::PLACEHOLDER,
        valign: egui::Align::Center,
        ..Default::default()
    };
    job.append(&icon.glyph(), 0.0, fmt(Icon::font(16.0)));
    if !label.is_empty() {
        job.append(label, 8.0, fmt(egui::FontId::proportional(theme::BODY)));
    }
    job.append(&Icon::ChevronDown.glyph(), 8.0, fmt(Icon::font(10.0)));
    let bg_slot = ui.painter().add(egui::Shape::Noop);
    let b = Button::new(egui::WidgetText::LayoutJob(job.into()))
        .frame_when_inactive(false)
        .fill(egui::Color32::TRANSPARENT)
        .min_size(vec2(COMMAND_HEIGHT, COMMAND_HEIGHT));
    let (r, _) = egui::containers::menu::MenuButton::from_button(b).ui(ui, add);
    if ui.is_rect_visible(r.rect) {
        let p = Palette::of(ui);
        if r.is_pointer_button_down_on() {
            ui.painter().set(bg_slot, egui::epaint::RectShape::filled(r.rect, 4.0, theme::mix(p.hover, p.text, 0.08)));
        } else {
            let t = crate::motion::fade(ui.ctx(), r.id, r.hovered());
            if t > 0.0 {
                let fill = crate::motion::lerp_color(egui::Color32::TRANSPARENT, p.hover, t);
                ui.painter().set(bg_slot, egui::epaint::RectShape::filled(r.rect, 4.0, fill));
            }
        }
    }
    if tooltip.is_empty() { r } else { r.on_hover_text(tooltip) }
}

/// A toggle drawn with one of the layout pictures ([`crate::ViewIcon`]), with an optional label:
/// Explorer's Details button and the two view switches in its status bar. No frame until
/// hovered; a soft fill while `on`.
pub fn view_toggle(ui: &mut Ui, icon: crate::ViewIcon, label: &str, on: bool, tooltip: &str) -> Response {
    let p = Palette::of(ui);
    let font = egui::FontId::proportional(theme::BODY);
    let galley = (!label.is_empty()).then(|| ui.painter().layout_no_wrap(label.to_string(), font, p.text));
    let text_w = galley.as_ref().map_or(0.0, |g| g.size().x + 8.0);
    let h = ui.spacing().interact_size.y.max(24.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(16.0 + 16.0 + text_w, h), Sense::click());
    if ui.is_rect_visible(rect) {
        let t = crate::motion::fade(ui.ctx(), resp.id, resp.hovered());
        if resp.is_pointer_button_down_on() {
            ui.painter().rect_filled(rect, 4.0, theme::mix(p.hover, p.text, 0.08));
        } else if on {
            let base = theme::mix(p.chrome, p.hover, 0.7);
            let fill = if t > 0.0 { crate::motion::lerp_color(base, p.hover, t) } else { base };
            ui.painter().rect_filled(rect, 4.0, fill);
        } else if t > 0.0 {
            ui.painter().rect_filled(rect, 4.0, crate::motion::lerp_color(egui::Color32::TRANSPARENT, p.hover, t));
        }
        let icon_rect =
            egui::Rect::from_min_size(egui::pos2(rect.left() + 8.0, rect.center().y - 8.0), vec2(16.0, 16.0));
        icon.paint(ui.painter(), icon_rect, p.text);
        if let Some(g) = galley {
            ui.painter().galley(egui::pos2(icon_rect.right() + 8.0, rect.center().y - g.size().y / 2.0), g, p.text);
        }
    }
    if tooltip.is_empty() { resp } else { resp.on_hover_text(tooltip) }
}

/// A thin vertical line between groups of command-bar buttons.
pub fn command_separator(ui: &mut Ui) {
    let p = Palette::of(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, 20.0), Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), egui::Stroke::new(1.0, p.divider));
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
    egui::Frame::central_panel(style).fill(style.visuals.extreme_bg_color)
}
