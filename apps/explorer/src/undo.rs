//! Undo / Redo (Ctrl+Z / Ctrl+Y) for what this window did: rename, new folder, move, copy and
//! delete to the Recycle Bin. Permanent deletes can't be undone, as in Explorer.

use std::path::{Path, PathBuf};

use explorer_sys::FileOp;

use crate::{App, Msg};

/// Records kept; older ones are dropped.
const DEPTH: usize = 50;

#[derive(Clone, Debug, PartialEq)]
pub enum Undoable {
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    NewFolder(PathBuf),
    /// (where it was, where it went)
    Moved(Vec<(PathBuf, PathBuf)>),
    /// (source, the copy made)
    Copied(Vec<(PathBuf, PathBuf)>),
    /// Original paths now in the Recycle Bin.
    Recycled(Vec<PathBuf>),
}

impl Undoable {
    pub fn label(&self) -> String {
        let n = |k: usize| if k == 1 { String::new() } else { format!(" {k} items") };
        match self {
            Undoable::Rename { .. } => "Rename".into(),
            Undoable::NewFolder(_) => "New folder".into(),
            Undoable::Moved(v) => format!("Move{}", n(v.len())),
            Undoable::Copied(v) => format!("Copy{}", n(v.len())),
            Undoable::Recycled(v) => format!("Delete{}", n(v.len())),
        }
    }
}

/// What a finished copy/move/recycle did, worked out from what existed before and after.
/// Items that replaced or were renamed around an existing target are left out: they can't be
/// put back exactly.
pub fn record(op: FileOp, sources: &[PathBuf], dest: Option<&Path>, pre_existing: &[bool]) -> Option<Undoable> {
    let target = |s: &PathBuf| dest.and_then(|d| s.file_name().map(|n| d.join(n)));
    let made = || {
        sources
            .iter()
            .zip(pre_existing)
            .filter_map(|(s, &pre)| target(s).filter(|t| !pre && t.exists()).map(|t| (s.clone(), t)))
    };
    let rec = match op {
        FileOp::Copy => Undoable::Copied(made().collect()),
        FileOp::Move => Undoable::Moved(made().filter(|(s, _)| !s.exists()).collect()),
        FileOp::Recycle => Undoable::Recycled(sources.iter().filter(|s| !s.exists()).cloned().collect()),
        FileOp::Delete => return None,
    };
    let empty = match &rec {
        Undoable::Copied(v) | Undoable::Moved(v) => v.is_empty(),
        Undoable::Recycled(v) => v.is_empty(),
        _ => false,
    };
    (!empty).then_some(rec)
}

/// For each source, whether its name already exists in `dest` (taken before the operation).
pub fn snapshot_targets(sources: &[PathBuf], dest: Option<&Path>) -> Vec<bool> {
    sources.iter().map(|s| dest.zip(s.file_name()).is_some_and(|(d, n)| d.join(n).exists())).collect()
}

/// Groups (from, to) moves by destination folder, for one file operation each.
fn by_parent(pairs: impl Iterator<Item = (PathBuf, PathBuf)>) -> Vec<(PathBuf, Vec<PathBuf>)> {
    let mut out: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
    for (src, to) in pairs {
        let Some(dir) = to.parent().map(Path::to_path_buf) else { continue };
        match out.iter_mut().find(|(d, _)| *d == dir) {
            Some((_, v)) => v.push(src),
            None => out.push((dir, vec![src])),
        }
    }
    out
}

impl App {
    pub fn push_undo(&mut self, u: Undoable) {
        self.undo_stack.push(u);
        if self.undo_stack.len() > DEPTH {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) {
        let Some(u) = self.undo_stack.pop() else {
            self.status = Some("Nothing to undo".into());
            return;
        };
        match self.apply(&u, true) {
            Ok(()) => {
                self.status = Some(format!("Undo {}", u.label()));
                self.redo_stack.push(u);
            }
            Err(e) => self.status = Some(format!("Could not undo {}: {e}", u.label())),
        }
        self.refresh();
    }

    pub fn redo(&mut self) {
        let Some(u) = self.redo_stack.pop() else {
            self.status = Some("Nothing to redo".into());
            return;
        };
        match self.apply(&u, false) {
            Ok(()) => {
                self.status = Some(format!("Redo {}", u.label()));
                self.undo_stack.push(u);
            }
            Err(e) => self.status = Some(format!("Could not redo {}: {e}", u.label())),
        }
        self.refresh();
    }

    /// Runs `u` backwards (`undo`) or again. Quick steps run here; file operations run in
    /// the background with Windows' own progress and confirmation.
    fn apply(&mut self, u: &Undoable, undo: bool) -> std::io::Result<()> {
        match (u, undo) {
            (Undoable::Rename { from, to }, true) => rename_no_replace(to, from),
            (Undoable::Rename { from, to }, false) => rename_no_replace(from, to),
            (Undoable::NewFolder(p), true) => match std::fs::remove_dir(p) {
                Ok(()) => Ok(()),
                // Something was put in it since: send it to the Recycle Bin instead.
                Err(_) => {
                    self.run_op_untracked(FileOp::Recycle, vec![p.clone()], None);
                    Ok(())
                }
            },
            (Undoable::NewFolder(p), false) => std::fs::create_dir(p),
            (Undoable::Moved(pairs), true) => {
                for (dir, srcs) in by_parent(pairs.iter().map(|(s, t)| (t.clone(), s.clone()))) {
                    self.run_op_untracked(FileOp::Move, srcs, Some(dir));
                }
                Ok(())
            }
            (Undoable::Moved(pairs), false) => {
                for (dir, srcs) in by_parent(pairs.iter().cloned()) {
                    self.run_op_untracked(FileOp::Move, srcs, Some(dir));
                }
                Ok(())
            }
            (Undoable::Copied(pairs), true) => {
                self.run_op_untracked(FileOp::Recycle, pairs.iter().map(|(_, c)| c.clone()).collect(), None);
                Ok(())
            }
            (Undoable::Copied(pairs), false) => {
                for (dir, srcs) in by_parent(pairs.iter().cloned()) {
                    self.run_op_untracked(FileOp::Copy, srcs, Some(dir));
                }
                Ok(())
            }
            (Undoable::Recycled(paths), true) => {
                let items = explorer_sys::recycle_bin_items();
                let mut missing = 0;
                for p in paths {
                    // The newest deletion of that path.
                    match items.iter().filter(|i| i.original == *p).max_by_key(|i| i.deleted) {
                        Some(it) => explorer_sys::restore(it)?,
                        None => missing += 1,
                    }
                }
                if missing > 0 {
                    return Err(std::io::Error::other(format!("{missing} item(s) are no longer in the Recycle Bin")));
                }
                Ok(())
            }
            (Undoable::Recycled(paths), false) => {
                self.run_op_untracked(FileOp::Recycle, paths.clone(), None);
                Ok(())
            }
        }
    }

    /// A file operation that is itself an undo/redo, so it records nothing.
    fn run_op_untracked(&mut self, op: FileOp, sources: Vec<PathBuf>, dest: Option<PathBuf>) {
        self.ops_running += 1;
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        let _ = std::thread::Builder::new().name("file-op".into()).spawn(move || {
            let r = explorer_sys::file_op(op, &sources, dest.as_deref());
            let _ = tx.send(Msg::OpDone(String::new(), r, None));
            ctx.request_repaint();
        });
    }
}

fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    let same_item = from.to_string_lossy().eq_ignore_ascii_case(&to.to_string_lossy());
    if to.exists() && !same_item {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{} exists", to.display())));
    }
    std::fs::rename(from, to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_only_what_was_made() {
        let d = std::env::temp_dir().join(format!("oxi-undo-{}", std::process::id()));
        let (src, dst) = (d.join("src"), d.join("dst"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        let (a, b) = (src.join("a.txt"), src.join("b.txt"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        std::fs::write(dst.join("b.txt"), "old").unwrap();
        let sources = vec![a.clone(), b.clone()];
        let pre = snapshot_targets(&sources, Some(&dst));
        assert_eq!(pre, [false, true]);
        // Pretend the copy ran: a was copied, b replaced the existing one.
        std::fs::copy(&a, dst.join("a.txt")).unwrap();
        assert_eq!(
            record(FileOp::Copy, &sources, Some(&dst), &pre),
            Some(Undoable::Copied(vec![(a, dst.join("a.txt"))]))
        );
        // Nothing moved (sources still exist): nothing to undo.
        assert_eq!(record(FileOp::Move, &sources, Some(&dst), &pre), None);
        assert_eq!(record(FileOp::Delete, &sources, None, &pre), None);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn groups_moves_by_folder() {
        let p = |s: &str| PathBuf::from(s);
        let g = by_parent([(p("/x/1"), p("/a/1")), (p("/x/2"), p("/b/2")), (p("/x/3"), p("/a/3"))].into_iter());
        assert_eq!(g, [(p("/a"), vec![p("/x/1"), p("/x/3")]), (p("/b"), vec![p("/x/2")])]);
    }
}
