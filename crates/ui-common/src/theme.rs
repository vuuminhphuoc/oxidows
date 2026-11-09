//! One look for every oxidows app, after Windows 11's own apps: Mica-grey backgrounds, the
//! user's accent color, 4 px corners, Segoe UI at 14 px, and the same palette for light and dark.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, Style, TextStyle, Visuals, vec2};

use crate::fonts;

/// The light/dark choice every app's settings offer.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    pub const ALL: [(Theme, &'static str); 3] =
        [(Theme::System, "Use system setting"), (Theme::Light, "Light"), (Theme::Dark, "Dark")];

    /// The value saved in settings files.
    pub fn key(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    pub fn parse(v: &str) -> Self {
        match v.trim() {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => Theme::System,
        }
    }
}

/// Body text size. Windows 11 uses 14 px Segoe UI; tables stay readable a little smaller.
pub const BODY: f32 = 13.5;

/// Colors the apps draw with beyond egui's widgets, for both themes.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// The user's accent color, tuned for the theme (lighter on dark).
    pub accent: Color32,
    /// Text drawn on an `accent` fill.
    pub on_accent: Color32,
    /// The main content area: file lists, tables.
    pub content: Color32,
    /// The window background behind navigation, toolbars and status bars.
    pub chrome: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    /// Thin borders and separators.
    pub divider: Color32,
    /// Hover and selection fills for navigation and list items.
    pub hover: Color32,
    pub selected: Color32,
    pub folder: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub warning: Color32,
}

impl Palette {
    pub fn get(dark: bool) -> Self {
        let accent = accent_color();
        if dark {
            Palette {
                accent: lighten(accent, 0.45),
                on_accent: Color32::from_rgb(0, 0, 0),
                content: Color32::from_rgb(0x1c, 0x1c, 0x1c),
                chrome: Color32::from_rgb(0x20, 0x20, 0x20),
                text: Color32::from_rgb(0xff, 0xff, 0xff),
                text_secondary: Color32::from_rgb(0xc5, 0xc5, 0xc5),
                divider: Color32::from_rgb(0x33, 0x33, 0x33),
                hover: Color32::from_rgb(0x2d, 0x2d, 0x2d),
                selected: mix(Color32::from_rgb(0x2d, 0x2d, 0x2d), accent, 0.28),
                folder: Color32::from_rgb(0xf8, 0xc8, 0x4a),
                danger: Color32::from_rgb(0xff, 0x99, 0xa4),
                success: Color32::from_rgb(0x6c, 0xcb, 0x5f),
                warning: Color32::from_rgb(0xfc, 0xe1, 0x00),
            }
        } else {
            Palette {
                accent: darken(accent, 0.15),
                on_accent: Color32::WHITE,
                content: Color32::from_rgb(0xff, 0xff, 0xff),
                chrome: Color32::from_rgb(0xf3, 0xf3, 0xf3),
                text: Color32::from_rgb(0x1a, 0x1a, 0x1a),
                text_secondary: Color32::from_rgb(0x5d, 0x5d, 0x5d),
                divider: Color32::from_rgb(0xe0, 0xe0, 0xe0),
                hover: Color32::from_rgb(0xe9, 0xe9, 0xe9),
                selected: mix(Color32::WHITE, accent, 0.22),
                folder: Color32::from_rgb(0xe8, 0xa2, 0x02),
                danger: Color32::from_rgb(0xc4, 0x2b, 0x1c),
                success: Color32::from_rgb(0x0f, 0x7b, 0x0f),
                warning: Color32::from_rgb(0x9d, 0x5d, 0x00),
            }
        }
    }

    /// The palette of the theme `ui` is drawn in.
    pub fn of(ui: &egui::Ui) -> Self {
        Self::get(ui.visuals().dark_mode)
    }
}

/// Fonts, both themes' styles and the chosen theme: the one call an app makes at start-up.
pub fn setup(ctx: &egui::Context, theme: Theme) {
    fonts::install(ctx);
    ctx.set_style_of(egui::Theme::Light, style(false));
    ctx.set_style_of(egui::Theme::Dark, style(true));
    apply(ctx, theme);
}

/// Switches theme; egui keeps the window's title bar in step.
pub fn apply(ctx: &egui::Context, theme: Theme) {
    ctx.set_theme(match theme {
        Theme::System => egui::ThemePreference::System,
        Theme::Light => egui::ThemePreference::Light,
        Theme::Dark => egui::ThemePreference::Dark,
    });
}

pub fn style(dark: bool) -> Style {
    let p = Palette::get(dark);
    let mut s = Style { visuals: visuals(&p, dark), ..Style::default() };
    let semibold = FontFamily::Name(fonts::SEMIBOLD.into());
    s.text_styles = [
        (TextStyle::Small, FontId::proportional(11.5)),
        (TextStyle::Body, FontId::proportional(BODY)),
        (TextStyle::Button, FontId::proportional(BODY)),
        (TextStyle::Monospace, FontId::monospace(12.5)),
        (TextStyle::Heading, FontId::new(20.0, semibold)),
    ]
    .into();
    let sp = &mut s.spacing;
    sp.item_spacing = vec2(8.0, 4.0);
    sp.button_padding = vec2(10.0, 4.0);
    sp.interact_size = vec2(36.0, 24.0);
    sp.menu_margin = Margin::same(6);
    sp.window_margin = Margin::same(12);
    sp.combo_width = 80.0;
    sp.icon_width = 16.0;
    sp.icon_width_inner = 10.0;
    sp.icon_spacing = 6.0;
    s
}

fn visuals(p: &Palette, dark: bool) -> Visuals {
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    let r = CornerRadius::same(4);
    v.panel_fill = p.chrome;
    v.window_fill = if dark { Color32::from_rgb(0x2c, 0x2c, 0x2c) } else { Color32::from_rgb(0xf9, 0xf9, 0xf9) };
    v.window_stroke = Stroke::new(1.0, p.divider);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(8);
    v.extreme_bg_color = p.content;
    v.text_edit_bg_color =
        Some(if dark { Color32::from_rgb(0x2d, 0x2d, 0x2d) } else { Color32::from_rgb(0xff, 0xff, 0xff) });
    v.faint_bg_color = if dark { Color32::from_rgb(0x23, 0x23, 0x23) } else { Color32::from_rgb(0xf7, 0xf7, 0xf7) };
    v.code_bg_color = v.faint_bg_color;
    v.hyperlink_color = p.accent;
    v.error_fg_color = p.danger;
    v.warn_fg_color = p.warning;
    v.selection.bg_fill = p.selected;
    v.selection.stroke = Stroke::new(1.0, p.text);
    // Windows 11 lists and tables have no zebra stripes: hover and selection carry the rows.
    v.striped = false;

    let button = if dark { Color32::from_rgb(0x2d, 0x2d, 0x2d) } else { Color32::from_rgb(0xfb, 0xfb, 0xfb) };
    let border = if dark { Color32::from_rgb(0x3a, 0x3a, 0x3a) } else { Color32::from_rgb(0xdc, 0xdc, 0xdc) };
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.chrome;
    w.noninteractive.weak_bg_fill = p.chrome;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.divider);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.bg_fill = button;
    w.inactive.weak_bg_fill = button;
    w.inactive.bg_stroke = Stroke::new(1.0, border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.bg_fill = p.hover;
    w.hovered.weak_bg_fill = p.hover;
    w.hovered.bg_stroke = Stroke::new(1.0, border);
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.expansion = 0.0;
    w.active.bg_fill = p.selected;
    w.active.weak_bg_fill = p.selected;
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.0, p.text);
    w.active.expansion = 0.0;
    w.open.bg_fill = p.hover;
    w.open.weak_bg_fill = p.hover;
    w.open.bg_stroke = Stroke::new(1.0, border);
    w.open.fg_stroke = Stroke::new(1.0, p.text);
    for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        wv.corner_radius = r;
    }
    v
}

/// Windows' accent color (Settings > Personalization > Colors); Windows 11's default blue
/// elsewhere. `OXI_ACCENT=#rrggbb` overrides it.
pub fn accent_color() -> Color32 {
    static ACCENT: std::sync::OnceLock<Color32> = std::sync::OnceLock::new();
    *ACCENT.get_or_init(|| {
        std::env::var("OXI_ACCENT")
            .ok()
            .and_then(|v| parse_hex(&v))
            .or_else(system_accent)
            .unwrap_or(Color32::from_rgb(0x00, 0x78, 0xd4))
    })
}

fn parse_hex(v: &str) -> Option<Color32> {
    let v = v.trim().trim_start_matches('#');
    if v.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(v, 16).ok()?;
    Some(Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

/// `HKCU\Software\Microsoft\Windows\DWM\AccentColor`, a DWORD laid out 0xAABBGGRR.
#[cfg(windows)]
fn system_accent() -> Option<Color32> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (key, name) = (wide("Software\\Microsoft\\Windows\\DWM"), wide("AccentColor"));
    let mut value: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: NUL-terminated strings; `value` and `size` are valid for writes of a DWORD.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut value).cast(),
            &mut size,
        )
    };
    (rc == 0).then(|| Color32::from_rgb(value as u8, (value >> 8) as u8, (value >> 16) as u8))
}

#[cfg(not(windows))]
fn system_accent() -> Option<Color32> {
    None
}

/// `a` moved towards `b` by `t` (0..=1).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

fn lighten(c: Color32, t: f32) -> Color32 {
    mix(c, Color32::WHITE, t)
}

fn darken(c: Color32, t: f32) -> Color32 {
    mix(c, Color32::BLACK, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_round_trip() {
        for (t, _) in Theme::ALL {
            assert_eq!(Theme::parse(t.key()), t);
        }
        assert_eq!(Theme::parse("bogus"), Theme::System);
    }

    #[test]
    fn hex() {
        assert_eq!(parse_hex("#0078d4"), Some(Color32::from_rgb(0, 0x78, 0xd4)));
        assert_eq!(parse_hex("12345"), None);
    }

    #[test]
    fn text_is_readable_on_both_backgrounds() {
        // WCAG contrast of at least 4.5:1 for body text and 3:1 for the accent.
        fn lum(c: Color32) -> f32 {
            let f = |v: u8| {
                let v = v as f32 / 255.0;
                if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
        }
        let contrast = |a: Color32, b: Color32| {
            let (x, y) = (lum(a), lum(b));
            (x.max(y) + 0.05) / (x.min(y) + 0.05)
        };
        for dark in [false, true] {
            let p = Palette::get(dark);
            for bg in [p.content, p.chrome, p.selected, p.hover] {
                assert!(contrast(p.text, bg) >= 4.5, "dark={dark}");
                assert!(contrast(p.text_secondary, bg) >= 4.5, "dark={dark}");
            }
            assert!(contrast(p.accent, p.chrome) >= 3.0, "dark={dark}");
        }
    }
}
