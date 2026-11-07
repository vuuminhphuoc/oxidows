//! UI shared by the oxidows egui apps, so they look and behave as one family: the Windows 11
//! theme (colors, accent, corners, spacing, fonts), in-app icons, common widgets, app icons and
//! the renderer choice.

mod fonts;
mod icons;
mod renderer;
pub mod theme;
mod widgets;

pub use fonts::{ICONS, SEMIBOLD, has_icon_font};
pub use icons::{ICON_SIZE, Icon};
pub use renderer::{Renderer, choose as choose_renderer, relaunch_with};
pub use theme::{Palette, Theme, setup};
pub use widgets::*;

/// The apps' own icons (256 px PNG) for the window and taskbar; the exe icons are the matching
/// `.ico` files in `assets/`, embedded by each app's build script.
pub mod app_icon {
    pub const TASKMGR_PNG: &[u8] = include_bytes!("../assets/taskmgr.png");
    pub const EXPLORER_PNG: &[u8] = include_bytes!("../assets/explorer.png");
}
