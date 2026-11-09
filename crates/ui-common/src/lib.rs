//! UI shared by the oxidows egui apps, so they look and behave as one family: the Windows 11
//! theme (colors, accent, corners, spacing, fonts), in-app icons, common widgets, app icons and
//! the renderer choice, and the About section with update checking.

pub mod default_app;
mod fonts;
mod icons;
mod menu;
mod renderer;
mod sort;
pub mod theme;
pub mod update;
mod widgets;

pub use fonts::{ICONS, SEMIBOLD, ensure_glyphs, has_icon_font};
pub use icons::{ICON_SIZE, Icon};
pub use menu::{MENU_ROW_HEIGHT, MenuIcon, MenuItem, ViewIcon, menu_check, menu_heading, menu_item};
pub use renderer::{Renderer, choose as choose_renderer, relaunch_with};
pub use sort::{SORT_HEADER_HEIGHT, SORT_HEADER_TOTAL_HEIGHT, SortBy, cmp_text, sort_header, sort_header_total};
pub use theme::{Palette, Theme, setup};
pub use update::{AppInfo, Updater};
pub use widgets::*;

/// The apps' own icons (256 px PNG) for the window and taskbar; the exe icons are the matching
/// `.ico` files in `assets/`, embedded by each app's build script.
pub mod app_icon {
    pub const TASKMGR_PNG: &[u8] = include_bytes!("../assets/taskmgr.png");
    pub const EXPLORER_PNG: &[u8] = include_bytes!("../assets/explorer.png");
}
