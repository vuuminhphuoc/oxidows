//! The system UI fonts for egui. egui's bundled fonts lack Vietnamese stacked diacritics
//! (ồ, ị, ả, ...), so names such as "Sơ Đồ Kịch Bản.pdf" drew as boxes. Segoe UI, the font
//! Windows' own apps use, covers Latin, Vietnamese, Greek and Cyrillic.
//!
//! Icons come from the icon font Windows ships for its own apps: Segoe Fluent Icons (Windows 11)
//! or Segoe MDL2 Assets (Windows 10). Both are read from the system at run time and never
//! bundled; where neither exists, [`crate::Icon`] falls back to egui's emoji font.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::{FontData, FontDefinitions, FontFamily};

/// The family [`crate::Icon`] glyphs are drawn with.
pub const ICONS: &str = "oxi-icons";
/// Segoe UI Semibold, for headings; the proportional font when it is missing.
pub const SEMIBOLD: &str = "oxi-semibold";

static FLUENT: AtomicBool = AtomicBool::new(false);

/// Whether a Segoe icon font was found, so [`crate::Icon`] draws its glyphs.
pub fn has_icon_font() -> bool {
    FLUENT.load(Ordering::Relaxed)
}

/// `%WINDIR%\Fonts`. `OXI_FONTS_DIR` overrides it, for screenshots on other systems.
pub(crate) fn fonts_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("OXI_FONTS_DIR") {
        return Some(PathBuf::from(d));
    }
    std::env::var_os("WINDIR").map(|w| Path::new(&w).join("Fonts"))
}

/// Puts Segoe UI in front of egui's fonts and registers the icon and heading families; egui's
/// fonts stay as fallbacks for emoji. Without the Segoe files the families still exist, backed
/// by egui's own fonts, so text styles that name them keep working.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions(fonts_dir().as_deref()));
}

fn definitions(dir: Option<&Path>) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let defaults = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut load = |file: &str, name: &str| -> bool {
        let Some(bytes) = dir.and_then(|d| std::fs::read(d.join(file)).ok()) else { return false };
        fonts.font_data.insert(name.to_string(), Arc::new(FontData::from_owned(bytes)));
        true
    };
    let ui = load("segoeui.ttf", "Segoe UI");
    let semibold = load("seguisb.ttf", "Segoe UI Semibold");
    // Windows 11 has both icon fonts; Fluent first, MDL2 fills in what it lacks.
    let fluent = load("SegoeIcons.ttf", "Segoe Fluent Icons");
    let mdl2 = load("segmdl2.ttf", "Segoe MDL2 Assets");
    FLUENT.store(fluent || mdl2, Ordering::Relaxed);

    let fam = |f: &str| FontFamily::Name(f.into());
    if ui {
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "Segoe UI".into());
        // Monospace keeps its own font; Segoe UI only fills in glyphs it lacks.
        fonts.families.entry(FontFamily::Monospace).or_default().push("Segoe UI".into());
    }
    let mut heading = Vec::new();
    if semibold {
        heading.push("Segoe UI Semibold".to_string());
    }
    heading.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(fam(SEMIBOLD), heading);
    let mut icons: Vec<String> = Vec::new();
    if fluent {
        icons.push("Segoe Fluent Icons".into());
    }
    if mdl2 {
        icons.push("Segoe MDL2 Assets".into());
    }
    icons.extend(defaults);
    fonts.families.insert(fam(ICONS), icons);
    fonts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_exist_without_system_fonts() {
        let f = definitions(None);
        assert!(f.families.contains_key(&FontFamily::Name(ICONS.into())));
        assert!(f.families.contains_key(&FontFamily::Name(SEMIBOLD.into())));
        assert!(!f.families[&FontFamily::Name(ICONS.into())].is_empty());
    }
}
