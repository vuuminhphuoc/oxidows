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
            self.drag = Some(Drag { paths, names: self.selected.clone(), source_dir: self.cur.clone() });
        }
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

    /// End of frame: drop, leave the window, or draw what would happen.
    pub fn finish_drag(&mut self, ctx: &egui::Context) {
        let hovered = self.frame_hover.take();
        if self.drag.is_none() {
            self.drag_hover.step(Instant::now(), None, DRAG_HOVER_DELAY);
            return;
        }
        let (down, pos, mods) = ctx.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos(), i.modifiers));
        let inside = pos.is_some_and(|p| ctx.content_rect().contains(p));
        if !down {
            self.drag_hover.step(Instant::now(), None, DRAG_HOVER_DELAY);
            let paths = std::mem::take(&mut self.drag).map(|d| d.paths).unwrap_or_default();
            if let Some(t) = self.drop_target.take() {
                self.drop_into(paths, t, mods);
            }
            return;
        }
        if !inside && cfg!(windows) {
            self.drag_hover.step(Instant::now(), None, DRAG_HOVER_DELAY);
            let paths = self.drag.as_ref().map(|d| d.paths.clone()).unwrap_or_default();
            self.drag = None;
            // The system takes over the mouse until the drop; the button-up never reaches us.
            self.release_pointer = true;
            match explorer_sys::drag_out(self.hwnd, &paths) {
                Ok(explorer_sys::DropResult::Moved) => self.status = Some(format!("Moved {} items", paths.len())),
                Ok(_) => {}
                Err(e) => self.status = Some(format!("Drag: {e}")),
            }
            self.refresh();
            return;
        }
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
        let Some(drag) = &self.drag else { return };
        let Some(pos) = pos else { return };
        let n = drag.paths.len();
        let text = match &self.drop_target {
            Some(t) => {
                let verb = match pick_op(&drag.paths, t, mods) {
                    FileOp::Copy => "Copy to",
                    FileOp::Move => "Move to",
                    _ => "Move to",
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
}
