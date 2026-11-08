#![forbid(unsafe_code)]
// Release builds on Windows run without a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod dnd;
mod grid;
mod nearby;
mod pane;
mod settings;
mod undo;
mod view;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use explorer_core::{Entry, History, Place, SortKey, child_path, classify, parent_path, unique_name, validate_name};
use explorer_sys::{Drive, FileOp};

/// Results from worker threads. Listing messages carry the generation they were started for,
/// so batches from a folder the user already left are dropped.
enum Msg {
    Batch(u64, Vec<Entry>),
    /// Finished listing: error text and whether signing in could fix it.
    Done(u64, Result<(), (String, bool)>, Duration),
    /// A sign-in to a share finished.
    Connected(PathBuf, std::io::Result<()>),
    Drives(Vec<Drive>),
    /// Phones and other devices under This PC.
    Devices(Vec<explorer_sys::ShellDevice>),
    /// A file operation finished, with what it did if that can be undone.
    OpDone(String, std::io::Result<bool>, Option<undo::Undoable>),
    /// The watched folder changed on disk.
    Changed(PathBuf),
    Icon(IconKey, Option<explorer_sys::IconImage>),
    Thumb(PathBuf, Option<explorer_sys::IconImage>),
    /// Every `Prop::ALL` value of one file.
    Props(PathBuf, Vec<String>),
    /// Subfolder names for the navigation tree, and display names of shell folders among them.
    Tree(PathBuf, Vec<String>, Vec<(PathBuf, String)>),
    Preview(PathBuf, pane::PreviewData),
}

/// Icons are cached per extension; only files that carry their own icon get one each.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum IconKey {
    Dir,
    Ext(String),
    File(PathBuf),
}

impl IconKey {
    /// Items that may look different from others of their type get their own icon, with the
    /// shell's overlays: links and junctions (arrow), cloud files (OneDrive status), and
    /// folders marked read-only/system, which is how a desktop.ini custom icon is enabled.
    pub fn for_entry(e: &Entry, dir: &Path) -> Self {
        use explorer_core::attr;
        const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
        let cloud = e.attrs & (attr::OFFLINE | attr::PINNED | attr::UNPINNED | RECALL_ON_DATA_ACCESS) != 0;
        if e.link || cloud || (e.is_dir && e.attrs & (attr::READONLY | attr::SYSTEM) != 0) {
            return IconKey::File(dir.join(&e.name));
        }
        if e.is_dir {
            return IconKey::Dir;
        }
        let ext = e.extension();
        if matches!(ext.as_str(), "exe" | "lnk" | "ico" | "url" | "msc" | "cpl" | "scr") {
            IconKey::File(dir.join(&e.name))
        } else {
            IconKey::Ext(ext)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewMode {
    ExtraLarge,
    Large,
    Medium,
    Small,
    List,
    Details,
    Tiles,
    Content,
}

impl ViewMode {
    /// In Explorer's order, with their Ctrl+Shift+number shortcuts.
    pub const ALL: [(ViewMode, &'static str, &'static str); 8] = [
        (ViewMode::ExtraLarge, "Extra large icons", "Ctrl+Shift+1"),
        (ViewMode::Large, "Large icons", "Ctrl+Shift+2"),
        (ViewMode::Medium, "Medium icons", "Ctrl+Shift+3"),
        (ViewMode::Small, "Small icons", "Ctrl+Shift+4"),
        (ViewMode::List, "List", "Ctrl+Shift+5"),
        (ViewMode::Details, "Details", "Ctrl+Shift+6"),
        (ViewMode::Tiles, "Tiles", "Ctrl+Shift+7"),
        (ViewMode::Content, "Content", "Ctrl+Shift+8"),
    ];

    pub fn name(self) -> &'static str {
        match self {
            ViewMode::ExtraLarge => "extralarge",
            ViewMode::Large => "icons",
            ViewMode::Medium => "medium",
            ViewMode::Small => "small",
            ViewMode::List => "list",
            ViewMode::Details => "details",
            ViewMode::Tiles => "tiles",
            ViewMode::Content => "content",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().map(|a| a.0).find(|m| m.name() == s)
    }
}

pub enum IconState {
    Pending,
    Ready(egui::TextureHandle),
    Failed,
}

enum Dialog {
    /// `others`: more selected items, renamed together as "text (1)", "text (2)", …
    Rename {
        original: String,
        others: Vec<String>,
        text: String,
        error: Option<&'static str>,
        opened: bool,
    },
    NewFolder {
        text: String,
        error: Option<&'static str>,
        opened: bool,
    },
    MapDrive(MapDrive),
    ConfirmPurge(Vec<Entry>),
}

pub struct MapDrive {
    pub letters: Vec<char>,
    pub letter: char,
    pub path: String,
    pub persistent: bool,
}

/// A tab that is not on screen: just enough to bring it back (the listing is re-read, which
/// takes milliseconds, rather than kept in memory).
pub struct TabSnap {
    pub cur: PathBuf,
    history: History,
    selected: HashSet<String>,
    filter: String,
    sort: SortKey,
    ascending: bool,
}

/// What the cached sorted view was computed from; recomputed only when this changes, so a
/// 100 000-item folder is not re-sorted every frame.
#[derive(PartialEq, Clone)]
struct ViewKey {
    version: u64,
    filter: String,
    sort: SortKey,
    ascending: bool,
    show_hidden: bool,
    group: (explorer_core::GroupBy, bool),
}

struct App {
    ctx: egui::Context,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,

    cur: PathBuf,
    history: History,
    entries: Vec<Entry>,
    /// Bumped whenever `entries` changes.
    version: u64,
    generation: u64,
    cancel: Arc<AtomicBool>,
    watch_cancel: Arc<AtomicBool>,
    /// A change was seen at this time; refresh once things settle (debounce bursts).
    changed_at: Option<Instant>,
    ctrl_v_was_down: bool,
    loading: bool,
    /// On refresh, keep showing the old listing until the first new batch arrives (no flicker).
    replace_on_next_batch: bool,
    list_error: Option<String>,
    list_time: Option<Duration>,

    view: Vec<usize>,
    view_key: Option<ViewKey>,
    filter: String,
    sort: SortKey,
    ascending: bool,
    show_hidden: bool,
    pub show_extensions: bool,
    /// Optional Details columns (right-click the header to choose).
    pub col_created: bool,
    pub col_attrs: bool,
    pub pane: pane::Pane,
    pub group_by: explorer_core::GroupBy,
    /// Show hidden + system ("protected operating system") files too.
    pub show_protected: bool,
    pub item_checkboxes: bool,
    pub compact: bool,
    pub group_ascending: bool,
    /// Groups of `view` when grouping is on (empty otherwise).
    pub groups: Vec<explorer_core::Group>,
    pub persist: settings::Persist,
    pub undo_stack: Vec<undo::Undoable>,
    pub drag: Option<dnd::Drag>,
    /// Folder under the pointer during a drag (set while drawing each frame).
    pub drop_target: Option<PathBuf>,
    release_pointer: bool,
    pub redo_stack: Vec<undo::Undoable>,
    preview: Option<(PathBuf, pane::PreviewState)>,

    /// Selected names (names survive refreshes; indices don't).
    selected: HashSet<String>,
    /// Keyboard cursor and shift-range anchor, as positions in `view`.
    cursor: Option<usize>,
    anchor: Option<usize>,
    scroll_to: Option<usize>,
    /// Select this name once it shows up (new folder, folder we came up from).
    pending_select: Option<String>,
    type_ahead: (String, Instant),

    address_edit: Option<String>,
    focus_search: bool,
    /// Query of the running/finished search through subfolders; `None` = normal listing.
    pub deep_search: Option<String>,
    pub needs_sign_in: bool,
    prompted: HashSet<PathBuf>,
    dialog: Option<Dialog>,
    status: Option<String>,
    ops_running: usize,

    pub icons: HashMap<IconKey, IconState>,
    pub view_mode: ViewMode,
    /// Thumbnails for the Icons view, per file; cleared when leaving a folder.
    pub thumbs: HashMap<PathBuf, IconState>,
    pub thumb_wanted: Vec<PathBuf>,
    thumb_tx: Option<Sender<(PathBuf, u32)>>,
    /// Property-system columns shown in Details.
    pub prop_cols: Vec<explorer_core::Prop>,
    /// Per file: `Prop::ALL` values, `None` while loading.
    pub props: HashMap<PathBuf, Option<Vec<String>>>,
    pub props_wanted: Vec<PathBuf>,
    props_tx: Option<Sender<PathBuf>>,
    /// Size the cached thumbnails were made at.
    thumb_px: u32,
    icon_tx: Option<Sender<IconKey>>,
    drives: Vec<Drive>,
    /// Phones and cameras under This PC, for the navigation pane.
    pub devices: Vec<explorer_sys::ShellDevice>,
    places: Vec<(&'static str, PathBuf)>,
    utc_offset: i64,
    started: Instant,
    timing_logged: bool,
    was_focused: bool,
    title: String,
    /// Main window handle (Windows), for the shell's menus and dialogs; 0 if unknown.
    pub hwnd: isize,
    /// Display names of shell items seen in listings, for breadcrumbs and tab titles.
    pub shell_labels: HashMap<PathBuf, String>,
    /// All tabs; the active one's slot is stale while it is shown (its state lives in `App`).
    pub tabs: Vec<TabSnap>,
    /// Navigation tree: subfolders of each expanded folder (loaded on first expand).
    pub tree: HashMap<PathBuf, Vec<String>>,
    tree_pending: HashSet<PathBuf>,
    pub active_tab: usize,
    pub nearby: nearby::Nearby,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, start: PathBuf, settings: settings::Settings) -> Self {
        ui_common::setup(&cc.egui_ctx, settings.theme);
        let (tx, rx) = channel();
        let mut app = Self {
            ctx: cc.egui_ctx.clone(),
            tx,
            rx,
            cur: start,
            history: History::default(),
            entries: Vec::new(),
            version: 0,
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            watch_cancel: Arc::new(AtomicBool::new(false)),
            changed_at: None,
            ctrl_v_was_down: false,
            loading: false,
            replace_on_next_batch: false,
            list_error: None,
            list_time: None,
            view: Vec::new(),
            view_key: None,
            filter: String::new(),
            sort: SortKey::Name,
            ascending: true,
            show_hidden: false,
            show_extensions: true,
            col_created: false,
            col_attrs: false,
            pane: pane::Pane::None,
            group_by: explorer_core::GroupBy::None,
            show_protected: false,
            item_checkboxes: false,
            compact: false,
            group_ascending: true,
            groups: Vec::new(),
            persist: settings::Persist::new(settings.clone()),
            undo_stack: Vec::new(),
            drag: None,
            drop_target: None,
            release_pointer: false,
            redo_stack: Vec::new(),
            preview: None,
            selected: HashSet::new(),
            cursor: None,
            anchor: None,
            scroll_to: None,
            pending_select: None,
            type_ahead: (String::new(), Instant::now()),
            address_edit: None,
            focus_search: false,
            deep_search: None,
            needs_sign_in: false,
            prompted: HashSet::new(),
            dialog: None,
            status: None,
            ops_running: 0,
            icons: HashMap::new(),
            view_mode: ViewMode::Details,
            thumbs: HashMap::new(),
            thumb_wanted: Vec::new(),
            thumb_tx: None,
            prop_cols: Vec::new(),
            props: HashMap::new(),
            props_wanted: Vec::new(),
            props_tx: None,
            thumb_px: 96,
            icon_tx: None,
            drives: Vec::new(),
            devices: Vec::new(),
            places: explorer_sys::known_folders(),
            utc_offset: explorer_sys::utc_offset_secs(),
            started: Instant::now(),
            timing_logged: false,
            was_focused: true,
            title: String::new(),
            hwnd: 0,
            shell_labels: HashMap::new(),
            tabs: Vec::new(),
            tree: HashMap::new(),
            tree_pending: HashSet::new(),
            active_tab: 0,
            nearby: nearby::Nearby::new(),
        };
        let s = app.persist.settings.clone();
        app.apply_settings(&s);
        let first = app.snapshot();
        app.tabs.push(first);
        app.start_listing(false);
        app.start_watch();
        app.start_icon_loader();
        app.load_drives();
        app
    }

    // ---- workers -------------------------------------------------------------------------

    /// Searches `cur` and everything below it. Results keep their path relative to `cur` as
    /// the name ("sub\\file.txt"), so opening, copying and deleting them works unchanged.
    ///
    /// On Windows the query goes to the shell first, exactly as Explorer's search box sends it:
    /// Windows Search answers from its index (contents and properties included) or crawls.
    /// Without it, the folder tree is walked here and the query's Advanced Query Syntax subset
    /// (`kind:`, `date:`, `size:`, `ext:`, `content:`, wildcards) is applied to each item.
    /// Links/junctions are not followed (no loops).
    pub fn start_deep_search(&mut self, query: String) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.generation += 1;
        self.loading = true;
        self.list_error = None;
        self.replace_on_next_batch = false;
        self.entries.clear();
        self.selected.clear();
        self.version += 1;
        self.deep_search = Some(query.clone());
        let (root, generation, cancel, tx, ctx) =
            (self.cur.clone(), self.generation, self.cancel.clone(), self.tx.clone(), self.ctx.clone());
        let offset = self.utc_offset;
        thread::Builder::new()
            .name("search".into())
            .spawn(move || {
                let t = Instant::now();
                let mut out =
                    SearchOut { found: Vec::new(), last_send: Instant::now(), tx: &tx, ctx: &ctx, generation };
                let shell = explorer_sys::shell_search(&root, &query, &cancel, &mut |p| {
                    if let Ok(rel) = p.strip_prefix(&root)
                        && !rel.as_os_str().is_empty()
                        && let Some(mut e) = explorer_sys::stat(&p)
                    {
                        e.name = rel.to_string_lossy().into_owned();
                        out.push(e);
                    }
                });
                if shell.is_err() && !cancel.load(Ordering::Relaxed) {
                    walk_search(&root, &query, offset, &cancel, &mut out);
                }
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                out.flush();
                let _ = tx.send(Msg::Done(generation, Ok(()), t.elapsed()));
                ctx.request_repaint();
            })
            .expect("spawn search thread");
    }

    pub fn end_deep_search(&mut self) {
        if self.deep_search.take().is_some() {
            self.start_listing(false);
        }
    }

    fn start_listing(&mut self, keep_old: bool) {
        self.deep_search = None;
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.generation += 1;
        self.loading = true;
        self.list_error = None;
        self.replace_on_next_batch = keep_old;
        if !keep_old {
            self.entries.clear();
            self.version += 1;
        }
        let (dir, generation, cancel, tx, ctx) =
            (self.cur.clone(), self.generation, self.cancel.clone(), self.tx.clone(), self.ctx.clone());
        let pinned = self.persist.settings.pinned.clone();
        thread::Builder::new()
            .name("list".into())
            .spawn(move || {
                let t = Instant::now();
                let r = match classify(&dir) {
                    // Computers and shares are shown as folders.
                    // The NetBIOS browse list, plus what the shell's Network folder finds through
                    // Function Discovery (WSD computers, media servers, phones on the LAN).
                    Place::Network => {
                        let names = explorer_sys::network_computers();
                        let mut seen: HashSet<String> =
                            names.iter().map(|n| n.trim_start_matches('\\').to_ascii_uppercase()).collect();
                        let batch = names
                            .into_iter()
                            .map(|n| Entry {
                                name: n.trim_start_matches('\\').to_string(),
                                is_dir: true,
                                note: Some("Computer".into()),
                                ..Default::default()
                            })
                            .collect();
                        let _ = tx.send(Msg::Batch(generation, batch));
                        ctx.request_repaint();
                        let _ =
                            explorer_sys::shell_list(Path::new(explorer_core::NETWORK_CLSID), &cancel, &mut |batch| {
                                let batch: Vec<Entry> = batch
                                    .into_iter()
                                    .filter_map(|mut e| {
                                        let parsing = explorer_core::shell_parsing(Path::new(&e.name)).into_owned();
                                        // A computer: shown by name, opened as \\NAME.
                                        if let Some(host) = parsing.strip_prefix(r"\\") {
                                            if !seen.insert(host.to_ascii_uppercase()) {
                                                return None;
                                            }
                                            e.name = host.to_string();
                                            e.label = None;
                                            e.is_dir = true;
                                        }
                                        Some(e)
                                    })
                                    .collect();
                                let _ = tx.send(Msg::Batch(generation, batch));
                                ctx.request_repaint();
                            });
                        Ok(())
                    }
                    Place::Shell(_) => explorer_sys::shell_list(&dir, &cancel, &mut |batch| {
                        let _ = tx.send(Msg::Batch(generation, batch));
                        ctx.request_repaint();
                    }),
                    // Name = the `$R` data path (unique), label = original name, note = its folder.
                    Place::RecycleBin => {
                        let batch = explorer_sys::recycle_bin_items()
                            .into_iter()
                            .map(|it| Entry {
                                name: it.data.to_string_lossy().into_owned(),
                                label: it.original.file_name().map(|n| n.to_string_lossy().into_owned()),
                                note: it.original.parent().map(|p| p.to_string_lossy().into_owned()),
                                is_dir: it.is_dir,
                                size: it.size,
                                modified: it.deleted,
                                ..Default::default()
                            })
                            .collect();
                        let _ = tx.send(Msg::Batch(generation, batch));
                        Ok(())
                    }
                    // Quick access, then recently opened files (newest first), from the
                    // shortcuts Windows keeps in the Recent folder.
                    Place::Home => {
                        let quick = explorer_sys::known_folders()
                            .into_iter()
                            .map(|(n, p)| (n.to_string(), p))
                            .chain(pinned.iter().map(|p| {
                                (
                                    p.file_name()
                                        .map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned()),
                                    p.clone(),
                                )
                            }))
                            .map(|(label, p)| Entry {
                                name: p.to_string_lossy().into_owned(),
                                label: Some(label),
                                note: Some("Quick access".into()),
                                is_dir: true,
                                ..Default::default()
                            });
                        let mut batch: Vec<Entry> = quick.collect();
                        batch.extend(recent_files(60));
                        let _ = tx.send(Msg::Batch(generation, batch));
                        Ok(())
                    }
                    Place::Libraries => {
                        let batch = explorer_sys::libraries()
                            .into_iter()
                            .map(|(name, folders)| Entry {
                                name: format!("{}\\{name}", explorer_core::LIBRARIES),
                                label: Some(name),
                                note: Some(format!(
                                    "Library · {} location{}",
                                    folders.len(),
                                    if folders.len() == 1 { "" } else { "s" }
                                )),
                                is_dir: true,
                                ..Default::default()
                            })
                            .collect();
                        let _ = tx.send(Msg::Batch(generation, batch));
                        Ok(())
                    }
                    // A library shows the contents of all its folders together.
                    Place::Library(name) => {
                        let folders = explorer_sys::libraries().into_iter().find(|(n, _)| *n == name).map(|l| l.1);
                        match folders {
                            None => Err(std::io::Error::new(std::io::ErrorKind::NotFound, "No such library")),
                            Some(folders) => {
                                for f in folders {
                                    let _ = explorer_sys::list_dir(&f, &cancel, &mut |batch| {
                                        let batch = batch
                                            .into_iter()
                                            .map(|mut e| {
                                                e.label = Some(e.name.clone());
                                                e.name = f.join(&e.name).to_string_lossy().into_owned();
                                                e
                                            })
                                            .collect();
                                        let _ = tx.send(Msg::Batch(generation, batch));
                                        ctx.request_repaint();
                                    });
                                }
                                Ok(())
                            }
                        }
                    }
                    // Every picture and video in the Pictures library, newest first.
                    Place::Gallery => {
                        let roots: Vec<PathBuf> = explorer_sys::libraries()
                            .into_iter()
                            .filter(|(n, _)| n == "Pictures")
                            .flat_map(|l| l.1)
                            .collect();
                        gallery(&roots, &cancel, &mut |batch| {
                            let _ = tx.send(Msg::Batch(generation, batch));
                            ctx.request_repaint();
                        });
                        Ok(())
                    }
                    // Known folders, then drives with their free space.
                    Place::ThisPc => {
                        let folders = explorer_sys::known_folders().into_iter().map(|(name, p)| Entry {
                            name: p.to_string_lossy().into_owned(),
                            label: Some(name.to_string()),
                            note: Some("Folder".into()),
                            is_dir: true,
                            ..Default::default()
                        });
                        let drives = explorer_sys::drives().into_iter().map(|d| Entry {
                            name: d.root.to_string_lossy().into_owned(),
                            label: Some(view::drive_label(&d)),
                            note: Some(if d.total > 0 {
                                format!(
                                    "{} free of {}",
                                    explorer_core::format_bytes(d.free),
                                    explorer_core::format_bytes(d.total)
                                )
                            } else {
                                "Drive".into()
                            }),
                            is_dir: true,
                            size: d.total,
                            ..Default::default()
                        });
                        let _ = tx.send(Msg::Batch(generation, folders.chain(drives).collect()));
                        ctx.request_repaint();
                        // Devices without a drive letter: phones and cameras (MTP), media servers,
                        // with their storage space like a drive's.
                        let devices: Vec<Entry> = explorer_sys::shell_devices()
                            .into_iter()
                            .map(|d| Entry {
                                name: d.path.to_string_lossy().into_owned(),
                                label: Some(d.label),
                                note: Some(if d.total > 0 {
                                    format!(
                                        "{} free of {}",
                                        explorer_core::format_bytes(d.free),
                                        explorer_core::format_bytes(d.total)
                                    )
                                } else {
                                    d.kind
                                }),
                                is_dir: true,
                                size: d.total,
                                ..Default::default()
                            })
                            .collect();
                        if !devices.is_empty() {
                            let _ = tx.send(Msg::Batch(generation, devices));
                        }
                        Ok(())
                    }
                    Place::Server(server) => explorer_sys::list_shares(&server).map(|shares| {
                        let batch = shares
                            .into_iter()
                            .map(|s| Entry { name: s.name, is_dir: true, ..Default::default() })
                            .collect();
                        let _ = tx.send(Msg::Batch(generation, batch));
                    }),
                    _ => explorer_sys::list_dir(&dir, &cancel, &mut |batch| {
                        let _ = tx.send(Msg::Batch(generation, batch));
                        ctx.request_repaint();
                    }),
                };
                let r = r.map_err(|e| (e.to_string(), explorer_sys::is_auth_error(&e)));
                let _ = tx.send(Msg::Done(generation, r, t.elapsed()));
                ctx.request_repaint();
            })
            .expect("spawn list thread");
    }

    /// Watches the current folder so changes made by other programs show up by themselves.
    fn start_watch(&mut self) {
        self.watch_cancel.store(true, Ordering::Relaxed);
        self.watch_cancel = Arc::new(AtomicBool::new(false));
        if classify(&self.cur).is_page() {
            return;
        }
        let (dir, cancel, tx, ctx) = (self.cur.clone(), self.watch_cancel.clone(), self.tx.clone(), self.ctx.clone());
        thread::Builder::new()
            .name("watch".into())
            .spawn(move || {
                // Failing to watch (e.g. some network shares) just means no auto-refresh.
                let _ = explorer_sys::watch_dir(&dir, &cancel, &mut || {
                    let _ = tx.send(Msg::Changed(dir.clone()));
                    ctx.request_repaint();
                });
            })
            .expect("spawn watch thread");
    }

    /// One thread turns icon requests into pixels, in the order rows became visible.
    fn start_icon_loader(&mut self) {
        if !cfg!(windows) {
            return; // the dev build keeps the emoji fallback
        }
        let (req_tx, req_rx) = channel::<IconKey>();
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        thread::Builder::new()
            .name("icons".into())
            .spawn(move || {
                while let Ok(key) = req_rx.recv() {
                    let img = match &key {
                        IconKey::Dir => explorer_sys::shell_icon(Path::new("folder"), true, false),
                        IconKey::Ext(e) => explorer_sys::shell_icon(Path::new(&format!("file.{e}")), false, false),
                        IconKey::File(p) => explorer_sys::shell_icon(p, false, true),
                    };
                    if tx.send(Msg::Icon(key, img)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawn icon thread");
        self.icon_tx = Some(req_tx);

        let (thumb_tx, thumb_rx) = channel::<(PathBuf, u32)>();
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        thread::Builder::new()
            .name("thumbs".into())
            .spawn(move || {
                while let Ok((p, px)) = thumb_rx.recv() {
                    let img = explorer_sys::thumbnail(&p, px);
                    if tx.send(Msg::Thumb(p, img)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawn thumbnail thread");
        self.thumb_tx = Some(thumb_tx);

        let (props_tx, props_rx) = channel::<PathBuf>();
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        thread::Builder::new()
            .name("props".into())
            .spawn(move || {
                while let Ok(p) = props_rx.recv() {
                    let v = explorer_sys::read_props(&p, &explorer_core::Prop::ALL);
                    if tx.send(Msg::Props(p, v)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawn property thread");
        self.props_tx = Some(props_tx);
    }

    /// Asks for the property columns of the rows drawn this frame.
    pub fn request_props(&mut self) {
        let wanted = std::mem::take(&mut self.props_wanted);
        let Some(tx) = &self.props_tx else { return };
        for p in wanted {
            if let std::collections::hash_map::Entry::Vacant(slot) = self.props.entry(p) {
                let _ = tx.send(slot.key().clone());
                slot.insert(None);
            }
        }
    }

    pub fn request_thumbs(&mut self) {
        let px = grid::thumb_px(self.view_mode);
        if px != self.thumb_px {
            self.thumbs.clear();
            self.thumb_px = px;
        }
        let wanted = std::mem::take(&mut self.thumb_wanted);
        let Some(tx) = &self.thumb_tx else { return };
        for p in wanted {
            if let std::collections::hash_map::Entry::Vacant(slot) = self.thumbs.entry(p) {
                let _ = tx.send((slot.key().clone(), px));
                slot.insert(IconState::Pending);
            }
        }
    }

    pub fn request_icons(&mut self, keys: Vec<IconKey>) {
        let Some(tx) = &self.icon_tx else { return };
        for k in keys {
            if let std::collections::hash_map::Entry::Vacant(slot) = self.icons.entry(k) {
                let _ = tx.send(slot.key().clone());
                slot.insert(IconState::Pending);
            }
        }
    }

    /// Lists the subfolders of `dir` for the tree, in the background.
    pub fn load_tree(&mut self, dir: PathBuf) {
        if !self.tree_pending.insert(dir.clone()) {
            return;
        }
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        thread::spawn(move || {
            let mut names = Vec::new();
            let mut labels = Vec::new();
            if matches!(classify(&dir), Place::Shell(_)) {
                // A phone's folders: named by the ID after the parent's path, shown by label.
                let prefix = format!("{}\\", dir.to_string_lossy());
                let _ = explorer_sys::shell_list(&dir, &AtomicBool::new(false), &mut |b| {
                    for e in b.into_iter().filter(|e| e.is_dir && !e.hidden) {
                        if let Some(id) = e.name.strip_prefix(&prefix) {
                            if let Some(l) = &e.label {
                                labels.push((PathBuf::from(&e.name), l.clone()));
                            }
                            names.push((id.to_string(), e.display().to_string()));
                        }
                    }
                });
                names.sort_by(|a, b| explorer_core::natural_cmp(&a.1, &b.1));
            } else {
                let _ = explorer_sys::list_dir(&dir, &AtomicBool::new(false), &mut |b| {
                    names.extend(
                        b.into_iter().filter(|e| e.is_dir && !e.hidden && !e.system).map(|e| (e.name, String::new())),
                    );
                });
                names.sort_by(|a, b| explorer_core::natural_cmp(&a.0, &b.0));
            }
            let names = names.into_iter().map(|(n, _)| n).collect();
            let _ = tx.send(Msg::Tree(dir, names, labels));
            ctx.request_repaint();
        });
    }

    fn load_drives(&self) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        thread::spawn(move || {
            let _ = tx.send(Msg::Drives(explorer_sys::drives()));
            ctx.request_repaint();
            // After the drives: asking a phone for its storage takes longer.
            let _ = tx.send(Msg::Devices(explorer_sys::shell_devices()));
            ctx.request_repaint();
        });
    }

    fn run_op(&mut self, op: FileOp, sources: Vec<PathBuf>, dest: Option<PathBuf>) {
        if sources.is_empty() {
            return;
        }
        let what = match op {
            FileOp::Copy => "Copied",
            FileOp::Move => "Moved",
            FileOp::Recycle => "Moved to Recycle Bin:",
            FileOp::Delete => "Deleted",
        };
        let desc = if sources.len() == 1 {
            format!("{what} {}", sources[0].file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()))
        } else {
            format!("{what} {} items", sources.len())
        };
        self.ops_running += 1;
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        thread::Builder::new()
            .name("file-op".into())
            .spawn(move || {
                let pre = undo::snapshot_targets(&sources, dest.as_deref());
                let r = explorer_sys::file_op(op, &sources, dest.as_deref());
                // Recorded even when cancelled part-way: whatever was done can be undone.
                let rec = if r.is_ok() { undo::record(op, &sources, dest.as_deref(), &pre) } else { None };
                let _ = tx.send(Msg::OpDone(desc, r, rec));
                ctx.request_repaint();
            })
            .expect("spawn file-op thread");
    }

    fn drain(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Batch(g, batch) if g == self.generation => {
                    if self.replace_on_next_batch {
                        self.replace_on_next_batch = false;
                        self.entries.clear();
                    }
                    for e in &batch {
                        // Chinese, Japanese or Korean names load a font that can draw them.
                        ui_common::ensure_glyphs(&self.ctx, e.display());
                        if e.name.starts_with(explorer_core::SHELL)
                            && let Some(l) = &e.label
                        {
                            self.shell_labels.insert(PathBuf::from(&e.name), l.clone());
                        }
                    }
                    self.entries.extend(batch);
                    self.version += 1;
                }
                Msg::Done(g, r, took) if g == self.generation => {
                    if self.replace_on_next_batch {
                        self.replace_on_next_batch = false;
                        self.entries.clear();
                        self.version += 1;
                    }
                    self.loading = false;
                    self.list_time = Some(took);
                    self.needs_sign_in = matches!(r, Err((_, true)));
                    self.list_error = r.err().map(|(e, _)| e);
                    // Like Explorer: a share that refuses the current login asks for one, once.
                    if self.needs_sign_in && self.prompted.insert(self.share_root()) {
                        self.sign_in();
                    }
                    if !self.timing_logged {
                        self.timing_logged = true;
                        if std::env::var_os("OXI_EXPLORER_TIMING").is_some() {
                            eprintln!(
                                "first listing: {} items in {took:?}, on screen after {:?}",
                                self.entries.len(),
                                self.started.elapsed()
                            );
                        }
                    }
                }
                Msg::Batch(..) | Msg::Done(..) => {}
                Msg::Connected(root, r) => match r {
                    Ok(()) => {
                        if self.share_root() == root {
                            self.refresh();
                        }
                    }
                    Err(e) => self.status = Some(format!("Could not connect to {}: {e}", root.display())),
                },
                Msg::Drives(d) => self.drives = d,
                Msg::Devices(d) => {
                    for dev in &d {
                        ui_common::ensure_glyphs(&self.ctx, &dev.label);
                        self.shell_labels.insert(dev.path.clone(), dev.label.clone());
                    }
                    self.devices = d;
                }
                Msg::Changed(dir) if dir == self.cur && self.deep_search.is_none() => {
                    self.changed_at.get_or_insert_with(Instant::now);
                }
                Msg::Changed(_) => {}
                Msg::Thumb(p, img) => {
                    // Only keep it if we're still in that folder (thumbnails are big).
                    if self.thumbs.contains_key(&p) {
                        let state = match img {
                            Some(i) => IconState::Ready(self.ctx.load_texture(
                                format!("thumb-{}", p.display()),
                                egui::ColorImage::from_rgba_premultiplied([i.width, i.height], &i.rgba),
                                egui::TextureOptions::LINEAR,
                            )),
                            None => IconState::Failed,
                        };
                        self.thumbs.insert(p, state);
                    }
                }
                Msg::Preview(p, data) => self.preview_loaded(p, data),
                Msg::Props(p, v) => {
                    if self.props.contains_key(&p) {
                        self.props.insert(p, Some(v));
                    }
                }
                Msg::Tree(dir, names, labels) => {
                    self.tree_pending.remove(&dir);
                    self.tree.insert(dir, names);
                    for (p, l) in labels {
                        ui_common::ensure_glyphs(&self.ctx, &l);
                        self.shell_labels.insert(p, l);
                    }
                }
                Msg::Icon(key, img) => {
                    let state = match img {
                        Some(i) => IconState::Ready(self.ctx.load_texture(
                            format!("icon-{key:?}"),
                            egui::ColorImage::from_rgba_unmultiplied([i.width, i.height], &i.rgba),
                            egui::TextureOptions::LINEAR,
                        )),
                        None => IconState::Failed,
                    };
                    self.icons.insert(key, state);
                }
                Msg::OpDone(desc, r, rec) => {
                    self.ops_running -= 1;
                    if let Some(u) = rec {
                        self.push_undo(u);
                    }
                    self.status = Some(match r {
                        // An undo/redo step: keep the "Undo …" message.
                        Ok(true) if desc.is_empty() => self.status.take().unwrap_or_default(),
                        Ok(true) => desc,
                        Ok(false) => "Cancelled".into(),
                        Err(e) => format!("Error: {e}"),
                    });
                    self.refresh();
                    self.load_drives();
                }
            }
        }
    }

    // ---- navigation ----------------------------------------------------------------------

    fn navigate(&mut self, to: PathBuf) {
        if to == self.cur {
            // As Explorer: the folder's own breadcrumb leaves search results for the folder.
            if self.deep_search.is_some() {
                return self.entered_new_folder(None);
            }
            return self.refresh();
        }
        let from = std::mem::replace(&mut self.cur, to);
        self.history.visit(from);
        self.entered_new_folder(None);
    }

    // ---- tabs ------------------------------------------------------------------------------

    fn snapshot(&mut self) -> TabSnap {
        TabSnap {
            cur: self.cur.clone(),
            history: std::mem::take(&mut self.history),
            selected: std::mem::take(&mut self.selected),
            filter: std::mem::take(&mut self.filter),
            sort: self.sort,
            ascending: self.ascending,
        }
    }

    fn load_tab(&mut self, t: TabSnap) {
        self.cur = t.cur;
        self.history = t.history;
        self.sort = t.sort;
        self.ascending = t.ascending;
        self.entered_new_folder(None);
        self.selected = t.selected;
        self.filter = t.filter;
    }

    pub fn switch_tab(&mut self, i: usize) {
        if i == self.active_tab || i >= self.tabs.len() {
            return;
        }
        let snap = self.snapshot();
        self.tabs[self.active_tab] = snap;
        let target = std::mem::replace(
            &mut self.tabs[i],
            TabSnap {
                cur: PathBuf::new(),
                history: History::default(),
                selected: HashSet::new(),
                filter: String::new(),
                sort: SortKey::Name,
                ascending: true,
            },
        );
        self.active_tab = i;
        self.load_tab(target);
        self.tabs[i].cur = self.cur.clone();
    }

    /// Opens `path` in a new tab after the current one; `focus` switches to it.
    pub fn new_tab(&mut self, path: PathBuf, focus: bool) {
        let t = TabSnap {
            cur: path,
            history: History::default(),
            selected: HashSet::new(),
            filter: String::new(),
            sort: self.sort,
            ascending: self.ascending,
        };
        let at = self.active_tab + 1;
        self.tabs.insert(at, t);
        if focus {
            // The active tab shifted nothing (inserted after it), so switching is safe.
            self.switch_tab(at);
        }
    }

    pub fn close_tab(&mut self, i: usize) {
        if self.tabs.len() <= 1 {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if i == self.active_tab {
            let next = if i + 1 < self.tabs.len() { i + 1 } else { i - 1 };
            self.switch_tab(next);
        }
        self.tabs.remove(i);
        if self.active_tab > i {
            self.active_tab -= 1;
        }
    }

    pub fn tab_title(&self, i: usize) -> String {
        let p = if i == self.active_tab { &self.cur } else { &self.tabs[i].cur };
        self.crumbs(p).last().map_or_else(|| p.to_string_lossy().into_owned(), |(l, _)| l.clone())
    }

    /// Breadcrumbs, with display names for shell items ("Galaxy S24 › Internal storage").
    pub fn crumbs(&self, p: &Path) -> Vec<(String, PathBuf)> {
        let mut c = explorer_core::location_crumbs(p);
        for (label, path) in &mut c {
            if let Some(l) = self.shell_labels.get(path) {
                label.clone_from(l);
            }
        }
        c
    }

    fn entered_new_folder(&mut self, select: Option<String>) {
        self.selected.clear();
        self.cursor = None;
        self.anchor = None;
        self.filter.clear();
        self.address_edit = None;
        self.pending_select = select;
        self.scroll_to = Some(0);
        self.changed_at = None;
        // Names for the breadcrumbs of a shell folder reached by typing or history.
        for (_, p) in explorer_core::location_crumbs(&self.cur) {
            if p.starts_with(explorer_core::SHELL)
                && !self.shell_labels.contains_key(&p)
                && let Some(n) = explorer_sys::shell_display_name(&p)
            {
                ui_common::ensure_glyphs(&self.ctx, &n);
                self.shell_labels.insert(p, n);
            }
        }
        self.thumbs.clear();
        self.thumb_wanted.clear();
        self.props.clear();
        self.start_listing(false);
        self.start_watch();
    }

    fn back(&mut self) {
        if let Some(p) = self.history.back(self.cur.clone()) {
            // Going back up to a parent selects the folder we were in.
            let child =
                (parent_path(&self.cur).as_deref() == Some(p.as_path())).then(|| self.name_in_parent(&p)).flatten();
            self.cur = p;
            self.entered_new_folder(child);
        }
    }

    /// Recent locations: several steps back or forward at once.
    pub fn jump_history(&mut self, steps: isize) {
        if let Some(p) = self.history.jump(self.cur.clone(), steps) {
            self.cur = p;
            self.entered_new_folder(None);
        }
    }

    fn forward(&mut self) {
        if let Some(p) = self.history.forward(self.cur.clone()) {
            self.cur = p;
            self.entered_new_folder(None);
        }
    }

    /// `\\server\share` for a UNC location, `\\server` for a server, else the path itself.
    pub fn share_root(&self) -> PathBuf {
        match classify(&self.cur) {
            Place::Unc { server, share } => PathBuf::from(format!(r"\\{server}\{share}")),
            Place::Server(server) => PathBuf::from(format!(r"\\{server}")),
            _ => self.cur.clone(),
        }
    }

    /// Asks Windows to sign in to the current share (its own credential dialog).
    pub fn sign_in(&mut self) {
        let root = self.share_root();
        // A server (share list) authenticates through its IPC$ share.
        let target = if matches!(classify(&self.cur), Place::Server(_)) { root.join("IPC$") } else { root.clone() };
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        thread::spawn(move || {
            let r = explorer_sys::connect(&target, true);
            let _ = tx.send(Msg::Connected(root, r));
            ctx.request_repaint();
        });
    }

    pub fn child(&self, name: &str) -> PathBuf {
        child_path(&self.cur, name)
    }

    /// The entry name the current folder has in `parent`'s listing, to select it after Up or
    /// Back: its last component, its full path where the parent names items by path (This
    /// PC, a phone's folders), the server name on the Network page.
    fn name_in_parent(&self, parent: &Path) -> Option<String> {
        match classify(&self.cur) {
            Place::Server(s) => Some(s),
            _ if self.cur.starts_with(explorer_core::SHELL) || classify(parent).names_are_paths() => {
                Some(self.cur.to_string_lossy().into_owned())
            }
            _ => Path::new(self.cur.to_string_lossy().trim_end_matches('\\'))
                .file_name()
                .map(|n| n.to_string_lossy().into_owned()),
        }
    }

    fn up(&mut self) {
        let Some(parent) = parent_path(&self.cur) else { return };
        let child = self.name_in_parent(&parent);
        let from = std::mem::replace(&mut self.cur, parent);
        self.history.visit(from);
        self.entered_new_folder(child);
    }

    fn refresh(&mut self) {
        // Folders may have been added or removed here; re-read this node of the tree too.
        if self.tree.remove(&self.cur).is_some() {
            let c = self.cur.clone();
            self.load_tree(c);
        }
        match self.deep_search.clone() {
            Some(q) => self.start_deep_search(q),
            None => self.start_listing(true),
        }
    }

    /// A page that isn't a real folder (Network, a server's shares, Recycle Bin, This PC):
    /// nothing can be created, renamed or pasted there.
    pub fn in_virtual_place(&self) -> bool {
        classify(&self.cur).is_page()
    }

    pub fn in_recycle_bin(&self) -> bool {
        classify(&self.cur) == Place::RecycleBin
    }

    fn bin_item(e: &Entry) -> explorer_sys::BinItem {
        let data = PathBuf::from(&e.name);
        let id = data.file_name().map(|n| n.to_string_lossy().replacen("$R", "$I", 1)).unwrap_or_default();
        explorer_sys::BinItem {
            original: Path::new(e.note.as_deref().unwrap_or("")).join(e.display()),
            deleted: e.modified,
            size: e.size,
            is_dir: e.is_dir,
            info: data.with_file_name(id),
            data,
        }
    }

    /// Restores the selected Recycle Bin items to where they were deleted from.
    pub fn restore_selection(&mut self) {
        let sel = self.selected_entries();
        let mut done = 0;
        for e in &sel {
            match explorer_sys::restore(&Self::bin_item(e)) {
                Ok(()) => done += 1,
                Err(err) => self.status = Some(format!("Could not restore {}: {err}", e.display())),
            }
        }
        if done > 0 {
            self.status = Some(format!("Restored {done} item(s)"));
        }
        self.refresh();
    }

    pub fn purge_selection(&mut self) {
        let sel = self.selected_entries();
        if sel.is_empty() {
            return;
        }
        self.dialog = Some(Dialog::ConfirmPurge(sel));
    }

    pub fn purge_items(&mut self, items: &[Entry]) {
        let mut done = 0;
        for e in items {
            match explorer_sys::purge(&Self::bin_item(e)) {
                Ok(()) => done += 1,
                Err(err) => self.status = Some(format!("Could not delete {}: {err}", e.display())),
            }
        }
        if done > 0 {
            self.status = Some(format!("Permanently deleted {done} item(s)"));
        }
        self.refresh();
    }

    pub fn empty_bin(&mut self) {
        if let Err(e) = explorer_sys::empty_recycle_bin(true) {
            self.status = Some(format!("Could not empty the Recycle Bin: {e}"));
        }
        if self.in_recycle_bin() {
            self.refresh();
        }
    }

    fn open_entry(&mut self, e: &Entry) {
        if self.in_recycle_bin() {
            return; // as in Explorer, items in the bin must be restored first
        }
        let path = self.child(&e.name);
        if e.is_dir {
            self.navigate(path);
        } else if path.starts_with(explorer_core::SHELL) {
            if let Err(err) = explorer_sys::shell_open(self.hwnd, &path) {
                self.status = Some(format!("Could not open {}: {err}", e.display()));
            }
        } else if let Err(err) = explorer_sys::open(&path) {
            self.status = Some(format!("Could not open {}: {err}", e.name));
        }
    }

    /// What the address box holds when editing starts: the path, or for a phone's folders
    /// the names the breadcrumbs show ("This PC\Galaxy S24\Internal storage\DCIM"), as
    /// Explorer does. `submit_address` reads both forms back.
    pub fn address_text(&self) -> String {
        if matches!(classify(&self.cur), Place::Shell(_)) {
            let crumbs = self.crumbs(&self.cur);
            // Only when every level has a name (raw IDs contain backslashes themselves).
            if crumbs.iter().all(|(l, _)| !l.is_empty() && !l.contains('\\')) {
                return crumbs.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>().join("\\");
            }
        }
        self.cur.to_string_lossy().into_owned()
    }

    /// The location behind a path typed with display names: "This PC\Galaxy S24\DCIM",
    /// "This PC\Windows (C:)\Users", "Control Panel\…". `None` if it isn't one, or a
    /// level isn't there.
    fn resolve_display_path(&mut self, text: &str) -> Option<PathBuf> {
        let mut parts = text.split(['\\', '/']).filter(|s| !s.is_empty());
        let root = parts.next()?;
        let mut rest: Vec<&str> = parts.collect();
        if rest.is_empty() {
            return None;
        }
        let mut cur = if root.eq_ignore_ascii_case("this pc") {
            PathBuf::from(explorer_core::THIS_PC)
        } else if root.eq_ignore_ascii_case("control panel") {
            explorer_core::shell_path(explorer_core::CONTROL_PANEL_CLSID)
        } else {
            return None;
        };
        while !rest.is_empty() {
            let seg = rest.remove(0);
            if classify(&cur) == Place::ThisPc {
                // A drive by its name or letter: the rest is an ordinary path under it.
                let drive = self.drives.iter().find(|d| {
                    let root = d.root.to_string_lossy();
                    view::drive_label(d).eq_ignore_ascii_case(seg)
                        || root.trim_end_matches('\\').eq_ignore_ascii_case(seg.trim_end_matches('\\'))
                });
                if let Some(d) = drive {
                    return Some(rest.iter().fold(d.root.clone(), |p, s| p.join(s)));
                }
            }
            let known = self
                .shell_labels
                .iter()
                .find(|(p, l)| l.eq_ignore_ascii_case(seg) && parent_path(p).as_deref() == Some(cur.as_path()))
                .map(|(p, _)| p.clone());
            cur = match known {
                Some(p) => p,
                None => {
                    // Not seen yet: ask the shell for this level's items.
                    let folder = if classify(&cur) == Place::ThisPc {
                        PathBuf::from(explorer_core::THIS_PC_CLSID)
                    } else {
                        cur.clone()
                    };
                    let mut found = None;
                    let _ = explorer_sys::shell_list(&folder, &AtomicBool::new(false), &mut |b| {
                        for e in b {
                            if let Some(l) = &e.label {
                                if found.is_none() && l.eq_ignore_ascii_case(seg) {
                                    found = Some(PathBuf::from(&e.name));
                                }
                                if e.name.starts_with(explorer_core::SHELL) {
                                    self.shell_labels.insert(PathBuf::from(&e.name), l.clone());
                                }
                            }
                        }
                    });
                    match found {
                        // A real folder (a drive the shell lists): the rest is a plain path.
                        Some(p) if !p.starts_with(explorer_core::SHELL) => {
                            return Some(rest.iter().fold(p, |p, s| p.join(s)));
                        }
                        Some(p) => p,
                        None => return None,
                    }
                }
            };
        }
        Some(cur)
    }

    fn submit_address(&mut self, text: &str) {
        let t = text.trim().trim_matches('"');
        if t.is_empty() {
            return;
        }
        if let Some(p) = self.resolve_display_path(t) {
            return self.go_or_open(p);
        }
        let env = |k: &str| std::env::var(k).ok();
        // shell:startup, shell:sendto, …; redirected known folders come from the system.
        let lower = t.to_ascii_lowercase();
        let t = if let Some(name) = lower.strip_prefix("shell:") {
            let (name, rest) = name.split_once(['\\', '/']).map_or((name, ""), |(n, r)| (n, r));
            let known = self.places.iter().find(|(n, _)| n.eq_ignore_ascii_case(name.trim())).map(|(_, p)| p.clone());
            let base = match known {
                Some(p) => Some(p.to_string_lossy().into_owned()),
                None => explorer_core::shell_folder_template(name).map(|tpl| explorer_core::expand_env(tpl, env)),
            };
            match base {
                Some(b) if rest.is_empty() => b,
                // Keep the user's spelling of the part after the folder name.
                Some(b) => format!("{b}\\{}", &t[t.len() - rest.len()..]),
                None => {
                    self.status = Some(format!("Unknown shell folder '{name}'"));
                    return;
                }
            }
        } else {
            explorer_core::expand_env(t, env)
        };
        let t = t.as_str();
        let p = PathBuf::from(t);
        let p = if t.eq_ignore_ascii_case("network") { PathBuf::from(explorer_core::NETWORK) } else { p };
        let p = if t.eq_ignore_ascii_case("this pc") { PathBuf::from(explorer_core::THIS_PC) } else { p };
        let p = if t.eq_ignore_ascii_case("recycle bin") { PathBuf::from(explorer_core::RECYCLE_BIN) } else { p };
        // A shell parsing name as copied from elsewhere ("::{20D04FE0-…}\\?\usb#…").
        let p = if t.starts_with("::{") { explorer_core::shell_path(t) } else { p };
        // A relative path is under the current folder; a phone's folder has no paths under it.
        let p = if p.is_relative() && classify(&p) == Place::Local && !classify(&self.cur).is_page() {
            self.cur.join(p)
        } else {
            p
        };
        if matches!(classify(&p), Place::Shell(_)) {
            return self.go_or_open(p);
        }
        if matches!(classify(&p), Place::Network | Place::Server(_)) || p.is_dir() {
            self.navigate(p);
        } else if p.exists() {
            if let Err(e) = explorer_sys::open(&p) {
                self.status = Some(format!("Could not open {}: {e}", p.display()));
            }
        } else {
            self.status = Some(format!("Can't find '{t}'. Check the spelling and try again."));
        }
    }

    /// Goes into a folder (a phone's too) or opens a file.
    fn go_or_open(&mut self, p: PathBuf) {
        if matches!(classify(&p), Place::Shell(_)) {
            if explorer_sys::shell_is_folder(&p) {
                self.navigate(p);
            } else if let Err(e) = explorer_sys::shell_open(self.hwnd, &p) {
                self.status = Some(format!("Can't find '{}': {e}", p.display()));
            }
        } else if classify(&p) != Place::Local || p.is_dir() {
            self.navigate(p);
        } else if p.exists() {
            if let Err(e) = explorer_sys::open(&p) {
                self.status = Some(format!("Could not open {}: {e}", p.display()));
            }
        } else {
            self.status = Some(format!("Can't find '{}'. Check the spelling and try again.", p.display()));
        }
    }

    // ---- view & selection ----------------------------------------------------------------

    pub fn update_view(&mut self) {
        let key = ViewKey {
            version: self.version,
            filter: self.filter.clone(),
            sort: self.sort,
            ascending: self.ascending,
            show_hidden: self.show_hidden && !self.show_protected,
            group: (self.group_by, self.group_ascending),
        };
        if self.view_key.as_ref() == Some(&key) {
            return;
        }
        // Search results already match the query (which may be `kind:…`, not a name).
        let searched = self.deep_search.as_deref().is_some_and(|q| q == self.filter.trim());
        self.view = explorer_core::sorted_view_with(
            &self.entries,
            if searched { "" } else { &self.filter },
            self.show_hidden,
            self.show_protected,
            self.sort,
            self.ascending,
        );
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        self.groups = explorer_core::group_view(
            &self.entries,
            &mut self.view,
            self.group_by,
            self.group_ascending,
            now,
            self.utc_offset,
        );
        self.view_key = Some(key);
        if let Some(name) = self.pending_select.clone()
            && let Some(pos) = self.view.iter().position(|&i| self.entries[i].name == name)
        {
            self.pending_select = None;
            self.select_only(pos);
            self.scroll_to = Some(pos);
        }
        // Keep the cursor on the same item after a re-sort or refresh.
        self.cursor =
            self.cursor.and_then(|_| self.view.iter().position(|&i| self.selected.contains(&self.entries[i].name)));
    }

    fn entry_at(&self, pos: usize) -> Option<&Entry> {
        self.view.get(pos).map(|&i| &self.entries[i])
    }

    fn select_only(&mut self, pos: usize) {
        self.selected.clear();
        if let Some(e) = self.entry_at(pos) {
            self.selected.insert(e.name.clone());
        }
        self.cursor = Some(pos);
        self.anchor = Some(pos);
    }

    fn select_range(&mut self, pos: usize) {
        let a = self.anchor.unwrap_or(pos);
        self.selected.clear();
        for p in a.min(pos)..=a.max(pos) {
            if let Some(e) = self.entry_at(p) {
                self.selected.insert(e.name.clone());
            }
        }
        self.cursor = Some(pos);
    }

    fn toggle(&mut self, pos: usize) {
        if let Some(name) = self.entry_at(pos).map(|e| e.name.clone())
            && !self.selected.remove(&name)
        {
            self.selected.insert(name);
        }
        self.cursor = Some(pos);
        self.anchor = Some(pos);
    }

    /// Selected paths in view order.
    fn selected_paths(&self) -> Vec<PathBuf> {
        self.view
            .iter()
            .map(|&i| &self.entries[i])
            .filter(|e| self.selected.contains(&e.name))
            .map(|e| self.child(&e.name))
            .collect()
    }

    fn selected_entries(&self) -> Vec<Entry> {
        self.view.iter().map(|&i| &self.entries[i]).filter(|e| self.selected.contains(&e.name)).cloned().collect()
    }

    // ---- commands ------------------------------------------------------------------------

    /// In a shell namespace folder (a phone) the shell does copy, paste and delete itself.
    fn in_shell_folder(&self) -> bool {
        matches!(classify(&self.cur), Place::Shell(_))
    }

    fn copy_selection(&mut self, cut: bool) {
        if self.in_recycle_bin() {
            return;
        }
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        if self.in_shell_folder() || paths.iter().any(|p| p.starts_with(explorer_core::SHELL)) {
            // The shell's data object: files on a phone can then be pasted anywhere.
            if let Err(e) = explorer_sys::shell_verb(self.hwnd, &self.cur, &paths, if cut { "cut" } else { "copy" }) {
                self.status = Some(format!("Clipboard error: {e}"));
            } else {
                self.status = Some(format!("{} {} item(s)", if cut { "Cut" } else { "Copied" }, paths.len()));
            }
            return;
        }
        self.status = Some(match explorer_sys::clipboard_set_files(&paths, cut) {
            Ok(()) => format!("{} {} item(s)", if cut { "Cut" } else { "Copied" }, paths.len()),
            Err(e) => format!("Clipboard error: {e}"),
        });
    }

    /// Runs a shell copy, move or delete (to or from a phone) off the UI thread; the shell
    /// shows its own progress and confirmation dialogs.
    fn run_shell_op(&mut self, desc: String, f: impl FnOnce(isize) -> std::io::Result<bool> + Send + 'static) {
        self.ops_running += 1;
        let (tx, ctx, hwnd) = (self.tx.clone(), self.ctx.clone(), self.hwnd);
        thread::Builder::new()
            .name("shell-op".into())
            .spawn(move || {
                let r = f(hwnd);
                let _ = tx.send(Msg::OpDone(desc, r, None));
                ctx.request_repaint();
            })
            .expect("spawn shell-op thread");
    }

    fn paste_into(&mut self, dest: PathBuf) {
        if matches!(classify(&dest), Place::Shell(_)) {
            // Into a phone folder: the shell's copy engine talks MTP (the folder's own Paste
            // verb does nothing there).
            return self.run_shell_op("Pasted".into(), move |hwnd| explorer_sys::shell_paste(hwnd, &dest));
        }
        if classify(&dest).is_page() {
            return;
        }
        let Some((files, cut)) = explorer_sys::clipboard_get_files() else {
            // Not files on disk (copied from a phone, say): the shell copies any data object
            // Explorer itself could paste.
            if cfg!(windows) {
                return self.run_shell_op("Pasted".into(), move |hwnd| explorer_sys::shell_paste(hwnd, &dest));
            }
            self.status = Some("Nothing to paste".into());
            return;
        };
        if files.is_empty() {
            return;
        }
        if cut {
            // A cut is used up by the paste, as in Explorer.
            let _ = explorer_sys::clipboard_set_files(&[], false);
        }
        self.run_op(if cut { FileOp::Move } else { FileOp::Copy }, files, Some(dest));
    }

    fn delete_selection(&mut self, permanently: bool) {
        if self.in_recycle_bin() {
            return self.purge_selection();
        }
        let paths = self.selected_paths();
        if self.in_shell_folder() {
            if paths.is_empty() {
                return;
            }
            let desc = format!("Deleted {} item(s)", paths.len());
            return self.run_shell_op(desc, move |hwnd| explorer_sys::shell_delete(hwnd, &paths));
        }
        self.run_op(if permanently { FileOp::Delete } else { FileOp::Recycle }, paths, None);
    }

    pub fn begin_rename(&mut self) {
        if self.in_virtual_place() {
            return;
        }
        // In view order, so the numbering follows what the user sees.
        let sel: Vec<String> =
            self.view.iter().map(|&i| &self.entries[i].name).filter(|n| self.selected.contains(*n)).cloned().collect();
        if let Some(first) = sel.first() {
            // In search results `name` is a relative path; only its last part is edited.
            let leaf = Path::new(first).file_name().map_or_else(|| first.clone(), |n| n.to_string_lossy().into_owned());
            self.dialog = Some(Dialog::Rename {
                original: first.clone(),
                others: sel[1..].to_vec(),
                text: leaf,
                error: None,
                opened: false,
            });
        }
    }

    pub fn begin_map_drive(&mut self) {
        let letters = explorer_sys::free_drive_letters();
        let Some(&letter) = letters.last() else {
            self.status = Some("No free drive letters".into());
            return;
        };
        // Pre-fill with the share being browsed, as Explorer does.
        let path = match classify(&self.cur) {
            Place::Unc { .. } => self.share_root().to_string_lossy().into_owned(),
            _ => r"\\".to_string(),
        };
        self.dialog = Some(Dialog::MapDrive(MapDrive { letters, letter, path, persistent: true }));
    }

    pub fn finish_map_drive(&mut self, m: &MapDrive) {
        let (letter, remote, persistent) = (m.letter, PathBuf::from(m.path.trim()), m.persistent);
        self.ops_running += 1;
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        thread::spawn(move || {
            let r = explorer_sys::map_drive(letter, &remote, persistent, true).map(|()| true);
            let _ = tx.send(Msg::OpDone(format!("Mapped {letter}: to {}", remote.display()), r, None));
            ctx.request_repaint();
        });
    }

    /// The shell's right-click menu for the selection (`for_items`) or for the folder itself.
    pub fn shell_menu(&mut self, for_items: bool, extended: bool) {
        let items = if for_items { self.selected_paths() } else { Vec::new() };
        match explorer_sys::shell_context_menu(self.hwnd, &self.cur, &items, extended) {
            Ok(explorer_sys::MenuOutcome::Host(verb)) if verb == "rename" => self.begin_rename(),
            Ok(explorer_sys::MenuOutcome::Host(_)) => {
                if let Some(p) = items.first() {
                    self.navigate(p.clone());
                }
            }
            // The shell's own "Pin to Quick access" pins in Windows Explorer; mirror it here.
            Ok(explorer_sys::MenuOutcome::Invoked(verb)) if verb == "pintohome" || verb == "unpinfromhome" => {
                for p in items.iter().filter(|p| p.is_dir()) {
                    if verb == "pintohome" { self.pin(p.clone()) } else { self.unpin(p) }
                }
                if items.is_empty() {
                    let c = self.cur.clone();
                    if verb == "pintohome" { self.pin(c) } else { self.unpin(&c) }
                }
            }
            Ok(explorer_sys::MenuOutcome::Invoked(verb)) => {
                // The shell changed something (new file, paste, delete…); the watcher sees most
                // of it, this catches the rest (e.g. drives, network).
                if !verb.is_empty() {
                    self.changed_at.get_or_insert_with(Instant::now);
                }
            }
            Ok(explorer_sys::MenuOutcome::Dismissed) => {}
            Err(e) => self.status = Some(format!("Menu: {e}")),
        }
    }

    pub fn unmap(&mut self, letter: char) {
        self.status = Some(match explorer_sys::unmap_drive(letter) {
            Ok(()) => format!("Disconnected {letter}:"),
            Err(e) => format!("Could not disconnect {letter}: {e}"),
        });
        self.load_drives();
    }

    pub fn begin_new_folder(&mut self) {
        if self.in_virtual_place() {
            return;
        }
        self.dialog =
            Some(Dialog::NewFolder { text: unique_name("New folder", &self.entries), error: None, opened: false });
    }

    /// Returns an error to show in the dialog, or `None` once done.
    fn finish_dialog(&mut self, dialog: &Dialog) -> Option<&'static str> {
        match dialog {
            Dialog::Rename { original, others, text, .. } if !others.is_empty() => {
                let base = text.trim_end();
                if let Err(e) = validate_name(base) {
                    return Some(e);
                }
                let names: Vec<String> = std::iter::once(original).chain(others).cloned().collect();
                let items: Vec<(String, bool)> =
                    names.iter().map(|n| (n.clone(), self.entries.iter().any(|e| &e.name == n && e.is_dir))).collect();
                let new_names = explorer_core::batch_names(base, &items);
                let mut done = Vec::new();
                let mut failed = 0;
                for (old, new) in names.iter().zip(&new_names) {
                    let from = self.cur.join(old);
                    let to = from.with_file_name(new);
                    if to.exists() || std::fs::rename(&from, &to).is_err() {
                        failed += 1;
                    } else {
                        done.push((from, to));
                    }
                }
                self.selected = done
                    .iter()
                    .filter_map(|(_, to)| to.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .collect();
                if failed > 0 {
                    self.status = Some(format!("Renamed {}, {failed} could not be renamed", done.len()));
                }
                if !done.is_empty() {
                    self.push_undo(undo::Undoable::Renames(done));
                }
                self.refresh();
                None
            }
            Dialog::Rename { original, text, .. } => {
                let new = text.trim_end();
                let from = self.cur.join(original);
                let old_leaf = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if new == old_leaf {
                    return None;
                }
                if let Err(e) = validate_name(new) {
                    return Some(e);
                }
                let to = from.with_file_name(new);
                // std::fs::rename replaces an existing target; refuse instead (a case-only
                // change of the same item is fine).
                if to.exists() && !new.eq_ignore_ascii_case(&old_leaf) {
                    return Some("There is already an item with that name");
                }
                match std::fs::rename(&from, &to) {
                    Ok(()) => {
                        self.push_undo(undo::Undoable::Rename { from: from.clone(), to: to.clone() });
                        self.selected.clear();
                        let parent = Path::new(original).parent().unwrap_or(Path::new(""));
                        self.pending_select = Some(parent.join(new).to_string_lossy().into_owned());
                        self.refresh();
                    }
                    Err(e) => self.status = Some(format!("Could not rename: {e}")),
                }
                None
            }
            Dialog::MapDrive(_) | Dialog::ConfirmPurge(_) => None,
            Dialog::NewFolder { text, .. } => {
                let name = text.trim_end();
                if let Err(e) = validate_name(name) {
                    return Some(e);
                }
                if self.entries.iter().any(|e| e.name.eq_ignore_ascii_case(name)) {
                    return Some("There is already an item with that name");
                }
                match std::fs::create_dir(self.cur.join(name)) {
                    Ok(()) => {
                        self.push_undo(undo::Undoable::NewFolder(self.cur.join(name)));
                        self.selected.clear();
                        self.pending_select = Some(name.to_string());
                        self.refresh();
                    }
                    Err(e) => self.status = Some(format!("Could not create folder: {e}")),
                }
                None
            }
        }
    }

    pub fn invert_selection(&mut self) {
        self.selected =
            self.view.iter().map(|&i| &self.entries[i].name).filter(|n| !self.selected.contains(*n)).cloned().collect();
    }

    /// Ctrl+N: another window on the same folder.
    pub fn new_window(&mut self) {
        let cur = self.cur.clone();
        self.open_window(&cur);
    }

    fn open_window(&mut self, folder: &Path) -> bool {
        let r = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg(folder).spawn());
        if let Err(e) = &r {
            self.status = Some(format!("Could not open a new window: {e}"));
        }
        r.is_ok()
    }

    /// Moves tab `from` to position `to` (dragging tabs along the bar).
    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let t = self.tabs.remove(from);
        self.tabs.insert(to, t);
        let a = self.active_tab;
        self.active_tab = if a == from {
            to
        } else if from < a && to >= a {
            a - 1
        } else if from > a && to <= a {
            a + 1
        } else {
            a
        };
    }

    /// A tab dragged out of the bar: its folder opens in a new window and the tab closes.
    pub fn tear_off_tab(&mut self, i: usize) {
        let folder = if i == self.active_tab { self.cur.clone() } else { self.tabs[i].cur.clone() };
        if self.open_window(&folder) {
            self.close_tab(i);
        }
    }

    pub fn show_properties(&mut self, path: &Path) {
        if let Err(e) = explorer_sys::properties(path) {
            self.status = Some(format!("Properties: {e}"));
        }
    }

    fn set_sort(&mut self, key: SortKey) {
        if self.sort == key {
            self.ascending = !self.ascending;
        } else {
            self.sort = key;
            // Names and types read best A→Z; dates and sizes newest/biggest first.
            self.ascending = matches!(key, SortKey::Name | SortKey::Type | SortKey::Attributes);
        }
    }

    /// Keyboard shortcuts that apply when no text field has focus.
    fn handle_keys(&mut self, ui: &mut egui::Ui) {
        use egui::{Key, Modifiers};
        let text_focused = ui.ctx().memory(|m| m.focused().is_some());
        let alt = Modifiers::ALT;
        let ctrl = Modifiers::COMMAND;

        // These work everywhere, even while typing in the search box.
        let keys = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8];
        if let Some(k) = ui.input_mut(|i| keys.iter().position(|&k| i.consume_key(ctrl | Modifiers::SHIFT, k))) {
            self.view_mode = ViewMode::ALL[k].0;
        }
        // Shift first: egui matches Alt+P "logically", so it would also eat Alt+Shift+P.
        let (details_pane, preview_pane) =
            ui.input_mut(|i| (i.consume_key(alt | Modifiers::SHIFT, Key::P), i.consume_key(alt, Key::P)));
        if details_pane {
            self.toggle_pane(pane::Pane::Details);
        } else if preview_pane {
            self.toggle_pane(pane::Pane::Preview);
        }
        let (new_tab, close_tab, prev_tab, next_tab) = ui.input_mut(|i| {
            (
                i.consume_key(ctrl, Key::T),
                i.consume_key(ctrl, Key::W),
                i.consume_key(ctrl | Modifiers::SHIFT, Key::Tab),
                i.consume_key(ctrl, Key::Tab),
            )
        });
        if new_tab {
            let home = PathBuf::from(explorer_core::HOME);
            self.new_tab(home, true);
        }
        if close_tab {
            self.close_tab(self.active_tab);
        }
        if next_tab || prev_tab {
            let n = self.tabs.len();
            let to = if next_tab { (self.active_tab + 1) % n } else { (self.active_tab + n - 1) % n };
            self.switch_tab(to);
        }
        let (back, fwd, up, f5, addr, find, newdir) = ui.input_mut(|i| {
            (
                i.consume_key(alt, Key::ArrowLeft) || i.pointer.button_pressed(egui::PointerButton::Extra1),
                i.consume_key(alt, Key::ArrowRight) || i.pointer.button_pressed(egui::PointerButton::Extra2),
                i.consume_key(alt, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::F5),
                i.consume_key(ctrl, Key::L) || i.consume_key(alt, Key::D),
                i.consume_key(ctrl, Key::F) || i.consume_key(Modifiers::NONE, Key::F3),
                i.consume_key(ctrl | Modifiers::SHIFT, Key::N),
            )
        });
        if back {
            self.back();
        }
        if fwd {
            self.forward();
        }
        if up {
            self.up();
        }
        if f5 {
            self.refresh();
            self.load_drives();
        }
        if addr {
            self.address_edit = Some(self.address_text());
        }
        if find {
            self.focus_search = true;
        }
        if newdir {
            self.begin_new_folder();
        }
        let ctrl_v = explorer_sys::ctrl_v_down();
        let paste_pressed = ctrl_v && !self.ctrl_v_was_down;
        self.ctrl_v_was_down = ctrl_v;
        if text_focused {
            return;
        }
        if paste_pressed {
            let c = self.cur.clone();
            self.paste_into(c);
        }

        let n = self.view.len();
        let mut moved: Option<(usize, bool)> = None; // (new position, extend selection)
        let mut events = Vec::new();
        ui.input_mut(|i| {
            let shift = i.modifiers.shift;
            for (key, step) in [(Key::ArrowDown, 1isize), (Key::ArrowUp, -1), (Key::PageDown, 20), (Key::PageUp, -20)] {
                if i.key_pressed(key) && n > 0 && !i.modifiers.alt {
                    let c = self.cursor.map_or(if step > 0 { -1 } else { n as isize }, |c| c as isize);
                    moved = Some(((c + step).clamp(0, n as isize - 1) as usize, shift));
                }
            }
            if i.key_pressed(Key::Home) && n > 0 {
                moved = Some((0, shift));
            }
            if i.key_pressed(Key::End) && n > 0 {
                moved = Some((n - 1, shift));
            }
            for (m, k, tag) in [
                (Modifiers::NONE, Key::Enter, "open"),
                (Modifiers::NONE, Key::Backspace, "back"),
                (Modifiers::NONE, Key::F2, "rename"),
                (Modifiers::SHIFT, Key::Delete, "nuke"),
                (Modifiers::NONE, Key::Delete, "delete"),
                (ctrl, Key::A, "all"),
                (ctrl, Key::N, "newwin"),
                (ctrl | Modifiers::SHIFT, Key::Z, "redo"),
                (ctrl, Key::Z, "undo"),
                (ctrl, Key::Y, "redo"),
                (Modifiers::ALT, Key::Enter, "props"),
                (Modifiers::NONE, Key::Escape, "none"),
            ] {
                if i.consume_key(m, k) {
                    events.push(tag);
                }
            }
            // Copy/cut/paste arrive as events, not key presses, in egui.
            for ev in &i.events {
                match ev {
                    egui::Event::Copy => events.push("copy"),
                    egui::Event::Cut => events.push("cut"),
                    // On Windows Ctrl+V is read directly (see `ctrl_v_down`), so ignore egui's
                    // text paste there to avoid pasting twice.
                    egui::Event::Paste(_) if !cfg!(windows) => events.push("paste"),
                    egui::Event::Text(t) if !i.modifiers.command => {
                        let (buf, at) = &mut self.type_ahead;
                        if at.elapsed() > Duration::from_millis(900) {
                            buf.clear();
                        }
                        buf.push_str(t);
                        *at = Instant::now();
                        events.push("typeahead");
                    }
                    _ => {}
                }
            }
        });
        if let Some((pos, extend)) = moved {
            if extend {
                self.select_range(pos)
            } else {
                self.select_only(pos)
            }
            self.scroll_to = Some(pos);
        }
        for ev in events {
            match ev {
                "open" => {
                    let sel = self.selected_entries();
                    if let [e] = sel.as_slice() {
                        self.open_entry(e);
                    } else {
                        for e in sel.iter().filter(|e| !e.is_dir) {
                            let _ = explorer_sys::open(&self.child(&e.name));
                        }
                    }
                }
                "back" => self.back(),
                "rename" => self.begin_rename(),
                "delete" => self.delete_selection(false),
                "nuke" => self.delete_selection(true),
                "all" => {
                    self.selected = self.view.iter().map(|&i| self.entries[i].name.clone()).collect();
                }
                "newwin" => self.new_window(),
                "undo" => self.undo(),
                "redo" => self.redo(),
                "props" => match self.selected_paths().as_slice() {
                    [p] => {
                        let p = p.clone();
                        self.show_properties(&p);
                    }
                    _ => {
                        let c = self.cur.clone();
                        self.show_properties(&c);
                    }
                },
                "none" => {
                    self.selected.clear();
                    self.cursor = None;
                }
                "copy" => self.copy_selection(false),
                "cut" => self.copy_selection(true),
                "paste" => {
                    let c = self.cur.clone();
                    self.paste_into(c);
                }
                "typeahead" => {
                    let prefix = self.type_ahead.0.to_lowercase();
                    let start = self.cursor.map_or(0, |c| if prefix.chars().count() == 1 { c + 1 } else { c });
                    let hit = (0..n)
                        .map(|k| (start + k) % n.max(1))
                        .find(|&p| self.entry_at(p).is_some_and(|e| e.display().to_lowercase().starts_with(&prefix)));
                    if let Some(p) = hit {
                        self.select_only(p);
                        self.scroll_to = Some(p);
                    }
                }
                _ => {}
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if self.hwnd == 0 {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(h) = frame.window_handle()
                && let RawWindowHandle::Win32(w) = h.as_raw()
            {
                self.hwnd = w.hwnd.get();
            }
        }
        self.drain();
        // Coming back to the window: pick up changes made elsewhere (cheap: one listing).
        let focused = ui.input(|i| i.viewport().focused.unwrap_or(true));
        if focused && !self.was_focused && !self.loading {
            self.refresh();
        }
        self.was_focused = focused;

        if let Some(t) = self.changed_at {
            let settle = Duration::from_millis(250);
            if t.elapsed() >= settle && !self.loading {
                self.changed_at = None;
                self.refresh();
            } else {
                ui.ctx().request_repaint_after(settle);
            }
        }

        // Files dropped on the window from another program: move within a drive, copy across
        // drives, as Explorer does.
        let dropped: Vec<PathBuf> = ui.input(|i| {
            i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect()
        });
        if !dropped.is_empty() {
            let mods = ui.input(|i| i.modifiers);
            let target = self.drop_target.clone().unwrap_or_else(|| self.cur.clone());
            self.drop_into(dropped, target, mods);
        }

        if self.dialog.is_none() {
            self.handle_keys(ui);
        }
        self.update_view();

        if let Some(t) = self.tabs.get_mut(self.active_tab) {
            t.cur.clone_from(&self.cur);
        }
        // The folder's name as in the breadcrumbs: "This PC", a phone's "Internal storage",
        // not the parsing name behind it.
        let title = match self.cur.file_name() {
            Some(n) if !classify(&self.cur).is_page() => n.to_string_lossy().into_owned(),
            _ => self.tab_title(self.active_tab),
        };
        if title != self.title {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Title(format!("{title} - File Explorer (oxidows)")));
            self.title = title;
        }

        let ctx = ui.ctx().clone();
        self.persist_tick(&ctx);
        self.drop_target = None;
        view::tab_bar(self, ui);
        view::toolbar(self, ui);
        view::status_bar(self, ui);
        view::sidebar(self, ui);
        pane::show(self, ui);
        let content = ui_common::content_frame(ui.style());
        egui::CentralPanel::default().frame(content).show(ui, |ui| view::file_list(self, ui));
        view::dialog(self, ui);
        nearby::show(self, ui);
        self.finish_drag(&ctx);
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        if std::mem::take(&mut self.release_pointer) {
            // The button went up inside the system drag loop; tell egui so it stops dragging.
            raw.events.push(egui::Event::PointerButton {
                pos: egui::Pos2::ZERO,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            });
            raw.events.push(egui::Event::PointerGone);
        }
    }
}

/// The newest `max` files in Windows' Recent folder (shortcuts to what was opened), resolved
/// to their targets; missing targets and folders are skipped.
/// Most items the Gallery collects.
const GALLERY_MAX: usize = 50_000;

/// Pictures and videos under `roots` (all subfolders), named by full path.
fn gallery(roots: &[PathBuf], cancel: &AtomicBool, sink: &mut dyn FnMut(Vec<Entry>)) {
    use explorer_core::query::Kind;
    let mut queue: std::collections::VecDeque<PathBuf> = roots.iter().cloned().collect();
    let mut count = 0;
    while let Some(dir) = queue.pop_front() {
        if cancel.load(Ordering::Relaxed) || count >= GALLERY_MAX {
            return;
        }
        let mut out = Vec::new();
        let _ = explorer_sys::list_dir(&dir, cancel, &mut |batch| {
            for mut e in batch {
                if e.is_dir {
                    if !e.link && !e.hidden {
                        queue.push_back(dir.join(&e.name));
                    }
                } else if matches!(Kind::of(&e), Some(Kind::Picture | Kind::Video)) && !e.hidden {
                    e.label = Some(e.name.clone());
                    e.name = dir.join(&e.name).to_string_lossy().into_owned();
                    out.push(e);
                }
            }
        });
        count += out.len();
        if !out.is_empty() {
            sink(out);
        }
    }
}

/// Search results on their way to the UI, sent in batches at most every 100 ms.
struct SearchOut<'a> {
    found: Vec<Entry>,
    last_send: Instant,
    tx: &'a Sender<Msg>,
    ctx: &'a egui::Context,
    generation: u64,
}

impl SearchOut<'_> {
    fn push(&mut self, e: Entry) {
        self.found.push(e);
        if self.found.len() >= explorer_sys::BATCH || self.last_send.elapsed().as_millis() > 100 {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if !self.found.is_empty() {
            let _ = self.tx.send(Msg::Batch(self.generation, std::mem::take(&mut self.found)));
            self.ctx.request_repaint();
        }
        self.last_send = Instant::now();
    }
}

/// Files bigger than this aren't read for `content:`.
const CONTENT_LIMIT: u64 = 32 << 20;

/// The search without Windows Search: walks `root` breadth first. Unreadable folders (access
/// denied) are skipped, as Explorer does.
fn walk_search(root: &Path, query: &str, offset: i64, cancel: &AtomicBool, out: &mut SearchOut) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let q = explorer_core::Query::parse(query, (now + offset).div_euclid(86_400));
    let contains = |path: &Path, needle: &str| {
        std::fs::metadata(path).is_ok_and(|m| m.len() <= CONTENT_LIMIT)
            && std::fs::read(path)
                .ok()
                .and_then(|b| explorer_core::decode_text(&b))
                .is_some_and(|t| t.to_lowercase().contains(needle))
    };
    let mut queue = std::collections::VecDeque::from([PathBuf::new()]);
    while let Some(rel) = queue.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let mut hits = Vec::new();
        let _ = explorer_sys::list_dir(&root.join(&rel), cancel, &mut |batch| {
            for e in batch {
                let rel_name = rel.join(&e.name);
                if e.is_dir && !e.link {
                    queue.push_back(rel_name.clone());
                }
                if q.matches(&e, offset) {
                    hits.push((rel_name, e));
                }
            }
        });
        for (rel_name, mut e) in hits {
            if let Some(needle) = &q.content
                && !contains(&root.join(&rel_name), needle)
            {
                continue;
            }
            e.name = rel_name.to_string_lossy().into_owned();
            out.push(e);
        }
    }
}

fn recent_files(max: usize) -> Vec<Entry> {
    let Some(dir) = explorer_core::shell_folder_template("recent")
        .map(|t| PathBuf::from(explorer_core::expand_env(t, |k| std::env::var(k).ok())))
    else {
        return Vec::new();
    };
    let mut links: Vec<(i64, PathBuf)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|d| d.path().extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")))
        .map(|d| {
            let t = d
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            (t, d.path())
        })
        .collect();
    links.sort_by_key(|l| std::cmp::Reverse(l.0));
    let mut seen = HashSet::new();
    links
        .into_iter()
        .filter_map(|(t, lnk)| {
            let target = explorer_sys::resolve_link(&lnk)?;
            let md = std::fs::metadata(&target).ok().filter(|m| m.is_file())?;
            seen.insert(target.clone()).then(|| Entry {
                name: target.to_string_lossy().into_owned(),
                label: target.file_name().map(|n| n.to_string_lossy().into_owned()),
                note: target.parent().map(|p| p.to_string_lossy().into_owned()),
                size: md.len(),
                modified: t,
                ..Default::default()
            })
        })
        .take(max)
        .collect()
}

fn main() -> eframe::Result {
    // OpenGL where there is a GPU driver, wgpu (WARP) otherwise; see `ui_common::choose_renderer`.
    let (renderer, auto) = ui_common::choose_renderer();
    let r = run(renderer);
    // Only OpenGL start-up failures: an error from the app itself would fail the same way again.
    let gl_failed =
        matches!(r, Err(eframe::Error::Glutin(_) | eframe::Error::NoGlutinConfigs(..) | eframe::Error::OpenGL(_)));
    if gl_failed && auto && renderer == ui_common::Renderer::Glow {
        eprintln!("OpenGL failed ({r:?}); starting again with wgpu");
        if ui_common::relaunch_with(ui_common::Renderer::Wgpu).is_ok() {
            return Ok(());
        }
    }
    r
}

fn run(renderer: ui_common::Renderer) -> eframe::Result {
    let start = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .filter(|p| p.is_dir() || classify(p) != Place::Local)
        // Like Explorer, open on Home.
        .unwrap_or_else(|| PathBuf::from(explorer_core::HOME));
    let settings = settings::Settings::load();
    let options = eframe::NativeOptions {
        renderer: match renderer {
            ui_common::Renderer::Glow => eframe::Renderer::Glow,
            ui_common::Renderer::Wgpu => eframe::Renderer::Wgpu,
        },
        viewport: egui::ViewportBuilder::default()
            .with_title("File Explorer (oxidows)")
            .with_icon(Arc::new(
                eframe::icon_data::from_png_bytes(ui_common::app_icon::EXPLORER_PNG).expect("embedded icon is a PNG"),
            ))
            .with_inner_size(settings.size)
            .with_maximized(settings.maximized)
            .with_min_inner_size([520.0, 320.0]),
        ..Default::default()
    };
    eframe::run_native("oxi-explorer", options, Box::new(|cc| Ok(Box::new(App::new(cc, start, settings)))))
}
