use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use crate::Entry;

/// Walks every folder under `root` with `threads` workers, calling `visit(rel_dir, entries)` for each folder read
/// (rel_dir relative to root, "" for root itself; entries as list_dir gives them). Links/junctions are not
/// entered (no loops), unreadable folders are skipped (as Explorer). Returns when done or `cancel` is set.
pub fn walk_parallel(root: &Path, threads: usize, cancel: &AtomicBool, visit: &(dyn Fn(&Path, Vec<Entry>) + Sync)) {
    let pool = (Mutex::new((VecDeque::from([PathBuf::new()]), 0usize)), Condvar::new());
    let (lock, cvar) = &pool;

    std::thread::scope(|s| {
        let n = threads.max(1);
        for _ in 0..n {
            s.spawn(|| {
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
                    let rel = loop {
                        if cancel.load(Ordering::Relaxed) {
                            break None;
                        }
                        if let Some(dir) = state.0.pop_front() {
                            state.1 += 1;
                            break Some(dir);
                        }
                        if state.1 == 0 {
                            break None;
                        }
                        let (s, _) =
                            cvar.wait_timeout(state, Duration::from_millis(50)).unwrap_or_else(|e| e.into_inner());
                        state = s;
                    };
                    drop(state);

                    let Some(rel_dir) = rel else {
                        cvar.notify_all();
                        break;
                    };

                    let abs_dir = root.join(&rel_dir);
                    let mut all_entries = Vec::new();
                    let mut subfolders = Vec::new();

                    let _ = crate::list_dir(&abs_dir, cancel, &mut |batch| {
                        for e in batch {
                            if e.is_dir && !e.link {
                                subfolders.push(rel_dir.join(&e.name));
                            }
                            all_entries.push(e);
                        }
                    });

                    if !cancel.load(Ordering::Relaxed) {
                        visit(&rel_dir, all_entries);
                    }

                    let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
                    if !cancel.load(Ordering::Relaxed) {
                        state.0.extend(subfolders);
                    }
                    state.1 -= 1;
                    cvar.notify_all();
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::fs;
    use std::sync::Mutex;

    #[test]
    fn walk_parallel_visits_tree() {
        let temp = std::env::temp_dir().join(format!(
            "test_walk_parallel_{}_{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())
        ));
        fs::create_dir_all(&temp).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(temp.clone());

        for i in 0..6 {
            fs::write(temp.join(format!("root_{i}.txt")), b"test").unwrap();
        }
        let dir_a = temp.join("dir_a");
        let dir_b = temp.join("dir_b");
        fs::create_dir_all(&dir_a).unwrap();
        fs::create_dir_all(&dir_b).unwrap();
        for i in 0..6 {
            fs::write(dir_a.join(format!("a_{i}.txt")), b"test").unwrap();
            fs::write(dir_b.join(format!("b_{i}.txt")), b"test").unwrap();
        }
        let sub_a = dir_a.join("sub_a");
        let sub_b = dir_b.join("sub_b");
        fs::create_dir_all(&sub_a).unwrap();
        fs::create_dir_all(&sub_b).unwrap();
        for i in 0..6 {
            fs::write(sub_a.join(format!("sub_a_{i}.txt")), b"test").unwrap();
            fs::write(sub_b.join(format!("sub_b_{i}.txt")), b"test").unwrap();
        }

        #[cfg(unix)]
        {
            let _ = std::os::unix::fs::symlink(&temp, sub_a.join("loop_link"));
        }

        let cancel = AtomicBool::new(false);
        let found_files = Mutex::new(HashSet::new());

        walk_parallel(&temp, 4, &cancel, &|rel, entries| {
            let mut set = found_files.lock().unwrap();
            for e in entries {
                if !e.is_dir {
                    let full_rel = rel.join(&e.name);
                    assert!(set.insert(full_rel), "duplicate file visited: {:?}", e.name);
                }
            }
        });

        let set = found_files.lock().unwrap();
        assert_eq!(set.len(), 30);
    }
}
