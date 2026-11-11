//! Windows implementation: Win32 file API, shell32 for operations, user32 clipboard.

use std::cell::Cell;
use std::ffi::{OsString, c_void};
use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};

use explorer_core::Entry;
use windows_sys::Win32::Foundation::{FILETIME, GetLastError, GlobalFree, HWND, INVALID_HANDLE_VALUE, POINT};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW, HBITMAP,
    HDC, ReleaseDC,
};
use windows_sys::Win32::NetworkManagement::NetManagement::{MAX_PREFERRED_LENGTH, NetApiBufferFree};
use windows_sys::Win32::NetworkManagement::WNet::{
    CONNECT_INTERACTIVE, CONNECT_PROMPT, CONNECT_UPDATE_PROFILE, NETRESOURCEW, RESOURCE_GLOBALNET, RESOURCETYPE_DISK,
    RESOURCEUSAGE_CONTAINER, WNetAddConnection2W, WNetCancelConnection2W, WNetCloseEnum, WNetEnumResourceW,
    WNetOpenEnumW,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FIND_FIRST_EX_LARGE_FETCH, FindClose,
    FindCloseChangeNotification, FindExInfoBasic, FindExSearchNameMatch, FindFirstChangeNotificationW,
    FindFirstFileExW, FindNextChangeNotification, FindNextFileW, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives,
    GetVolumeInformationW, NetShareEnum, SHARE_INFO_1, STYPE_SPECIAL, WIN32_FIND_DATAW,
};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoTaskMemFree,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
    OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Ole::CF_HDROP;
use windows_sys::Win32::System::Threading::WaitForSingleObject;
use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL};
use windows_sys::Win32::UI::Shell::{
    AssocQueryStringW, DragQueryFileW, ExtractIconExW, FO_COPY, FO_DELETE, FO_MOVE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION,
    FOF_NOCONFIRMMKDIR, FOF_NOERRORUI, FOF_RENAMEONCOLLISION, FOF_SILENT, FOF_WANTNUKEWARNING, FOLDERID_Desktop,
    FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Profile, FOLDERID_Videos,
    SEE_MASK_INVOKEIDLIST, SHELLEXECUTEINFOW, SHERB_NOCONFIRMATION, SHEmptyRecycleBinW, SHFILEINFOW, SHFILEOPSTRUCTW,
    SHFileOperationW, SHGFI_ADDOVERLAYS, SHGFI_ICON, SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES,
    SHGetFileInfoW, SHGetKnownFolderPath, SHLoadIndirectString, ShellExecuteExW, ShellExecuteW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyIcon, GetIconInfo, HICON, HWND_MESSAGE, ICONINFO, SW_HIDE, SW_SHOWNORMAL,
};
use windows_sys::core::GUID;

use crate::{BATCH, BinItem, Drive, DriveKind, FileOp, IconImage, Share};

const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_NO_MORE_FILES: u32 = 18;
const ERROR_CANCELLED: i32 = 1223;
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;
const DRIVE_CDROM: u32 = 5;
const TIME_ZONE_ID_DAYLIGHT: u32 = 2;
const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_MOVE: u32 = 2;
/// 100 ns intervals between 1601-01-01 and 1970-01-01.
const FILETIME_UNIX_EPOCH: i64 = 116_444_736_000_000_000;

pub(crate) fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

fn from_wide(buf: &[u16]) -> OsString {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    OsString::from_wide(&buf[..n])
}

/// Paths past MAX_PATH need the `\\?\` prefix for the ANSI-era API limits to go away.
pub(crate) fn long_path(p: &Path) -> PathBuf {
    let s = p.as_os_str();
    if s.len() < 240 || s.to_string_lossy().starts_with(r"\\?\") {
        return p.to_path_buf();
    }
    let s = s.to_string_lossy();
    match s.strip_prefix(r"\\") {
        Some(unc) => PathBuf::from(format!(r"\\?\UNC\{unc}")),
        None => PathBuf::from(format!(r"\\?\{s}")),
    }
}

pub(crate) fn filetime_parts_to_unix(low: u32, high: u32) -> i64 {
    filetime_to_unix(FILETIME { dwLowDateTime: low, dwHighDateTime: high })
}

fn filetime_to_unix(ft: FILETIME) -> i64 {
    let t = ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64;
    if t == 0 { 0 } else { (t - FILETIME_UNIX_EPOCH) / 10_000_000 }
}

/// COM must be initialised on a thread before the shell is used from it. Calling it twice is
/// harmless (S_FALSE), and a thread winit already set up as OLE STA keeps its mode.
pub(crate) fn com_init() {
    thread_local!(static DONE: Cell<bool> = const { Cell::new(false) });
    DONE.with(|d| {
        if !d.get() {
            unsafe { CoInitializeEx(null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32) };
            d.set(true);
        }
    });
}

pub(crate) fn entry_flags(a: u32) -> (bool, bool, bool) {
    let hidden = a & FILE_ATTRIBUTE_HIDDEN != 0;
    let system = a & FILE_ATTRIBUTE_SYSTEM != 0 && hidden;
    let link = a & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    (hidden, system, link)
}

fn entry_from(fd: &WIN32_FIND_DATAW) -> Entry {
    let a = fd.dwFileAttributes;
    let is_dir = a & FILE_ATTRIBUTE_DIRECTORY != 0;
    let (hidden, system, link) = entry_flags(a);
    Entry {
        name: from_wide(&fd.cFileName).to_string_lossy().into_owned(),
        is_dir,
        size: if is_dir { 0 } else { ((fd.nFileSizeHigh as u64) << 32) | fd.nFileSizeLow as u64 },
        modified: filetime_to_unix(fd.ftLastWriteTime),
        created: filetime_to_unix(fd.ftCreationTime),
        attrs: a,
        hidden,
        system,
        link,
        label: None,
        note: None,
    }
}

/// One file or folder's entry (its name is the last path component).
pub fn stat(path: &Path) -> Option<Entry> {
    let p = wide(long_path(path));
    let mut fd: WIN32_FIND_DATAW = unsafe { zeroed() };
    let h = unsafe {
        FindFirstFileExW(
            p.as_ptr(),
            FindExInfoBasic,
            (&mut fd as *mut WIN32_FIND_DATAW).cast::<c_void>(),
            FindExSearchNameMatch,
            null(),
            0,
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return None;
    }
    unsafe { FindClose(h) };
    Some(entry_from(&fd))
}

pub fn list_dir(dir: &Path, cancel: &AtomicBool, sink: &mut dyn FnMut(Vec<Entry>)) -> io::Result<()> {
    let pattern = wide(long_path(&dir.join("*")));
    let mut fd: WIN32_FIND_DATAW = unsafe { zeroed() };
    let h = unsafe {
        FindFirstFileExW(
            pattern.as_ptr(),
            FindExInfoBasic,
            (&mut fd as *mut WIN32_FIND_DATAW).cast::<c_void>(),
            FindExSearchNameMatch,
            null(),
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };
    if h == INVALID_HANDLE_VALUE {
        let err = unsafe { GetLastError() };
        // An empty drive root has no "." entry and reports "not found".
        return if err == ERROR_FILE_NOT_FOUND { Ok(()) } else { Err(io::Error::from_raw_os_error(err as i32)) };
    }
    let mut batch = Vec::with_capacity(256);
    let result = loop {
        let name = &fd.cFileName;
        let dot = name[0] == '.' as u16 && (name[1] == 0 || (name[1] == '.' as u16 && name[2] == 0));
        if !dot {
            batch.push(entry_from(&fd));
            if batch.len() >= BATCH {
                sink(std::mem::replace(&mut batch, Vec::with_capacity(BATCH)));
            }
        }
        if cancel.load(Ordering::Relaxed) {
            break Ok(());
        }
        if unsafe { FindNextFileW(h, &mut fd) } == 0 {
            let err = unsafe { GetLastError() };
            break if err == ERROR_NO_MORE_FILES { Ok(()) } else { Err(io::Error::from_raw_os_error(err as i32)) };
        }
    };
    unsafe { FindClose(h) };
    if !batch.is_empty() {
        sink(batch);
    }
    result
}

pub fn drives() -> Vec<Drive> {
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let root = format!("{}:\\", (b'A' + i as u8) as char);
        let w = wide(&root);
        let kind = match unsafe { GetDriveTypeW(w.as_ptr()) } {
            DRIVE_FIXED => DriveKind::Fixed,
            DRIVE_REMOVABLE => DriveKind::Removable,
            DRIVE_REMOTE => DriveKind::Network,
            DRIVE_CDROM => DriveKind::Optical,
            _ => DriveKind::Other,
        };
        let (mut label, mut total, mut free) = (String::new(), 0u64, 0u64);
        // A sleeping network share or empty optical tray can stall for seconds; skip those.
        if matches!(kind, DriveKind::Fixed | DriveKind::Removable) {
            let mut name = [0u16; 261];
            let ok = unsafe {
                GetVolumeInformationW(
                    w.as_ptr(),
                    name.as_mut_ptr(),
                    name.len() as u32,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    0,
                )
            };
            if ok != 0 {
                label = from_wide(&name).to_string_lossy().into_owned();
            }
            unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut free, &mut total, null_mut()) };
        }
        out.push(Drive { root: PathBuf::from(root), label, kind, total, free });
    }
    out
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    let mut p = null_mut();
    let hr = unsafe { SHGetKnownFolderPath(id, 0, null_mut(), &mut p) };
    let out = if hr >= 0 && !p.is_null() {
        let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
        Some(PathBuf::from(OsString::from_wide(unsafe { std::slice::from_raw_parts(p, len) })))
    } else {
        None
    };
    unsafe { CoTaskMemFree(p as *const c_void) };
    out
}

pub fn known_folders() -> Vec<(&'static str, PathBuf)> {
    [
        ("Desktop", &FOLDERID_Desktop),
        ("Downloads", &FOLDERID_Downloads),
        ("Documents", &FOLDERID_Documents),
        ("Pictures", &FOLDERID_Pictures),
        ("Music", &FOLDERID_Music),
        ("Videos", &FOLDERID_Videos),
    ]
    .into_iter()
    .filter_map(|(n, id)| known_folder(id).filter(|p| p.is_dir()).map(|p| (n, p)))
    .collect()
}

pub fn home_dir() -> PathBuf {
    known_folder(&FOLDERID_Profile)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(r"C:\"))
}

pub fn utc_offset_secs() -> i64 {
    let mut tz: TIME_ZONE_INFORMATION = unsafe { zeroed() };
    let id = unsafe { GetTimeZoneInformation(&mut tz) };
    let bias = tz.Bias + if id == TIME_ZONE_ID_DAYLIGHT { tz.DaylightBias } else { tz.StandardBias };
    -(bias as i64) * 60
}

fn shell_execute(verb: &str, file: &str, params: Option<&str>, dir: Option<&Path>) -> io::Result<()> {
    com_init();
    let (verb, file) = (wide(verb), wide(file));
    let params = params.map(wide);
    let dir = dir.map(wide);
    let r = unsafe {
        ShellExecuteW(
            null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params.as_ref().map_or(null(), |p| p.as_ptr()),
            dir.as_ref().map_or(null(), |d| d.as_ptr()),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute returns a fake HINSTANCE: > 32 means success, otherwise an error code.
    if r as usize > 32 { Ok(()) } else { Err(io::Error::from_raw_os_error(r as i32)) }
}

/// Runs a program without a window, e.g. `runas` for an elevated command.
pub(crate) fn shell_execute_hidden(verb: &str, file: &str, params: &str) -> io::Result<()> {
    com_init();
    let (verb, file, params) = (wide(verb), wide(file), wide(params));
    let r = unsafe { ShellExecuteW(null_mut(), verb.as_ptr(), file.as_ptr(), params.as_ptr(), null(), SW_HIDE) };
    if r as usize > 32 { Ok(()) } else { Err(io::Error::from_raw_os_error(r as i32)) }
}

pub fn open(path: &Path) -> io::Result<()> {
    shell_execute("open", &path.to_string_lossy(), None, path.parent())
}

pub fn properties(path: &Path) -> io::Result<()> {
    com_init();
    let (verb, file) = (wide("properties"), wide(path));
    let mut info: SHELLEXECUTEINFOW = unsafe { zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_INVOKEIDLIST;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.nShow = SW_SHOWNORMAL;
    if unsafe { ShellExecuteExW(&mut info) } != 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

pub fn open_terminal(dir: &Path) -> io::Result<()> {
    let d = dir.to_string_lossy();
    // Windows Terminal ignores the working directory argument; it wants -d. Quote, and keep a
    // trailing backslash from escaping the closing quote ("C:\" -> "C:\\").
    let quoted = if d.ends_with('\\') { format!("-d \"{d}\\\"") } else { format!("-d \"{d}\"") };
    shell_execute("open", "wt.exe", Some(&quoted), Some(dir))
        .or_else(|_| shell_execute("open", "cmd.exe", None, Some(dir)))
}

/// A list of paths as the shell wants it: each NUL-terminated, plus a final NUL.
fn double_nul(paths: &[PathBuf]) -> Vec<u16> {
    let mut v = Vec::new();
    for p in paths {
        v.extend(p.as_os_str().encode_wide());
        v.push(0);
    }
    v.push(0);
    v
}

pub fn file_op(op: FileOp, sources: &[PathBuf], dest: Option<&Path>, interactive: bool) -> io::Result<bool> {
    if sources.is_empty() {
        return Ok(true);
    }
    com_init();
    let from = double_nul(sources);
    let to = dest.map(|d| double_nul(&[d.to_path_buf()]));
    let same_folder = dest.is_some_and(|d| sources.iter().all(|s| s.parent() == Some(d)));
    let (func, flags) = match op {
        // Pasting into the folder the files came from makes "name - Copy", as Explorer does.
        FileOp::Copy if same_folder => (FO_COPY, FOF_ALLOWUNDO | FOF_RENAMEONCOLLISION),
        FileOp::Copy => (FO_COPY, FOF_ALLOWUNDO),
        FileOp::Move if same_folder => return Ok(true),
        FileOp::Move => (FO_MOVE, FOF_ALLOWUNDO),
        // Modern Explorer does not ask before recycling; the shell still warns when an item is
        // too big for the bin and would be deleted for good.
        FileOp::Recycle => (FO_DELETE, FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING),
        FileOp::Delete => (FO_DELETE, 0),
    };
    let mut s: SHFILEOPSTRUCTW = unsafe { zeroed() };
    s.wFunc = func;
    s.pFrom = from.as_ptr();
    s.pTo = to.as_ref().map_or(null(), |t| t.as_ptr());
    let quiet = if interactive { 0 } else { FOF_SILENT | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_NOCONFIRMMKDIR };
    s.fFlags = (flags | quiet) as u16;
    let r = unsafe { SHFileOperationW(&mut s) };
    if r == ERROR_CANCELLED || s.fAnyOperationsAborted != 0 {
        return Ok(false);
    }
    // SHFileOperation returns its own codes, most of which are also Win32 error numbers.
    if r == 0 { Ok(true) } else { Err(io::Error::other(format!("file operation failed (code 0x{r:X})"))) }
}

/// Clipboard ownership needs a window; a message-only one is invisible and costs nothing.
fn clipboard_window() -> HWND {
    thread_local!(static HWND_CELL: Cell<usize> = const { Cell::new(0) });
    HWND_CELL.with(|c| {
        if c.get() == 0 {
            let class = wide("STATIC");
            let h = unsafe {
                CreateWindowExW(0, class.as_ptr(), null(), 0, 0, 0, 0, 0, HWND_MESSAGE, null_mut(), null_mut(), null())
            };
            c.set(h as usize);
        }
        c.get() as HWND
    })
}

/// Another program may hold the clipboard for a moment; retry briefly.
fn open_clipboard() -> io::Result<ClipboardGuard> {
    let hwnd = clipboard_window();
    for _ in 0..10 {
        if unsafe { OpenClipboard(hwnd) } != 0 {
            return Ok(ClipboardGuard);
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    Err(io::Error::last_os_error())
}

struct ClipboardGuard;
impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe { CloseClipboard() };
    }
}

#[repr(C)]
struct DropFiles {
    p_files: u32,
    pt: POINT,
    f_nc: i32,
    f_wide: i32,
}

fn drop_effect_format() -> u32 {
    let name = wide("Preferred DropEffect");
    unsafe { RegisterClipboardFormatW(name.as_ptr()) }
}

/// Copies `bytes` into a movable global block and hands it to the clipboard.
unsafe fn set_clip_bytes(format: u32, bytes: &[u8]) -> io::Result<()> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
        if h.is_null() {
            return Err(io::Error::last_os_error());
        }
        let p = GlobalLock(h).cast::<u8>();
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        GlobalUnlock(h);
        if SetClipboardData(format, h).is_null() {
            GlobalFree(h);
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

pub fn clipboard_set_files(paths: &[PathBuf], cut: bool) -> io::Result<()> {
    let header = DropFiles { p_files: size_of::<DropFiles>() as u32, pt: POINT { x: 0, y: 0 }, f_nc: 0, f_wide: 1 };
    let mut bytes: Vec<u8> = unsafe {
        std::slice::from_raw_parts((&header as *const DropFiles).cast::<u8>(), size_of::<DropFiles>()).to_vec()
    };
    for c in double_nul(paths) {
        bytes.extend_from_slice(&c.to_le_bytes());
    }
    let effect = if cut { DROPEFFECT_MOVE } else { DROPEFFECT_COPY };
    let _g = open_clipboard()?;
    unsafe {
        EmptyClipboard();
        set_clip_bytes(CF_HDROP as u32, &bytes)?;
        set_clip_bytes(drop_effect_format(), &effect.to_le_bytes())?;
    }
    Ok(())
}

pub fn clipboard_get_files() -> Option<(Vec<PathBuf>, bool)> {
    if unsafe { IsClipboardFormatAvailable(CF_HDROP as u32) } == 0 {
        return None;
    }
    let _g = open_clipboard().ok()?;
    let h = unsafe { GetClipboardData(CF_HDROP as u32) };
    if h.is_null() {
        return None;
    }
    let count = unsafe { DragQueryFileW(h, u32::MAX, null_mut(), 0) };
    let mut files = Vec::with_capacity(count as usize);
    for i in 0..count {
        let len = unsafe { DragQueryFileW(h, i, null_mut(), 0) };
        let mut buf = vec![0u16; len as usize + 1];
        unsafe { DragQueryFileW(h, i, buf.as_mut_ptr(), buf.len() as u32) };
        files.push(PathBuf::from(from_wide(&buf)));
    }
    Some((files, read_cut()))
}

/// Whether the clipboard's Preferred DropEffect says "move" (a cut). The clipboard is open.
fn read_cut() -> bool {
    let he = unsafe { GetClipboardData(drop_effect_format()) };
    if he.is_null() {
        return false;
    }
    let p = unsafe { GlobalLock(he) }.cast::<u32>();
    if p.is_null() {
        return false;
    }
    let cut = unsafe { p.read_unaligned() } & DROPEFFECT_MOVE != 0;
    unsafe { GlobalUnlock(he) };
    cut
}

/// Whether what is on the clipboard (files or shell items) was cut rather than copied.
pub fn clipboard_has_data() -> bool {
    let idlist = wide("Shell IDList Array");
    let shell = unsafe { RegisterClipboardFormatW(idlist.as_ptr()) };
    unsafe {
        IsClipboardFormatAvailable(CF_HDROP as u32) != 0 || (shell != 0 && IsClipboardFormatAvailable(shell) != 0)
    }
}

pub(crate) fn clipboard_is_cut() -> bool {
    open_clipboard().is_ok_and(|_g| read_cut())
}

pub fn clipboard_seq() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

const WAIT_OBJECT_0: u32 = 0;

pub fn watch_dir(dir: &Path, cancel: &AtomicBool, on_change: &mut dyn FnMut()) -> io::Result<()> {
    let w = wide(long_path(dir));
    let filter = FILE_NOTIFY_CHANGE_FILE_NAME
        | FILE_NOTIFY_CHANGE_DIR_NAME
        | FILE_NOTIFY_CHANGE_ATTRIBUTES
        | FILE_NOTIFY_CHANGE_SIZE
        | FILE_NOTIFY_CHANGE_LAST_WRITE;
    let h = unsafe { FindFirstChangeNotificationW(w.as_ptr(), 0, filter) };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let result = loop {
        if cancel.load(Ordering::Relaxed) {
            break Ok(());
        }
        // Wake up now and then only to notice `cancel`.
        if unsafe { WaitForSingleObject(h, 300) } == WAIT_OBJECT_0 {
            on_change();
            if unsafe { FindNextChangeNotification(h) } == 0 {
                break Err(io::Error::last_os_error());
            }
        }
    };
    unsafe { FindCloseChangeNotification(h) };
    result
}

pub fn ctrl_v_down() -> bool {
    unsafe { GetKeyState(VK_CONTROL as i32) < 0 && GetKeyState(b'V' as i32) < 0 }
}

const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;

/// Reads a bitmap as 32-bit top-down BGRA.
pub(crate) unsafe fn bitmap_bgra(dc: HDC, bmp: HBITMAP, w: i32, h: i32) -> Option<Vec<u8>> {
    let mut bi: BITMAPINFO = unsafe { zeroed() };
    bi.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w,
        biHeight: -h,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..unsafe { zeroed() }
    };
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let lines = unsafe { GetDIBits(dc, bmp, 0, h as u32, buf.as_mut_ptr().cast(), &mut bi, DIB_RGB_COLORS) };
    (lines == h).then_some(buf)
}

pub(crate) unsafe fn icon_image_from_hicon(hicon: HICON) -> Option<IconImage> {
    if hicon.is_null() {
        return None;
    }
    let mut ii: ICONINFO = unsafe { zeroed() };
    let got = unsafe { GetIconInfo(hicon, &mut ii) } != 0;
    let mut out = None;
    if got && !ii.hbmColor.is_null() {
        let mut bm: BITMAP = unsafe { zeroed() };
        unsafe { GetObjectW(ii.hbmColor, size_of::<BITMAP>() as i32, (&mut bm as *mut BITMAP).cast()) };
        let (bw, bh) = (bm.bmWidth, bm.bmHeight);
        if bw > 0 && bh > 0 && bw <= 256 && bh <= 256 {
            let dc = unsafe { GetDC(null_mut()) };
            if let Some(mut px) = unsafe { bitmap_bgra(dc, ii.hbmColor, bw, bh) } {
                // Old-style icons carry no alpha; transparency then comes from the AND mask.
                if px.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
                    let mask = if ii.hbmMask.is_null() { None } else { unsafe { bitmap_bgra(dc, ii.hbmMask, bw, bh) } };
                    for (i, p) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                        let transparent = mask.as_ref().is_some_and(|m| m[i * 4] != 0);
                        p[3] = if transparent { 0 } else { 255 };
                    }
                }
                for p in px.as_chunks_mut::<4>().0.iter_mut() {
                    p.swap(0, 2); // BGRA -> RGBA
                }
                out = Some(IconImage { width: bw as usize, height: bh as usize, rgba: px });
            }
            unsafe { ReleaseDC(null_mut(), dc) };
        }
    }
    unsafe {
        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor);
        }
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask);
        }
        DestroyIcon(hicon);
    }
    out
}

/// Resolves the shell icon for a file extension when SHGetFileInfoW falls back to the generic icon.
fn type_icon(ext: &str) -> Option<IconImage> {
    const ASSOCSTR_DEFAULTICON: i32 = 15;
    let dot_ext = wide(if ext.starts_with('.') { ext.to_string() } else { format!(".{ext}") });
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let hr = unsafe {
        AssocQueryStringW(0, ASSOCSTR_DEFAULTICON, dot_ext.as_ptr(), std::ptr::null(), buf.as_mut_ptr(), &mut len)
    };
    let mut path_str = if hr == 0 { unsafe { pwstr(buf.as_ptr()) } } else { String::new() };
    if path_str.starts_with('@') {
        let mut out = [0u16; 1024];
        let wide_val = wide(&path_str);
        let hr = unsafe {
            SHLoadIndirectString(wide_val.as_ptr(), out.as_mut_ptr(), out.len() as u32, std::ptr::null_mut())
        };
        if hr == 0 {
            path_str = unsafe { pwstr(out.as_ptr()) };
        }
    }
    // If the resolved icon path is an image file (e.g. PhotosLogoExtensions.png), load via thumbnail.
    if !path_str.is_empty() {
        let p = Path::new(&path_str);
        if let Some(ext) = p.extension().and_then(|e| e.to_str())
            && matches!(ext.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "ico" | "bmp")
            && p.exists()
            && let Some(img) = crate::thumbs::thumbnail(p, 16)
        {
            return Some(img);
        }
        let (file, idx) = if let Some((f, i)) = path_str.rsplit_once(',') {
            (f.trim_matches('"'), i.trim().parse::<i32>().unwrap_or(0))
        } else {
            (path_str.trim_matches('"'), 0)
        };
        let wide_file = wide(file);
        let mut small_hicon = null_mut();
        let n = unsafe { ExtractIconExW(wide_file.as_ptr(), idx, std::ptr::null_mut(), &mut small_hicon, 1) };
        if n > 0
            && !small_hicon.is_null()
            && let Some(img) = unsafe { icon_image_from_hicon(small_hicon) }
        {
            return Some(img);
        }
    }
    // Standard fallback icons for image types from imageres.dll / shell32.dll.
    let lower = ext.to_ascii_lowercase();
    let fallback = match lower.as_str() {
        "png" => Some((r"C:\Windows\System32\imageres.dll", -83)),
        "jpg" | "jpeg" => Some((r"C:\Windows\System32\imageres.dll", -72)),
        "gif" | "bmp" | "webp" | "ico" | "tiff" | "heic" | "svg" => Some((r"C:\Windows\System32\shell32.dll", -16823)),
        _ => None,
    };
    if let Some((dll, idx)) = fallback {
        let wide_file = wide(dll);
        let mut small_hicon = null_mut();
        let n = unsafe { ExtractIconExW(wide_file.as_ptr(), idx, std::ptr::null_mut(), &mut small_hicon, 1) };
        if n > 0
            && !small_hicon.is_null()
            && let Some(img) = unsafe { icon_image_from_hicon(small_hicon) }
        {
            return Some(img);
        }
    }
    None
}

pub fn shell_icon(path: &Path, is_dir: bool, per_file: bool) -> Option<IconImage> {
    com_init();
    let w = wide(path);
    let mut info: SHFILEINFOW = unsafe { zeroed() };
    let attrs = if is_dir { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_NORMAL };
    // Per item: with the shell's overlays (shortcut arrow, OneDrive status, sharing).
    let flags = SHGFI_ICON
        | SHGFI_SMALLICON
        | SHGFI_SYSICONINDEX
        | if per_file { SHGFI_ADDOVERLAYS } else { SHGFI_USEFILEATTRIBUTES };
    let ok = if path.starts_with(explorer_core::SHELL) {
        // Not a file-system path: ask by item ID list.
        let pidl = crate::shell_ns::pidl(path).ok()?;
        let flags = SHGFI_ICON | SHGFI_SMALLICON | windows_sys::Win32::UI::Shell::SHGFI_PIDL;
        unsafe { SHGetFileInfoW(pidl.as_ptr() as *const u16, 0, &mut info, size_of::<SHFILEINFOW>() as u32, flags) }
    } else {
        unsafe { SHGetFileInfoW(w.as_ptr(), attrs, &mut info, size_of::<SHFILEINFOW>() as u32, flags) }
    };
    if ok == 0 || info.hIcon.is_null() {
        return None;
    }
    // If the shell returned the generic document icon (iIcon == 0) for a file,
    // check whether this extension has an associated shell icon (e.g. UWP Photos,
    // or system icons in imageres.dll/shell32.dll for images).
    if !is_dir
        && info.iIcon == 0
        && let Some(ext) = path.extension().and_then(|e| e.to_str())
        && let Some(img) = type_icon(ext)
    {
        unsafe { DestroyIcon(info.hIcon) };
        return Some(img);
    }
    unsafe { icon_image_from_hicon(info.hIcon) }
}

const NERR_SUCCESS: u32 = 0;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_NO_MORE_ITEMS: u32 = 259;
const RESOURCEDISPLAYTYPE_SERVER: u32 = 2;

unsafe fn pwstr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
}

pub fn list_shares(server: &str) -> io::Result<Vec<Share>> {
    let name = wide(format!(r"\\{}", server.trim_start_matches('\\')));
    let mut out = Vec::new();
    let mut resume = 0u32;
    loop {
        let (mut buf, mut read, mut total) = (null_mut::<u8>(), 0u32, 0u32);
        let r = unsafe {
            NetShareEnum(name.as_ptr(), 1, &mut buf, MAX_PREFERRED_LENGTH, &mut read, &mut total, &mut resume)
        };
        if r != NERR_SUCCESS && r != ERROR_MORE_DATA {
            return Err(io::Error::from_raw_os_error(r as i32));
        }
        if !buf.is_null() {
            let items = unsafe { std::slice::from_raw_parts(buf.cast::<SHARE_INFO_1>(), read as usize) };
            for it in items {
                // Disk shares only (type 0), and not the hidden admin ones (C$, ADMIN$).
                if it.shi1_type & !STYPE_SPECIAL == 0 && it.shi1_type & STYPE_SPECIAL == 0 {
                    let name = unsafe { pwstr(it.shi1_netname) };
                    if !name.ends_with('$') {
                        out.push(Share { name, remark: unsafe { pwstr(it.shi1_remark) } });
                    }
                }
            }
            unsafe { NetApiBufferFree(buf.cast()) };
        }
        if r != ERROR_MORE_DATA {
            break;
        }
    }
    out.sort_by_key(|s| s.name.to_lowercase());
    Ok(out)
}

fn add_connection(local: Option<&str>, remote: &Path, flags: u32) -> io::Result<()> {
    let mut remote_w = wide(remote.as_os_str().to_string_lossy().trim_end_matches('\\'));
    let mut local_w = local.map(wide);
    let mut nr: NETRESOURCEW = unsafe { zeroed() };
    nr.dwType = RESOURCETYPE_DISK;
    nr.lpRemoteName = remote_w.as_mut_ptr();
    nr.lpLocalName = local_w.as_mut().map_or(null_mut(), |l| l.as_mut_ptr());
    com_init();
    let r = unsafe { WNetAddConnection2W(&nr, null(), null(), flags) };
    if r == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(r as i32)) }
}

pub fn connect(remote: &Path, prompt: bool) -> io::Result<()> {
    add_connection(None, remote, if prompt { CONNECT_INTERACTIVE | CONNECT_PROMPT } else { 0 })
}

pub fn map_drive(letter: char, remote: &Path, persistent: bool, interactive: bool) -> io::Result<()> {
    let local = format!("{}:", letter.to_ascii_uppercase());
    let flags = if persistent { CONNECT_UPDATE_PROFILE } else { 0 } | if interactive { CONNECT_INTERACTIVE } else { 0 };
    add_connection(Some(&local), remote, flags)
}

pub fn unmap_drive(letter: char) -> io::Result<()> {
    let local = wide(format!("{}:", letter.to_ascii_uppercase()));
    let r = unsafe { WNetCancelConnection2W(local.as_ptr(), CONNECT_UPDATE_PROFILE, 0) };
    if r == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(r as i32)) }
}

/// Walks the network provider tree (providers -> domains/workgroups -> servers).
fn enum_servers(container: Option<&NETRESOURCEW>, depth: u32, out: &mut Vec<String>) {
    if depth > 3 || out.len() >= 1000 {
        return;
    }
    let mut h = null_mut();
    let r = unsafe {
        WNetOpenEnumW(
            RESOURCE_GLOBALNET,
            RESOURCETYPE_DISK,
            RESOURCEUSAGE_CONTAINER,
            container.map_or(null(), |c| c as *const NETRESOURCEW),
            &mut h,
        )
    };
    if r != 0 {
        return;
    }
    let mut buf = vec![0u64; 16 * 1024 / 8]; // 16 KB, 8-byte aligned for NETRESOURCEW
    loop {
        let mut count = u32::MAX;
        let mut size = (buf.len() * 8) as u32;
        let r = unsafe { WNetEnumResourceW(h, &mut count, buf.as_mut_ptr().cast(), &mut size) };
        if r == ERROR_NO_MORE_ITEMS || (r != 0 && r != ERROR_MORE_DATA) {
            break;
        }
        if r == ERROR_MORE_DATA {
            buf.resize((size as usize).div_ceil(8), 0);
            continue;
        }
        let items = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<NETRESOURCEW>(), count as usize) };
        for nr in items {
            if nr.dwDisplayType == RESOURCEDISPLAYTYPE_SERVER {
                let name = unsafe { pwstr(nr.lpRemoteName) };
                if !name.is_empty() && !out.contains(&name) {
                    out.push(name);
                }
            } else if nr.dwUsage & RESOURCEUSAGE_CONTAINER != 0 {
                enum_servers(Some(nr), depth + 1, out);
            }
        }
    }
    unsafe { WNetCloseEnum(h) };
}

pub fn network_computers() -> Vec<String> {
    com_init();
    let mut out = Vec::new();
    enum_servers(None, 0, &mut out);
    out.sort_by_key(|s| s.to_lowercase());
    out
}

pub fn free_drive_letters() -> Vec<char> {
    let used = unsafe { GetLogicalDrives() };
    // A and B are left out, as Explorer does.
    (2..26u32).filter(|i| used & (1 << i) == 0).map(|i| (b'A' + i as u8) as char).collect()
}

pub fn recycle_bin_items() -> Vec<BinItem> {
    let mut out = Vec::new();
    for d in drives().into_iter().filter(|d| d.kind == DriveKind::Fixed) {
        let bin = d.root.join("$Recycle.Bin");
        let Ok(users) = std::fs::read_dir(&bin) else { continue };
        // Other users' folders are not readable; ours is (the SID folder of this account).
        for user in users.flatten() {
            let Ok(files) = std::fs::read_dir(user.path()) else { continue };
            for f in files.flatten() {
                let name = f.file_name().to_string_lossy().into_owned();
                let Some(id) = name.strip_prefix("$I") else { continue };
                let data = user.path().join(format!("$R{id}"));
                let Ok(md) = std::fs::symlink_metadata(&data) else { continue };
                let Some((size, deleted, original)) =
                    std::fs::read(f.path()).ok().and_then(|b| crate::parse_bin_info(&b))
                else {
                    continue;
                };
                out.push(BinItem { original, deleted, size, is_dir: md.is_dir(), data, info: f.path() });
            }
        }
    }
    out.sort_by_key(|i| std::cmp::Reverse(i.deleted));
    out
}

pub fn empty_recycle_bin(confirm: bool) -> io::Result<()> {
    com_init();
    let hr = unsafe { SHEmptyRecycleBinW(null_mut(), null(), if confirm { 0 } else { SHERB_NOCONFIRMATION }) };
    // S_OK, or E_UNEXPECTED when it was already empty.
    if hr >= 0 || hr == 0x8000_FFFF_u32 as i32 { Ok(()) } else { Err(io::Error::from_raw_os_error(hr)) }
}
