//! Search through the Windows shell, as Explorer's search box does: a `search-ms:` query is
//! bound as a shell folder and enumerated. Windows Search answers it from its index (names,
//! file contents, properties, the whole Advanced Query Syntax) where the location is indexed,
//! and crawls the folder otherwise.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::UI::Shell::{
    BHID_EnumItems, IEnumShellItems, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
};
use windows::core::{PCWSTR, PWSTR};

/// Percent-encodes everything except unreserved characters, for a `search-ms:` parameter.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The `search-ms:` URL for `query` under `root`.
pub fn search_url(root: &Path, query: &str) -> String {
    format!(
        "search-ms:displayname={}&crumb=location:{}&query={}",
        encode(&format!("Search Results in {}", root.display())),
        encode(&root.to_string_lossy()),
        encode(query)
    )
}

/// Calls `found` with each result's path until done or `cancel` is set.
pub fn shell_search(root: &Path, query: &str, cancel: &AtomicBool, found: &mut dyn FnMut(PathBuf)) -> io::Result<()> {
    crate::windows::com_init();
    let url: Vec<u16> = search_url(root, query).encode_utf16().chain(Some(0)).collect();
    let to_io = |e: windows::core::Error| io::Error::other(e.message());
    let folder: IShellItem = unsafe { SHCreateItemFromParsingName(PCWSTR(url.as_ptr()), None) }.map_err(to_io)?;
    let items: IEnumShellItems = unsafe { folder.BindToHandler(None, &BHID_EnumItems) }.map_err(to_io)?;
    let mut buf: [Option<IShellItem>; 32] = Default::default();
    while !cancel.load(Ordering::Relaxed) {
        let mut n = 0u32;
        unsafe { items.Next(&mut buf, Some(&mut n)) }.map_err(to_io)?;
        if n == 0 {
            break;
        }
        for item in buf.iter_mut().take(n as usize).filter_map(Option::take) {
            // Results outside the file system (mail, web history) have no path.
            if let Ok(p) = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) } {
                let path = unsafe { from_pwstr(p) };
                found(path);
            }
        }
    }
    Ok(())
}

unsafe fn from_pwstr(p: PWSTR) -> PathBuf {
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _)) };
    PathBuf::from(s)
}
