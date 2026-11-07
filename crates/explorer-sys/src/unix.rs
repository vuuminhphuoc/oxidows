//! Development fallback (Linux/macOS) so the UI can be built and screenshotted off Windows.
//! Not a product target: no Recycle Bin, a process-local clipboard, no Properties dialog.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

use explorer_core::Entry;

use crate::{BATCH, BinItem, Drive, DriveKind, FileOp, IconImage, Share};

static CLIPBOARD: Mutex<Option<(Vec<PathBuf>, bool)>> = Mutex::new(None);

pub fn list_dir(dir: &Path, cancel: &AtomicBool, sink: &mut dyn FnMut(Vec<Entry>)) -> io::Result<()> {
    let mut batch = Vec::with_capacity(256);
    for de in std::fs::read_dir(dir)? {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(de) = de else { continue };
        let name = de.file_name().to_string_lossy().into_owned();
        let link = de.file_type().is_ok_and(|t| t.is_symlink());
        let md = std::fs::metadata(de.path()).or_else(|_| de.metadata());
        let secs = |t: io::Result<std::time::SystemTime>| {
            t.ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64)
        };
        let (is_dir, size, modified, created, readonly) = match md {
            Ok(m) => (
                m.is_dir(),
                if m.is_dir() { 0 } else { m.len() },
                secs(m.modified()),
                secs(m.created()),
                m.permissions().readonly(),
            ),
            Err(_) => (false, 0, 0, 0, false),
        };
        use explorer_core::attr;
        let attrs = if is_dir { attr::DIRECTORY } else { attr::ARCHIVE }
            | if readonly { attr::READONLY } else { 0 }
            | if name.starts_with('.') { attr::HIDDEN } else { 0 }
            | if link { attr::REPARSE } else { 0 };
        batch.push(Entry {
            hidden: name.starts_with('.'),
            name,
            is_dir,
            size,
            modified,
            created,
            attrs,
            system: false,
            link,
            label: None,
            note: None,
        });
        if batch.len() >= BATCH {
            sink(std::mem::replace(&mut batch, Vec::with_capacity(BATCH)));
        }
    }
    if !batch.is_empty() {
        sink(batch);
    }
    Ok(())
}

pub fn drives() -> Vec<Drive> {
    vec![Drive { root: PathBuf::from("/"), label: "Root".into(), kind: DriveKind::Fixed, total: 0, free: 0 }]
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

pub fn known_folders() -> Vec<(&'static str, PathBuf)> {
    let home = home_dir();
    ["Desktop", "Downloads", "Documents", "Pictures", "Music", "Videos"]
        .into_iter()
        .map(|n| (n, home.join(n)))
        .filter(|(_, p)| p.is_dir())
        .collect()
}

pub fn utc_offset_secs() -> i64 {
    0
}

pub fn open(path: &Path) -> io::Result<()> {
    std::process::Command::new("xdg-open").arg(path).spawn().map(drop)
}

pub fn properties(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "Properties is only available on Windows"))
}

pub fn open_terminal(_dir: &Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "Only available on Windows"))
}

fn copy_rec(from: &Path, to: &Path) -> io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for de in std::fs::read_dir(from)? {
            let de = de?;
            copy_rec(&de.path(), &to.join(de.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(drop)
    }
}

pub fn file_op(op: FileOp, sources: &[PathBuf], dest: Option<&Path>, _interactive: bool) -> io::Result<bool> {
    for s in sources {
        let target = || -> io::Result<PathBuf> {
            let d = dest.ok_or_else(|| io::Error::other("no destination"))?;
            Ok(d.join(s.file_name().ok_or_else(|| io::Error::other("bad source"))?))
        };
        match op {
            FileOp::Copy => {
                let t = target()?;
                if t.exists() {
                    return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} exists", t.display())));
                }
                copy_rec(s, &t)?
            }
            FileOp::Move => std::fs::rename(s, target()?)?,
            FileOp::Recycle => {
                return Err(io::Error::new(io::ErrorKind::Unsupported, "No Recycle Bin here; use Shift+Delete"));
            }
            FileOp::Delete if s.is_dir() => std::fs::remove_dir_all(s)?,
            FileOp::Delete => std::fs::remove_file(s)?,
        }
    }
    Ok(true)
}

pub fn clipboard_set_files(paths: &[PathBuf], cut: bool) -> io::Result<()> {
    *CLIPBOARD.lock().unwrap() = Some((paths.to_vec(), cut));
    Ok(())
}

pub fn clipboard_get_files() -> Option<(Vec<PathBuf>, bool)> {
    CLIPBOARD.lock().unwrap().clone()
}

/// Polls the folder's modification time once a second (development only).
pub fn watch_dir(dir: &Path, cancel: &AtomicBool, on_change: &mut dyn FnMut()) -> io::Result<()> {
    let mtime = || std::fs::metadata(dir).and_then(|m| m.modified()).ok();
    let mut last = mtime();
    while !cancel.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let now = mtime();
        if now != last {
            last = now;
            on_change();
        }
    }
    Ok(())
}

pub fn ctrl_v_down() -> bool {
    false
}

pub fn shell_icon(_path: &Path, _is_dir: bool, _per_file: bool) -> Option<IconImage> {
    None
}

fn no_smb() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, "SMB browsing is only available on Windows")
}
pub fn list_shares(_server: &str) -> io::Result<Vec<Share>> {
    Err(no_smb())
}
pub fn connect(_remote: &Path, _prompt: bool) -> io::Result<()> {
    Err(no_smb())
}
pub fn map_drive(_letter: char, _remote: &Path, _persistent: bool, _interactive: bool) -> io::Result<()> {
    Err(no_smb())
}
pub fn unmap_drive(_letter: char) -> io::Result<()> {
    Err(no_smb())
}
pub fn network_computers() -> Vec<String> {
    Vec::new()
}
pub fn free_drive_letters() -> Vec<char> {
    Vec::new()
}

pub fn recycle_bin_items() -> Vec<BinItem> {
    Vec::new()
}
pub fn empty_recycle_bin(_confirm: bool) -> io::Result<()> {
    Err(no_smb())
}

/// One file or folder's entry. Development stand-in: lists the parent.
pub fn stat(path: &Path) -> Option<Entry> {
    let (parent, name) = (path.parent()?, path.file_name()?.to_string_lossy().into_owned());
    let mut out = None;
    let _ = list_dir(parent, &AtomicBool::new(false), &mut |batch| {
        if out.is_none() {
            out = batch.into_iter().find(|e| e.name == name);
        }
    });
    out
}
