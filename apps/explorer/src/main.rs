#![forbid(unsafe_code)]
// Release builds on Windows run without a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod view;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use explorer_core::{
    Entry, History, Place, SortKey, child_path, classify, parent_path, sorted_view, unique_name, validate_name,
};
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
    OpDone(String, std::io::Result<bool>),
    /// The watched folder changed on disk.
    Changed(PathBuf),
    Icon(IconKey, Option<explorer_sys::IconImage>),
}

/// Icons are cached per extension; only files that carry their own icon get one each.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum IconKey {
    Dir,
    Ext(String),
    File(PathBuf),
}

impl IconKey {
    pub fn for_entry(e: &Entry, dir: &Path) -> Self {
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

pub enum IconState {
    Pending,
    Ready(egui::TextureHandle),
    Failed,
}

enum Dialog {
    Rename { original: String, text: String, error: Option<&'static str>, opened: bool },
    NewFolder { text: String, error: Option<&'static str>, opened: bool },
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
    icon_tx: Option<Sender<IconKey>>,
    drives: Vec<Drive>,
    places: Vec<(&'static str, PathBuf)>,
    home: PathBuf,
    utc_offset: i64,
    started: Instant,
    timing_logged: bool,
    was_focused: bool,
    title: String,
    /// Main window handle (Windows), for the shell's menus and dialogs; 0 if unknown.
    pub hwnd: isize,
    /// All tabs; the active one's slot is stale while it is shown (its state lives in `App`).
    pub tabs: Vec<TabSnap>,
    pub active_tab: usize,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, start: PathBuf) -> Self {
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
            icon_tx: None,
            drives: Vec::new(),
            places: explorer_sys::known_folders(),
            home: explorer_sys::home_dir(),
            utc_offset: explorer_sys::utc_offset_secs(),
            started: Instant::now(),
            timing_logged: false,
            was_focused: true,
            title: String::new(),
            hwnd: 0,
            tabs: Vec::new(),
            active_tab: 0,
        };
        let first = app.snapshot();
        app.tabs.push(first);
        app.start_listing(false);
        app.start_watch();
        app.start_icon_loader();
        app.load_drives();
        app
    }

    // ---- workers -------------------------------------------------------------------------

    /// Searches `cur` and everything below it for names containing `query`. Results keep their
    /// path relative to `cur` as the name ("sub\\file.txt"), so opening, copying and deleting
    /// them works unchanged. Links/junctions are not followed (no loops).
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
        thread::Builder::new()
            .name("search".into())
            .spawn(move || {
                let t = Instant::now();
                let needle = query.to_lowercase();
                let mut queue = std::collections::VecDeque::from([PathBuf::new()]);
                let mut found = Vec::new();
                let mut last_send = Instant::now();
                while let Some(rel) = queue.pop_front() {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    // Unreadable folders (access denied) are skipped, as Explorer does.
                    let _ = explorer_sys::list_dir(&root.join(&rel), &cancel, &mut |batch| {
                        for mut e in batch {
                            let rel_name = rel.join(&e.name);
                            if e.is_dir && !e.link {
                                queue.push_back(rel_name.clone());
                            }
                            if e.name.to_lowercase().contains(&needle) {
                                e.name = rel_name.to_string_lossy().into_owned();
                                found.push(e);
                            }
                        }
                    });
                    if !found.is_empty() && (found.len() >= explorer_sys::BATCH || last_send.elapsed().as_millis() > 100) {
                        let _ = tx.send(Msg::Batch(generation, std::mem::take(&mut found)));
                        ctx.request_repaint();
                        last_send = Instant::now();
                    }
                }
                if !found.is_empty() {
                    let _ = tx.send(Msg::Batch(generation, found));
                }
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
        thread::Builder::new()
            .name("list".into())
            .spawn(move || {
                let t = Instant::now();
                let r = match classify(&dir) {
                    // Computers and shares are shown as folders.
                    Place::Network => {
                        let names = explorer_sys::network_computers();
                        let batch = names
                            .into_iter()
                            .map(|n| Entry { name: n.trim_start_matches('\\').to_string(), is_dir: true, ..Default::default() })
                            .collect();
                        let _ = tx.send(Msg::Batch(generation, batch));
                        Ok(())
                    }
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
                    Place::Server(server) => explorer_sys::list_shares(&server).map(|shares| {
                        let batch = shares.into_iter().map(|s| Entry { name: s.name, is_dir: true, ..Default::default() }).collect();
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
        if matches!(classify(&self.cur), Place::Network | Place::Server(_) | Place::RecycleBin) {
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

    fn load_drives(&self) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        thread::spawn(move || {
            let _ = tx.send(Msg::Drives(explorer_sys::drives()));
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
                let r = explorer_sys::file_op(op, &sources, dest.as_deref());
                let _ = tx.send(Msg::OpDone(desc, r));
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
                Msg::Changed(dir) if dir == self.cur && self.deep_search.is_none() => {
                    self.changed_at.get_or_insert_with(Instant::now);
                }
                Msg::Changed(_) => {}
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
                Msg::OpDone(desc, r) => {
                    self.ops_running -= 1;
                    self.status = Some(match r {
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
            TabSnap { cur: PathBuf::new(), history: History::default(), selected: HashSet::new(), filter: String::new(), sort: SortKey::Name, ascending: true },
        );
        self.active_tab = i;
        self.load_tab(target);
        self.tabs[i].cur = self.cur.clone();
    }

    /// Opens `path` in a new tab after the current one; `focus` switches to it.
    pub fn new_tab(&mut self, path: PathBuf, focus: bool) {
        let t = TabSnap { cur: path, history: History::default(), selected: HashSet::new(), filter: String::new(), sort: self.sort, ascending: self.ascending };
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
        explorer_core::location_crumbs(p).last().map_or_else(|| p.to_string_lossy().into_owned(), |(l, _)| l.clone())
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
        self.start_listing(false);
        self.start_watch();
    }

    fn back(&mut self) {
        if let Some(p) = self.history.back(self.cur.clone()) {
            // Going back up to a parent selects the folder we were in.
            let child = (parent_path(&self.cur).as_deref() == Some(p.as_path()))
                .then(|| self.cur.file_name().map(|n| n.to_string_lossy().into_owned()))
                .flatten();
            self.cur = p;
            self.entered_new_folder(child);
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

    fn up(&mut self) {
        let Some(parent) = parent_path(&self.cur) else { return };
        let child = match classify(&self.cur) {
            Place::Server(s) => Some(s),
            _ => Path::new(self.cur.to_string_lossy().trim_end_matches('\\'))
                .file_name()
                .map(|n| n.to_string_lossy().into_owned()),
        };
        let from = std::mem::replace(&mut self.cur, parent);
        self.history.visit(from);
        self.entered_new_folder(child);
    }

    fn refresh(&mut self) {
        match self.deep_search.clone() {
            Some(q) => self.start_deep_search(q),
            None => self.start_listing(true),
        }
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
        } else if let Err(err) = explorer_sys::open(&path) {
            self.status = Some(format!("Could not open {}: {err}", e.name));
        }
    }

    fn submit_address(&mut self, text: &str) {
        let t = text.trim().trim_matches('"');
        if t.is_empty() {
            return;
        }
        let p = PathBuf::from(t);
        let p = if t.eq_ignore_ascii_case("network") { PathBuf::from(explorer_core::NETWORK) } else { p };
        let p = if p.is_relative() && classify(&p) == Place::Local { self.cur.join(p) } else { p };
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

    // ---- view & selection ----------------------------------------------------------------

    fn update_view(&mut self) {
        let key = ViewKey {
            version: self.version,
            filter: self.filter.clone(),
            sort: self.sort,
            ascending: self.ascending,
            show_hidden: self.show_hidden,
        };
        if self.view_key.as_ref() == Some(&key) {
            return;
        }
        self.view = sorted_view(&self.entries, &self.filter, self.show_hidden, self.sort, self.ascending);
        self.view_key = Some(key);
        if let Some(name) = self.pending_select.clone()
            && let Some(pos) = self.view.iter().position(|&i| self.entries[i].name == name)
        {
            self.pending_select = None;
            self.select_only(pos);
            self.scroll_to = Some(pos);
        }
        // Keep the cursor on the same item after a re-sort or refresh.
        self.cursor = self.cursor.and_then(|_| {
            self.view.iter().position(|&i| self.selected.contains(&self.entries[i].name))
        });
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

    fn copy_selection(&mut self, cut: bool) {
        if self.in_recycle_bin() {
            return;
        }
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        self.status = Some(match explorer_sys::clipboard_set_files(&paths, cut) {
            Ok(()) => format!("{} {} item(s)", if cut { "Cut" } else { "Copied" }, paths.len()),
            Err(e) => format!("Clipboard error: {e}"),
        });
    }

    fn paste_into(&mut self, dest: PathBuf) {
        let Some((files, cut)) = explorer_sys::clipboard_get_files() else {
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
        self.run_op(if permanently { FileOp::Delete } else { FileOp::Recycle }, paths, None);
    }

    pub fn begin_rename(&mut self) {
        if self.in_recycle_bin() {
            return;
        }
        let sel = self.selected_entries();
        if let [e] = sel.as_slice() {
            // In search results `name` is a relative path; only its last part is edited.
            let leaf = Path::new(&e.name).file_name().map_or_else(|| e.name.clone(), |n| n.to_string_lossy().into_owned());
            self.dialog = Some(Dialog::Rename { original: e.name.clone(), text: leaf, error: None, opened: false });
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
            let _ = tx.send(Msg::OpDone(format!("Mapped {letter}: to {}", remote.display()), r));
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
        self.dialog =
            Some(Dialog::NewFolder { text: unique_name("New folder", &self.entries), error: None, opened: false });
    }

    /// Returns an error to show in the dialog, or `None` once done.
    fn finish_dialog(&mut self, dialog: &Dialog) -> Option<&'static str> {
        match dialog {
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
            self.ascending = matches!(key, SortKey::Name | SortKey::Type);
        }
    }

    /// Keyboard shortcuts that apply when no text field has focus.
    fn handle_keys(&mut self, ui: &mut egui::Ui) {
        use egui::{Key, Modifiers};
        let text_focused = ui.ctx().memory(|m| m.focused().is_some());
        let alt = Modifiers::ALT;
        let ctrl = Modifiers::COMMAND;

        // These work everywhere, even while typing in the search box.
        let (new_tab, close_tab, next_tab, prev_tab) = ui.input_mut(|i| {
            (
                i.consume_key(ctrl, Key::T),
                i.consume_key(ctrl, Key::W),
                i.consume_key(ctrl, Key::Tab),
                i.consume_key(ctrl | Modifiers::SHIFT, Key::Tab),
            )
        });
        if new_tab {
            let home = self.home.clone();
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
            self.address_edit = Some(self.cur.to_string_lossy().into_owned());
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
            if extend { self.select_range(pos) } else { self.select_only(pos) }
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
        let dropped: Vec<PathBuf> = ui.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if !dropped.is_empty() {
            let root = |p: &Path| p.components().next().map(|c| c.as_os_str().to_ascii_lowercase());
            let same_drive = dropped.iter().all(|p| root(p) == root(&self.cur));
            let op = if same_drive { FileOp::Move } else { FileOp::Copy };
            self.run_op(op, dropped, Some(self.cur.clone()));
        }

        if self.dialog.is_none() {
            self.handle_keys(ui);
        }
        self.update_view();

        if let Some(t) = self.tabs.get_mut(self.active_tab) {
            t.cur.clone_from(&self.cur);
        }
        let title = match self.cur.file_name() {
            Some(n) => n.to_string_lossy().into_owned(),
            None => self.cur.to_string_lossy().into_owned(),
        };
        if title != self.title {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Title(format!("{title} - Explorer (Rust)")));
            self.title = title;
        }

        view::tab_bar(self, ui);
        view::toolbar(self, ui);
        view::status_bar(self, ui);
        view::sidebar(self, ui);
        egui::CentralPanel::default().show(ui, |ui| view::file_list(self, ui));
        view::dialog(self, ui);
    }
}

/// wgpu on Windows: it falls back to the WARP software rasteriser on machines without a GPU
/// driver (VMs, RDP, CI runners), where OpenGL is only the 1.1 GDI stub. `OXI_RENDERER=glow`
/// or `wgpu` overrides.
fn renderer() -> eframe::Renderer {
    match std::env::var("OXI_RENDERER").as_deref() {
        Ok("glow") => eframe::Renderer::Glow,
        Ok("wgpu") => eframe::Renderer::Wgpu,
        _ if cfg!(windows) => eframe::Renderer::Wgpu,
        _ => eframe::Renderer::Glow,
    }
}

fn main() -> eframe::Result {
    let start = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(explorer_sys::home_dir);
    let options = eframe::NativeOptions {
        renderer: renderer(),
        viewport: egui::ViewportBuilder::default()
            .with_title("Explorer (Rust)")
            .with_inner_size([1040.0, 660.0])
            .with_min_inner_size([520.0, 320.0]),
        ..Default::default()
    };
    eframe::run_native("oxi-explorer", options, Box::new(|cc| Ok(Box::new(App::new(cc, start)))))
}
