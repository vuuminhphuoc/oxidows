//! Remembered settings, saved as `key=value` lines in %APPDATA%\oxidows\explorer.ini
//! (`$XDG_CONFIG_HOME/oxidows/explorer.ini` elsewhere): window size, view options, columns,
//! panes, theme, and each folder's own view and sort once the user changes them there.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use explorer_core::SortKey;

use crate::pane::Pane;
use crate::{App, ViewMode};

/// Folders whose view is remembered; the least recently changed are forgotten first.
const MAX_FOLDERS: usize = 400;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FolderView {
    pub view: ViewMode,
    pub sort: SortKey,
    pub ascending: bool,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Settings {
    pub size: [f32; 2],
    pub maximized: bool,
    pub folder: FolderView,
    pub show_hidden: bool,
    pub show_extensions: bool,
    pub col_created: bool,
    pub col_attrs: bool,
    pub pane: Pane,
    pub theme: Theme,
    /// Lower-cased folder path → (order changed, view).
    pub folders: BTreeMap<String, (u64, FolderView)>,
    /// Folders pinned to Quick access, in order.
    pub pinned: Vec<PathBuf>,
    pub group_by: explorer_core::GroupBy,
    pub show_protected: bool,
    pub item_checkboxes: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            size: [1040.0, 660.0],
            maximized: false,
            folder: FolderView { view: ViewMode::Details, sort: SortKey::Name, ascending: true },
            show_hidden: false,
            show_extensions: true,
            col_created: false,
            col_attrs: false,
            pane: Pane::None,
            theme: Theme::System,
            folders: BTreeMap::new(),
            pinned: Vec::new(),
            group_by: explorer_core::GroupBy::None,
            show_protected: false,
            item_checkboxes: false,
        }
    }
}

fn sort_name(k: SortKey) -> &'static str {
    match k {
        SortKey::Name => "name",
        SortKey::Modified => "modified",
        SortKey::Type => "type",
        SortKey::Size => "size",
        SortKey::Created => "created",
        SortKey::Attributes => "attributes",
    }
}

fn parse_sort(s: &str) -> Option<SortKey> {
    Some(match s {
        "name" => SortKey::Name,
        "modified" => SortKey::Modified,
        "type" => SortKey::Type,
        "size" => SortKey::Size,
        "created" => SortKey::Created,
        "attributes" => SortKey::Attributes,
        _ => return None,
    })
}

impl FolderView {
    /// "icons,size,desc"
    fn to_text(self) -> String {
        let view = self.view.name();
        format!("{view},{},{}", sort_name(self.sort), if self.ascending { "asc" } else { "desc" })
    }

    fn parse(s: &str) -> Option<Self> {
        let mut it = s.split(',');
        let view = ViewMode::parse(it.next()?)?;
        let sort = parse_sort(it.next()?)?;
        let ascending = it.next()? != "desc";
        Some(Self { view, sort, ascending })
    }
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("oxidows").join("explorer.ini"))
}

pub fn folder_key(p: &std::path::Path) -> String {
    p.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase()
}

impl Settings {
    pub fn parse(text: &str) -> Self {
        let mut s = Self::default();
        let flag = |v: &str| v == "true";
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            if let Some(folder) = k.strip_prefix("folder:") {
                // "folder:<path>=<n>,<view>"
                if let Some((n, view)) = v.split_once(',')
                    && let (Ok(n), Some(view)) = (n.parse(), FolderView::parse(view))
                {
                    s.folders.insert(folder.to_string(), (n, view));
                }
                continue;
            }
            match k {
                "width" => s.size[0] = v.parse().unwrap_or(s.size[0]),
                "height" => s.size[1] = v.parse().unwrap_or(s.size[1]),
                "maximized" => s.maximized = flag(v),
                "view" => s.folder = FolderView::parse(v).unwrap_or(s.folder),
                "show_hidden" => s.show_hidden = flag(v),
                "show_extensions" => s.show_extensions = v != "false",
                "col_created" => s.col_created = flag(v),
                "col_attrs" => s.col_attrs = flag(v),
                "pane" => {
                    s.pane = match v {
                        "preview" => Pane::Preview,
                        "details" => Pane::Details,
                        _ => Pane::None,
                    }
                }
                "show_protected" => s.show_protected = flag(v),
                "item_checkboxes" => s.item_checkboxes = flag(v),
                "group_by" => {
                    use explorer_core::GroupBy;
                    s.group_by = match v {
                        "name" => GroupBy::Name,
                        "modified" => GroupBy::Modified,
                        "type" => GroupBy::Type,
                        "size" => GroupBy::Size,
                        _ => GroupBy::None,
                    }
                }
                "pin" if !v.is_empty() => s.pinned.push(PathBuf::from(v)),
                "theme" => {
                    s.theme = match v {
                        "light" => Theme::Light,
                        "dark" => Theme::Dark,
                        _ => Theme::System,
                    }
                }
                _ => {}
            }
        }
        s.size = [s.size[0].clamp(520.0, 8000.0), s.size[1].clamp(320.0, 8000.0)];
        s
    }

    pub fn to_text(&self) -> String {
        let pane = match self.pane {
            Pane::None => "none",
            Pane::Preview => "preview",
            Pane::Details => "details",
        };
        let theme = match self.theme {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        };
        let mut out = format!(
            "width={}\nheight={}\nmaximized={}\nview={}\nshow_hidden={}\nshow_extensions={}\ncol_created={}\n\
             col_attrs={}\npane={pane}\ntheme={theme}\n",
            self.size[0].round(),
            self.size[1].round(),
            self.maximized,
            self.folder.to_text(),
            self.show_hidden,
            self.show_extensions,
            self.col_created,
            self.col_attrs,
        );
        let group = match self.group_by {
            explorer_core::GroupBy::None => "none",
            explorer_core::GroupBy::Name => "name",
            explorer_core::GroupBy::Modified => "modified",
            explorer_core::GroupBy::Type => "type",
            explorer_core::GroupBy::Size => "size",
        };
        out.push_str(&format!(
            "group_by={group}\nshow_protected={}\nitem_checkboxes={}\n",
            self.show_protected, self.item_checkboxes
        ));
        for p in &self.pinned {
            out.push_str(&format!("pin={}\n", p.display()));
        }
        for (k, (n, v)) in &self.folders {
            out.push_str(&format!("folder:{k}={n},{}\n", v.to_text()));
        }
        out
    }

    pub fn load() -> Self {
        path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    fn save(&self) -> std::io::Result<()> {
        let p = path().ok_or_else(|| std::io::Error::other("no settings folder"))?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Write then rename, so a crash never leaves half a file.
        let tmp = p.with_extension("ini.tmp");
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(tmp, p)
    }

    pub fn remember_folder(&mut self, key: String, view: FolderView) {
        let n = self.folders.values().map(|(n, _)| *n).max().unwrap_or(0) + 1;
        self.folders.insert(key, (n, view));
        while self.folders.len() > MAX_FOLDERS {
            let oldest = self.folders.iter().min_by_key(|(_, (n, _))| *n).map(|(k, _)| k.clone());
            match oldest {
                Some(k) => self.folders.remove(&k),
                None => break,
            };
        }
    }

    pub fn apply_theme(&self, ctx: &egui::Context) {
        ctx.set_theme(match self.theme {
            Theme::System => egui::ThemePreference::System,
            Theme::Light => egui::ThemePreference::Light,
            Theme::Dark => egui::ThemePreference::Dark,
        });
    }
}

/// Tracks what the user changes and writes the file a moment after the last change.
pub struct Persist {
    pub settings: Settings,
    saved: Settings,
    changed_at: Option<Instant>,
    /// Folder and view seen last frame: a view change without a folder change is the
    /// user's choice for this folder.
    last: Option<(PathBuf, FolderView)>,
}

impl Persist {
    pub fn new(settings: Settings) -> Self {
        Self { saved: settings.clone(), settings, changed_at: None, last: None }
    }
}

impl App {
    pub fn is_pinned(&self, p: &std::path::Path) -> bool {
        self.persist.settings.pinned.iter().any(|q| folder_key(q) == folder_key(p))
    }

    pub fn pin(&mut self, p: PathBuf) {
        if !self.is_pinned(&p) {
            self.persist.settings.pinned.push(p);
        }
    }

    pub fn unpin(&mut self, p: &std::path::Path) {
        self.persist.settings.pinned.retain(|q| folder_key(q) != folder_key(p));
    }

    fn folder_view(&self) -> FolderView {
        FolderView { view: self.view_mode, sort: self.sort, ascending: self.ascending }
    }

    pub fn apply_settings(&mut self, s: &Settings) {
        self.view_mode = s.folder.view;
        self.sort = s.folder.sort;
        self.ascending = s.folder.ascending;
        self.show_hidden = s.show_hidden;
        self.show_extensions = s.show_extensions;
        self.col_created = s.col_created;
        self.col_attrs = s.col_attrs;
        self.pane = s.pane;
        self.group_by = s.group_by;
        self.show_protected = s.show_protected;
        self.item_checkboxes = s.item_checkboxes;
    }

    /// Once per frame: per-folder views, and saving changes.
    pub fn persist_tick(&mut self, ctx: &egui::Context) {
        let now_view = self.folder_view();
        match self.persist.last.take() {
            Some((dir, before)) if dir == self.cur => {
                if before != now_view {
                    self.persist.settings.remember_folder(folder_key(&self.cur), now_view);
                    self.persist.settings.folder = now_view;
                }
            }
            _ => {
                // Entered a folder: its own view if it has one.
                let v = match self.persist.settings.folders.get(&folder_key(&self.cur)) {
                    Some(&(_, v)) => v,
                    // This PC opens as tiles until the user picks another view there.
                    None if self.cur.as_os_str() == explorer_core::THIS_PC => {
                        FolderView { view: ViewMode::Tiles, ..self.persist.settings.folder }
                    }
                    // Home lists recent files newest first.
                    None if self.cur.as_os_str() == explorer_core::HOME => {
                        FolderView { view: ViewMode::Details, sort: SortKey::Modified, ascending: false }
                    }
                    None => self.persist.settings.folder,
                };
                {
                    self.view_mode = v.view;
                    self.sort = v.sort;
                    self.ascending = v.ascending;
                }
            }
        }
        self.persist.last = Some((self.cur.clone(), self.folder_view()));

        let s = &mut self.persist.settings;
        let (size, maximized) = ctx.input(|i| {
            let vp = i.viewport();
            (vp.inner_rect.map(|r| [r.width(), r.height()]), vp.maximized.unwrap_or(false))
        });
        s.maximized = maximized;
        if let Some(size) = size
            && !maximized
            && size[0] >= 1.0
        {
            s.size = size;
        }
        s.show_hidden = self.show_hidden;
        s.show_extensions = self.show_extensions;
        s.col_created = self.col_created;
        s.col_attrs = self.col_attrs;
        s.pane = self.pane;
        s.group_by = self.group_by;
        s.show_protected = self.show_protected;
        s.item_checkboxes = self.item_checkboxes;

        if self.persist.settings != self.persist.saved {
            let at = *self.persist.changed_at.get_or_insert_with(Instant::now);
            let settle = Duration::from_secs(1);
            if at.elapsed() >= settle {
                self.persist.changed_at = None;
                self.persist.saved = self.persist.settings.clone();
                if let Err(e) = self.persist.saved.save() {
                    self.status = Some(format!("Could not save settings: {e}"));
                }
            } else {
                ctx.request_repaint_after(settle);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut s = Settings {
            size: [1200.0, 700.0],
            maximized: true,
            folder: FolderView { view: ViewMode::Large, sort: SortKey::Size, ascending: false },
            show_hidden: true,
            show_extensions: false,
            col_created: true,
            col_attrs: true,
            pane: Pane::Details,
            theme: Theme::Light,
            folders: BTreeMap::new(),
            pinned: Vec::new(),
            group_by: explorer_core::GroupBy::None,
            show_protected: false,
            item_checkboxes: false,
        };
        s.remember_folder(
            r"c:\users\a\pictures".into(),
            FolderView { view: ViewMode::Large, sort: SortKey::Modified, ascending: false },
        );
        s.group_by = explorer_core::GroupBy::Type;
        s.item_checkboxes = true;
        s.pinned = vec![PathBuf::from(r"D:\Projects"), PathBuf::from(r"\\nas\share")];
        assert_eq!(Settings::parse(&s.to_text()), s);
        assert_eq!(Settings::parse("garbage\nwidth=abc\nview=nope"), Settings::default());
    }

    #[test]
    fn folder_memory_is_bounded() {
        let mut s = Settings::default();
        let v = s.folder;
        for i in 0..MAX_FOLDERS + 10 {
            s.remember_folder(format!("c:\\f{i}"), v);
        }
        assert_eq!(s.folders.len(), MAX_FOLDERS);
        assert!(!s.folders.contains_key("c:\\f0"));
        assert!(s.folders.contains_key(&format!("c:\\f{}", MAX_FOLDERS + 9)));
    }
}
