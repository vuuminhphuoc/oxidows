//! The system UI fonts for egui. egui's bundled fonts lack Vietnamese stacked diacritics
//! (ồ, ị, ả, ...), so names such as "Sơ Đồ Kịch Bản.pdf" drew as boxes. Segoe UI, the font
//! Windows' own apps use, covers Latin, Vietnamese, Greek and Cyrillic.
//!
//! Icons come from the icon font Windows ships for its own apps: Segoe Fluent Icons (Windows 11)
//! or Segoe MDL2 Assets (Windows 10). Both are read from the system at run time and never
//! bundled; where neither exists, [`crate::Icon`] falls back to egui's emoji font.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
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

/// Fallback fonts for scripts neither Segoe UI nor egui's fonts cover, loaded the first time
/// such text is about to be shown: they are large (Microsoft YaHei is ~20 MB), and most
/// sessions never need them. Bit set once a script's font is in.
static LOADED: AtomicU8 = AtomicU8::new(0);
const HAN: u8 = 1;
const HANGUL: u8 = 2;

fn script_of(c: char) -> u8 {
    match c as u32 {
        // Hangul syllables and jamo.
        0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => HANGUL,
        // CJK radicals, kana, ideographs, compatibility and full-width forms.
        0x2E80..=0x2FFF | 0x3000..=0x312F | 0x3190..=0x9FFF | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF => {
            HAN
        }
        0x20000..=0x3FFFF => HAN,
        _ => 0,
    }
}

/// Makes sure `text` can be drawn: adds Microsoft YaHei (Chinese, Japanese) or Malgun Gothic
/// (Korean) as a fallback when `text` needs it. Cheap for text that doesn't ("荣耀Magic8 Pro"
/// otherwise drew as boxes).
pub fn ensure_glyphs(ctx: &egui::Context, text: &str) {
    if text.is_ascii() {
        return;
    }
    let need = text.chars().fold(0, |acc, c| acc | script_of(c)) & !LOADED.load(Ordering::Relaxed);
    if need == 0 {
        return;
    }
    LOADED.fetch_or(need, Ordering::Relaxed);
    let Some(dir) = fonts_dir() else { return };
    for (bit, file, name) in [(HAN, "msyh.ttc", "Microsoft YaHei"), (HANGUL, "malgun.ttf", "Malgun Gothic")] {
        if need & bit == 0 {
            continue;
        }
        let Ok(bytes) = std::fs::read(dir.join(file)) else { continue };
        ctx.add_font(FontInsert::new(
            name,
            FontData::from_owned(bytes),
            // Headings too: the semibold family is its own list, copied from Proportional at start-up.
            [FontFamily::Proportional, FontFamily::Monospace, FontFamily::Name(SEMIBOLD.into())]
                .into_iter()
                .map(|family| InsertFontFamily { family, priority: FontPriority::Lowest })
                .collect(),
        ));
    }
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
