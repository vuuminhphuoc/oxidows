//! Remembered settings, saved as `key=value` lines in %APPDATA%\oxidows\explorer.ini
//! (`$XDG_CONFIG_HOME/oxidows/explorer.ini` elsewhere): window size, view options, columns,
//! panes, theme, and each folder's own view, sort and grouping once the user changes them there.
//!
//! As in Windows, a view belongs to one folder: changing it changes only that folder. A folder
//! never changed takes the default of its kind (Pictures and Videos show large icons, Downloads
//! is grouped by date), or the view the user applied to every folder of that kind (Options ▸
//! Apply to all folders).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use explorer_core::{GroupBy, SortKey};

use crate::pane::Pane;
use crate::{App, ViewMode};

/// Folders whose view is remembered; the least recently changed are forgotten first.
const MAX_FOLDERS: usize = 400;

pub use ui_common::Theme;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FolderView {
    pub view: ViewMode,
    pub sort: SortKey,
    pub ascending: bool,
    pub group: GroupBy,
    pub group_ascending: bool,
}

impl FolderView {
    pub const DETAILS: FolderView = FolderView {
        view: ViewMode::Details,
        sort: SortKey::Name,
        ascending: true,
        group: GroupBy::None,
        group_ascending: true,
    };

    pub const fn with(view: ViewMode) -> Self {
        FolderView { view, ..Self::DETAILS }
    }
}

/// Windows' folder types ("Optimize this folder for"), each with its own default view.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum FolderKind {
    General,
    Documents,
    Pictures,
    Music,
    Videos,
}

impl FolderKind {
    pub const ALL: [FolderKind; 5] =
        [FolderKind::General, FolderKind::Documents, FolderKind::Pictures, FolderKind::Music, FolderKind::Videos];

    pub fn key(self) -> &'static str {
        match self {
            FolderKind::General => "general",
            FolderKind::Documents => "documents",
            FolderKind::Pictures => "pictures",
            FolderKind::Music => "music",
            FolderKind::Videos => "videos",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            FolderKind::General => "General items",
            FolderKind::Documents => "Documents",
            FolderKind::Pictures => "Pictures",
            FolderKind::Music => "Music",
            FolderKind::Videos => "Videos",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.key() == s)
    }

    /// Windows' own default view for this kind of folder.
    pub fn builtin_view(self) -> FolderView {
        match self {
            FolderKind::Pictures | FolderKind::Videos => FolderView::with(ViewMode::Large),
            FolderKind::General | FolderKind::Documents | FolderKind::Music => FolderView::DETAILS,
        }
    }

    /// The kind of the folder `key` (see [`folder_key`]): the kind of the known folder (Pictures,
    /// Music, ...) it is in, else General.
    pub fn of(key: &str, known: &[(&str, PathBuf)]) -> Self {
        let mut best: Option<(usize, FolderKind)> = None;
        for (name, path) in known {
            let kind = match *name {
                "Documents" => FolderKind::Documents,
                "Pictures" => FolderKind::Pictures,
                "Music" => FolderKind::Music,
                "Videos" => FolderKind::Videos,
                _ => continue,
            };
            let k = folder_key(path);
            if is_within(key, &k) && best.is_none_or(|(len, _)| k.len() > len) {
                best = Some((k.len(), kind));
            }
        }
        best.map_or(FolderKind::General, |(_, k)| k)
    }
}

/// `key` is `folder` or inside it (both [`folder_key`]s).
fn is_within(key: &str, folder: &str) -> bool {
    !folder.is_empty() && key.strip_prefix(folder).is_some_and(|rest| rest.is_empty() || rest.starts_with(['\\', '/']))
}

/// Downloads' own default, as in Windows 11: newest first, grouped by date.
pub const DOWNLOADS_VIEW: FolderView = FolderView {
    view: ViewMode::Details,
    sort: SortKey::Modified,
    ascending: false,
    group: GroupBy::Modified,
    group_ascending: false,
};
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum StartPage {
    #[default]
    Home,
    ThisPc,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Settings {
    pub size: [f32; 2],
    pub maximized: bool,
    /// Views the user applied to every folder of a kind; the others use the built-in default.
    pub templates: BTreeMap<FolderKind, FolderView>,
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
    pub show_protected: bool,
    pub item_checkboxes: bool,
    /// Compact view: less space between items.
    pub compact: bool,
    /// Property-system columns shown in Details, in order.
    pub prop_cols: Vec<explorer_core::Prop>,
    pub share: ShareSettings,
    /// Files starred as Favorites by the user.
    pub favorites: Vec<PathBuf>,
    /// Frequently opened folders: path -> (opens, last opened timestamp).
    pub frequent: BTreeMap<PathBuf, (u64, i64)>,
    /// Sections collapsed on Home ("quick_access", "favorites", "recent").
    pub home_collapsed: std::collections::HashSet<String>,
    /// Where File Explorer opens at startup (Home or This PC).
    pub start_page: StartPage,
    /// Browse folders: open folders in a new window/tab instead of same window.
    pub browse_new_window: bool,
    /// Single-click to open an item (point to select) vs double-click.
    pub single_click: bool,
    /// Privacy: show recently used files on Home.
    pub privacy_recent_files: bool,
    /// Privacy: show frequently used folders on Home.
    pub privacy_frequent_folders: bool,
    /// View: Display the full path in the title bar.
    pub title_full_path: bool,
    /// View: Hide empty drives in This PC / navigation pane.
    pub hide_empty_drives: bool,
    /// View: Show drive letters before drive names (default false = after).
    pub drive_letters_before: bool,
    /// View: Always show icons, never thumbnails.
    pub never_thumbnails: bool,
    /// View: Show status bar at the bottom.
    pub show_status_bar: bool,
    /// View: Automatically expand navigation pane to open folder.
    pub nav_expand_to_open: bool,
    /// View: Show all folders in the navigation pane.
    pub nav_show_all_folders: bool,
    /// View: Show libraries in the navigation pane.
    pub show_libraries: bool,
    /// Search: Always search file names and contents in non-indexed locations.
    pub search_contents: bool,
}
/// Nearby sharing (LocalSend protocol).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct ShareSettings {
    pub on: bool,
    /// Name other devices see; empty = the computer name.
    pub alias: String,
    /// Where received files go; `None` = Downloads.
    pub dir: Option<PathBuf>,
    /// Accept without asking.
    pub quick_save: bool,
    /// Password of the Wi-Fi Direct network, made once.
    pub hotspot_pass: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            size: [1040.0, 660.0],
            maximized: false,
            templates: BTreeMap::new(),
            show_hidden: false,
            show_extensions: true,
            col_created: false,
            col_attrs: false,
            pane: Pane::None,
            theme: Theme::System,
            folders: BTreeMap::new(),
            pinned: Vec::new(),
            show_protected: false,
            item_checkboxes: false,
            compact: false,
            prop_cols: Vec::new(),
            share: ShareSettings::default(),
            favorites: Vec::new(),
            frequent: BTreeMap::new(),
            home_collapsed: std::collections::HashSet::new(),
            start_page: StartPage::Home,
            browse_new_window: false,
            single_click: false,
            privacy_recent_files: true,
            privacy_frequent_folders: true,
            title_full_path: false,
            hide_empty_drives: true,
            drive_letters_before: false,
            never_thumbnails: false,
            show_status_bar: true,
            nav_expand_to_open: false,
            nav_show_all_folders: false,
            show_libraries: true,
            search_contents: false,
        }
    }
}

fn group_name(g: GroupBy) -> &'static str {
    match g {
        GroupBy::None => "none",
        GroupBy::Name => "name",
        GroupBy::Modified => "modified",
        GroupBy::Type => "type",
        GroupBy::Size => "size",
    }
}

fn parse_group(s: &str) -> GroupBy {
    match s {
        "name" => GroupBy::Name,
        "modified" => GroupBy::Modified,
        "type" => GroupBy::Type,
        "size" => GroupBy::Size,
        _ => GroupBy::None,
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
    /// "icons,size,desc,type,asc": view, sort, then group by (older files stop after the sort).
    fn to_text(self) -> String {
        let dir = |asc: bool| if asc { "asc" } else { "desc" };
        let view = self.view.name();
        let (sort, group) = (sort_name(self.sort), group_name(self.group));
        format!("{view},{sort},{},{group},{}", dir(self.ascending), dir(self.group_ascending))
    }

    fn parse(s: &str) -> Option<Self> {
        let mut it = s.split(',');
        let view = ViewMode::parse(it.next()?)?;
        let sort = parse_sort(it.next()?)?;
        let ascending = it.next()? != "desc";
        let group = it.next().map_or(GroupBy::None, parse_group);
        let group_ascending = it.next() != Some("desc");
        Some(Self { view, sort, ascending, group, group_ascending })
    }
}

/// %APPDATA%\\oxidows (or the XDG config folder).
pub fn config_dir() -> Option<PathBuf> {
    path().and_then(|p| p.parent().map(PathBuf::from))
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
            if let Some(kind) = k.strip_prefix("template:").and_then(FolderKind::parse) {
                if let Some(view) = FolderView::parse(v) {
                    s.templates.insert(kind, view);
                }
                continue;
            }
            if let Some(folder) = k.strip_prefix("frequent:") {
                if let Some((opens, last)) = v.split_once(',')
                    && let (Ok(opens), Ok(last)) = (opens.parse(), last.parse())
                {
                    s.frequent.insert(PathBuf::from(folder), (opens, last));
                }
                continue;
            }
            match k {
                "width" => s.size[0] = v.parse().unwrap_or(s.size[0]),
                "height" => s.size[1] = v.parse().unwrap_or(s.size[1]),
                "maximized" => s.maximized = flag(v),
                // "view" and "group_by" were one view for every folder before views became
                // per folder; they are dropped.
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
                "compact" => s.compact = flag(v),
                "columns" => s.prop_cols = v.split(',').filter_map(explorer_core::Prop::parse).collect(),
                "pin" if !v.is_empty() => s.pinned.push(PathBuf::from(v)),
                "share" => s.share.on = flag(v),
                "share_alias" => s.share.alias = v.to_string(),
                "share_dir" if !v.is_empty() => s.share.dir = Some(PathBuf::from(v)),
                "share_quick_save" => s.share.quick_save = flag(v),
                "hotspot_pass" => s.share.hotspot_pass = v.to_string(),
                "theme" => s.theme = Theme::parse(v),
                "favorite" if !v.is_empty() => s.favorites.push(PathBuf::from(v)),
                "favorites" if !v.is_empty() => {
                    for p in v.split(',') {
                        let p = p.trim();
                        if !p.is_empty() {
                            s.favorites.push(PathBuf::from(p));
                        }
                    }
                }
                "home_collapsed" if !v.is_empty() => {
                    for name in v.split(',') {
                        let name = name.trim();
                        if !name.is_empty() {
                            s.home_collapsed.insert(name.to_string());
                        }
                    }
                }
                "start_page" => {
                    s.start_page = match v {
                        "this_pc" | "thispc" => StartPage::ThisPc,
                        _ => StartPage::Home,
                    };
                }
                "browse_new_window" => s.browse_new_window = flag(v),
                "single_click" => s.single_click = flag(v),
                "privacy_recent_files" => s.privacy_recent_files = v != "false",
                "privacy_frequent_folders" => s.privacy_frequent_folders = v != "false",
                "title_full_path" => s.title_full_path = flag(v),
                "hide_empty_drives" => s.hide_empty_drives = v != "false",
                "drive_letters_before" => s.drive_letters_before = flag(v),
                "never_thumbnails" => s.never_thumbnails = flag(v),
                "show_status_bar" => s.show_status_bar = v != "false",
                "nav_expand_to_open" => s.nav_expand_to_open = flag(v),
                "nav_show_all_folders" => s.nav_show_all_folders = flag(v),
                "show_libraries" => s.show_libraries = v != "false",
                "search_contents" => s.search_contents = flag(v),
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
        let theme = self.theme.key();
        let mut out = format!(
            "width={}\nheight={}\nmaximized={}\nshow_hidden={}\nshow_extensions={}\ncol_created={}\n\
             col_attrs={}\npane={pane}\ntheme={theme}\n",
            self.size[0].round(),
            self.size[1].round(),
            self.maximized,
            self.show_hidden,
            self.show_extensions,
            self.col_created,
            self.col_attrs,
        );
        out.push_str(&format!(
            "show_protected={}\nitem_checkboxes={}\ncompact={}\n",
            self.show_protected, self.item_checkboxes, self.compact
        ));
        let cols: Vec<&str> = self.prop_cols.iter().map(|p| p.key()).collect();
        out.push_str(&format!("columns={}\n", cols.join(",")));
        for p in &self.pinned {
            out.push_str(&format!("pin={}\n", p.display()));
        }
        let sh = &self.share;
        out.push_str(&format!(
            "share={}\nshare_alias={}\nshare_dir={}\nshare_quick_save={}\nhotspot_pass={}\n",
            sh.on,
            sh.alias,
            sh.dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default(),
            sh.quick_save,
            sh.hotspot_pass
        ));
        for (k, v) in &self.templates {
            out.push_str(&format!("template:{}={}\n", k.key(), v.to_text()));
        }
        for (k, (n, v)) in &self.folders {
            out.push_str(&format!("folder:{k}={n},{}\n", v.to_text()));
        }
        out.push_str(&format!(
            "start_page={}\nbrowse_new_window={}\nsingle_click={}\nprivacy_recent_files={}\n\
             privacy_frequent_folders={}\ntitle_full_path={}\nhide_empty_drives={}\ndrive_letters_before={}\n\
             never_thumbnails={}\nshow_status_bar={}\nnav_expand_to_open={}\nnav_show_all_folders={}\n\
             show_libraries={}\nsearch_contents={}\n",
            match self.start_page {
                StartPage::Home => "home",
                StartPage::ThisPc => "this_pc",
            },
            self.browse_new_window,
            self.single_click,
            self.privacy_recent_files,
            self.privacy_frequent_folders,
            self.title_full_path,
            self.hide_empty_drives,
            self.drive_letters_before,
            self.never_thumbnails,
            self.show_status_bar,
            self.nav_expand_to_open,
            self.nav_show_all_folders,
            self.show_libraries,
            self.search_contents,
        ));
        for p in &self.favorites {
            out.push_str(&format!("favorite={}\n", p.display()));
        }
        if !self.home_collapsed.is_empty() {
            let mut col: Vec<&str> = self.home_collapsed.iter().map(String::as_str).collect();
            col.sort();
            out.push_str(&format!("home_collapsed={}\n", col.join(",")));
        }
        for (p, (opens, last)) in &self.frequent {
            out.push_str(&format!("frequent:{}={opens},{last}\n", p.display()));
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

    /// The view of a folder never changed: what the user applied to its kind, else Windows'
    /// default for it.
    pub fn default_view(&self, key: &str, known: &[(&str, PathBuf)]) -> FolderView {
        let kind = FolderKind::of(key, known);
        if let Some(v) = self.templates.get(&kind) {
            return *v;
        }
        let downloads = known.iter().any(|(n, p)| *n == "Downloads" && folder_key(p) == key);
        if downloads { DOWNLOADS_VIEW } else { kind.builtin_view() }
    }

    /// Options ▸ Apply to all folders: every folder of this kind gets `view`, including the ones
    /// whose own view was remembered.
    pub fn apply_to_kind(&mut self, kind: FolderKind, view: FolderView, known: &[(&str, PathBuf)]) {
        self.templates.insert(kind, view);
        self.folders.retain(|k, _| FolderKind::of(k, known) != kind);
    }

    /// Options ▸ Reset folders: every folder back to its kind's built-in default.
    pub fn reset_folders(&mut self) {
        self.templates.clear();
        self.folders.clear();
    }

    pub fn apply_theme(&self, ctx: &egui::Context) {
        ui_common::theme::apply(ctx, self.theme);
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

    pub fn is_favorite(&self, p: &std::path::Path) -> bool {
        self.persist.settings.favorites.iter().any(|q| folder_key(q) == folder_key(p))
    }

    pub fn add_favorite(&mut self, p: PathBuf) {
        if !self.is_favorite(&p) {
            self.persist.settings.favorites.push(p);
        }
    }

    pub fn remove_favorite(&mut self, p: &std::path::Path) {
        self.persist.settings.favorites.retain(|q| folder_key(q) != folder_key(p));
    }

    pub fn record_frequent_folder(&mut self, p: PathBuf) {
        if !self.persist.settings.privacy_frequent_folders {
            return;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let entry = self.persist.settings.frequent.entry(p).or_insert((0, 0));
        entry.0 += 1;
        entry.1 = now;
    }

    pub fn remove_frequent_folder(&mut self, p: &std::path::Path) {
        self.persist.settings.frequent.retain(|q, _| folder_key(q) != folder_key(p));
    }

    pub fn folder_view(&self) -> FolderView {
        FolderView {
            view: self.view_mode,
            sort: self.sort,
            ascending: self.ascending,
            group: self.group_by,
            group_ascending: self.group_ascending,
        }
    }

    fn set_folder_view(&mut self, v: FolderView) {
        self.view_mode = v.view;
        self.sort = v.sort;
        self.ascending = v.ascending;
        self.group_by = v.group;
        self.group_ascending = v.group_ascending;
    }

    /// Shows the current folder as remembered or as its default again (after Reset folders).
    pub fn reload_folder_view(&mut self) {
        self.persist.last = None;
    }

    pub fn apply_settings(&mut self, s: &Settings) {
        self.show_hidden = s.show_hidden;
        self.show_extensions = s.show_extensions;
        self.col_created = s.col_created;
        self.col_attrs = s.col_attrs;
        self.pane = s.pane;
        self.show_protected = s.show_protected;
        self.item_checkboxes = s.item_checkboxes;
        self.compact = s.compact;
        self.prop_cols = s.prop_cols.clone();
    }

    /// Once per frame: per-folder views, and saving changes.
    pub fn persist_tick(&mut self, ctx: &egui::Context) {
        let now_view = self.folder_view();
        match self.persist.last.take() {
            Some((dir, before)) if dir == self.cur => {
                // Changed here: remembered for this folder only.
                if before != now_view {
                    self.persist.settings.remember_folder(folder_key(&self.cur), now_view);
                }
            }
            _ => {
                // Entered a folder: its own view if it has one, else the default for its kind.
                let key = folder_key(&self.cur);
                let page = |p: &str| self.cur.as_os_str() == p;
                let newest_first =
                    |view| FolderView { view, sort: SortKey::Modified, ascending: false, ..FolderView::DETAILS };
                let v = match self.persist.settings.folders.get(&key) {
                    Some(&(_, v)) => v,
                    // This PC and Libraries open as tiles until the user picks another view there.
                    None if page(explorer_core::THIS_PC) || page(explorer_core::LIBRARIES) => {
                        FolderView::with(ViewMode::Tiles)
                    }
                    // The Gallery: big thumbnails, newest first.
                    None if page(explorer_core::GALLERY) => newest_first(ViewMode::Large),
                    // Home lists recent files newest first.
                    None if page(explorer_core::HOME) => newest_first(ViewMode::Details),
                    None => self.persist.settings.default_view(&key, &self.places),
                };
                self.set_folder_view(v);
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
        s.show_protected = self.show_protected;
        s.item_checkboxes = self.item_checkboxes;
        s.compact = self.compact;
        s.prop_cols = self.prop_cols.clone();

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
            templates: BTreeMap::from([(FolderKind::Music, FolderView::with(ViewMode::Tiles))]),
            show_hidden: true,
            show_extensions: false,
            col_created: true,
            col_attrs: true,
            pane: Pane::Details,
            theme: Theme::Light,
            folders: BTreeMap::new(),
            pinned: Vec::new(),
            show_protected: false,
            item_checkboxes: false,
            compact: false,
            prop_cols: vec![explorer_core::Prop::Dimensions, explorer_core::Prop::Authors],
            share: ShareSettings::default(),
            favorites: Vec::new(),
            frequent: BTreeMap::new(),
            home_collapsed: std::collections::HashSet::new(),
            start_page: StartPage::Home,
            browse_new_window: false,
            single_click: false,
            privacy_recent_files: true,
            privacy_frequent_folders: true,
            title_full_path: false,
            hide_empty_drives: true,
            drive_letters_before: false,
            never_thumbnails: false,
            show_status_bar: true,
            nav_expand_to_open: false,
            nav_show_all_folders: false,
            show_libraries: true,
            search_contents: false,
        };
        s.remember_folder(
            r"c:\users\a\pictures".into(),
            FolderView { view: ViewMode::Large, sort: SortKey::Modified, ascending: false, ..FolderView::DETAILS },
        );
        s.remember_folder(
            r"d:\work".into(),
            FolderView { group: GroupBy::Type, group_ascending: false, ..FolderView::DETAILS },
        );
        s.item_checkboxes = true;
        s.compact = true;
        s.pinned = vec![PathBuf::from(r"D:\Projects"), PathBuf::from(r"\\nas\share")];
        s.share = ShareSettings {
            on: true,
            alias: "Phước's PC".into(),
            dir: Some(PathBuf::from(r"D:\Phone")),
            quick_save: true,
            hotspot_pass: "k3x9q2mz".into(),
        };
        s.favorites = vec![PathBuf::from("/home/user/document.txt"), PathBuf::from("/home/user/photo.png")];
        s.frequent = BTreeMap::from([
            (PathBuf::from("/home/user/work"), (12, 1700000000)),
            (PathBuf::from("/home/user/downloads"), (8, 1700001000)),
        ]);
        s.home_collapsed = std::collections::HashSet::from(["quick_access".into(), "recent".into()]);
        s.start_page = StartPage::ThisPc;
        s.browse_new_window = true;
        s.single_click = true;
        s.privacy_recent_files = false;
        s.privacy_frequent_folders = false;
        s.title_full_path = true;
        s.hide_empty_drives = false;
        s.drive_letters_before = true;
        s.never_thumbnails = true;
        s.show_status_bar = false;
        s.nav_expand_to_open = true;
        s.nav_show_all_folders = true;
        s.show_libraries = false;
        s.search_contents = true;
        assert_eq!(Settings::parse(&s.to_text()), s);
        assert_eq!(Settings::parse("garbage\nwidth=abc\nview=nope"), Settings::default());
    }

    #[test]
    fn favorites_and_frequent_round_trip() {
        let text = "favorite=/docs/a.txt\nfavorite=/docs/b.png\nhome_collapsed=favorites,recent\n\
                    frequent:/docs/work=15,1700000000\n";
        let s = Settings::parse(text);
        assert_eq!(s.favorites, vec![PathBuf::from("/docs/a.txt"), PathBuf::from("/docs/b.png")]);
        assert!(s.home_collapsed.contains("favorites"));
        assert!(s.home_collapsed.contains("recent"));
        assert_eq!(s.frequent.get(&PathBuf::from("/docs/work")), Some(&(15, 1700000000)));

        // Also check comma-separated favorites= line format
        let s2 = Settings::parse("favorites=/docs/a.txt,/docs/b.png\n");
        assert_eq!(s2.favorites, vec![PathBuf::from("/docs/a.txt"), PathBuf::from("/docs/b.png")]);
    }

    #[test]
    fn folder_memory_is_bounded() {
        let mut s = Settings::default();
        let v = FolderView::DETAILS;
        for i in 0..MAX_FOLDERS + 10 {
            s.remember_folder(format!("c:\\f{i}"), v);
        }
        assert_eq!(s.folders.len(), MAX_FOLDERS);
        assert!(!s.folders.contains_key("c:\\f0"));
        assert!(s.folders.contains_key(&format!("c:\\f{}", MAX_FOLDERS + 9)));
    }

    /// Views saved before group by was per folder still load.
    #[test]
    fn old_folder_lines_load() {
        let s = Settings::parse("view=icons,size,desc\ngroup_by=type\nfolder:c:\\a=3,tiles,name,asc\n");
        assert_eq!(s.folders["c:\\a"], (3, FolderView::with(ViewMode::Tiles)));
        assert!(s.templates.is_empty());
    }

    fn known() -> Vec<(&'static str, PathBuf)> {
        vec![
            ("Downloads", PathBuf::from(r"C:\Users\A\Downloads")),
            ("Pictures", PathBuf::from(r"C:\Users\A\Pictures")),
            ("Videos", PathBuf::from(r"C:\Users\A\Videos")),
            ("Music", PathBuf::from(r"D:\Music")),
        ]
    }

    #[test]
    fn defaults_follow_the_folder_kind() {
        let s = Settings::default();
        let k = known();
        assert_eq!(s.default_view(r"c:\users\a\pictures", &k).view, ViewMode::Large);
        assert_eq!(s.default_view(r"c:\users\a\pictures\trip 2026", &k).view, ViewMode::Large);
        assert_eq!(s.default_view(r"c:\users\a\picturesque", &k), FolderView::DETAILS);
        assert_eq!(s.default_view(r"c:\users\a\videos", &k).view, ViewMode::Large);
        assert_eq!(s.default_view(r"d:\music\album", &k), FolderView::DETAILS);
        assert_eq!(s.default_view(r"c:\users\a\downloads", &k), DOWNLOADS_VIEW);
        assert_eq!(s.default_view(r"c:\users\a\downloads\sub", &k), FolderView::DETAILS);
        assert_eq!(s.default_view(r"e:\", &k), FolderView::DETAILS);
    }

    #[test]
    fn apply_to_all_folders_of_a_kind_and_reset() {
        let k = known();
        let mut s = Settings::default();
        let tiles = FolderView::with(ViewMode::Tiles);
        let small = FolderView::with(ViewMode::Small);
        s.remember_folder(r"c:\users\a\pictures\x".into(), small);
        s.remember_folder(r"e:\work".into(), small);
        s.apply_to_kind(FolderKind::Pictures, tiles, &k);
        assert_eq!(s.default_view(r"c:\users\a\pictures\y", &k), tiles);
        assert!(!s.folders.contains_key(r"c:\users\a\pictures\x"));
        // Other kinds keep their own views.
        assert_eq!(s.folders[r"e:\work"].1, small);
        assert_eq!(s.default_view(r"e:\other", &k), FolderView::DETAILS);
        s.reset_folders();
        assert!(s.folders.is_empty());
        assert_eq!(s.default_view(r"c:\users\a\pictures\y", &k).view, ViewMode::Large);
    }
}
