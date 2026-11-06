//! OS layer for the File Explorer.
//!
//! Windows is the real target. A folder is read with one `FindFirstFileExW(FindExInfoBasic,
//! FIND_FIRST_EX_LARGE_FETCH)` pass: name, size, time and attributes come with each record, so
//! there is no per-file stat and no 8.3 short-name lookup. Copy, move and delete go through
//! `SHFileOperationW`, which gives the standard progress, conflict and Recycle Bin behaviour.
//! The clipboard uses `CF_HDROP`, so copy/paste works both ways with the built-in Explorer.
//!
//! The Unix module exists only so the app can be developed and screenshotted off Windows.
//! All `unsafe` in the explorer lives in this crate.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub use explorer_core::Entry;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

#[cfg(windows)]
mod shell_menu;
#[cfg(windows)]
mod thumbs;

#[cfg(not(windows))]
mod unix;
#[cfg(not(windows))]
use unix as imp;

/// Entries are handed over in batches of this many so a huge folder shows up progressively.
pub const BATCH: usize = 2048;

/// Reads `dir` and calls `sink` with batches of entries ("." and ".." skipped). Stops early,
/// returning `Ok`, once `cancel` is set (the user navigated elsewhere).
pub fn list_dir(dir: &Path, cancel: &AtomicBool, sink: &mut dyn FnMut(Vec<Entry>)) -> io::Result<()> {
    imp::list_dir(dir, cancel, sink)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveKind {
    Fixed,
    Removable,
    Network,
    Optical,
    Other,
}

#[derive(Clone, Debug)]
pub struct Drive {
    /// "C:\" on Windows, a mount point elsewhere.
    pub root: PathBuf,
    /// Volume label ("Windows", "Data"); may be empty.
    pub label: String,
    pub kind: DriveKind,
    pub total: u64,
    pub free: u64,
}

/// Mounted drives with label and free space. Network and optical drives are not queried for
/// size (that can block for seconds on a sleeping share or empty tray).
pub fn drives() -> Vec<Drive> {
    imp::drives()
}

/// Quick-access places (Desktop, Downloads, Documents, …) that exist on this machine.
pub fn known_folders() -> Vec<(&'static str, PathBuf)> {
    imp::known_folders()
}

pub fn home_dir() -> PathBuf {
    imp::home_dir()
}

/// Seconds to add to UTC for local time (current offset, including daylight saving).
pub fn utc_offset_secs() -> i64 {
    imp::utc_offset_secs()
}

/// Opens a file with its default program, or runs it (`ShellExecuteW` "open").
pub fn open(path: &Path) -> io::Result<()> {
    imp::open(path)
}

/// The shell's Properties dialog for a file or folder.
pub fn properties(path: &Path) -> io::Result<()> {
    imp::properties(path)
}

/// Opens a terminal (Windows Terminal if installed, else cmd) in `dir`.
pub fn open_terminal(dir: &Path) -> io::Result<()> {
    imp::open_terminal(dir)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOp {
    Copy,
    Move,
    /// To the Recycle Bin.
    Recycle,
    /// Permanently, after the shell's own warning.
    Delete,
}

/// Runs a shell file operation and blocks until it finishes (call from a worker thread).
/// Windows shows its own progress and "replace or skip" dialogs. Returns `Ok(false)` when the
/// user cancelled part of it.
pub fn file_op(op: FileOp, sources: &[PathBuf], dest: Option<&Path>) -> io::Result<bool> {
    imp::file_op(op, sources, dest, true)
}

/// Like [`file_op`] but never shows UI: no progress, no confirmation, existing targets are
/// overwritten. For tests and scripted use.
pub fn file_op_silent(op: FileOp, sources: &[PathBuf], dest: Option<&Path>) -> io::Result<bool> {
    imp::file_op(op, sources, dest, false)
}

/// Blocks until `dir`'s contents change (a file or folder added, removed, renamed, resized or
/// written), calling `on_change` each time, until `cancel` is set. Windows uses
/// `FindFirstChangeNotificationW`; the wait costs no CPU.
pub fn watch_dir(dir: &Path, cancel: &AtomicBool, on_change: &mut dyn FnMut()) -> io::Result<()> {
    imp::watch_dir(dir, cancel, on_change)
}

/// Ctrl+V held right now. egui only reports Ctrl+V when the clipboard holds text, so pasting
/// files needs this (Windows `GetKeyState`; always false elsewhere).
pub fn ctrl_v_down() -> bool {
    imp::ctrl_v_down()
}

/// Puts paths on the system clipboard as files (`CF_HDROP` + "Preferred DropEffect"), so the
/// built-in Explorer can paste them too. `cut` marks them to be moved.
pub fn clipboard_set_files(paths: &[PathBuf], cut: bool) -> io::Result<()> {
    imp::clipboard_set_files(paths, cut)
}

/// Files on the system clipboard and whether they were cut (move) rather than copied.
pub fn clipboard_get_files() -> Option<(Vec<PathBuf>, bool)> {
    imp::clipboard_get_files()
}

/// A small (16 px at 100 % scaling) icon as straight RGBA, row-major, top row first.
#[derive(Clone, Debug)]
pub struct IconImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// The shell's small icon. With `per_file` false only the extension (or "folder") matters and
/// the disk is not touched (`SHGFI_USEFILEATTRIBUTES`), so one call serves every `.txt`. Use
/// `per_file` for items whose icon is inside the file itself: `.exe`, `.lnk`, `.ico`.
/// Call from a worker thread; `None` where there is no shell (non-Windows) or on failure.
pub fn shell_icon(path: &Path, is_dir: bool, per_file: bool) -> Option<IconImage> {
    imp::shell_icon(path, is_dir, per_file)
}

// ---- SMB / network -------------------------------------------------------------------------

/// A shared folder on a server, as `\\server` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Share {
    pub name: String,
    pub remark: String,
}

/// Disk shares of `server` (a name or IP, without backslashes), hidden `$` shares left out.
/// Uses `NetShareEnum`, i.e. the SMB server's own list.
pub fn list_shares(server: &str) -> io::Result<Vec<Share>> {
    imp::list_shares(server)
}

/// Opens an SMB session to `remote` (`\\server\share`). With `prompt`, Windows asks for a
/// user name and password (and can remember them) when the current login is not accepted.
pub fn connect(remote: &Path, prompt: bool) -> io::Result<()> {
    imp::connect(remote, prompt)
}

/// Maps `letter:` to `remote`. `persistent` reconnects it at sign-in. `interactive` lets
/// Windows ask for credentials.
pub fn map_drive(letter: char, remote: &Path, persistent: bool, interactive: bool) -> io::Result<()> {
    imp::map_drive(letter, remote, persistent, interactive)
}

/// Disconnects a mapped drive and forgets it at sign-in.
pub fn unmap_drive(letter: char) -> io::Result<()> {
    imp::unmap_drive(letter)
}

/// Computers visible on the local network (`\\NAME`), from the network provider's browse
/// list. Can take seconds and depends on Network Discovery; call from a worker thread.
pub fn network_computers() -> Vec<String> {
    imp::network_computers()
}

/// Drive letters not in use, for "Map network drive".
pub fn free_drive_letters() -> Vec<char> {
    imp::free_drive_letters()
}

/// Errors that a login (another user name / password) could fix.
pub fn is_auth_error(e: &io::Error) -> bool {
    // ACCESS_DENIED, INVALID_PASSWORD, LOGON_FAILURE, NOT_AUTHENTICATED, ACCOUNT_RESTRICTION,
    // SESSION_CREDENTIAL_CONFLICT is not here: it needs a disconnect first.
    matches!(e.raw_os_error(), Some(5 | 86 | 1326 | 1244 | 1327))
}

/// What happened with the shell's right-click menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuOutcome {
    Dismissed,
    /// The shell ran the command (its canonical verb, may be empty for third-party items).
    Invoked(String),
    /// A command the app has to carry out itself: "rename", or "open"/"explore" on folders.
    Host(String),
}

/// Shows the shell's own context menu at the mouse pointer for `items` (full paths, all in
/// `dir`), or the folder background menu when `items` is empty. `hwnd` is the app window.
/// `extended` (Shift held) adds the extended verbs, as in Explorer. Blocks until closed.
/// Not available off Windows (`Unsupported`).
pub fn shell_context_menu(hwnd: isize, dir: &Path, items: &[PathBuf], extended: bool) -> io::Result<MenuOutcome> {
    #[cfg(windows)]
    {
        shell_menu::show(hwnd, dir, items, extended)
    }
    #[cfg(not(windows))]
    {
        let _ = (hwnd, dir, items, extended);
        Err(io::Error::new(io::ErrorKind::Unsupported, "shell menu is Windows-only"))
    }
}

/// The canonical verbs ("open", "properties", "delete", …) the shell menu would offer for
/// `items`, without showing it. Windows only.
pub fn shell_menu_verbs(dir: &Path, items: &[PathBuf]) -> io::Result<Vec<String>> {
    #[cfg(windows)]
    {
        shell_menu::verbs(dir, items)
    }
    #[cfg(not(windows))]
    {
        let _ = (dir, items);
        Err(io::Error::new(io::ErrorKind::Unsupported, "shell menu is Windows-only"))
    }
}

/// Where a `.lnk` shortcut points. Windows only (`None` elsewhere).
pub fn resolve_link(lnk: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        shell_menu::resolve_link(lnk)
    }
    #[cfg(not(windows))]
    {
        let _ = lnk;
        None
    }
}

/// What the drop target did with a drag from [`drag_out`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropResult {
    /// Cancelled, or dropped where files aren't accepted.
    None,
    Copied,
    /// The target moved the files (they are gone from here).
    Moved,
    Linked,
}

/// Starts a system drag of `items` (all in one folder) so they can be dropped on other
/// programs. Call while the mouse button is held; blocks until the drop. Windows only.
pub fn drag_out(hwnd: isize, items: &[PathBuf]) -> io::Result<DropResult> {
    #[cfg(windows)]
    {
        shell_menu::drag_out(hwnd, items)
    }
    #[cfg(not(windows))]
    {
        let _ = (hwnd, items);
        Err(io::Error::new(io::ErrorKind::Unsupported, "drag out is Windows-only"))
    }
}

// ---- Recycle Bin ---------------------------------------------------------------------------

/// One deleted item. Windows keeps `$R…` (the data) and `$I…` (where it came from, when) side
/// by side in `X:\$Recycle.Bin\<user SID>\`; that is all a restore needs.
#[derive(Clone, Debug)]
pub struct BinItem {
    /// Where it was deleted from, full path.
    pub original: PathBuf,
    /// Seconds since 1970 UTC.
    pub deleted: i64,
    pub size: u64,
    pub is_dir: bool,
    /// The `$R…` data file or folder.
    pub data: PathBuf,
    /// The `$I…` record.
    pub info: PathBuf,
}

/// Everything in the current user's Recycle Bin on every fixed drive, newest first.
pub fn recycle_bin_items() -> Vec<BinItem> {
    imp::recycle_bin_items()
}

/// Puts an item back where it was deleted from (fails if something is there now).
pub fn restore(item: &BinItem) -> io::Result<()> {
    if item.original.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", item.original.display()),
        ));
    }
    if let Some(parent) = item.original.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&item.data, &item.original)?;
    let _ = std::fs::remove_file(&item.info);
    Ok(())
}

/// Deletes an item from the bin for good.
pub fn purge(item: &BinItem) -> io::Result<()> {
    if item.is_dir {
        std::fs::remove_dir_all(&item.data)?
    } else {
        std::fs::remove_file(&item.data)?
    }
    let _ = std::fs::remove_file(&item.info);
    Ok(())
}

/// Empties the bin on all drives; with `confirm`, Windows asks first.
pub fn empty_recycle_bin(confirm: bool) -> io::Result<()> {
    imp::empty_recycle_bin(confirm)
}

/// Parses a `$I` record: version 1 (Vista–8.1, fixed 260-char path) or 2 (Windows 10+).
pub fn parse_bin_info(bytes: &[u8]) -> Option<(u64, i64, PathBuf)> {
    let u64_at = |o: usize| bytes.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()));
    let version = u64_at(0)?;
    let size = u64_at(8)?;
    let ft = u64_at(16)? as i64;
    let deleted = if ft == 0 { 0 } else { (ft - 116_444_736_000_000_000) / 10_000_000 };
    let wide: &[u8] = match version {
        1 => bytes.get(24..24 + 520)?,
        2 => {
            let n = u32::from_le_bytes(bytes.get(24..28)?.try_into().ok()?) as usize;
            bytes.get(28..28 + n * 2)?
        }
        _ => return None,
    };
    let units: Vec<u16> =
        wide.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|&c| c != 0).collect();
    Some((size, deleted, PathBuf::from(String::from_utf16_lossy(&units))))
}

/// A thumbnail at most `size` px on its longer side (pictures, videos, documents; for other
/// files the shell returns the large icon). RGBA, alpha premultiplied. Call from a worker
/// thread; `None` off Windows or on failure.
pub fn thumbnail(path: &Path, size: u32) -> Option<IconImage> {
    #[cfg(windows)]
    {
        thumbs::thumbnail(path, size)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, size);
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn bin_info_v2() {
        let path: Vec<u16> = "C:\\Users\\a\\x.txt".encode_utf16().chain(Some(0)).collect();
        let mut b = Vec::new();
        b.extend(2u64.to_le_bytes());
        b.extend(1234u64.to_le_bytes());
        b.extend((116_444_736_000_000_000u64 + 10_000_000 * 1_700_000_000).to_le_bytes());
        b.extend((path.len() as u32).to_le_bytes());
        for c in &path {
            b.extend(c.to_le_bytes());
        }
        let (size, deleted, p) = super::parse_bin_info(&b).unwrap();
        assert_eq!((size, deleted), (1234, 1_700_000_000));
        assert_eq!(p.to_string_lossy(), "C:\\Users\\a\\x.txt");
        assert!(super::parse_bin_info(&b[..10]).is_none());
    }
}
