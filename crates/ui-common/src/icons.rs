//! In-app icons shared by every oxidows app, so the same action or place has the same glyph in
//! each of them. Glyphs come from the system's Segoe icon font (see [`crate::fonts`]); each icon
//! also names an emoji drawn by egui's own font where that font is missing.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId, RichText, WidgetText};

use crate::fonts;

macro_rules! icons {
    ($($name:ident = $cp:literal, $fallback:literal;)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Icon { $($name),* }

        impl Icon {
            pub const ALL: &[Icon] = &[$(Icon::$name),*];

            /// Code point in Segoe Fluent Icons / Segoe MDL2 Assets.
            pub const fn codepoint(self) -> char {
                match self { $(Icon::$name => $cp),* }
            }

            /// Drawn by egui's default fonts when no Segoe icon font is installed.
            pub const fn fallback(self) -> &'static str {
                match self { $(Icon::$name => $fallback),* }
            }
        }
    };
}

icons! {
    // Navigation and commands.
    Menu = '\u{E700}', "☰";
    Back = '\u{E72B}', "⬅";
    Forward = '\u{E72A}', "➡";
    Up = '\u{E74A}', "⬆";
    Refresh = '\u{E72C}', "⟳";
    ChevronDown = '\u{E70D}', "⏷";
    ChevronUp = '\u{E70E}', "⏶";
    ChevronRight = '\u{E76C}', "⏵";
    More = '\u{E712}', "…";
    Add = '\u{E710}', "+";
    Close = '\u{E711}', "×";
    Search = '\u{E721}', "🔍";
    Settings = '\u{E713}', "⚙";
    Pin = '\u{E718}', "📌";
    Delete = '\u{E74D}', "🗑";
    NewWindow = '\u{E8A7}', "🗗";
    Shield = '\u{E7EF}', "🛡";
    Stopwatch = '\u{E916}', "⏱";
    Leaf = '\u{EC0A}', "🍃";
    Info = '\u{E946}', "ℹ";
    Warning = '\u{E7BA}', "⚠";
    Share = '\u{E72D}', "📤";
    Send = '\u{E724}', "➡";
    Message = '\u{E8BD}', "✉";
    Phone = '\u{E8EA}', "📱";
    Wifi = '\u{E701}', "📶";
    CheckMark = '\u{E73E}', "✔";
    NewFolder = '\u{E8F4}', "📁";
    Undo = '\u{E7A7}', "↶";
    Redo = '\u{E7A6}', "↷";
    Restore = '\u{E777}', "↺";
    Paste = '\u{E77F}', "📋";
    SelectAll = '\u{E8B3}', "▣";
    SelectNone = '\u{E8E6}', "□";
    InvertSelection = '\u{E8B4}', "◩";
    Unpin = '\u{E77A}', "📌";
    Terminal = '\u{E756}', "⌨";
    Properties = '\u{E90F}', "🔧";
    GroupBy = '\u{F168}', "☰";
    Theme = '\u{E790}', "🎨";
    Show = '\u{E7B3}', "👁";
    DisconnectDrive = '\u{E8CD}', "⏏";
    Cut = '\u{E8C6}', "✂";
    Copy = '\u{E8C8}', "⧉";
    Rename = '\u{E8AC}', "✎";
    Sort = '\u{E8CB}', "⇅";
    View = '\u{E8A9}', "▦";
    Filter = '\u{E71C}', "⏷";
    OpenPane = '\u{E8A0}', "◨";
    Play = '\u{E768}', "▶";
    Stop = '\u{E71A}', "■";
    // Places and files.
    Home = '\u{E80F}', "🏠";
    Gallery = '\u{E8B9}', "🖼";
    Desktop = '\u{E7F4}', "🖥";
    Downloads = '\u{E896}', "⬇";
    Document = '\u{E8A5}', "📄";
    Pictures = '\u{E91B}', "🖼";
    Music = '\u{E8D6}', "🎵";
    Videos = '\u{E714}', "🎞";
    Folder = '\u{E8B7}', "📁";
    FolderFilled = '\u{E8D5}', "📁";
    ThisPc = '\u{E977}', "💻";
    Drive = '\u{EDA2}', "🖴";
    NetworkDrive = '\u{E8CE}', "🖧";
    Usb = '\u{E88E}', "💾";
    Disc = '\u{E958}', "💿";
    Libraries = '\u{E8F1}', "📚";
    Network = '\u{E968}', "🖧";
    RecycleBin = '\u{E74D}', "🗑";
    Cloud = '\u{E753}', "☁";
    // Task Manager pages.
    Processes = '\u{F0E2}', "▦";
    Performance = '\u{E9D9}', "📈";
    AppHistory = '\u{E81C}', "🕓";
    Startup = '\u{EC4A}', "🚀";
    Users = '\u{E716}', "👥";
    Details = '\u{E8FD}', "☰";
    Services = '\u{EA86}', "🧩";
}

impl Icon {
    /// The glyph to draw, from the icon font when there is one.
    pub fn glyph(self) -> String {
        if fonts::has_icon_font() { self.codepoint().to_string() } else { self.fallback().to_string() }
    }

    pub fn font(size: f32) -> FontId {
        FontId::new(size, FontFamily::Name(fonts::ICONS.into()))
    }

    /// The icon alone, at `size` points.
    pub fn sized(self, size: f32) -> RichText {
        RichText::new(self.glyph()).font(Self::font(size))
    }

    /// The icon alone, at the size of body text.
    pub fn rich(self) -> RichText {
        self.sized(ICON_SIZE)
    }

    /// The icon followed by `text`, as one label: for buttons, menu entries and list rows.
    /// Colors follow the widget that draws it.
    pub fn with(self, text: impl Into<String>) -> WidgetText {
        self.with_sized(text, ICON_SIZE, crate::theme::BODY)
    }

    pub fn with_sized(self, text: impl Into<String>, icon: f32, body: f32) -> WidgetText {
        let mut job = LayoutJob::default();
        let fmt = |font_id| TextFormat {
            font_id,
            color: Color32::PLACEHOLDER,
            valign: egui::Align::Center,
            ..Default::default()
        };
        job.append(&self.glyph(), 0.0, fmt(Self::font(icon)));
        let text = text.into();
        if !text.is_empty() {
            job.append(&text, icon * 0.75, fmt(FontId::proportional(body)));
        }
        WidgetText::LayoutJob(job.into())
    }
}

/// Icons beside body text: Windows 11 draws 16 px glyphs next to 14 px text.
pub const ICON_SIZE: f32 = 15.0;

#[cfg(test)]
mod tests {
    use super::Icon;

    #[test]
    fn codepoints_are_private_use() {
        for i in Icon::ALL {
            let c = i.codepoint() as u32;
            assert!((0xE000..=0xF8FF).contains(&c), "{i:?}");
            assert!(!i.fallback().is_empty());
        }
    }

    /// Every icon is in Windows 11's icon font (checked where the font exists: Windows, or
    /// `OXI_FONTS_DIR`). Segoe MDL2 Assets (Windows 10) is reported, not required.
    #[test]
    fn glyphs_exist_in_the_system_icon_font() {
        use skrifa::MetadataProvider;
        let Some(dir) = crate::fonts::fonts_dir() else { return };
        for file in ["SegoeIcons.ttf", "segmdl2.ttf"] {
            let Ok(bytes) = std::fs::read(dir.join(file)) else {
                eprintln!("{file}: not installed");
                continue;
            };
            let font = skrifa::FontRef::new(&bytes).expect("a TrueType font");
            let cmap = font.charmap();
            let missing: Vec<&Icon> = Icon::ALL.iter().filter(|i| cmap.map(i.codepoint()).is_none()).collect();
            eprintln!("{file}: {} of {} icons, missing {missing:?}", Icon::ALL.len() - missing.len(), Icon::ALL.len());
            if file == "SegoeIcons.ttf" {
                assert!(missing.is_empty(), "missing in {file}: {missing:?}");
            }
        }
    }
}
