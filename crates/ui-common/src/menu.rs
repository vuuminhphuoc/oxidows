//! Menu rows as Windows 11 draws them: an icon, the label, and the shortcut keys right-aligned
//! in the same row (not in a tooltip), a dot before the chosen option of a group ("Details" in
//! Explorer's View menu) or a check mark before a setting that is on ("Hidden items"), and a
//! chevron on a row that opens a submenu.
//!
//! [`ViewIcon`] draws the pictures of Explorer's layouts (Extra large icons ... Content, the
//! Preview and Details panes), which the icon font has no glyphs for, so each row shows what
//! that layout looks like.

use egui::containers::menu::SubMenu;
use egui::{Color32, CornerRadius, FontId, InnerResponse, Painter, Pos2, Rect, Response, Sense, Stroke, Ui};
use egui::{pos2, vec2};

use crate::{Icon, theme};

/// Height of one menu row.
pub const MENU_ROW_HEIGHT: f32 = 28.0;
/// Where the dot or check mark is centered, from the row's left edge.
const MARK_X: f32 = 10.0;
/// Where the 16 px icon starts.
const ICON_X: f32 = 22.0;
const ICON_SIZE: f32 = 16.0;
/// Where the label starts: icon plus Windows 11's 12 px gap.
const LABEL_X: f32 = ICON_X + ICON_SIZE + 12.0;
const RIGHT_PAD: f32 = 12.0;
/// Least space between the label and the shortcut keys.
const SHORTCUT_GAP: f32 = 32.0;

/// The picture at the start of a menu row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MenuIcon {
    #[default]
    None,
    Glyph(Icon),
    View(ViewIcon),
}

impl From<Icon> for MenuIcon {
    fn from(i: Icon) -> Self {
        MenuIcon::Glyph(i)
    }
}

impl From<ViewIcon> for MenuIcon {
    fn from(i: ViewIcon) -> Self {
        MenuIcon::View(i)
    }
}

impl From<Option<Icon>> for MenuIcon {
    fn from(i: Option<Icon>) -> Self {
        i.map_or(MenuIcon::None, MenuIcon::Glyph)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mark {
    #[default]
    None,
    /// The chosen option of a group.
    Dot,
    /// A setting that is on.
    Check,
}

/// One row of a menu. Build it, then [`MenuItem::ui`] for a command or [`MenuItem::submenu`]
/// for a row that opens more rows.
#[must_use]
pub struct MenuItem<'a> {
    label: &'a str,
    icon: MenuIcon,
    shortcut: Option<&'a str>,
    mark: Mark,
    enabled: bool,
}

impl<'a> MenuItem<'a> {
    pub fn new(label: &'a str) -> Self {
        MenuItem { label, icon: MenuIcon::None, shortcut: None, mark: Mark::None, enabled: true }
    }

    pub fn icon(mut self, icon: impl Into<MenuIcon>) -> Self {
        self.icon = icon.into();
        self
    }

    /// Keys drawn right-aligned in the row, e.g. "Ctrl+Shift+6".
    pub fn shortcut(mut self, keys: &'a str) -> Self {
        self.shortcut = Some(keys);
        self
    }

    /// Marks the row as the chosen option of a group (a dot before the icon).
    pub fn selected(mut self, on: bool) -> Self {
        self.mark = if on { Mark::Dot } else { Mark::None };
        self
    }

    /// Marks the row as a setting that is on (a check mark before the icon).
    pub fn checked(mut self, on: bool) -> Self {
        self.mark = if on { Mark::Check } else { Mark::None };
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Draws the row; `clicked()` on the response runs the command.
    pub fn ui(self, ui: &mut Ui) -> Response {
        self.draw(ui, false)
    }

    /// Draws the row with a chevron, and `content` in a submenu while it is hovered.
    pub fn submenu<R>(self, ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> (Response, Option<InnerResponse<R>>) {
        let enabled = self.enabled;
        let r = self.draw(ui, true);
        let inner = if enabled { SubMenu::new().show(ui, &r, content) } else { None };
        (r, inner)
    }

    fn draw(self, ui: &mut Ui, submenu: bool) -> Response {
        let body = FontId::proportional(theme::BODY);
        let p = crate::Palette::of(ui);
        let text_color = if self.enabled { p.text } else { p.text_secondary };
        let label = ui.painter().layout_no_wrap(self.label.to_string(), body.clone(), text_color);
        let keys = self.shortcut.map(|k| ui.painter().layout_no_wrap(k.to_string(), body.clone(), p.text_secondary));
        let mut width = LABEL_X + label.size().x + RIGHT_PAD;
        if let Some(k) = &keys {
            width += SHORTCUT_GAP + k.size().x;
        }
        if submenu {
            width += SHORTCUT_GAP;
        }
        let sense = if self.enabled { Sense::click() } else { Sense::hover() };
        // In a menu (a justified layout) the row takes the menu's full width.
        let resp = ui.allocate_response(vec2(width, MENU_ROW_HEIGHT), sense);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, self.enabled, self.label));
        let rect = resp.rect;
        if !ui.is_rect_visible(rect) {
            return resp;
        }
        let painter = ui.painter();
        if self.enabled && (resp.hovered() || resp.has_focus()) {
            painter.rect_filled(rect, CornerRadius::same(4), ui.visuals().widgets.hovered.weak_bg_fill);
        }
        let cy = rect.center().y;
        match self.mark {
            Mark::None => {}
            Mark::Dot => {
                painter.circle_filled(pos2(rect.left() + MARK_X, cy), 2.5, text_color);
            }
            Mark::Check => {
                let g = painter.layout_no_wrap(Icon::CheckMark.glyph(), Icon::font(12.0), text_color);
                painter.galley(pos2(rect.left() + MARK_X - g.size().x / 2.0, cy - g.size().y / 2.0), g, text_color);
            }
        }
        let icon_rect =
            Rect::from_min_size(pos2(rect.left() + ICON_X, cy - ICON_SIZE / 2.0), vec2(ICON_SIZE, ICON_SIZE));
        match self.icon {
            MenuIcon::None => {}
            MenuIcon::Glyph(i) => {
                let g = painter.layout_no_wrap(i.glyph(), Icon::font(ICON_SIZE), text_color);
                let at = icon_rect.center() - g.size() / 2.0;
                painter.galley(at, g, text_color);
            }
            MenuIcon::View(v) => v.paint(painter, icon_rect, text_color),
        }
        painter.galley(pos2(rect.left() + LABEL_X, cy - label.size().y / 2.0), label, text_color);
        let mut right = rect.right() - RIGHT_PAD;
        if submenu {
            let g = painter.layout_no_wrap(Icon::ChevronRight.glyph(), Icon::font(10.0), text_color);
            let w = g.size().x;
            painter.galley(pos2(right - w, cy - g.size().y / 2.0), g, text_color);
            right -= w + 12.0;
        }
        if let Some(k) = keys {
            painter.galley(pos2(right - k.size().x, cy - k.size().y / 2.0), k, p.text_secondary);
        }
        resp
    }
}

/// A command row: icon, label and (optionally) shortcut keys.
pub fn menu_item(ui: &mut Ui, icon: impl Into<MenuIcon>, label: &str, shortcut: Option<&str>) -> Response {
    let mut item = MenuItem::new(label).icon(icon);
    if let Some(k) = shortcut {
        item = item.shortcut(k);
    }
    item.ui(ui)
}

/// A setting that is on or off; clicking the row flips `on`.
pub fn menu_check(ui: &mut Ui, icon: impl Into<MenuIcon>, label: &str, on: &mut bool) -> Response {
    let mut r = MenuItem::new(label).icon(icon).checked(*on).ui(ui);
    if r.clicked() {
        *on = !*on;
        r.mark_changed();
    }
    r
}

/// A small heading above a group of rows ("View").
pub fn menu_heading(ui: &mut Ui, text: &str) -> Response {
    let c = crate::Palette::of(ui).text_secondary;
    ui.horizontal(|ui| {
        ui.add_space(ICON_X - ui.spacing().item_spacing.x);
        ui.add(egui::Label::new(egui::RichText::new(text).color(c).size(12.0)).selectable(false))
    })
    .inner
}

/// Pictures of Explorer's layouts and panes, drawn in a 16 px square in the icon font's thin
/// line style. Each one shows how items are laid out in that view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ViewIcon {
    /// One big picture.
    ExtraLarge,
    /// Two by two pictures.
    Large,
    /// Three by three small pictures.
    Medium,
    /// Small pictures with their names beside them, in two columns.
    Small,
    /// One column of small pictures and names.
    List,
    /// Rows with columns of details.
    Details,
    /// Medium pictures with two lines of text beside them.
    Tiles,
    /// Rows with a picture, two lines of text and a divider between items.
    Content,
    /// Rows pushed together.
    Compact,
    /// A window with the preview pane on the right.
    PreviewPane,
    /// A window with the details pane on the right.
    DetailsPane,
}

impl ViewIcon {
    pub const ALL: [ViewIcon; 11] = [
        ViewIcon::ExtraLarge,
        ViewIcon::Large,
        ViewIcon::Medium,
        ViewIcon::Small,
        ViewIcon::List,
        ViewIcon::Details,
        ViewIcon::Tiles,
        ViewIcon::Content,
        ViewIcon::Compact,
        ViewIcon::PreviewPane,
        ViewIcon::DetailsPane,
    ];

    /// Draws the picture into `rect` (meant for 16 × 16 points, scales with it).
    pub fn paint(self, painter: &Painter, rect: Rect, color: Color32) {
        let s = rect.width() / 16.0;
        let at = |x: f32, y: f32| -> Pos2 { rect.min + vec2(x, y) * s };
        let stroke = Stroke::new((1.0 * s).max(1.0), color);
        // An outlined box (a picture), its outline centered on the given edges.
        let boxed = |x: f32, y: f32, w: f32, h: f32, r: f32| {
            let b = Rect::from_min_max(at(x, y), at(x + w, y + h));
            painter.rect_stroke(b, CornerRadius::same((r * s).round() as u8), stroke, egui::StrokeKind::Middle);
        };
        // A filled square (a small picture): too small to read as an outline.
        let dot = |x: f32, y: f32, w: f32| {
            painter.rect_filled(Rect::from_min_size(at(x, y), vec2(w, w) * s), CornerRadius::same(1), color);
        };
        // A line of text, centered on `y`.
        let line = |x: f32, y: f32, w: f32| {
            let r = Rect::from_center_size(at(x + w / 2.0, y), vec2(w * s, stroke.width));
            painter.rect_filled(r, CornerRadius::ZERO, color);
        };
        match self {
            ViewIcon::ExtraLarge => {
                boxed(1.5, 1.5, 13.0, 13.0, 2.0);
                // A landscape in it, so it reads as a picture.
                painter.line(vec![at(4.0, 11.5), at(7.0, 8.0), at(9.0, 10.0), at(10.5, 8.5), at(12.5, 11.0)], stroke);
                painter.circle_filled(at(10.5, 5.0), 1.2 * s, color);
            }
            ViewIcon::Large => {
                for (x, y) in [(1.5, 1.5), (9.0, 1.5), (1.5, 9.0), (9.0, 9.0)] {
                    boxed(x, y, 5.5, 5.5, 1.0);
                }
            }
            ViewIcon::Medium => {
                for y in [1.5, 6.5, 11.5] {
                    for x in [1.5, 6.5, 11.5] {
                        dot(x, y, 3.0);
                    }
                }
            }
            ViewIcon::Small => {
                for y in [2.0, 6.5, 11.0] {
                    for x in [1.0, 9.0] {
                        dot(x, y, 3.0);
                        line(x + 4.0, y + 1.5, 2.5);
                    }
                }
            }
            ViewIcon::List => {
                for y in [2.0, 6.5, 11.0] {
                    dot(1.0, y, 3.0);
                    line(5.5, y + 1.5, 9.5);
                }
            }
            ViewIcon::Details => {
                for y in [2.0, 6.5, 11.0] {
                    dot(1.0, y, 3.0);
                    line(5.5, y + 1.5, 4.5);
                    line(11.5, y + 1.5, 3.5);
                }
            }
            ViewIcon::Tiles => {
                for y in [1.5, 9.0] {
                    boxed(1.5, y, 5.5, 5.5, 1.0);
                    line(9.0, y + 1.5, 6.0);
                    line(9.0, y + 4.0, 4.0);
                }
            }
            ViewIcon::Content => {
                for y in [1.5, 9.5] {
                    boxed(1.5, y, 4.5, 4.5, 0.5);
                    line(8.0, y + 1.0, 6.5);
                    line(8.0, y + 3.5, 4.0);
                }
                line(1.0, 8.0, 14.0);
            }
            ViewIcon::Compact => {
                line(2.0, 6.5, 12.0);
                line(2.0, 9.5, 12.0);
                // Arrows pressing the rows together.
                painter.line(vec![at(5.5, 1.5), at(8.0, 4.0), at(10.5, 1.5)], stroke);
                painter.line(vec![at(5.5, 14.5), at(8.0, 12.0), at(10.5, 14.5)], stroke);
            }
            ViewIcon::PreviewPane | ViewIcon::DetailsPane => {
                boxed(1.5, 2.5, 13.0, 11.0, 2.0);
                painter.line_segment([at(9.5, 2.5), at(9.5, 13.5)], stroke);
                if self == ViewIcon::PreviewPane {
                    let r = Rect::from_min_max(at(11.0, 5.0), at(13.0, 11.0));
                    painter.rect_filled(r, CornerRadius::same(1), color);
                } else {
                    for y in [5.5, 8.0, 10.5] {
                        line(11.0, y, 2.0);
                    }
                }
            }
        }
    }
}

/// Context menu for column headers: toggle columns on/off, keeping an optional fixed column always on.
pub fn column_chooser_menu<T: Copy + PartialEq>(
    ui: &mut Ui,
    all: &[T],
    columns: &mut Vec<T>,
    fixed: Option<(&str, T)>,
    title: impl Fn(T) -> &'static str,
) {
    if let Some((name, fixed_col)) = fixed {
        MenuItem::new(name).checked(true).enabled(false).ui(ui);
        for &c in all {
            if c == fixed_col {
                continue;
            }
            let mut on = columns.contains(&c);
            if menu_check(ui, None, title(c), &mut on).clicked() {
                if on {
                    if !columns.contains(&c) {
                        columns.push(c);
                        columns.sort_by_key(|x| all.iter().position(|a| a == x).unwrap_or(0));
                    }
                } else {
                    columns.retain(|x| *x != c);
                }
            }
        }
    } else {
        for &c in all {
            let mut on = columns.contains(&c);
            if menu_check(ui, None, title(c), &mut on).clicked() {
                if on {
                    if !columns.contains(&c) {
                        columns.push(c);
                        columns.sort_by_key(|x| all.iter().position(|a| a == x).unwrap_or(0));
                    }
                } else {
                    columns.retain(|x| *x != c);
                }
            }
        }
    }
}
