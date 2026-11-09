//! Client for Everything search (voidtools) over its documented WM_COPYDATA IPC.
//!
//! All IPC constants and struct layouts are taken from `everything_ipc.h`.

use std::io;
#[cfg(windows)]
use std::path::Path;
#[cfg(windows)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
use crate::Entry;

pub const EVERYTHING_WM_IPC: u32 = 0x0400; // WM_USER
pub const EVERYTHING_IPC_WNDCLASSW: &str = "EVERYTHING_TASKBAR_NOTIFICATION";
pub const EVERYTHING_IPC_WNDCLASSW_15A: &str = "EVERYTHING_TASKBAR_NOTIFICATION_(1.5a)";
pub const EVERYTHING_IPC_IS_DB_LOADED: usize = 401;
pub const EVERYTHING_IPC_COPYDATA_QUERY2W: usize = 18;

pub const EVERYTHING_IPC_SORT_NAME_ASCENDING: u32 = 1;

pub const EVERYTHING_IPC_QUERY2_REQUEST_NAME: u32 = 0x00000001;
pub const EVERYTHING_IPC_QUERY2_REQUEST_PATH: u32 = 0x00000002;
pub const EVERYTHING_IPC_QUERY2_REQUEST_FULL_PATH_AND_NAME: u32 = 0x00000004;
pub const EVERYTHING_IPC_QUERY2_REQUEST_EXTENSION: u32 = 0x00000008;
pub const EVERYTHING_IPC_QUERY2_REQUEST_SIZE: u32 = 0x00000010;
pub const EVERYTHING_IPC_QUERY2_REQUEST_DATE_CREATED: u32 = 0x00000020;
pub const EVERYTHING_IPC_QUERY2_REQUEST_DATE_MODIFIED: u32 = 0x00000040;
pub const EVERYTHING_IPC_QUERY2_REQUEST_DATE_ACCESSED: u32 = 0x00000080;
pub const EVERYTHING_IPC_QUERY2_REQUEST_ATTRIBUTES: u32 = 0x00000100;

pub const EVERYTHING_IPC_FOLDER: u32 = 0x00000001;
pub const EVERYTHING_IPC_DRIVE: u32 = 0x00000002;
pub const EVERYTHING_IPC_ROOT: u32 = 0x00000002;

#[cfg(windows)]
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x00000010;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawItem {
    pub flags: u32,
    pub path: String,
    pub size: u64,
    pub date_created: (u32, u32),
    pub date_modified: (u32, u32),
    pub attributes: u32,
}

/// Reads a wide string formatted as DWORD length in WCHARs (excluding NUL) + characters + NUL.
fn read_wide_string(buf: &[u8], cur: &mut usize) -> io::Result<String> {
    if *cur + 4 > buf.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "string length out of bounds"));
    }
    let char_len = u32::from_le_bytes(buf[*cur..*cur + 4].try_into().unwrap()) as usize;
    *cur += 4;
    let text_bytes =
        char_len.checked_mul(2).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "string chars overflow"))?;
    let total_bytes =
        text_bytes.checked_add(2).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "string total overflow"))?;
    if *cur + total_bytes > buf.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "string data out of bounds"));
    }
    let chars_slice = &buf[*cur..*cur + text_bytes];
    let u16s: Vec<u16> = chars_slice.chunks_exact(2).map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]])).collect();
    *cur += total_bytes;
    Ok(String::from_utf16_lossy(&u16s))
}

/// Pure parsing of an EVERYTHING_IPC_LIST2 buffer into raw items.
/// Unaligned-safe and bounds-checked; returns Err(InvalidData) on truncated or malformed buffers.
pub fn parse_list2(buf: &[u8]) -> io::Result<Vec<RawItem>> {
    if buf.len() < 20 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "LIST2 header too small"));
    }
    let numitems = u32::from_le_bytes(buf[4..8].try_into().unwrap()) as usize;
    let request_flags = u32::from_le_bytes(buf[12..16].try_into().unwrap());

    let items_bytes = numitems
        .checked_mul(8)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "numitems byte count overflow"))?;
    let items_end = 20usize
        .checked_add(items_bytes)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "items end overflow"))?;
    if buf.len() < items_end {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "LIST2 buffer too small for items array"));
    }

    let mut items = Vec::with_capacity(numitems);
    for i in 0..numitems {
        let item_pos = 20 + i * 8;
        let flags = u32::from_le_bytes(buf[item_pos..item_pos + 4].try_into().unwrap());
        let data_offset = u32::from_le_bytes(buf[item_pos + 4..item_pos + 8].try_into().unwrap()) as usize;
        let mut cur = data_offset;

        let mut path = String::new();
        let mut size = 0u64;
        let mut date_created = (0u32, 0u32);
        let mut date_modified = (0u32, 0u32);
        let mut attributes = 0u32;

        if request_flags & EVERYTHING_IPC_QUERY2_REQUEST_NAME != 0 {
            read_wide_string(buf, &mut cur)?;
        }
        if request_flags & EVERYTHING_IPC_QUERY2_REQUEST_PATH != 0 {
            read_wide_string(buf, &mut cur)?;
        }
        if request_flags & EVERYTHING_IPC_QUERY2_REQUEST_FULL_PATH_AND_NAME != 0 {
            path = read_wide_string(buf, &mut cur)?;
        }
        if request_flags & EVERYTHING_IPC_QUERY2_REQUEST_EXTENSION != 0 {
            read_wide_string(buf, &mut cur)?;
        }
        if request_flags & EVERYTHING_IPC_QUERY2_REQUEST_SIZE != 0 {
            if cur.checked_add(8).is_none_or(|end| end > buf.len()) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "size out of bounds"));
            }
            size = u64::from_le_bytes(buf[cur..cur + 8].try_into().unwrap());
            cur += 8;
        }
        if request_flags & EVERYTHING_IPC_QUERY2_REQUEST_DATE_CREATED != 0 {
            if cur.checked_add(8).is_none_or(|end| end > buf.len()) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "date_created out of bounds"));
            }
            let low = u32::from_le_bytes(buf[cur..cur + 4].try_into().unwrap());
            let high = u32::from_le_bytes(buf[cur + 4..cur + 8].try_into().unwrap());
            date_created = (low, high);
            cur += 8;
        }
        if request_flags & EVERYTHING_IPC_QUERY2_REQUEST_DATE_MODIFIED != 0 {
            if cur.checked_add(8).is_none_or(|end| end > buf.len()) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "date_modified out of bounds"));
            }
            let low = u32::from_le_bytes(buf[cur..cur + 4].try_into().unwrap());
            let high = u32::from_le_bytes(buf[cur + 4..cur + 8].try_into().unwrap());
            date_modified = (low, high);
            cur += 8;
        }
        if request_flags & EVERYTHING_IPC_QUERY2_REQUEST_DATE_ACCESSED != 0 {
            if cur.checked_add(8).is_none_or(|end| end > buf.len()) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "date_accessed out of bounds"));
            }
            cur += 8;
        }
        if request_flags & EVERYTHING_IPC_QUERY2_REQUEST_ATTRIBUTES != 0 {
            if cur.checked_add(4).is_none_or(|end| end > buf.len()) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "attributes out of bounds"));
            }
            attributes = u32::from_le_bytes(buf[cur..cur + 4].try_into().unwrap());
        }

        items.push(RawItem { flags, path, size, date_created, date_modified, attributes });
    }
    Ok(items)
}

#[cfg(windows)]
const REPLY_MSG_ID: usize = 0x0e5e;

#[cfg(windows)]
unsafe extern "system" fn reply_wndproc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{DefWindowProcW, GWLP_USERDATA, GetWindowLongPtrW, WM_COPYDATA};

    if msg == WM_COPYDATA {
        unsafe {
            let cds = &*(lparam as *const COPYDATASTRUCT);
            if cds.dwData == REPLY_MSG_ID {
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const std::cell::RefCell<Option<Vec<u8>>>;
                if !ptr.is_null() && !cds.lpData.is_null() {
                    let slice = std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize);
                    (*ptr).borrow_mut().replace(slice.to_vec());
                    return 1;
                }
            }
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[cfg(windows)]
struct WindowGuard(windows_sys::Win32::Foundation::HWND);

#[cfg(windows)]
impl Drop for WindowGuard {
    fn drop(&mut self) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyWindow, GWLP_USERDATA, SetWindowLongPtrW};
        unsafe {
            SetWindowLongPtrW(self.0, GWLP_USERDATA, 0);
            DestroyWindow(self.0);
        }
    }
}

/// Names matching `words` (all of them, case-insensitive substring; `*`/`?` wildcards match the whole name)
/// under `root`, from a running Everything. `found` gets each result with `name` = full path.
///
/// Only `FULL_PATH_AND_NAME | SIZE | DATE_MODIFIED` are requested from Everything (see [`fill_from_disk`]).
#[cfg(windows)]
pub fn search(root: &Path, words: &[String], cancel: &AtomicBool, found: &mut dyn FnMut(Entry)) -> io::Result<()> {
    search_timed(root, words, cancel, found).map(|_| ())
}

/// Like [`search`], but returns the duration until the WM_COPYDATA reply arrived from Everything.
#[cfg(windows)]
pub fn search_timed(
    root: &Path,
    words: &[String],
    cancel: &AtomicBool,
    found: &mut dyn FnMut(Entry),
) -> io::Result<Duration> {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use std::time::Instant;
    use windows_sys::Win32::Foundation::{LPARAM, WPARAM};
    use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, FindWindowW, GWLP_USERDATA, HWND_MESSAGE, MSG, MsgWaitForMultipleObjects,
        PM_REMOVE, PeekMessageW, QS_ALLINPUT, RegisterClassExW, SendMessageW, SetWindowLongPtrW, TranslateMessage,
        WM_COPYDATA, WNDCLASSEXW,
    };

    let class1: Vec<u16> = "EVERYTHING_TASKBAR_NOTIFICATION\0".encode_utf16().collect();
    let class2: Vec<u16> = "EVERYTHING_TASKBAR_NOTIFICATION_(1.5a)\0".encode_utf16().collect();
    let mut everything_hwnd = unsafe { FindWindowW(class1.as_ptr(), null()) };
    if everything_hwnd.is_null() {
        everything_hwnd = unsafe { FindWindowW(class2.as_ptr(), null()) };
    }
    if everything_hwnd.is_null() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "Everything is not running"));
    }

    let loaded = unsafe { SendMessageW(everything_hwnd, EVERYTHING_WM_IPC, EVERYTHING_IPC_IS_DB_LOADED as WPARAM, 0) };
    if loaded == 0 {
        return Err(io::Error::new(io::ErrorKind::NotFound, "Everything database is not loaded (indexing)"));
    }

    static REGISTER_CLASS_ONCE: std::sync::Once = std::sync::Once::new();
    const REPLY_CLASS_NAME: &[u16] = &[
        b'O' as u16,
        b'x' as u16,
        b'i' as u16,
        b'E' as u16,
        b'v' as u16,
        b'R' as u16,
        b'e' as u16,
        b'p' as u16,
        b'l' as u16,
        b'y' as u16,
        0,
    ];

    REGISTER_CLASS_ONCE.call_once(|| {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(reply_wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: null_mut(),
            hIcon: null_mut(),
            hCursor: null_mut(),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: REPLY_CLASS_NAME.as_ptr(),
            hIconSm: null_mut(),
        };
        unsafe { RegisterClassExW(&wc) };
    });

    let reply_hwnd = unsafe {
        CreateWindowExW(
            0,
            REPLY_CLASS_NAME.as_ptr(),
            null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            null_mut(),
            null_mut(),
            null(),
        )
    };
    if reply_hwnd.is_null() {
        return Err(io::Error::last_os_error());
    }

    let reply_data: RefCell<Option<Vec<u8>>> = RefCell::new(None);
    unsafe {
        SetWindowLongPtrW(reply_hwnd, GWLP_USERDATA, &reply_data as *const _ as isize);
    }
    let _guard = WindowGuard(reply_hwnd);

    let mut root_str = root.to_string_lossy().into_owned();
    if !root_str.ends_with('\\') {
        root_str.push('\\');
    }
    let search_str =
        if words.is_empty() { format!("\"{root_str}\"") } else { format!("\"{root_str}\" {}", words.join(" ")) };
    let search_utf16: Vec<u16> = search_str.encode_utf16().chain(std::iter::once(0)).collect();

    let req_flags = EVERYTHING_IPC_QUERY2_REQUEST_FULL_PATH_AND_NAME
        | EVERYTHING_IPC_QUERY2_REQUEST_SIZE
        | EVERYTHING_IPC_QUERY2_REQUEST_DATE_MODIFIED;

    let mut query_buf = Vec::with_capacity(28 + search_utf16.len() * 2);
    query_buf.extend_from_slice(&(reply_hwnd as usize as u32).to_le_bytes());
    query_buf.extend_from_slice(&(REPLY_MSG_ID as u32).to_le_bytes());
    query_buf.extend_from_slice(&0u32.to_le_bytes()); // search_flags
    query_buf.extend_from_slice(&0u32.to_le_bytes()); // offset
    query_buf.extend_from_slice(&500_000u32.to_le_bytes()); // max_results
    query_buf.extend_from_slice(&req_flags.to_le_bytes());
    query_buf.extend_from_slice(&EVERYTHING_IPC_SORT_NAME_ASCENDING.to_le_bytes());
    for &ch in &search_utf16 {
        query_buf.extend_from_slice(&ch.to_le_bytes());
    }

    let cds = COPYDATASTRUCT {
        dwData: EVERYTHING_IPC_COPYDATA_QUERY2W,
        cbData: query_buf.len() as u32,
        lpData: query_buf.as_ptr() as *mut c_void,
    };
    let send_ret =
        unsafe { SendMessageW(everything_hwnd, WM_COPYDATA, reply_hwnd as WPARAM, &cds as *const _ as LPARAM) };
    if send_ret == 0 {
        return Err(io::Error::other("Everything query rejected (SendMessage returned 0)"));
    }

    let start = Instant::now();
    let timeout = Duration::from_secs(10);
    let mut reply_took = Duration::ZERO;
    while reply_data.borrow().is_none() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(reply_took);
        }
        if start.elapsed() >= timeout {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "Everything query timed out"));
        }
        unsafe {
            MsgWaitForMultipleObjects(0, null(), 0, 50, QS_ALLINPUT);
            if reply_data.borrow().is_some() {
                reply_took = start.elapsed();
                break;
            }
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
                if reply_data.borrow().is_some() {
                    break;
                }
            }
            if reply_data.borrow().is_some() {
                reply_took = start.elapsed();
                break;
            }
        }
    }
    if reply_took.is_zero() {
        reply_took = start.elapsed();
    }

    let reply_bytes = reply_data.into_inner().unwrap();
    let raw_items = parse_list2(&reply_bytes)?;

    let root_str = root.to_string_lossy();
    let root_buf;
    let (root_with_slash, root_no_slash): (&str, &str) = if root_str.ends_with('\\') {
        (root_str.as_ref(), root_str.trim_end_matches('\\'))
    } else {
        root_buf = format!("{root_str}\\");
        (root_buf.as_str(), root_str.as_ref())
    };
    let prefix_len = root_with_slash.len();

    let mut entries = Vec::with_capacity(raw_items.len());
    for item in raw_items {
        // `get` rather than slicing: a non-ASCII name can put a char boundary inside the prefix length.
        let matches_prefix = item.path.get(..prefix_len).is_some_and(|p| p.eq_ignore_ascii_case(root_with_slash));
        let matches_root = !matches_prefix && item.path.eq_ignore_ascii_case(root_no_slash);
        if !matches_prefix && !matches_root {
            continue;
        }
        let is_dir = item.flags & EVERYTHING_IPC_FOLDER != 0;
        entries.push(Entry {
            name: item.path,
            is_dir,
            size: if is_dir { 0 } else { item.size },
            modified: crate::windows::filetime_parts_to_unix(item.date_modified.0, item.date_modified.1),
            attrs: if is_dir { FILE_ATTRIBUTE_DIRECTORY } else { 0 },
            ..Default::default()
        });
    }
    fill_from_disk(&mut entries, cancel);
    for e in entries {
        if cancel.load(Ordering::Relaxed) {
            return Ok(reply_took);
        }
        found(e);
    }

    Ok(reply_took)
}

/// Everything 1.4 indexes only names, sizes and modified dates by default: asking it for attributes or the
/// creation date makes it read every result from disk on one thread (7.9 s instead of 0.3 s for 72k hits).
/// They are read here on several threads instead, so hidden and system files stay hidden as in the listing
/// and the Date created column is filled. An item that can't be read keeps what Everything gave.
#[cfg(windows)]
fn fill_from_disk(entries: &mut [Entry], cancel: &AtomicBool) {
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileAttributesExW, GetFileExInfoStandard, WIN32_FILE_ATTRIBUTE_DATA,
    };

    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8);
    let chunk = entries.len().div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for part in entries.chunks_mut(chunk) {
            s.spawn(move || {
                for e in part {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let path = crate::windows::wide(crate::windows::long_path(Path::new(&e.name)));
                    let mut d: WIN32_FILE_ATTRIBUTE_DATA = unsafe { std::mem::zeroed() };
                    let ok = unsafe {
                        GetFileAttributesExW(
                            path.as_ptr(),
                            GetFileExInfoStandard,
                            (&mut d as *mut WIN32_FILE_ATTRIBUTE_DATA).cast(),
                        )
                    };
                    if ok == 0 {
                        continue;
                    }
                    let (hidden, system, link) = crate::windows::entry_flags(d.dwFileAttributes);
                    e.attrs = d.dwFileAttributes;
                    (e.hidden, e.system, e.link) = (hidden, system, link);
                    e.created = crate::windows::filetime_parts_to_unix(
                        d.ftCreationTime.dwLowDateTime,
                        d.ftCreationTime.dwHighDateTime,
                    );
                }
            });
        }
    });
}

/// True only for a local fixed NTFS or ReFS volume.
#[cfg(windows)]
pub fn everything_can_scope(root: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumeInformationW, GetVolumePathNameW};

    const DRIVE_FIXED: u32 = 3;
    let mut root_wide: Vec<u16> = root.as_os_str().encode_wide().collect();
    root_wide.push(0);

    let mut vol_buf = [0u16; 512];
    let ok = unsafe { GetVolumePathNameW(root_wide.as_ptr(), vol_buf.as_mut_ptr(), vol_buf.len() as u32) };
    if ok == 0 {
        return false;
    }

    let drive_type = unsafe { GetDriveTypeW(vol_buf.as_ptr()) };
    if drive_type != DRIVE_FIXED {
        return false;
    }

    let mut fs_name = [0u16; 64];
    let ok = unsafe {
        GetVolumeInformationW(
            vol_buf.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs_name.as_mut_ptr(),
            fs_name.len() as u32,
        )
    };
    if ok == 0 {
        return false;
    }

    let len = fs_name.iter().position(|&c| c == 0).unwrap_or(fs_name.len());
    let fs = String::from_utf16_lossy(&fs_name[..len]);
    fs.eq_ignore_ascii_case("NTFS") || fs.eq_ignore_ascii_case("ReFS")
}

#[cfg(not(windows))]
pub fn everything_can_scope(_root: &std::path::Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_list2_valid_and_truncated() {
        let req_flags = EVERYTHING_IPC_QUERY2_REQUEST_FULL_PATH_AND_NAME
            | EVERYTHING_IPC_QUERY2_REQUEST_SIZE
            | EVERYTHING_IPC_QUERY2_REQUEST_DATE_CREATED
            | EVERYTHING_IPC_QUERY2_REQUEST_DATE_MODIFIED
            | EVERYTHING_IPC_QUERY2_REQUEST_ATTRIBUTES;

        let mut buf = Vec::new();
        // Header: 20 bytes
        buf.extend_from_slice(&2u32.to_le_bytes()); // totitems
        buf.extend_from_slice(&2u32.to_le_bytes()); // numitems
        buf.extend_from_slice(&0u32.to_le_bytes()); // offset
        buf.extend_from_slice(&req_flags.to_le_bytes()); // request_flags
        buf.extend_from_slice(&1u32.to_le_bytes()); // sort_type (NAME_ASCENDING)

        // Items array: 2 items * 8 bytes = 16 bytes
        // Offsets will start at 20 + 16 = 36 bytes.
        let item1_offset = 36u32;
        buf.extend_from_slice(&EVERYTHING_IPC_FOLDER.to_le_bytes()); // flags
        buf.extend_from_slice(&item1_offset.to_le_bytes()); // data_offset

        // Construct item1 data: folder
        let path1_u16: Vec<u16> = r"C:\Test\SubFolder".encode_utf16().collect();
        let mut item1_data = Vec::new();
        item1_data.extend_from_slice(&(path1_u16.len() as u32).to_le_bytes());
        for &ch in &path1_u16 {
            item1_data.extend_from_slice(&ch.to_le_bytes());
        }
        item1_data.extend_from_slice(&0u16.to_le_bytes()); // NUL
        item1_data.extend_from_slice(&0u64.to_le_bytes()); // size
        item1_data.extend_from_slice(&1111u32.to_le_bytes()); // created low
        item1_data.extend_from_slice(&2222u32.to_le_bytes()); // created high
        item1_data.extend_from_slice(&3333u32.to_le_bytes()); // modified low
        item1_data.extend_from_slice(&4444u32.to_le_bytes()); // modified high
        item1_data.extend_from_slice(&0x0010u32.to_le_bytes()); // attributes (FILE_ATTRIBUTE_DIRECTORY)

        let item2_offset = (item1_offset as usize + item1_data.len()) as u32;
        buf.extend_from_slice(&0u32.to_le_bytes()); // flags (file)
        buf.extend_from_slice(&item2_offset.to_le_bytes()); // data_offset

        // Construct item2 data: file
        let path2_u16: Vec<u16> = r"C:\Test\SubFolder\file.txt".encode_utf16().collect();
        let mut item2_data = Vec::new();
        item2_data.extend_from_slice(&(path2_u16.len() as u32).to_le_bytes());
        for &ch in &path2_u16 {
            item2_data.extend_from_slice(&ch.to_le_bytes());
        }
        item2_data.extend_from_slice(&0u16.to_le_bytes()); // NUL
        item2_data.extend_from_slice(&12345u64.to_le_bytes()); // size
        item2_data.extend_from_slice(&5555u32.to_le_bytes()); // created low
        item2_data.extend_from_slice(&6666u32.to_le_bytes()); // created high
        item2_data.extend_from_slice(&7777u32.to_le_bytes()); // modified low
        item2_data.extend_from_slice(&8888u32.to_le_bytes()); // modified high
        item2_data.extend_from_slice(&0x0020u32.to_le_bytes()); // attributes (FILE_ATTRIBUTE_ARCHIVE)

        buf.extend_from_slice(&item1_data);
        buf.extend_from_slice(&item2_data);

        // Parse valid buffer
        let items = parse_list2(&buf).expect("parse_list2 should succeed on valid buffer");
        assert_eq!(items.len(), 2);

        // Check item 1 (folder)
        assert_eq!(items[0].flags, EVERYTHING_IPC_FOLDER);
        assert_eq!(items[0].path, r"C:\Test\SubFolder");
        assert_eq!(items[0].size, 0);
        assert_eq!(items[0].date_created, (1111, 2222));
        assert_eq!(items[0].date_modified, (3333, 4444));
        assert_eq!(items[0].attributes, 0x0010);

        // Check item 2 (file)
        assert_eq!(items[1].flags, 0);
        assert_eq!(items[1].path, r"C:\Test\SubFolder\file.txt");
        assert_eq!(items[1].size, 12345);
        assert_eq!(items[1].date_created, (5555, 6666));
        assert_eq!(items[1].date_modified, (7777, 8888));
        assert_eq!(items[1].attributes, 0x0020);

        // Truncated buffers should give Err(InvalidData), never panic
        for trunc_len in [0, 4, 19, 20, 24, 35, 36, item2_offset as usize, buf.len() - 1] {
            assert!(parse_list2(&buf[..trunc_len]).is_err(), "truncated at len {trunc_len} should return Err");
        }
    }
}
