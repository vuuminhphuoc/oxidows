//! Split view (Ctrl+Shift+S): a second folder beside the main one, to move and copy files between
//! two folders by dragging them across, with the keyboard (Ctrl+Alt+C copies and Ctrl+Alt+M moves
//! the selection to the other pane) or with the Copy and Move buttons. Each pane has its own folder,
//! history, selection and sorting; the main pane keeps the tabs, the navigation pane and all of
//! Explorer's views. Drops use the same rules as everywhere else in the window (`dnd`): move within
//! a drive, copy across drives, Ctrl copies, Shift moves.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Layout, RichText, Sense, Ui};
use egui_extras::{Column, TableBuilder};
use explorer_core::{Entry, History, Place, SortKey, classify, format_size_kb, format_time, icon_for, parent_path};
use explorer_sys::FileOp;
use ui_common::Icon;

use crate::{App, IconKey, IconState, Msg};

/// A name being edited in place.
struct Rename {
    original: String,
    text: String,
    opened: bool,
    error: Option<&'static str>,
}

pub struct Split {
    pub cur: PathBuf,
    history: History,
    entries: Vec<Entry>,
    /// `entries` sorted and filtered, as indices.
    view: Vec<usize>,
    /// What `view` was made from: (entries version, sort, ascending, hidden, protected).
    view_key: Option<(u64, SortKey, bool, bool, bool)>,
    version: u64,
    selected: HashSet<String>,
    cursor: Option<usize>,
    anchor: Option<usize>,
    scroll_to: Option<usize>,
    sort: SortKey,
    ascending: bool,
    generation: u64,
    cancel: Arc<AtomicBool>,
    watch_cancel: Arc<AtomicBool>,
    loading: bool,
    replace_next: bool,
    error: Option<String>,
    /// A change was seen at this time; refresh once things settle.
    changed_at: Option<Instant>,
    /// The address box while it is being edited.
    address: Option<String>,
    address_select_all: bool,
    renaming: Option<Rename>,
    /// Select this name once it is listed (a new folder, the folder we came up from).
    pending_select: Option<String>,
    pending_rename: bool,
    type_ahead: (String, Instant),
    /// The pane has the keyboard: the shortcuts act on it, not on the main pane.
    pub active: bool,
    /// The whole pane, and the file list inside it, as drawn last frame.
    pub rect: Option<egui::Rect>,
    pub list_rect: Option<egui::Rect>,
    tx: Sender<Msg>,
    ctx: egui::Context,
}

/// Whether the split pane can show `p`: folders and shares, and the list of drives.
pub fn accepts(p: &Path) -> bool {
    matches!(classify(p), Place::Local | Place::Unc { .. } | Place::ThisPc)
}

fn shown(p: &Path) -> String {
    if classify(p) == Place::ThisPc { "This PC".into() } else { p.display().to_string() }
}

fn drive_entries(drives: &[explorer_sys::Drive], letters_before: bool) -> Vec<Entry> {
    drives
        .iter()
        .map(|d| Entry {
            name: d.root.to_string_lossy().into_owned(),
            label: Some(crate::view::drive_label(d, letters_before)),
            is_dir: true,
            note: Some("Drive".into()),
            ..Default::default()
        })
        .collect()
}

impl Split {
    pub fn new(cur: PathBuf, tx: Sender<Msg>, ctx: egui::Context) -> Self {
        Self {
            cur,
            history: History::default(),
            entries: Vec::new(),
            view: Vec::new(),
            view_key: None,
            version: 0,
            selected: HashSet::new(),
            cursor: None,
            anchor: None,
            scroll_to: None,
            sort: SortKey::Name,
            ascending: true,
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            watch_cancel: Arc::new(AtomicBool::new(false)),
            loading: false,
            replace_next: false,
            error: None,
            changed_at: None,
            address: None,
            address_select_all: false,
            renaming: None,
            pending_select: None,
            pending_rename: false,
            type_ahead: (String::new(), Instant::now()),
            active: false,
            rect: None,
            list_rect: None,
            tx,
            ctx,
        }
    }

    /// Stops the background work (the pane is closing).
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.watch_cancel.store(true, Ordering::Relaxed);
    }

    pub fn child(&self, name: &str) -> PathBuf {
        explorer_core::child_path(&self.cur, name)
    }

    /// Reads the folder again. With `keep_old` the old listing stays on screen until the first new
    /// batch arrives (no flicker).
    pub fn start_listing(&mut self, keep_old: bool, drives: &[explorer_sys::Drive], letters_before: bool) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.generation += 1;
        self.error = None;
        self.changed_at = None;
        if classify(&self.cur) == Place::ThisPc {
            self.entries = drive_entries(drives, letters_before);
            self.version += 1;
            self.loading = false;
            self.watch_cancel.store(true, Ordering::Relaxed);
            return;
        }
        self.loading = true;
        self.replace_next = keep_old;
        if !keep_old {
            self.entries.clear();
            self.version += 1;
        }
        let (dir, generation, cancel, tx, ctx) =
            (self.cur.clone(), self.generation, self.cancel.clone(), self.tx.clone(), self.ctx.clone());
        thread::Builder::new()
            .name("split-list".into())
            .spawn(move || {
                let r = explorer_sys::list_dir(&dir, &cancel, &mut |batch| {
                    let _ = tx.send(Msg::SplitBatch(generation, batch));
                    ctx.request_repaint();
                });
                let _ = tx.send(Msg::SplitDone(generation, r.map_err(|e| e.to_string())));
                ctx.request_repaint();
            })
            .expect("spawn split list thread");
        // Watch it, so changes made by other programs (and by the other pane) show up by themselves.
        self.watch_cancel.store(true, Ordering::Relaxed);
        self.watch_cancel = Arc::new(AtomicBool::new(false));
        let (dir, cancel, tx, ctx) = (self.cur.clone(), self.watch_cancel.clone(), self.tx.clone(), self.ctx.clone());
        thread::Builder::new()
            .name("split-watch".into())
            .spawn(move || {
                let _ = explorer_sys::watch_dir(&dir, &cancel, &mut || {
                    let _ = tx.send(Msg::SplitChanged(dir.clone()));
                    ctx.request_repaint();
                });
            })
            .expect("spawn split watch thread");
    }

    pub fn on_batch(&mut self, generation: u64, batch: Vec<Entry>) {
        if generation != self.generation {
            return;
        }
        if std::mem::take(&mut self.replace_next) {
            self.entries.clear();
        }
        for e in &batch {
            ui_common::ensure_glyphs(&self.ctx, e.display());
        }
        self.entries.extend(batch);
        self.version += 1;
    }

    pub fn on_done(&mut self, generation: u64, r: Result<(), String>) {
        if generation != self.generation {
            return;
        }
        if std::mem::take(&mut self.replace_next) {
            self.entries.clear();
            self.version += 1;
        }
        self.loading = false;
        self.error = r.err();
    }

    pub fn on_changed(&mut self, dir: &Path) {
        if dir == self.cur {
            self.changed_at.get_or_insert_with(Instant::now);
        }
    }

    /// Goes to `to` (a folder, a share or This PC), remembering where it came from.
    pub fn navigate(&mut self, to: PathBuf, drives: &[explorer_sys::Drive], letters_before: bool) {
        self.commit_rename_quietly();
        if to == self.cur {
            return self.start_listing(true, drives, letters_before);
        }
        let from = std::mem::replace(&mut self.cur, to);
        self.history.visit(from);
        self.entered(None, drives, letters_before);
    }

    fn entered(&mut self, select: Option<String>, drives: &[explorer_sys::Drive], letters_before: bool) {
        self.selected.clear();
        self.cursor = None;
        self.anchor = None;
        self.address = None;
        self.pending_select = select;
        self.scroll_to = Some(0);
        self.start_listing(false, drives, letters_before);
    }

    pub fn back(&mut self, drives: &[explorer_sys::Drive], letters_before: bool) {
        if let Some(p) = self.history.back(self.cur.clone()) {
            self.cur = p;
            self.entered(None, drives, letters_before);
        }
    }

    pub fn forward(&mut self, drives: &[explorer_sys::Drive], letters_before: bool) {
        if let Some(p) = self.history.forward(self.cur.clone()) {
            self.cur = p;
            self.entered(None, drives, letters_before);
        }
    }

    /// One level up, selecting the folder we came from.
    pub fn up(&mut self, drives: &[explorer_sys::Drive], letters_before: bool) {
        let Some(parent) = parent_path(&self.cur) else { return };
        if !accepts(&parent) {
            return;
        }
        let name = self.cur.file_name().map(|n| n.to_string_lossy().into_owned()).or_else(|| {
            // A drive root is listed under This PC by its path.
            Some(self.cur.to_string_lossy().into_owned())
        });
        let from = std::mem::replace(&mut self.cur, parent);
        self.history.visit(from);
        self.entered(name, drives, letters_before);
    }

    fn commit_rename_quietly(&mut self) {
        self.renaming = None;
    }

    /// Recomputes the sorted view when what it depends on changed.
    fn update_view(&mut self, show_hidden: bool, show_protected: bool) {
        let key = (self.version, self.sort, self.ascending, show_hidden, show_protected);
        if self.view_key == Some(key) {
            return;
        }
        self.view_key = Some(key);
        self.view =
            explorer_core::sorted_view_with(&self.entries, "", show_hidden, show_protected, self.sort, self.ascending);
        if let Some(name) = self.pending_select.clone()
            && let Some(pos) = self.view.iter().position(|&i| self.entries[i].name == name)
        {
            self.pending_select = None;
            self.select_only(pos);
            self.scroll_to = Some(pos);
            if std::mem::take(&mut self.pending_rename) {
                self.begin_rename();
            }
        }
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

    fn click(&mut self, pos: usize, mods: egui::Modifiers) {
        if mods.shift {
            self.select_range(pos);
        } else if mods.command || mods.ctrl {
            self.toggle(pos);
        } else {
            self.select_only(pos);
        }
    }

    /// Selected paths in view order.
    pub fn selected_paths(&self) -> Vec<PathBuf> {
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

    fn is_page(&self) -> bool {
        classify(&self.cur).is_page()
    }

    /// Whether `name` (an item of this folder) is being dragged.
    fn is_dragged(&self, app: &App, name: &str) -> bool {
        app.drag.as_ref().is_some_and(|d| d.source_dir == self.cur && d.names.contains(name))
    }

    fn begin_rename(&mut self) {
        if self.is_page() {
            return;
        }
        let Some(name) = self.cursor.and_then(|c| self.entry_at(c)).map(|e| e.name.clone()).or_else(|| {
            let sel = self.selected_entries();
            (sel.len() == 1).then(|| sel[0].name.clone())
        }) else {
            return;
        };
        self.renaming = Some(Rename { text: name.clone(), original: name, opened: false, error: None });
    }

    /// New folder, named and selected for typing its name.
    fn new_folder(&mut self, app: &mut App) {
        if self.is_page() {
            return;
        }
        let name = explorer_core::unique_name("New folder", &self.entries);
        match std::fs::create_dir(self.cur.join(&name)) {
            Ok(()) => {
                self.pending_select = Some(name);
                self.pending_rename = true;
                self.view_key = None;
                self.start_listing(true, &app.drives, app.persist.settings.drive_letters_before);
            }
            Err(e) => app.status = Some(format!("Could not create the folder: {e}")),
        }
    }

    /// Applies the rename in the box; returns the problem with the name, if any.
    fn finish_rename(&mut self, app: &mut App, r: &Rename) -> Option<&'static str> {
        let new = r.text.trim_end();
        if new == r.original {
            return None;
        }
        if let Err(e) = explorer_core::validate_name(new) {
            return Some(e);
        }
        let from = self.cur.join(&r.original);
        let to = from.with_file_name(new);
        if to.exists() && !new.eq_ignore_ascii_case(&r.original) {
            return Some("There is already an item with that name");
        }
        match std::fs::rename(&from, &to) {
            Ok(()) => {
                app.push_undo(crate::undo::Undoable::Rename { from, to });
                self.pending_select = Some(new.to_string());
                self.view_key = None;
                self.start_listing(true, &app.drives, app.persist.settings.drive_letters_before);
            }
            Err(e) => app.status = Some(format!("Could not rename: {e}")),
        }
        None
    }
}

/// What the pane asked for while drawing; applied afterwards, when `app` is free again.
#[derive(Default)]
struct Out {
    close: bool,
    navigate: Option<PathBuf>,
    back: bool,
    forward: bool,
    up: bool,
    refresh: bool,
    sort: Option<SortKey>,
    open: Option<usize>,
    click: Option<(usize, egui::Modifiers)>,
    context_select: Option<usize>,
    clear: bool,
    drag_start: Option<String>,
    drop_target: Option<PathBuf>,
    cmd: Option<Cmd>,
    activate: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cmd {
    Cut,
    Copy,
    Paste,
    PasteInto(usize),
    Rename,
    Delete,
    DeleteForever,
    NewFolder,
    Refresh,
    Properties,
    ShellMenu,
    CopyPath,
    /// To the folder in the other pane.
    CopyToOther,
    MoveToOther,
}

/// Draws the split pane at the right of the main file list.
pub fn show(app: &mut App, ui: &mut Ui) {
    let Some(mut s) = app.split.take() else { return };
    s.update_view(app.show_hidden, app.show_protected);
    let mut out = Out::default();
    let width = (ui.available_width() * 0.42).max(260.0);
    let frame = ui_common::content_frame(ui.style());
    let panel = egui::Panel::right("split-pane")
        .resizable(true)
        .default_size(width)
        .min_size(220.0)
        .frame(frame)
        .show(ui, |ui| pane_ui(app, &mut s, ui, &mut out));
    let rect = panel.response.rect;
    s.rect = Some(rect);
    let p = ui_common::Palette::of(ui);
    // The divider between the panes, and the accent line on top of the one that has the keyboard.
    ui.painter().vline(rect.left(), rect.y_range(), egui::Stroke::new(1.0, p.divider));
    if s.active {
        let r = egui::Rect::from_min_size(rect.left_top(), egui::vec2(rect.width(), 2.0));
        ui.painter().rect_filled(r, 0.0, p.accent);
    }
    let close = out.close;
    apply(app, &mut s, ui.ctx(), out);
    if close {
        s.stop();
    } else {
        app.split = Some(s);
    }
}

fn pane_ui(app: &mut App, s: &mut Split, ui: &mut Ui, out: &mut Out) {
    ui.spacing_mut().item_spacing.x = 2.0;
    let on_page = s.is_page();
    let n_sel = s.selected.len();
    let other_ok = !crate::split::other_pane_is_page(app);
    ui.horizontal(|ui| {
        if ui_common::tool_button(ui, Icon::Back, s.history.can_back(), "Back (Alt+←)").clicked() {
            out.back = true;
        }
        if ui_common::tool_button(ui, Icon::Forward, s.history.can_forward(), "Forward (Alt+→)").clicked() {
            out.forward = true;
        }
        if ui_common::tool_button(ui, Icon::Up, parent_path(&s.cur).is_some_and(|p| accepts(&p)), "Up (Alt+↑)")
            .clicked()
        {
            out.up = true;
        }
        if ui_common::tool_button(ui, Icon::Refresh, true, "Refresh (F5)").clicked() {
            out.refresh = true;
        }
        ui.add_space(4.0);
        // Right to left: the buttons take their width first and the address fills what is left, so the
        // row never asks for more than the pane has (a pane that grows to fit its content, every frame).
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui_common::tool_button(ui, Icon::Close, true, "Close the split view (Ctrl+Shift+S)").clicked() {
                out.close = true;
            }
            let can = n_sel > 0 && other_ok && !on_page;
            if ui_common::tool_button(ui, Icon::Send, can, "Move the selected items to the other pane (Ctrl+Alt+M)")
                .clicked()
            {
                out.cmd = Some(Cmd::MoveToOther);
            }
            if ui_common::tool_button(ui, Icon::Copy, can, "Copy the selected items to the other pane (Ctrl+Alt+C)")
                .clicked()
            {
                out.cmd = Some(Cmd::CopyToOther);
            }
            ui.add_space(4.0);
            let id = egui::Id::new("split-address");
            let mut text = s.address.clone().unwrap_or_else(|| shown(&s.cur));
            let w = (ui.available_width() - 4.0).max(60.0);
            let r = ui.add(egui::TextEdit::singleline(&mut text).id(id).desired_width(w));
            if r.has_focus() || r.changed() {
                s.address = Some(text.clone());
                out.activate = true;
            }
            if std::mem::take(&mut s.address_select_all)
                && let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id)
            {
                let n = text.chars().count();
                state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(n),
                )));
                state.store(ui.ctx(), id);
            }
            if r.lost_focus() {
                let submit = ui.input(|i| i.key_pressed(egui::Key::Enter));
                s.address = None;
                if submit {
                    let t = text.trim().trim_matches('"');
                    out.navigate = Some(if t.eq_ignore_ascii_case("this pc") {
                        PathBuf::from(explorer_core::THIS_PC)
                    } else {
                        PathBuf::from(t)
                    });
                }
            }
        });
    });
    ui.add_space(2.0);
    list_ui(app, s, ui, out);
}

/// The accent line over the main file list while it is the pane that has the keyboard (split view only).
pub fn mark_main(app: &App, ui: &Ui) {
    if app.split.as_ref().is_some_and(|s| !s.active)
        && let Some(l) = app.list_rect
    {
        let p = ui_common::Palette::of(ui);
        let right = app.split.as_ref().and_then(|s| s.rect).map_or(l.right(), |r| r.left().min(l.right()));
        let line = egui::Rect::from_min_max(l.left_top(), egui::pos2(right, l.top() + 2.0));
        ui.painter().rect_filled(line, 0.0, p.accent);
    }
}

/// Whether the main pane's folder can't take files (This PC, Home…).
pub fn other_pane_is_page(app: &App) -> bool {
    classify(&app.cur).is_page()
}

fn list_ui(app: &mut App, s: &mut Split, ui: &mut Ui, out: &mut Out) {
    let list_rect = ui.available_rect_before_wrap();
    s.list_rect = Some(list_rect);
    let mut bg_out = None;
    let bg = ui.interact(list_rect, ui.id().with("split-bg"), Sense::click());
    if bg.clicked() {
        out.clear = true;
        out.activate = true;
    }
    let shell_menus = app.hwnd != 0 && !s.is_page();
    bg.context_menu(|ui| background_menu(app, s, ui, &mut bg_out, shell_menus));
    if let Some(c) = bg_out {
        out.cmd = Some(c);
        out.activate = true;
    }

    if s.view.is_empty() && !s.loading {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            let msg = if s.error.is_some() { "This folder can't be opened." } else { "This folder is empty." };
            ui.label(RichText::new(msg).weak());
        });
    }

    let row_h = if app.compact { 18.0 } else { 22.0 };
    let weak = ui.visuals().weak_text_color();
    let text_color = ui.visuals().text_color();
    let dragging = app.dragging();
    let utc = app.utc_offset;
    let n = s.view.len();
    let mut wanted: Vec<IconKey> = Vec::new();
    let mut hover_target: Option<PathBuf> = None;
    let mut renaming = s.renaming.take();
    let mut rename_done: Option<bool> = None;
    let mut table = TableBuilder::new(ui)
        .resizable(true)
        .sense(Sense::click_and_drag())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::remainder().at_least(90.0).clip(true))
        .column(Column::initial(120.0).at_least(60.0).clip(true))
        .column(Column::initial(70.0).at_least(40.0).clip(true))
        .min_scrolled_height(0.0)
        .max_scroll_height((list_rect.height() - 24.0).max(40.0));
    table = table.id_salt("split-table");
    if let Some(pos) = s.scroll_to.take().filter(|&p| p < n) {
        table = table.scroll_to_row(pos, None);
    }
    let sorted_key = s.sort;
    let ascending = s.ascending;
    table
        .header(ui_common::SORT_HEADER_HEIGHT, |mut h| {
            for (key, title) in [(SortKey::Name, "Name"), (SortKey::Modified, "Date modified"), (SortKey::Size, "Size")]
            {
                h.col(|ui| {
                    let sorted = (sorted_key == key).then_some(!ascending);
                    if ui_common::sort_header(ui, title, sorted).clicked() {
                        out.sort = Some(key);
                    }
                });
            }
        })
        .body(|body| {
            body.rows(row_h, n, |mut row| {
                let pos = row.index();
                let e = &s.entries[s.view[pos]];
                row.set_selected(s.selected.contains(&e.name));
                let path = s.child(&e.name);
                let cut = !app.cut_paths.is_empty() && app.cut_paths.contains(&explorer_core::path_key(&path));
                let dim = e.hidden || e.system || cut;
                let name_color = if dim { weak } else { text_color };
                let tint = if cut { egui::Color32::WHITE.gamma_multiply(0.5) } else { egui::Color32::WHITE };
                let mut name_rect = egui::Rect::NOTHING;
                row.col(|ui| {
                    let key = IconKey::for_entry(e, &s.cur);
                    match app.icons.get(&key) {
                        Some(IconState::Ready(tex)) => {
                            ui.add(egui::Image::new((tex.id(), egui::vec2(16.0, 16.0))).tint(tint));
                        }
                        state => {
                            if state.is_none() {
                                wanted.push(key);
                            }
                            let c = if e.is_dir { ui_common::Palette::of(ui).folder } else { text_color };
                            ui.label(RichText::new(icon_for(e)).color(c));
                        }
                    }
                    ui.add_space(4.0);
                    match renaming.as_mut().filter(|r| r.original == e.name) {
                        Some(r) => rename_box(ui, r, e.is_dir, &mut rename_done),
                        None => {
                            ui.add(
                                egui::Label::new(RichText::new(e.display()).color(name_color))
                                    .selectable(false)
                                    .truncate(),
                            );
                        }
                    }
                    if e.link {
                        ui.label(RichText::new(" ⤴").color(weak));
                    }
                    name_rect = ui.min_rect();
                });
                row.col(|ui| {
                    if e.modified != 0 {
                        ui.add(
                            egui::Label::new(RichText::new(format_time(e.modified, utc)).color(weak)).selectable(false),
                        );
                    }
                });
                row.col(|ui| {
                    if !e.is_dir {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add(
                                egui::Label::new(RichText::new(format_size_kb(e.size)).color(weak)).selectable(false),
                            );
                        });
                    }
                });
                let resp = row.response();
                if resp.drag_started() && renaming.is_none() {
                    out.drag_start = Some(e.name.clone());
                }
                if e.is_dir && !s.is_dragged(app, &e.name) {
                    crate::view::drop_hover(&resp, dragging, &path, &mut hover_target);
                }
                let is_renaming = renaming.as_ref().is_some_and(|r| r.original == e.name);
                if resp.double_clicked() {
                    out.open = Some(pos);
                    out.activate = true;
                } else if resp.clicked() {
                    out.click = Some((pos, resp.ctx.input(|i| i.modifiers)));
                    out.activate = true;
                } else if !is_renaming && resp.secondary_clicked() {
                    out.context_select = Some(pos);
                    out.activate = true;
                }
                let _ = name_rect;
                if is_renaming {
                    return;
                }
                let is_dir = e.is_dir;
                resp.context_menu(|ui| {
                    item_menu(app, ui, out, pos, is_dir, shell_menus);
                });
            });
        });
    if let Some(done) = rename_done {
        if let Some(r) = renaming.take_if(|_| done) {
            if let Some(err) = s.finish_rename(app, &r) {
                renaming = Some(Rename { error: Some(err), ..r });
            }
        } else {
            renaming = None;
        }
    }
    s.renaming = renaming;
    // Footer: how many items, how many selected.
    let total = s.view.len();
    let sel = s.selected.len();
    let text = match (sel, s.loading) {
        (_, true) if total == 0 => "Loading…".to_string(),
        (0, _) => format!("{total} items"),
        _ => format!("{total} items   {sel} selected"),
    };
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(RichText::new(text).weak().size(12.0));
        if let Some(e) = &s.error {
            ui.label(RichText::new(e).color(ui.visuals().error_fg_color).size(12.0));
        }
    });
    app.request_icons(wanted);
    if let Some(t) = hover_target {
        out.drop_target = Some(t);
    }
}

fn rename_box(ui: &mut Ui, r: &mut Rename, is_dir: bool, done: &mut Option<bool>) {
    let esc = ui.input(|i| {
        i.raw.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::Escape, pressed: true, .. }))
    });
    let resp = ui.add(
        egui::TextEdit::singleline(&mut r.text)
            .margin(egui::vec2(3.0, 1.0))
            .desired_width(ui.available_width().max(60.0)),
    );
    if !r.opened {
        r.opened = true;
        resp.request_focus();
        let n = r.text.chars().count();
        let stem = match r.text.rfind('.') {
            Some(i) if i > 0 && !is_dir => r.text[..i].chars().count(),
            _ => n,
        };
        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), resp.id) {
            let range = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(stem));
            state.cursor.set_char_range(Some(range));
            state.store(ui.ctx(), resp.id);
        }
    } else if resp.lost_focus() {
        *done = Some(!esc);
    }
    if resp.changed() {
        r.error = None;
    }
    if let Some(e) = r.error {
        egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), resp.id.with("err"), &resp)
            .show(|ui| ui.colored_label(ui.visuals().error_fg_color, e));
    }
}

fn item_menu(app: &mut App, ui: &mut Ui, out: &mut Out, pos: usize, is_dir: bool, shell: bool) {
    use ui_common::menu_item;
    let (cmd, open) = (&mut out.cmd, &mut out.open);
    if menu_item(ui, Icon::OpenFile, "Open", Some("Enter")).clicked() {
        *open = Some(pos);
        ui.close();
    }
    ui.separator();
    if menu_item(ui, Icon::Cut, "Cut", Some("Ctrl+X")).clicked() {
        *cmd = Some(Cmd::Cut);
        ui.close();
    }
    if menu_item(ui, Icon::Copy, "Copy", Some("Ctrl+C")).clicked() {
        *cmd = Some(Cmd::Copy);
        ui.close();
    }
    if is_dir && app.can_paste() && menu_item(ui, Icon::Paste, "Paste into folder", None).clicked() {
        *cmd = Some(Cmd::PasteInto(pos));
        ui.close();
    }
    if menu_item(ui, Icon::Rename, "Rename", Some("F2")).clicked() {
        *cmd = Some(Cmd::Rename);
        ui.close();
    }
    if menu_item(ui, Icon::Delete, "Delete", Some("Delete")).clicked() {
        *cmd = Some(Cmd::Delete);
        ui.close();
    }
    ui.separator();
    let other = !other_pane_is_page(app);
    ui.add_enabled_ui(other, |ui| {
        if menu_item(ui, Icon::Copy, "Copy to the other pane", Some("Ctrl+Alt+C")).clicked() {
            *cmd = Some(Cmd::CopyToOther);
            ui.close();
        }
        if menu_item(ui, Icon::Send, "Move to the other pane", Some("Ctrl+Alt+M")).clicked() {
            *cmd = Some(Cmd::MoveToOther);
            ui.close();
        }
    });
    ui.separator();
    if menu_item(ui, Icon::Link, "Copy as path", Some("Ctrl+Shift+C")).clicked() {
        *cmd = Some(Cmd::CopyPath);
        ui.close();
    }
    if menu_item(ui, Icon::Properties, "Properties", Some("Alt+Enter")).clicked() {
        *cmd = Some(Cmd::Properties);
        ui.close();
    }
    if shell {
        ui.separator();
        if menu_item(ui, Icon::More, "Show more options", Some("Shift+F10")).clicked() {
            *cmd = Some(Cmd::ShellMenu);
            ui.close();
        }
    }
}

fn background_menu(app: &mut App, _s: &Split, ui: &mut Ui, cmd: &mut Option<Cmd>, shell: bool) {
    use ui_common::menu_item;
    if menu_item(ui, Icon::NewFolder, "New folder", Some("Ctrl+Shift+N")).clicked() {
        *cmd = Some(Cmd::NewFolder);
        ui.close();
    }
    if app.can_paste() && menu_item(ui, Icon::Paste, "Paste", Some("Ctrl+V")).clicked() {
        *cmd = Some(Cmd::Paste);
        ui.close();
    }
    if menu_item(ui, Icon::Refresh, "Refresh", Some("F5")).clicked() {
        *cmd = Some(Cmd::Refresh);
        ui.close();
    }
    if shell {
        ui.separator();
        if menu_item(ui, Icon::More, "Show more options", Some("Shift+F10")).clicked() {
            *cmd = Some(Cmd::ShellMenu);
            ui.close();
        }
    }
}

/// Carries out what the pane asked for.
fn apply(app: &mut App, s: &mut Split, ctx: &egui::Context, out: Out) {
    let (drives, lb) = (app.drives.clone(), app.persist.settings.drive_letters_before);
    if out.activate {
        s.active = true;
    }
    if out.back {
        s.back(&drives, lb);
    }
    if out.forward {
        s.forward(&drives, lb);
    }
    if out.up {
        s.up(&drives, lb);
    }
    if out.refresh {
        s.start_listing(true, &drives, lb);
    }
    if let Some(to) = out.navigate {
        if accepts(&to) && (to == Path::new(explorer_core::THIS_PC) || to.is_dir()) {
            s.navigate(to, &drives, lb);
        } else {
            app.status = Some(format!("Cannot open {}", to.display()));
        }
    }
    if let Some(key) = out.sort {
        if s.sort == key {
            s.ascending = !s.ascending;
        } else {
            s.sort = key;
            // Sizes and dates read best newest or biggest first, as in Explorer.
            s.ascending = key == SortKey::Name;
        }
    }
    if out.clear {
        s.selected.clear();
        s.cursor = None;
    }
    if let Some((pos, mods)) = out.click {
        if let Some(r) = s.renaming.take() {
            let _ = s.finish_rename(app, &r);
        }
        s.click(pos, mods);
    }
    if let Some(pos) = out.context_select
        && let Some(name) = s.entry_at(pos).map(|e| e.name.clone())
        && !s.selected.contains(&name)
    {
        s.select_only(pos);
    }
    if let Some(pos) = out.open {
        s.select_only(pos);
        s.open_entry(app, pos, ctx, &drives, lb);
    }
    if let Some(name) = out.drag_start {
        if !s.selected.contains(&name) {
            s.selected = HashSet::from([name]);
        }
        let paths = s.selected_paths();
        if !paths.is_empty() {
            let names = s.selected.clone();
            app.drag = Some(crate::dnd::Drag { paths, names, source_dir: s.cur.clone(), button: None });
            s.active = true;
        }
    }
    if let Some(t) = out.drop_target {
        app.drop_target = Some(t);
    }
    if let Some(cmd) = out.cmd {
        s.run(app, cmd, ctx);
    }
    // Changes seen by the watcher settle for a moment, then the folder is read again.
    if let Some(t) = s.changed_at {
        let settle = Duration::from_millis(250);
        if t.elapsed() >= settle && !s.loading {
            s.start_listing(true, &drives, lb);
        } else {
            ctx.request_repaint_after(settle);
        }
    }
}

impl Split {
    fn open_entry(
        &mut self,
        app: &mut App,
        pos: usize,
        _ctx: &egui::Context,
        drives: &[explorer_sys::Drive],
        lb: bool,
    ) {
        let Some(e) = self.entry_at(pos).cloned() else { return };
        let path = self.child(&e.name);
        if e.is_dir {
            if accepts(&path) {
                self.navigate(path, drives, lb);
            }
        } else if let Err(err) = explorer_sys::open(&path) {
            app.status = Some(format!("Could not open {}: {err}", e.name));
        }
    }

    /// Commands on the selection (menus, keys, buttons).
    fn run(&mut self, app: &mut App, cmd: Cmd, ctx: &egui::Context) {
        let paths = self.selected_paths();
        let page = self.is_page();
        match cmd {
            Cmd::Cut | Cmd::Copy if !paths.is_empty() && !page => {
                let cut = cmd == Cmd::Cut;
                app.status = Some(match explorer_sys::clipboard_set_files(&paths, cut) {
                    Ok(()) => {
                        app.sync_cut_state();
                        format!("{} {} item(s)", if cut { "Cut" } else { "Copied" }, paths.len())
                    }
                    Err(e) => format!("Clipboard error: {e}"),
                });
            }
            Cmd::Paste if !page => app.paste_into(self.cur.clone()),
            Cmd::PasteInto(pos) => {
                if let Some(e) = self.entry_at(pos).filter(|e| e.is_dir) {
                    let dest = self.child(&e.name);
                    app.paste_into(dest);
                }
            }
            Cmd::Rename => self.begin_rename(),
            Cmd::Delete | Cmd::DeleteForever if !paths.is_empty() && !page => {
                let op = if cmd == Cmd::DeleteForever { FileOp::Delete } else { FileOp::Recycle };
                app.run_op(op, paths, None);
            }
            Cmd::NewFolder => self.new_folder(app),
            Cmd::Refresh => self.start_listing(true, &app.drives, app.persist.settings.drive_letters_before),
            Cmd::Properties => match paths.as_slice() {
                [p] => app.show_properties(p),
                _ if !page => {
                    let c = self.cur.clone();
                    app.show_properties(&c);
                }
                _ => {}
            },
            Cmd::CopyPath if !paths.is_empty() => {
                let text = paths.iter().map(|p| format!("\"{}\"", p.display())).collect::<Vec<_>>().join("\r\n");
                ctx.copy_text(text);
            }
            Cmd::ShellMenu if !page => {
                let outcome = explorer_sys::shell_context_menu(app.hwnd, &self.cur, &paths, false);
                match outcome {
                    Ok(explorer_sys::MenuOutcome::Host(verb)) if verb == "rename" => self.begin_rename(),
                    Ok(explorer_sys::MenuOutcome::Host(_)) => {}
                    Ok(explorer_sys::MenuOutcome::Invoked(v)) if !v.is_empty() => {
                        self.changed_at.get_or_insert_with(Instant::now);
                    }
                    Ok(_) => {}
                    Err(e) => app.status = Some(format!("Menu: {e}")),
                }
            }
            Cmd::CopyToOther | Cmd::MoveToOther if !paths.is_empty() && !page => {
                let dest = app.cur.clone();
                app.send_to(paths, dest, cmd == Cmd::MoveToOther);
            }
            _ => {}
        }
    }
}

impl App {
    /// Opens the split pane (at `path`, else beside the folder shown now) or closes it.
    pub fn toggle_split(&mut self) {
        if let Some(s) = self.split.take() {
            s.stop();
            return;
        }
        let start = if accepts(&self.cur) { self.cur.clone() } else { explorer_sys::home_dir() };
        self.open_split(start);
    }

    pub fn open_split(&mut self, path: PathBuf) {
        let path = if accepts(&path) { path } else { explorer_sys::home_dir() };
        let mut s = Split::new(path, self.tx.clone(), self.ctx.clone());
        s.start_listing(false, &self.drives, self.persist.settings.drive_letters_before);
        self.split = Some(s);
    }

    /// Copies or moves `paths` into `dest` (the folder of the other pane).
    pub fn send_to(&mut self, paths: Vec<PathBuf>, dest: PathBuf, mv: bool) {
        if let Some(why) = crate::dnd::refuse(&paths, &dest) {
            if !why.is_empty() {
                self.status = Some(why.into());
            } else {
                self.status = Some("The items are already in that folder".into());
            }
            return;
        }
        self.run_op(if mv { FileOp::Move } else { FileOp::Copy }, paths, Some(dest));
    }

    /// Whether the split pane has the keyboard.
    pub fn split_active(&self) -> bool {
        self.split.as_ref().is_some_and(|s| s.active)
    }

    /// The split pane shows a folder files can go into.
    pub fn split_has_folder(&self) -> bool {
        self.split.as_ref().is_some_and(|s| !s.is_page())
    }

    /// Copies or moves the main pane's selection into the split pane's folder.
    pub fn send_to_split(&mut self, mv: bool) {
        let Some(dest) = self.split.as_ref().map(|s| s.cur.clone()) else { return };
        if classify(&dest).is_page() {
            self.status = Some("Open a folder in the other pane first".into());
            return;
        }
        let paths = self.selected_paths();
        self.send_to(paths, dest, mv);
    }

    /// The split pane's folder when `pos` is over its file list (where a drop goes into that folder).
    pub fn split_list_target(&self, pos: egui::Pos2) -> Option<PathBuf> {
        let s = self.split.as_ref()?;
        (s.list_rect.is_some_and(|r| r.contains(pos)) && !s.is_page()).then(|| s.cur.clone())
    }

    /// The folder to drop into when the pointer is over the blank part of either pane (only with the
    /// split view open).
    pub fn split_drop_fallback(&self, pos: egui::Pos2) -> Option<PathBuf> {
        self.split.as_ref()?;
        if let Some(t) = self.split_list_target(pos) {
            return Some(t);
        }
        if self.list_rect.is_some_and(|r| r.contains(pos)) && !classify(&self.cur).is_page() {
            return Some(self.cur.clone());
        }
        None
    }

    /// Keys while the split pane is the active one. Returns after handling them all.
    pub fn split_keys(&mut self, ui: &mut Ui) {
        use egui::{Key, Modifiers};
        let Some(mut s) = self.split.take() else { return };
        let ctx = ui.ctx().clone();
        let (drives, lb) = (self.drives.clone(), self.persist.settings.drive_letters_before);
        let text_focused = ctx.memory(|m| m.focused().is_some());
        let alt = Modifiers::ALT;
        let ctrl = Modifiers::COMMAND;
        let ctrl_alt = Modifiers::COMMAND | Modifiers::ALT;
        let (back, fwd, up, f5, addr, newdir, to_other, move_other, f11) = ui.input_mut(|i| {
            let plain = !i.modifiers.alt && !i.modifiers.shift;
            (
                // Keys with more modifiers first: egui matches Alt+Ctrl+C when only Ctrl+C is asked for.
                i.consume_key(alt, Key::ArrowLeft) || i.pointer.button_pressed(egui::PointerButton::Extra1),
                i.consume_key(alt, Key::ArrowRight) || i.pointer.button_pressed(egui::PointerButton::Extra2),
                i.consume_key(alt, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::F5),
                i.consume_key(ctrl, Key::L)
                    || i.consume_key(alt, Key::D)
                    || (plain && i.consume_key(Modifiers::NONE, Key::F4)),
                i.consume_key(ctrl | Modifiers::SHIFT, Key::N),
                // egui-winit turns Ctrl+C into a Copy event (no key press), also with Alt held.
                i.modifiers.alt && i.modifiers.command && i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                i.consume_key(ctrl_alt, Key::M),
                plain && i.consume_key(Modifiers::NONE, Key::F11),
            )
        });
        if back {
            s.back(&drives, lb);
        }
        if fwd {
            s.forward(&drives, lb);
        }
        if up {
            s.up(&drives, lb);
        }
        if f5 {
            s.start_listing(true, &drives, lb);
            self.load_drives();
        }
        if f11 {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
        if addr {
            s.address = Some(shown(&s.cur));
            s.address_select_all = true;
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("split-address")));
        }
        if newdir {
            s.new_folder(self);
        }
        if to_other {
            s.run(self, Cmd::CopyToOther, &ctx);
        }
        if move_other {
            s.run(self, Cmd::MoveToOther, &ctx);
        }
        let ctrl_v = explorer_sys::ctrl_v_down();
        let paste_pressed = ctrl_v && !self.ctrl_v_was_down;
        self.ctrl_v_was_down = ctrl_v;
        if text_focused || s.renaming.is_some() {
            self.split = Some(s);
            return;
        }
        if paste_pressed {
            s.run(self, Cmd::Paste, &ctx);
        }
        let n = s.view.len();
        let mut moved: Option<(usize, bool)> = None;
        let mut events: Vec<&'static str> = Vec::new();
        let popup_open = egui::Popup::is_any_open(&ctx);
        ui.input_mut(|i| {
            let shift = i.modifiers.shift;
            for key in [Key::ArrowUp, Key::ArrowDown] {
                if !i.key_pressed(key) || n == 0 || i.modifiers.alt {
                    continue;
                }
                let to = match (s.cursor.filter(|&c| c < n), key) {
                    (None, Key::ArrowUp) => Some(n - 1),
                    (None, _) => Some(0),
                    (Some(c), Key::ArrowUp) => c.checked_sub(1),
                    (Some(c), _) => Some(c + 1).filter(|&p| p < n),
                };
                if let Some(to) = to {
                    moved = Some((to, shift));
                }
            }
            for (key, step) in [(Key::PageDown, 20isize), (Key::PageUp, -20)] {
                if i.key_pressed(key) && n > 0 && !i.modifiers.alt {
                    let c = s.cursor.map_or(if step > 0 { -1 } else { n as isize }, |c| c as isize);
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
                (ctrl | Modifiers::SHIFT, Key::C, "copypath"),
                (Modifiers::SHIFT, Key::F10, "shellmenu"),
                (Modifiers::NONE, Key::Enter, "open"),
                (Modifiers::NONE, Key::Backspace, "back"),
                (Modifiers::NONE, Key::F2, "rename"),
                (Modifiers::SHIFT, Key::Delete, "nuke"),
                (Modifiers::NONE, Key::Delete, "delete"),
                (ctrl, Key::D, "delete"),
                (ctrl, Key::A, "all"),
                (ctrl | Modifiers::SHIFT, Key::Z, "redo"),
                (ctrl, Key::Z, "undo"),
                (ctrl, Key::Y, "redo"),
                (Modifiers::ALT, Key::Enter, "props"),
                (Modifiers::NONE, Key::F6, "switch"),
                (Modifiers::NONE, Key::Escape, "none"),
            ] {
                if k == Key::Escape && popup_open {
                    continue;
                }
                if i.consume_key(m, k) {
                    events.push(tag);
                }
            }
            for ev in &i.events {
                match ev {
                    egui::Event::Copy if i.modifiers.alt => {}
                    egui::Event::Copy if !events.contains(&"copypath") => events.push("copy"),
                    egui::Event::Cut => events.push("cut"),
                    egui::Event::Paste(_) if !cfg!(windows) => events.push("paste"),
                    egui::Event::Text(t) if !i.modifiers.command && !i.modifiers.alt => {
                        let (buf, at) = &mut s.type_ahead;
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
                s.select_range(pos)
            } else {
                s.select_only(pos)
            }
            s.scroll_to = Some(pos);
        }
        for ev in events {
            match ev {
                "open" => {
                    let sel = s.selected_entries();
                    if let [e] = sel.as_slice() {
                        if let Some(pos) = s.view.iter().position(|&i| s.entries[i].name == e.name) {
                            s.open_entry(self, pos, &ctx, &drives, lb);
                        }
                    } else {
                        for e in sel.iter().filter(|e| !e.is_dir) {
                            let _ = explorer_sys::open(&s.child(&e.name));
                        }
                    }
                }
                "back" => s.up(&drives, lb),
                "rename" => s.run(self, Cmd::Rename, &ctx),
                "delete" => s.run(self, Cmd::Delete, &ctx),
                "nuke" => s.run(self, Cmd::DeleteForever, &ctx),
                "all" => s.selected = s.view.iter().map(|&i| s.entries[i].name.clone()).collect(),
                "undo" => self.undo(),
                "redo" => self.redo(),
                "props" => s.run(self, Cmd::Properties, &ctx),
                "switch" => s.active = false,
                "none" => {
                    s.selected.clear();
                    s.cursor = None;
                }
                "copy" => s.run(self, Cmd::Copy, &ctx),
                "cut" => s.run(self, Cmd::Cut, &ctx),
                "paste" => s.run(self, Cmd::Paste, &ctx),
                "copypath" => s.run(self, Cmd::CopyPath, &ctx),
                "shellmenu" => {
                    if cfg!(windows) {
                        s.run(self, Cmd::ShellMenu, &ctx)
                    }
                }
                "typeahead" => {
                    let prefix = s.type_ahead.0.to_lowercase();
                    let start = s.cursor.map_or(0, |c| if prefix.chars().count() == 1 { c + 1 } else { c });
                    let hit = (0..n)
                        .map(|k| (start + k) % n.max(1))
                        .find(|&p| s.entry_at(p).is_some_and(|e| e.display().to_lowercase().starts_with(&prefix)));
                    if let Some(p) = hit {
                        s.select_only(p);
                        s.scroll_to = Some(p);
                    }
                }
                _ => {}
            }
        }
        self.split = Some(s);
    }

    /// F6, with the main pane active: moves the keys to the split pane (and back, in `split_keys`).
    pub fn split_switch_pane(&mut self) {
        if let Some(s) = &mut self.split {
            s.active = !s.active;
        }
    }

    /// Re-reads the split pane's folder (a file operation ended, the window got focus again).
    pub fn refresh_split(&mut self) {
        let (drives, lb) = (self.drives.clone(), self.persist.settings.drive_letters_before);
        if let Some(s) = &mut self.split {
            s.start_listing(true, &drives, lb);
        }
    }

    /// The drives list changed: This PC in the split pane shows it.
    pub fn split_drives_changed(&mut self) {
        let (drives, lb) = (self.drives.clone(), self.persist.settings.drive_letters_before);
        if let Some(s) = &mut self.split
            && classify(&s.cur) == Place::ThisPc
        {
            s.start_listing(true, &drives, lb);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_folders_and_drives_only() {
        assert!(accepts(Path::new(r"C:\Users")));
        assert!(accepts(Path::new(r"\\nas\share\dir")));
        assert!(accepts(Path::new(explorer_core::THIS_PC)));
        assert!(!accepts(Path::new(explorer_core::HOME)));
        assert!(!accepts(Path::new(explorer_core::RECYCLE_BIN)));
        assert!(!accepts(Path::new(explorer_core::NETWORK)));
    }

    #[test]
    fn this_pc_lists_drives() {
        let d = explorer_sys::Drive {
            root: PathBuf::from(r"C:\"),
            label: "Windows".into(),
            kind: explorer_sys::DriveKind::Fixed,
            total: 100,
            free: 50,
        };
        let e = drive_entries(&[d], false);
        assert_eq!(e.len(), 1);
        assert!(e[0].is_dir);
        assert_eq!(e[0].name, r"C:\");
        assert_eq!(e[0].display(), "Windows (C:)");
        assert_eq!(explorer_core::child_path(Path::new(explorer_core::THIS_PC), &e[0].name), PathBuf::from(r"C:\"));
    }
}
