//! UI plumbing shared by the oxidows egui apps.

mod fonts;
mod renderer;

pub use fonts::install as install_fonts;
pub use renderer::{Renderer, choose as choose_renderer, relaunch_with};
