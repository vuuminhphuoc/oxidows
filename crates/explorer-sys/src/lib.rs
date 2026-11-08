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
mod default_handler;
#[cfg(windows)]
mod libraries;
#[cfg(windows)]
mod preview;
#[cfg(windows)]
mod props;
#[cfg(windows)]
mod search;
#[cfg(windows)]
mod shell_menu;
#[cfg(windows)]
mod shell_ns;
#[cfg(windows)]
mod thumbs;
#[cfg(windows)]
mod wifi_direct;
#[cfg(windows)]
pub use preview::PreviewHost;

/// Whether files with extension `ext` ("docx") have a registered preview handler.
pub fn has_preview_handler(ext: &str) -> bool {
    #[cfg(windows)]
    return preview::handler_for(ext).is_some();
    #[cfg(not(windows))]
    {
        let _ = ext;
        false
    }
}

/// The security zone a downloaded file came from (its Mark of the Web: the `Zone.Identifier`
/// stream): 3 = Internet, 4 = Restricted. `None` when it has none (made locally, FAT drives).
pub fn zone_of(path: &Path) -> Option<u32> {
    #[cfg(windows)]
    {
        use std::io::Read;
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        let mut text = String::new();
        std::fs::File::open(stream).ok()?.take(64 * 1024).read_to_string(&mut text).ok()?;
        explorer_core::parse_zone_id(&text)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotspotStatus {
    Starting,
    /// Phones can join now.
    Started,
    Stopped,
    Failed(String),
}

/// A Wi-Fi Direct group this PC owns, which phones join as an ordinary Wi-Fi network. Stops when
/// dropped.
pub struct Hotspot {
    #[cfg(windows)]
    inner: wifi_direct::Hotspot,
}

impl Hotspot {
    /// `ssid` up to 32 bytes; `passphrase` 8 to 63 characters.
    pub fn start(ssid: &str, passphrase: &str) -> io::Result<Self> {
        #[cfg(windows)]
        return Ok(Self { inner: wifi_direct::Hotspot::start(ssid, passphrase)? });
        #[cfg(not(windows))]
        {
            let _ = (ssid, passphrase);
            Err(io::Error::new(io::ErrorKind::Unsupported, "Wi-Fi Direct needs Windows"))
        }
    }

    pub fn status(&self) -> HotspotStatus {
        #[cfg(windows)]
        return self.inner.status();
        #[cfg(not(windows))]
        HotspotStatus::Stopped
    }
}

/// What Windows Firewall does with connections to this exe from phones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirewallState {
    /// The nearby sharing rule is in place.
    Allowed,
    /// No rule yet: private networks may work after the first-run prompt, public ones do not.
    Missing,
    /// A block rule (left by a cancelled first-run prompt, or added by hand) stops phones.
    Blocked,
}

/// Reads the inbound firewall rules for this exe.
pub fn firewall_state() -> FirewallState {
    #[cfg(windows)]
    return wifi_direct::firewall_state();
    #[cfg(not(windows))]
    FirewallState::Allowed
}

/// Replaces this exe's inbound firewall rules with one allow rule on all network profiles (asks
/// for admin rights).
pub fn allow_in_firewall() -> io::Result<()> {
    #[cfg(windows)]
    return wifi_direct::allow_in_firewall();
    #[cfg(not(windows))]
    Ok(())
}

/// Off Windows there are no preview handlers; this stands in so the app needs no `cfg`.
#[cfg(not(windows))]
pub struct PreviewHost;

#[cfg(not(windows))]
impl PreviewHost {
    pub fn open(_parent: isize, _path: &Path, _rect: [i32; 4]) -> io::Result<Self> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "preview handlers need Windows"))
    }
    pub fn place(&mut self, _rect: Option<[i32; 4]>) {}
}

#[cfg(not(windows))]
mod unix;
#[cfg(not(windows))]
use unix as imp;

/// Entries are handed over in batches of this many so a huge folder shows up progressively.
pub const BATCH: usize = 2048;

/// Display text of each property column for `path` ("" where it has none; always "" off
/// Windows). Slow-ish (opens the file's property handler): call off the UI thread.
pub fn read_props(path: &Path, props: &[explorer_core::Prop]) -> Vec<String> {
    #[cfg(windows)]
    return props::read_props(path, props);
    #[cfg(not(windows))]
    {
        let _ = path;
        vec![String::new(); props.len()]
    }
}

/// The libraries (display name, folders), e.g. ("Documents", [C:\\Users\\a\\Documents]).
/// Without library files (or off Windows) the four standard ones are made from the known
/// folders.
pub fn libraries() -> Vec<(String, Vec<PathBuf>)> {
    #[cfg(windows)]
    let found = libraries::libraries();
    #[cfg(not(windows))]
    let found: Vec<(String, Vec<PathBuf>)> = Vec::new();
    if !found.is_empty() {
        return found;
    }
    known_folders()
        .into_iter()
        .filter(|(n, _)| matches!(*n, "Documents" | "Music" | "Pictures" | "Videos"))
        .map(|(n, p)| (n.to_string(), vec![p]))
        .collect()
}

/// Whether folders (and Win+E) open in oxi-explorer for this user.
pub fn is_default_file_manager() -> bool {
    #[cfg(windows)]
    return default_handler::is_default();
    #[cfg(not(windows))]
    false
}

/// Makes `exe` (or, with `None`, the built-in Explorer again) open folders, drives and Win+E,
/// for this user only.
pub fn set_default_file_manager(exe: Option<&Path>) -> io::Result<()> {
    #[cfg(windows)]
    return match exe {
        Some(exe) => default_handler::set_default(exe),
        None => default_handler::unset_default(),
    };
    #[cfg(not(windows))]
    {
        let _ = exe;
        Err(io::Error::new(io::ErrorKind::Unsupported, "only on Windows"))
    }
}

/// Lists a shell namespace folder (`::shell\\…`, or This PC / Network by CLSID): phones,
/// Control Panel, media servers. Items are named by full path (`::shell\\…` or a real path)
/// with their display name as `label`. Off Windows: `Unsupported`.
pub fn shell_list(path: &Path, cancel: &AtomicBool, sink: &mut dyn FnMut(Vec<Entry>)) -> io::Result<()> {
    #[cfg(windows)]
    return shell_ns::list(path, cancel, sink);
    #[cfg(not(windows))]
    {
        let _ = (path, cancel, sink);
        Err(io::Error::new(io::ErrorKind::Unsupported, "the shell namespace needs Windows"))
    }
}

/// Runs a shell item's default verb (opens an applet, a file on a phone…).
pub fn shell_open(hwnd: isize, path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    return shell_ns::open(hwnd, path);
    #[cfg(not(windows))]
    {
        let _ = (hwnd, path);
        Err(io::Error::new(io::ErrorKind::Unsupported, "the shell namespace needs Windows"))
    }
}

/// Whether a shell item is a folder one can go into.
pub fn shell_is_folder(path: &Path) -> bool {
    #[cfg(windows)]
    return shell_ns::is_folder(path);
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

/// A shell item's display name ("Galaxy S24").
pub fn shell_display_name(path: &Path) -> Option<String> {
    #[cfg(windows)]
    return shell_ns::display_name(path);
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

/// Runs a canonical verb of the shell's menu for `items` (or the folder `dir` when empty)
/// without showing it: "copy", "cut", "paste", "delete", "properties".
pub fn shell_verb(hwnd: isize, dir: &Path, items: &[PathBuf], verb: &str) -> io::Result<()> {
    #[cfg(windows)]
    return shell_menu::invoke_verb(hwnd, dir, items, verb);
    #[cfg(not(windows))]
    {
        let _ = (hwnd, dir, items, verb);
        Err(io::Error::new(io::ErrorKind::Unsupported, "shell verbs need Windows"))
    }
}

/// A phone, camera or media player under This PC: a shell folder with no drive letter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellDevice {
    /// `::shell\…` app path.
    pub path: PathBuf,
    /// "Galaxy S24".
    pub label: String,
    /// "Portable Device".
    pub kind: String,
    /// All its storages together, in bytes; 0 when unknown (locked phone, camera).
    pub total: u64,
    pub free: u64,
}

/// Phones and other devices under This PC, with their storage space. Talks to each device
/// (tens of milliseconds); call off the UI thread. Empty off Windows.
pub fn shell_devices() -> Vec<ShellDevice> {
    #[cfg(windows)]
    return shell_ns::devices();
    #[cfg(not(windows))]
    Vec::new()
}

/// Free bytes of a phone's storage (`::shell\…\SID-{…}`).
pub fn shell_free_space(path: &Path) -> Option<u64> {
    #[cfg(windows)]
    return shell_ns::free_space(path);
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

/// Pastes the clipboard (files, or shell items such as a phone's photos) into `dest`, which
/// may be a phone folder, through the shell's copy engine. Blocks; call off the UI thread.
/// `Ok(false)`: cancelled, or nothing to paste.
pub fn shell_paste(hwnd: isize, dest: &Path) -> io::Result<bool> {
    #[cfg(windows)]
    return shell_ns::paste(hwnd, dest);
    #[cfg(not(windows))]
    {
        let _ = (hwnd, dest);
        Err(io::Error::new(io::ErrorKind::Unsupported, "the shell namespace needs Windows"))
    }
}

/// Deletes shell items (files on a phone), with the shell's confirmation. Blocks.
pub fn shell_delete(hwnd: isize, items: &[PathBuf]) -> io::Result<bool> {
    #[cfg(windows)]
    return shell_ns::delete(hwnd, items);
    #[cfg(not(windows))]
    {
        let _ = (hwnd, items);
        Err(io::Error::new(io::ErrorKind::Unsupported, "the shell namespace needs Windows"))
    }
}

/// One file or folder's entry; its `name` is the last path component.
pub fn stat(path: &Path) -> Option<Entry> {
    imp::stat(path)
}

/// Searches `root` (and below) through the Windows shell: the Windows Search index where the
/// location is indexed (file contents and every property included), a crawl otherwise. Calls
/// `found` with each result's path. Elsewhere: `Unsupported`, and the app's own search runs.
pub fn shell_search(root: &Path, query: &str, cancel: &AtomicBool, found: &mut dyn FnMut(PathBuf)) -> io::Result<()> {
    #[cfg(windows)]
    return search::shell_search(root, query, cancel, found);
    #[cfg(not(windows))]
    {
        let _ = (root, query, cancel, found);
        Err(io::Error::new(io::ErrorKind::Unsupported, "shell search needs Windows"))
    }
}

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
