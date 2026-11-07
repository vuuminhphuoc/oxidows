//! The system UI font for egui. egui's bundled fonts lack Vietnamese stacked diacritics
//! (ồ, ị, ả, ...), so names such as "Sơ Đồ Kịch Bản.pdf" drew as boxes. Segoe UI, the font
//! Windows' own apps use, covers Latin, Vietnamese, Greek and Cyrillic.

use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily};

/// Puts Segoe UI in front of egui's fonts; egui's stay as fallbacks for emoji and icons.
/// Does nothing when the font file is missing (Windows N/Server Core, other systems).
pub fn install(ctx: &egui::Context) {
    if let Some(fonts) = definitions() {
        ctx.set_fonts(fonts);
    }
}

fn definitions() -> Option<FontDefinitions> {
    let dir = std::env::var_os("WINDIR")?;
    let bytes = std::fs::read(std::path::Path::new(&dir).join("Fonts").join("segoeui.ttf")).ok()?;
    let mut fonts = FontDefinitions::default();
    let name = "Segoe UI".to_string();
    fonts.font_data.insert(name.clone(), Arc::new(FontData::from_owned(bytes)));
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, name.clone());
    // Monospace keeps its own font; Segoe UI only fills in glyphs it lacks.
    fonts.families.entry(FontFamily::Monospace).or_default().push(name);
    Some(fonts)
}
