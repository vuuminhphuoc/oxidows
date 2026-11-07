//! Drag and drop from the file list: onto a folder in the list, the navigation pane or the
//! Recycle Bin inside the window, and out of the window to any program (a system OLE drag
//! that starts as soon as the pointer leaves the window, carrying the shell's own data object).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use eframe::egui;
use explorer_core::RECYCLE_BIN;
use explorer_sys::FileOp;

use crate::App;

pub struct Drag {
    pub paths: Vec<PathBuf>,
    /// Names in the current folder, so they don't light up as their own targets.
    pub names: HashSet<String>,
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
            self.drag = Some(Drag { paths, names: self.selected.clone() });
        }
    }

    /// Whether `name` (an item in the current folder) is being dragged.
    pub fn is_dragged(&self, name: &str) -> bool {
        self.drag.as_ref().is_some_and(|d| d.names.contains(name))
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
        let Some(drag) = &self.drag else { return };
        let (down, pos, mods) = ctx.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos(), i.modifiers));
        let inside = pos.is_some_and(|p| ctx.content_rect().contains(p));
        if !down {
            let paths = std::mem::take(&mut self.drag).map(|d| d.paths).unwrap_or_default();
            if let Some(t) = self.drop_target.take() {
                self.drop_into(paths, t, mods);
            }
            return;
        }
        if !inside && cfg!(windows) {
            let paths = drag.paths.clone();
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
}
