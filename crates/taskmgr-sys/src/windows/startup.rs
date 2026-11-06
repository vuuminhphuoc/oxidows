//! Startup apps: the Run registry keys and Startup folders, with the enabled flag Windows keeps
//! under `Explorer\StartupApproved` (the same place the built-in Task Manager writes).

use std::ffi::c_void;
use std::io;
use std::path::PathBuf;
use std::ptr::{null, null_mut};

use taskmgr_core::StartupItem;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, KEY_WOW64_64KEY, REG_BINARY,
    REG_OPTION_NON_VOLATILE, RegCloseKey, RegCreateKeyExW, RegEnumValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW,
};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN32: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: key we opened.
        unsafe { RegCloseKey(self.0) };
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn open(root: HKEY, path: &str, access: u32) -> Option<Key> {
    let mut k = null_mut();
    let p = wide(path);
    // SAFETY: NUL-terminated path, out-pointer to a local.
    (unsafe { RegOpenKeyExW(root, p.as_ptr(), 0, access | KEY_WOW64_64KEY, &mut k) } == ERROR_SUCCESS).then(|| Key(k))
}

/// (name, data as text) for every value of a key.
fn values(key: &Key) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for i in 0.. {
        let mut name = vec![0u16; 512];
        let mut nlen = name.len() as u32;
        let mut data = vec![0u8; 8192];
        let mut dlen = data.len() as u32;
        let mut ty = 0u32;
        // SAFETY: buffers sized by nlen / dlen.
        let r = unsafe {
            RegEnumValueW(key.0, i, name.as_mut_ptr(), &mut nlen, null(), &mut ty, data.as_mut_ptr(), &mut dlen)
        };
        if r != ERROR_SUCCESS {
            break;
        }
        let name = String::from_utf16_lossy(&name[..nlen as usize]);
        let words: Vec<u16> =
            data[..dlen as usize].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let text = String::from_utf16_lossy(&words).trim_end_matches('\0').to_string();
        out.push((name, text));
    }
    out
}

/// Missing value = enabled. First byte even (0x02, 0x06) = enabled, odd (0x03, 0x07) = disabled.
fn approved(root: HKEY, sub: &str, name: &str) -> bool {
    let Some(k) = open(root, &format!(r"{APPROVED}\{sub}"), KEY_READ) else { return true };
    let n = wide(name);
    let mut data = [0u8; 16];
    let mut len = data.len() as u32;
    // SAFETY: buffer of `len` bytes.
    let r = unsafe { RegQueryValueExW(k.0, n.as_ptr(), null(), null_mut(), data.as_mut_ptr(), &mut len) };
    r != ERROR_SUCCESS || len == 0 || data[0] & 1 == 0
}

fn root_name(root: HKEY) -> &'static str {
    if root == HKEY_CURRENT_USER { "HKCU" } else { "HKLM" }
}

fn startup_dirs() -> Vec<(PathBuf, HKEY)> {
    let mut v = Vec::new();
    if let Some(a) = std::env::var_os("APPDATA") {
        v.push((PathBuf::from(a).join(r"Microsoft\Windows\Start Menu\Programs\Startup"), HKEY_CURRENT_USER));
    }
    if let Some(p) = std::env::var_os("ProgramData") {
        v.push((PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs\Startup"), HKEY_LOCAL_MACHINE));
    }
    v
}

pub fn startup_items() -> Vec<StartupItem> {
    let mut out = Vec::new();
    for (root, path, sub) in [
        (HKEY_CURRENT_USER, RUN, "Run"),
        (HKEY_LOCAL_MACHINE, RUN, "Run"),
        (HKEY_LOCAL_MACHINE, RUN32, "Run32"),
    ] {
        let Some(k) = open(root, path, KEY_READ) else { continue };
        for (name, command) in values(&k) {
            if name.is_empty() {
                continue;
            }
            out.push(StartupItem {
                enabled: approved(root, sub, &name),
                name,
                command,
                location: format!(r"{}\{path}", root_name(root)),
                per_user: root == HKEY_CURRENT_USER,
            });
        }
    }
    for (dir, root) in startup_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let file = e.file_name().to_string_lossy().into_owned();
            if file.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            out.push(StartupItem {
                enabled: approved(root, "StartupFolder", &file),
                name: file.trim_end_matches(".lnk").to_string(),
                command: e.path().display().to_string(),
                location: dir.display().to_string(),
                per_user: root == HKEY_CURRENT_USER,
            });
        }
    }
    out.sort_by_cached_key(|s| s.name.to_lowercase());
    out
}

pub fn set_startup_enabled(item: &StartupItem, enabled: bool) -> io::Result<()> {
    let root = if item.per_user { HKEY_CURRENT_USER } else { HKEY_LOCAL_MACHINE };
    let (sub, value_name) = if item.location.ends_with("Startup") {
        // Folder items are keyed by file name.
        ("StartupFolder", PathBuf::from(&item.command).file_name().map(|f| f.to_string_lossy().into_owned()))
    } else if item.location.contains("WOW6432Node") {
        ("Run32", Some(item.name.clone()))
    } else {
        ("Run", Some(item.name.clone()))
    };
    let value_name = value_name.ok_or_else(|| io::Error::other("no value name"))?;
    let path = wide(&format!(r"{APPROVED}\{sub}"));
    let mut k = null_mut();
    // SAFETY: NUL-terminated path, out-pointer to a local.
    let r = unsafe {
        RegCreateKeyExW(
            root,
            path.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_WOW64_64KEY,
            null(),
            &mut k,
            null_mut(),
        )
    };
    if r != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(r as i32));
    }
    let k = Key(k);
    let mut data = [0u8; 12];
    data[0] = if enabled { 0x02 } else { 0x03 };
    if !enabled {
        // Bytes 4..12 hold the FILETIME the entry was disabled; Task Manager shows it as "disabled on".
        let ft = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| (d.as_nanos() / 100) as u64 + 116_444_736_000_000_000)
            .unwrap_or(0);
        data[4..].copy_from_slice(&ft.to_le_bytes());
    }
    let n = wide(&value_name);
    // SAFETY: 12-byte REG_BINARY value.
    let r = unsafe {
        RegSetValueExW(k.0, n.as_ptr(), 0, REG_BINARY, data.as_ptr().cast::<c_void>().cast(), data.len() as u32)
    };
    if r != ERROR_SUCCESS { Err(io::Error::from_raw_os_error(r as i32)) } else { Ok(()) }
}
