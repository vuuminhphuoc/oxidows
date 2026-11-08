//! The system UI font for egui. egui's bundled fonts lack Vietnamese stacked diacritics
//! (ồ, ị, ả, ...), so names such as "Sơ Đồ Kịch Bản.pdf" drew as boxes. Segoe UI, the font
//! Windows' own apps use, covers Latin, Vietnamese, Greek and Cyrillic.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
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
    let Some(dir) = std::env::var_os("WINDIR") else { return };
    for (bit, file, name) in [(HAN, "msyh.ttc", "Microsoft YaHei"), (HANGUL, "malgun.ttf", "Malgun Gothic")] {
        if need & bit == 0 {
            continue;
        }
        let Ok(bytes) = std::fs::read(std::path::Path::new(&dir).join("Fonts").join(file)) else { continue };
        ctx.add_font(FontInsert::new(
            name,
            FontData::from_owned(bytes),
            [FontFamily::Proportional, FontFamily::Monospace]
                .into_iter()
                .map(|family| InsertFontFamily { family, priority: FontPriority::Lowest })
                .collect(),
        ));
    }
}
