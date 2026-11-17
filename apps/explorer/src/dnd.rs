//! Drag and drop from the file list: onto a folder in the list, the navigation pane or the
//! Recycle Bin inside the window, and out of the window to any program (a system OLE drag
//! that starts as soon as the pointer leaves the window, carrying the shell's own data object).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use std::time::{Duration, Instant};

use eframe::egui;
use explorer_core::RECYCLE_BIN;
use explorer_sys::FileOp;

use crate::App;

/// Trace for the end-to-end drag and drop test (`OXI_DND_DEBUG=1`).
pub fn dbg_log(args: std::fmt::Arguments) {
    if std::env::var_os("OXI_DND_DEBUG").is_some() {
        eprintln!("dnd: {args}");
    }
}

pub const DRAG_HOVER_DELAY: Duration = Duration::from_millis(600);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HoverTarget {
    Tree(egui::Id),
    Tab(usize),
}

#[derive(Debug, Default)]
pub struct HoverTimer {
    active: Option<(HoverTarget, Instant)>,
}

impl HoverTimer {
    /// Advances the timer for `now`; fires once `hovered` has stayed steady for `delay`.
    pub fn step(
        &mut self,
        now: Instant,
        hovered: Option<HoverTarget>,
        delay: Duration,
    ) -> (Option<HoverTarget>, Option<Duration>) {
        let Some(target) = hovered else {
            self.active = None;
            return (None, None);
        };
        match self.active {
            Some((prev, since)) if prev == target => {
                let elapsed = now.saturating_duration_since(since);
                if elapsed >= delay {
                    self.active = None;
                    (Some(target), None)
                } else {
                    (None, Some(delay - elapsed))
                }
            }
            _ => {
                self.active = Some((target, now));
                (None, Some(delay))
            }
        }
    }
}

pub struct Drag {
    pub paths: Vec<PathBuf>,
    /// Names in `source_dir`, so items in that folder don't light up as their own targets.
    pub names: HashSet<String>,
    pub source_dir: PathBuf,
    /// The button that started the drag, known from the first frame on.
    pub button: Option<egui::PointerButton>,
}
fn root(p: &Path) -> Option<std::ffi::OsString> {
    p.components().next().map(|c| c.as_os_str().to_ascii_lowercase())
}

/// Copy or move, as Explorer decides: Ctrl copies, Shift moves, otherwise move on the same
/// drive and copy across drives.
pub fn pick_op(sources: &[PathBuf], target: &Path, mods: egui::Modifiers) -> FileOp {
    if target == Path::new(RECYCLE_BIN) {
        return FileOp::Recycle;
    }
    if mods.ctrl {
        FileOp::Copy
    } else if mods.shift || sources.iter().all(|p| root(p) == root(target)) {
        FileOp::Move
    } else {
        FileOp::Copy
    }
}

/// Why `sources` can't go into `target`, if they can't.
pub fn refuse(sources: &[PathBuf], target: &Path) -> Option<&'static str> {
    if target == Path::new(RECYCLE_BIN) {
        return None;
    }
    if sources.iter().any(|s| target.starts_with(s)) {
        return Some("The destination folder is inside the source folder");
    }
    if sources.iter().all(|s| s.parent() == Some(target)) {
        return Some("");
    }
    None
}

impl App {
    /// Call for each item that can be dragged.
    pub fn drag_source(&mut self, name: &str) {
        if !self.selected.contains(name) {
            self.selected = HashSet::from([name.to_string()]);
        }
        let paths = self.selected_paths();
        if !paths.is_empty() {
            dbg_log(format_args!("drag_source {paths:?}"));
            self.drag = Some(Drag { paths, names: self.selected.clone(), source_dir: self.cur.clone(), button: None });
        }
    }

    /// Starts dragging `paths` (all in one folder) without touching the selection.
    pub fn drag_paths(&mut self, paths: Vec<PathBuf>) {
        let Some(first) = paths.first() else { return };
        let source_dir = first.parent().map(Path::to_path_buf).unwrap_or_default();
        let names = paths.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).collect();
        self.drag = Some(Drag { paths, names, source_dir, button: None });
    }

    /// A drag is under way: one started in this window, or files another program drags over it.
    pub fn dragging(&self) -> bool {
        self.drag.is_some() || self.drop_host.as_ref().is_some_and(|h| h.active())
    }

    /// Whether `name` (an item in the current folder) is being dragged.
    pub fn is_dragged(&self, name: &str) -> bool {
        self.drag.as_ref().is_some_and(|d| d.source_dir == self.cur && d.names.contains(name))
    }

    /// Files dropped on the window, or a drag released over `target`.
    pub fn drop_into(&mut self, sources: Vec<PathBuf>, target: PathBuf, mods: egui::Modifiers) {
        if let Some(why) = refuse(&sources, &target) {
            if !why.is_empty() {
                self.status = Some(why.into());
            }
            return;
        }
        let op = pick_op(&sources, &target, mods);
        let dest = (op != FileOp::Recycle).then_some(target);
        self.run_op(op, sources, dest);
    }

    /// A drag released over `target` that the shell has to decide: Alt or Ctrl+Shift make
    /// shortcuts, and a right-button drag asks (Copy here / Move here / Create shortcuts here).
    fn shell_drop_into(&mut self, sources: Vec<PathBuf>, target: PathBuf, keys: explorer_sys::DragKeys) {
        if let Some(why) = refuse(&sources, &target) {
            if !why.is_empty() {
                self.status = Some(why.into());
            }
            return;
        }
        match explorer_sys::shell_drop(self.hwnd, &sources, &target, keys) {
            Ok(explorer_sys::DropResult::Moved) => {
                // The shell reports a move even when the user chose Skip in the Replace dialog (or
                // Cancel part-way), so count what actually left the folder.
                let gone = || sources.iter().filter(|p| std::fs::symlink_metadata(p).is_err()).count();
                let mut moved = gone();
                for _ in 0..5 {
                    if moved > 0 {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    moved = gone();
                }
                self.status = Some(moved_text(moved, sources.len()));
            }
            Ok(explorer_sys::DropResult::Copied) => self.status = Some(format!("Copied {} items", sources.len())),
            Ok(explorer_sys::DropResult::Linked) => self.status = Some("Created shortcuts".into()),
            Ok(explorer_sys::DropResult::None) => {
                // The shell's menu ("Move here") reports no effect even when it did the work, and a
                // Skip or a cancelled menu leaves the files: look at the disk instead of keeping a
                // message from an earlier action.
                let gone = sources.iter().filter(|p| std::fs::symlink_metadata(p).is_err()).count();
                self.status = (gone > 0).then(|| moved_text(gone, sources.len()));
            }
            Err(e) => self.status = Some(format!("Drop: {e}")),
        }
        self.refresh();
    }

    /// End of frame: drop, leave the window, or draw what would happen.
    pub fn finish_drag(&mut self, ctx: &egui::Context) {
        let hovered = self.frame_hover.take();
        let external = self.drop_host.as_ref().is_some_and(|h| h.active());
        if external {
            // Another program is dragging files over the window: the shell does the drop. Tell it
            // which folder the pointer is over: a folder item or tree row, else this folder when
            // over the file list.
            self.hover_actions(ctx, hovered);
            let pos = ctx.input(|i| i.pointer.latest_pos());
            let over_list = pos.is_some_and(|p| self.list_rect.is_some_and(|r| r.contains(p)));
            let over_split = pos.and_then(|p| self.split_list_target(p));
            let target = self
                .drop_target
                .take()
                .or_else(|| self.tab_drop.take())
                .or(over_split)
                .or_else(|| over_list.then(|| self.cur.clone()));
            if let Some(host) = &self.drop_host {
                host.set_target(target.as_deref());
            }
            return;
        }
        if self.drag.is_none() {
            self.drag_hover.step(Instant::now(), None, DRAG_HOVER_DELAY);
            return;
        }
        let (primary, secondary, pos, mods) =
            ctx.input(|i| (i.pointer.primary_down(), i.pointer.secondary_down(), i.pointer.latest_pos(), i.modifiers));
        // The button the drag started with; the drag ends when that one is let go.
        let button = match self.drag.as_ref().and_then(|d| d.button) {
            Some(b) => b,
            None => {
                let b =
                    if secondary && !primary { egui::PointerButton::Secondary } else { egui::PointerButton::Primary };
                if let Some(d) = self.drag.as_mut() {
                    d.button = Some(b);
                }
                b
            }
        };
        let right = button == egui::PointerButton::Secondary;
        let down = if right { secondary } else { primary };
        // With the split view open, the blank part of either pane is the folder it shows.
        if self.drop_target.is_none()
            && self.tab_drop.is_none()
            && let Some(p) = pos
        {
            self.drop_target = self.split_drop_fallback(p);
        }
        let inside = pos.is_some_and(|p| ctx.content_rect().contains(p));
        if !down {
            self.drag_hover.step(Instant::now(), None, DRAG_HOVER_DELAY);
            let paths = std::mem::take(&mut self.drag).map(|d| d.paths).unwrap_or_default();
            let target = self.drop_target.take().or_else(|| self.tab_drop.take());
            if let Some(t) = target {
                let recycle = t == Path::new(RECYCLE_BIN);
                if cfg!(windows) && !recycle && (right || mods.alt || (mods.ctrl && mods.shift)) {
                    let keys = explorer_sys::DragKeys { ctrl: mods.ctrl, shift: mods.shift, alt: mods.alt, right };
                    self.shell_drop_into(paths, t, keys);
                } else {
                    self.drop_into(paths, t, mods);
                }
            }
            return;
        }
        if !inside && cfg!(windows) {
            dbg_log(format_args!("leaving the window at {pos:?}, rect {:?}", ctx.content_rect()));
            self.drag_hover.step(Instant::now(), None, DRAG_HOVER_DELAY);
            let paths = self.drag.as_ref().map(|d| d.paths.clone()).unwrap_or_default();
            self.drag = None;
            // The system takes over the mouse until the drop; the button-up never reaches us.
            self.release_pointer = true;
            let outcome = explorer_sys::drag_out(self.hwnd, &paths);
            dbg_log(format_args!("drag_out returned {outcome:?}"));
            match outcome {
                Ok(explorer_sys::DropResult::Moved) => self.status = Some(format!("Moved {} items", paths.len())),
                Ok(_) => {}
                Err(e) => self.status = Some(format!("Drag: {e}")),
            }
            self.refresh();
            return;
        }
        self.hover_actions(ctx, hovered);
        let Some(drag) = &self.drag else { return };
        let Some(pos) = pos else { return };
        let n = drag.paths.len();
        let target = self.drop_target.clone().or_else(|| self.tab_drop.clone());
        let text = match &target {
            Some(t) => {
                let verb = if right && t != Path::new(RECYCLE_BIN) {
                    "Drop in"
                } else if t != Path::new(RECYCLE_BIN) && (mods.alt || (mods.ctrl && mods.shift)) {
                    "Create shortcut in"
                } else {
                    match pick_op(&drag.paths, t, mods) {
                        FileOp::Copy => "Copy to",
                        FileOp::Move => "Move to",
                        _ => "Move to",
                    }
                };
                let name = if t == Path::new(RECYCLE_BIN) {
                    "Recycle Bin".to_string()
                } else {
                    t.file_name().map_or_else(|| t.display().to_string(), |n| n.to_string_lossy().into_owned())
                };
                format!("{verb} {name}")
            }
            None if n == 1 => drag.paths[0].file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(),
            None => format!("{n} items"),
        };
        egui::Area::new(egui::Id::new("drag-label"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos + egui::vec2(14.0, 14.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| ui.add(egui::Label::new(text).extend()));
            });
        ctx.request_repaint();
    }

    /// Folder hover during a drag: after a short wait a tab is opened or a tree folder expanded.
    fn hover_actions(&mut self, ctx: &egui::Context, hovered: Option<(HoverTarget, Option<PathBuf>)>) {
        let (target, tree_path) = match hovered {
            Some((t, p)) => (Some(t), p),
            None => (None, None),
        };
        let (fired, repaint_after) = self.drag_hover.step(Instant::now(), target, DRAG_HOVER_DELAY);
        if let Some(wait) = repaint_after {
            ctx.request_repaint_after(wait);
        }
        match fired {
            Some(HoverTarget::Tab(i)) => self.switch_tab(i),
            Some(HoverTarget::Tree(id)) => {
                let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ctx, id, false);
                state.set_open(true);
                state.store(ctx);
                if let Some(p) = tree_path
                    && !self.tree.contains_key(&p)
                {
                    self.load_tree(p);
                }
                ctx.request_repaint();
            }
            None => {}
        }
    }
}

/// Status line for a move that left `moved` of `total` items gone from their folder.
pub fn moved_text(moved: usize, total: usize) -> String {
    match moved {
        0 => "Nothing was moved".into(),
        n if n == total => format!("Moved {n} items"),
        n => format!("Moved {n} of {total} items"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_and_refusals() {
        let p = |s: &str| PathBuf::from(s);
        let none = egui::Modifiers::NONE;
        let ctrl = egui::Modifiers { ctrl: true, ..none };
        assert_eq!(pick_op(&[p("/a/x")], &p("/a/y"), none), FileOp::Move);
        assert_eq!(pick_op(&[p("/a/x")], &p("/a/y"), ctrl), FileOp::Copy);
        assert_eq!(pick_op(&[p("/a/x")], &p(RECYCLE_BIN), none), FileOp::Recycle);
        assert!(refuse(&[p("/a/x")], &p("/a/x/sub")).is_some());
        assert_eq!(refuse(&[p("/a/x")], &p("/a")), Some(""));
        assert_eq!(refuse(&[p("/a/x")], &p("/b")), None);
    }

    #[test]
    fn hover_timer_steps() {
        let mut timer = HoverTimer::default();
        let t0 = Instant::now();
        let tab1 = HoverTarget::Tab(1);
        let tab2 = HoverTarget::Tab(2);
        let delay = Duration::from_millis(600);

        assert_eq!(timer.step(t0, None, delay), (None, None));
        assert_eq!(timer.step(t0, Some(tab1), delay), (None, Some(delay)));
        assert_eq!(
            timer.step(t0 + Duration::from_millis(250), Some(tab1), delay),
            (None, Some(Duration::from_millis(350)))
        );
        // Moving to another target restarts the 600 ms wait.
        let t1 = t0 + Duration::from_millis(400);
        assert_eq!(timer.step(t1, Some(tab2), delay), (None, Some(delay)));
        assert_eq!(
            timer.step(t1 + Duration::from_millis(599), Some(tab2), delay),
            (None, Some(Duration::from_millis(1)))
        );
        // Reaches delay: fires once and resets.
        assert_eq!(timer.step(t1 + delay, Some(tab2), delay), (Some(tab2), None));
        // Staying on the same target starts a fresh cycle if still eligible.
        assert_eq!(timer.step(t1 + Duration::from_millis(700), Some(tab2), delay), (None, Some(delay)));
        assert_eq!(timer.step(t1 + Duration::from_millis(800), None, delay), (None, None));
    }

    #[test]
    fn moved_status_counts_what_left() {
        assert_eq!(moved_text(0, 2), "Nothing was moved");
        assert_eq!(moved_text(2, 2), "Moved 2 items");
        assert_eq!(moved_text(1, 3), "Moved 1 of 3 items");
    }
}
