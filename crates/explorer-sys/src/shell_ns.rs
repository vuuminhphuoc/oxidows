//! Shell namespace folders that aren't file-system paths: phones and cameras (MTP), Control
//! Panel, media servers, everything Explorer shows under This PC and Network that has no
//! drive letter. Items are listed through `IShellItem` and named `::shell\<parsing name>`
//! (or by their real path when they have one); opening, the context menu, copy and paste go
//! back to the shell, which knows how to talk to the device.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use explorer_core::{Entry, shell_parsing, shell_path};
use windows::Win32::Foundation::HWND;
use windows::Win32::Storage::EnhancedStorage::{PKEY_DateModified, PKEY_ItemTypeText, PKEY_Size};
use windows::Win32::System::Com::{CoTaskMemAlloc, CoTaskMemFree};
use windows::Win32::System::SystemServices::{SFGAO_FILESYSTEM, SFGAO_FOLDER, SFGAO_HIDDEN, SFGAO_STREAM};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_EnumItems, BHID_SFObject, IEnumShellItems, IFileOperation, ILCombine, ILFree, ILGetSize, IShellFolder,
    IShellItem, IShellItem2, IShellItemArray, SHCreateItemFromIDList, SHCreateItemFromParsingName,
    SHCreateShellItemArrayFromDataObject, SHCreateShellItemArrayFromIDLists, SHGetIDListFromObject, SHParseDisplayName,
    SIGDN, SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING,
};
use windows::core::{Interface, PCWSTR};

fn to_io(e: windows::core::Error) -> io::Error {
    let m = e.message();
    // Many shell and WPD errors come without a system message.
    io::Error::other(if m.is_empty() { format!("error 0x{:08X}", e.code().0) } else { m })
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// An absolute item ID list, freed on drop.
pub(crate) struct Pidl(*mut ITEMIDLIST);

impl Pidl {
    pub(crate) fn as_ptr(&self) -> *const ITEMIDLIST {
        self.0
    }

    /// Hands the list over; the caller frees it with `ILFree`.
    pub(crate) fn into_raw(self) -> *mut ITEMIDLIST {
        let p = self.0;
        std::mem::forget(self);
        p
    }

    fn bytes(&self) -> Vec<u8> {
        let n = unsafe { ILGetSize(Some(self.0)) } as usize;
        unsafe { std::slice::from_raw_parts(self.0 as *const u8, n) }.to_vec()
    }

    fn from_bytes(b: &[u8]) -> Option<Pidl> {
        let p = unsafe { CoTaskMemAlloc(b.len()) } as *mut u8;
        if p.is_null() {
            return None;
        }
        unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), p, b.len()) };
        Some(Pidl(p.cast()))
    }
}

impl Drop for Pidl {
    fn drop(&mut self) {
        unsafe { ILFree(Some(self.0)) };
    }
}

/// ID lists of shell folders seen in listings, by lowercase parsing name. A phone's storage
/// (`SID-{10001,,…}`) has a parsing name the shell itself cannot parse back, so items under
/// it are found from their parent's ID list instead.
static FOLDER_IDS: Mutex<Option<HashMap<String, Vec<u8>>>> = Mutex::new(None);
/// More than this many cached folders and the cache starts over (it only saves round trips).
const MAX_CACHED: usize = 4096;

fn remember(parsing: &str, pidl: &Pidl) {
    let mut g = FOLDER_IDS.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    if map.len() >= MAX_CACHED {
        map.clear();
    }
    map.insert(parsing.to_lowercase(), pidl.bytes());
}

fn cached(parsing: &str) -> Option<Pidl> {
    let g = FOLDER_IDS.lock().unwrap_or_else(|e| e.into_inner());
    Pidl::from_bytes(g.as_ref()?.get(&parsing.to_lowercase())?)
}

fn pidl_of(item: &IShellItem) -> io::Result<Pidl> {
    unsafe { SHGetIDListFromObject(item) }.map(Pidl).map_err(to_io)
}

/// The ID list of `parsing`: from the cache, by parsing the whole name, or else level by
/// level from the closest ancestor that can be found (parsing the last ID inside its parent,
/// or matching it among the parent's children).
fn resolve(parsing: &str, depth: usize) -> io::Result<Pidl> {
    if let Some(p) = cached(parsing) {
        return Ok(p);
    }
    let w = wide(parsing);
    let mut raw = std::ptr::null_mut();
    let whole = unsafe { SHParseDisplayName(PCWSTR(w.as_ptr()), None, &mut raw, 0, None) };
    if whole.is_ok() && !raw.is_null() {
        return Ok(Pidl(raw));
    }
    let segs = explorer_core::shell_segments(parsing);
    if segs.len() < 2 || depth > 64 {
        return Err(whole.map_err(to_io).err().unwrap_or_else(|| io::Error::other("not found")));
    }
    let (last, up) = (&segs[segs.len() - 1], segs[..segs.len() - 1].join("\\"));
    let parent = resolve(&up, depth + 1)?;
    let parent_item: IShellItem = unsafe { SHCreateItemFromIDList(parent.as_ptr()) }.map_err(to_io)?;
    // Parsing inside the parent works for objects on a phone's storage.
    let child = (|| {
        let folder: IShellFolder = unsafe { parent_item.BindToHandler(None, &BHID_SFObject) }.ok()?;
        let lw = wide(last);
        let mut rel = std::ptr::null_mut();
        unsafe { folder.ParseDisplayName(HWND::default(), None, PCWSTR(lw.as_ptr()), None, &mut rel, null_mut()) }
            .ok()?;
        if rel.is_null() {
            return None;
        }
        let rel = Pidl(rel);
        let full = unsafe { ILCombine(Some(parent.as_ptr()), Some(rel.as_ptr())) };
        (!full.is_null()).then(|| Pidl(full))
    })();
    let found = match child {
        Some(p) => p,
        // Not parseable (the storage itself): look for it among the parent's children.
        None => {
            let items: IEnumShellItems = unsafe { parent_item.BindToHandler(None, &BHID_EnumItems) }.map_err(to_io)?;
            let mut hit = None;
            loop {
                let mut buf = [None];
                let mut n = 0u32;
                if unsafe { items.Next(&mut buf, Some(&mut n)) }.is_err() || n == 0 {
                    break;
                }
                let Some(c) = buf[0].take() else { break };
                if name(&c, SIGDN_PARENTRELATIVEPARSING).is_some_and(|s| s.eq_ignore_ascii_case(last)) {
                    hit = Some(pidl_of(&c)?);
                    break;
                }
            }
            hit.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{last} is no longer there")))?
        }
    };
    remember(parsing, &found);
    Ok(found)
}

/// The absolute ID list of a path or `::shell\\…` item.
pub(crate) fn pidl(path: &Path) -> io::Result<Pidl> {
    crate::windows::com_init();
    let parsing = shell_parsing(path);
    if !path.starts_with(explorer_core::SHELL) {
        let w = wide(&parsing);
        let mut raw = std::ptr::null_mut();
        unsafe { SHParseDisplayName(PCWSTR(w.as_ptr()), None, &mut raw, 0, None) }.map_err(to_io)?;
        return Ok(Pidl(raw));
    }
    resolve(&parsing, 0)
}

pub(crate) fn item(path: &Path) -> io::Result<IShellItem> {
    crate::windows::com_init();
    if !path.starts_with(explorer_core::SHELL) {
        let w = wide(&shell_parsing(path));
        return unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None) }.map_err(to_io);
    }
    let p = pidl(path)?;
    unsafe { SHCreateItemFromIDList(p.as_ptr()) }.map_err(to_io)
}

fn name(item: &IShellItem, kind: SIGDN) -> Option<String> {
    let p = unsafe { item.GetDisplayName(kind) }.ok()?;
    let s = unsafe { p.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

/// A folder in the shell's sense (not a zip file, which is a folder *and* a stream).
pub fn is_folder(path: &Path) -> bool {
    item(path).is_ok_and(|i| {
        let a = unsafe { i.GetAttributes(SFGAO_FOLDER | SFGAO_STREAM) }.unwrap_or_default();
        a.contains(SFGAO_FOLDER) && !a.contains(SFGAO_STREAM)
    })
}

/// The display name ("Galaxy S24", "Internal storage").
pub fn display_name(path: &Path) -> Option<String> {
    name(&item(path).ok()?, SIGDN_NORMALDISPLAY)
}

fn entry_of(child: &IShellItem) -> Option<Entry> {
    let mask = SFGAO_FOLDER | SFGAO_FILESYSTEM | SFGAO_STREAM | SFGAO_HIDDEN;
    let a = unsafe { child.GetAttributes(mask) }.unwrap_or_default();
    let display = name(child, SIGDN_NORMALDISPLAY)?;
    let fs = a.contains(SFGAO_FILESYSTEM).then(|| name(child, SIGDN_FILESYSPATH)).flatten();
    let full = match fs {
        Some(p) => PathBuf::from(p),
        None => shell_path(&name(child, SIGDN_DESKTOPABSOLUTEPARSING)?),
    };
    let i2: Option<IShellItem2> = child.cast().ok();
    let size = i2.as_ref().and_then(|i| unsafe { i.GetUInt64(&PKEY_Size) }.ok()).unwrap_or(0);
    let modified = i2
        .as_ref()
        .and_then(|i| unsafe { i.GetFileTime(&PKEY_DateModified) }.ok())
        .map_or(0, |ft| crate::windows::filetime_parts_to_unix(ft.dwLowDateTime, ft.dwHighDateTime));
    let kind = i2.as_ref().and_then(|i| unsafe { i.GetString(&PKEY_ItemTypeText) }.ok()).map(|p| {
        let s = unsafe { p.to_string() }.unwrap_or_default();
        unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
        s
    });
    let is_dir = a.contains(SFGAO_FOLDER) && !a.contains(SFGAO_STREAM);
    Some(Entry {
        name: full.to_string_lossy().into_owned(),
        label: Some(display),
        note: kind.filter(|k| !k.is_empty()),
        is_dir,
        size: if is_dir { 0 } else { size },
        modified,
        hidden: a.contains(SFGAO_HIDDEN),
        attrs: if is_dir { explorer_core::attr::DIRECTORY } else { explorer_core::attr::ARCHIVE },
        ..Default::default()
    })
}

/// The children of the shell folder `path` (an app path or a parsing name), in batches.
pub fn list(path: &Path, cancel: &AtomicBool, sink: &mut dyn FnMut(Vec<Entry>)) -> io::Result<()> {
    let folder = item(path)?;
    let items: IEnumShellItems = unsafe { folder.BindToHandler(None, &BHID_EnumItems) }.map_err(to_io)?;
    let mut buf: [Option<IShellItem>; 64] = std::array::from_fn(|_| None);
    while !cancel.load(Ordering::Relaxed) {
        let mut n = 0u32;
        unsafe { items.Next(&mut buf, Some(&mut n)) }.map_err(to_io)?;
        if n == 0 {
            break;
        }
        let batch: Vec<Entry> = buf
            .iter_mut()
            .take(n as usize)
            .filter_map(Option::take)
            .filter_map(|c| {
                let e = entry_of(&c)?;
                // Folders are entered next: keep their ID lists (see `FOLDER_IDS`).
                if e.is_dir
                    && e.name.starts_with(explorer_core::SHELL)
                    && let Ok(p) = pidl_of(&c)
                {
                    remember(&shell_parsing(Path::new(&e.name)), &p);
                }
                Some(e)
            })
            .collect();
        if !batch.is_empty() {
            sink(batch);
        }
    }
    Ok(())
}

/// Runs the item's default verb, as a double-click in Explorer (a Control Panel applet, a
/// photo on a phone, which the shell copies to a temporary file first).
pub fn open(hwnd: isize, path: &Path) -> io::Result<()> {
    use windows_sys::Win32::UI::Shell::{SEE_MASK_INVOKEIDLIST, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let id = pidl(path)?;
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_INVOKEIDLIST;
    info.hwnd = hwnd as _;
    info.lpIDList = id.as_ptr() as *mut _;
    info.nShow = SW_SHOWNORMAL;
    let ok = unsafe { ShellExecuteExW(&mut info) } != 0;
    if ok { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Runs one `IFileOperation` (the engine behind Explorer's copy, move and delete, with its
/// progress, conflict and confirmation dialogs). `Ok(false)` when the user cancelled.
fn file_operation(hwnd: isize, queue: impl FnOnce(&IFileOperation) -> windows::core::Result<()>) -> io::Result<bool> {
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
    use windows::Win32::UI::Shell::{FOF_ALLOWUNDO, FOF_NOCONFIRMMKDIR, FileOperation};
    let op: IFileOperation = unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_ALL) }.map_err(to_io)?;
    unsafe {
        if hwnd != 0 {
            op.SetOwnerWindow(HWND(hwnd as *mut _)).map_err(to_io)?;
        }
        op.SetOperationFlags(FOF_ALLOWUNDO | FOF_NOCONFIRMMKDIR).map_err(to_io)?;
    }
    queue(&op).map_err(to_io)?;
    let done = unsafe { op.PerformOperations() };
    if unsafe { op.GetAnyOperationsAborted() }.is_ok_and(|b| b.as_bool()) {
        return Ok(false);
    }
    done.map_err(to_io)?;
    Ok(true)
}

/// Shell items for `paths` (files on disk or `::shell\…` items), as one array.
fn item_array(paths: &[PathBuf]) -> io::Result<IShellItemArray> {
    let pidls = paths.iter().map(|p| pidl(p)).collect::<io::Result<Vec<_>>>()?;
    let ptrs: Vec<*const ITEMIDLIST> = pidls.iter().map(Pidl::as_ptr).collect();
    unsafe { SHCreateShellItemArrayFromIDLists(&ptrs) }.map_err(to_io)
}

/// Pastes the clipboard into `dest`: files copied or cut on disk, or shell items with no
/// path (a phone's photos, copied through the shell's own data object). Works both ways
/// between a phone and the disk, where the folder's own Paste verb does nothing. Blocks
/// while the shell copies; call from a worker thread. `Ok(false)`: cancelled or nothing to
/// paste.
pub fn paste(hwnd: isize, dest: &Path) -> io::Result<bool> {
    use windows::Win32::System::Ole::{OleGetClipboard, OleInitialize};
    // OleGetClipboard needs OLE on this thread, not just COM.
    let _ = unsafe { OleInitialize(None) };
    crate::windows::com_init();
    let target = item(dest)?;
    let (items, cut) = match crate::windows::clipboard_get_files() {
        Some((files, cut)) if !files.is_empty() => (item_array(&files)?, cut),
        _ => {
            let Ok(data) = (unsafe { OleGetClipboard() }) else { return Ok(false) };
            let Ok(items) = (unsafe { SHCreateShellItemArrayFromDataObject::<_, IShellItemArray>(&data) }) else {
                return Ok(false);
            };
            (items, crate::windows::clipboard_is_cut())
        }
    };
    file_operation(hwnd, |op| unsafe {
        if cut { op.MoveItems(&items, &target) } else { op.CopyItems(&items, &target) }
    })
}

/// Deletes shell items (a phone's files go for good, after the shell's own confirmation).
/// Blocks; call from a worker thread.
pub fn delete(hwnd: isize, items: &[PathBuf]) -> io::Result<bool> {
    crate::windows::com_init();
    let arr = item_array(items)?;
    file_operation(hwnd, |op| unsafe { op.DeleteItems(&arr) })
}
