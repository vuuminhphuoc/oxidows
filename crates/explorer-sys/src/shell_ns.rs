//! Shell namespace folders that aren't file-system paths: phones and cameras (MTP), Control
//! Panel, media servers, everything Explorer shows under This PC and Network that has no
//! drive letter. Items are listed through `IShellItem` and named `::shell\<parsing name>`
//! (or by their real path when they have one); opening, the context menu, copy and paste go
//! back to the shell, which knows how to talk to the device.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use explorer_core::{Entry, shell_parsing, shell_path};
use windows::Win32::Storage::EnhancedStorage::{PKEY_DateModified, PKEY_ItemTypeText, PKEY_Size};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::SystemServices::{SFGAO_FILESYSTEM, SFGAO_FOLDER, SFGAO_HIDDEN, SFGAO_STREAM};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, IEnumShellItems, IShellItem, IShellItem2, SHCreateItemFromParsingName, SIGDN,
    SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY,
};
use windows::core::{Interface, PCWSTR};

fn to_io(e: windows::core::Error) -> io::Error {
    io::Error::other(e.message())
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub(crate) fn item(path: &Path) -> io::Result<IShellItem> {
    crate::windows::com_init();
    let w = wide(&shell_parsing(path));
    unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None) }.map_err(to_io)
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
        let batch: Vec<Entry> =
            buf.iter_mut().take(n as usize).filter_map(Option::take).filter_map(|c| entry_of(&c)).collect();
        if !batch.is_empty() {
            sink(batch);
        }
    }
    Ok(())
}

/// Runs the item's default verb, as a double-click in Explorer (a Control Panel applet, a
/// photo on a phone, which the shell copies to a temporary file first).
pub fn open(hwnd: isize, path: &Path) -> io::Result<()> {
    use windows::Win32::UI::Shell::{ILFree, SHParseDisplayName};
    use windows_sys::Win32::UI::Shell::{SEE_MASK_INVOKEIDLIST, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    crate::windows::com_init();
    let w = wide(&shell_parsing(path));
    let mut pidl = std::ptr::null_mut();
    unsafe { SHParseDisplayName(PCWSTR(w.as_ptr()), None, &mut pidl, 0, None) }.map_err(to_io)?;
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_INVOKEIDLIST;
    info.hwnd = hwnd as _;
    info.lpIDList = pidl.cast();
    info.nShow = SW_SHOWNORMAL;
    let ok = unsafe { ShellExecuteExW(&mut info) } != 0;
    let r = if ok { Ok(()) } else { Err(io::Error::last_os_error()) };
    unsafe { ILFree(Some(pidl)) };
    r
}
