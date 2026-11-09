//! Startup apps: the Run registry keys and Startup folders, with the enabled flag Windows keeps
//! under `Explorer\StartupApproved` (the same place the built-in Task Manager writes).

use std::ffi::c_void;
use std::io;
use std::path::PathBuf;
use std::ptr::{null, null_mut};

use taskmgr_core::StartupItem;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, KEY_WOW64_64KEY, REG_BINARY, REG_DWORD,
    REG_OPTION_NON_VOLATILE, RegCloseKey, RegCreateKeyExW, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW,
    RegQueryValueExW, RegSetValueExW,
};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN32: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";
/// Packaged (Store) apps' startup tasks: `<package family>\<task id>` with a `State` DWORD.
const APPX: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData";
const APPX_PREFIX: &str = "Packaged app: ";

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
        let words: Vec<u16> = data[..dlen as usize].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        let text = String::from_utf16_lossy(&words).trim_end_matches('\0').to_string();
        out.push((name, text));
    }
    out
}

/// Missing value = enabled. First byte even (0x02, 0x06) = enabled, odd (0x03, 0x07) = disabled.
fn approved(root: HKEY, sub: &str, name: &str) -> bool {
    let Some(k) = open(root, &format!(r"{APPROVED}\{sub}"), KEY_READ) else {
        return true;
    };
    let n = wide(name);
    let mut data = [0u8; 16];
    let mut len = data.len() as u32;
    // SAFETY: buffer of `len` bytes.
    let r = unsafe { RegQueryValueExW(k.0, n.as_ptr(), null(), null_mut(), data.as_mut_ptr(), &mut len) };
    r != ERROR_SUCCESS || len == 0 || data[0] & 1 == 0
}

fn subkeys(key: &Key) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0.. {
        let mut name = vec![0u16; 512];
        let mut len = name.len() as u32;
        // SAFETY: buffer of `len` characters.
        let r =
            unsafe { RegEnumKeyExW(key.0, i, name.as_mut_ptr(), &mut len, null(), null_mut(), null_mut(), null_mut()) };
        if r != ERROR_SUCCESS {
            break;
        }
        out.push(String::from_utf16_lossy(&name[..len as usize]));
    }
    out
}

fn dword(key: &Key, value: &str) -> Option<u32> {
    let n = wide(value);
    let mut v = 0u32;
    let mut len = 4u32;
    // SAFETY: 4-byte buffer.
    let r = unsafe { RegQueryValueExW(key.0, n.as_ptr(), null(), null_mut(), (&raw mut v).cast(), &mut len) };
    (r == ERROR_SUCCESS).then_some(v)
}

/// Startup tasks of packaged apps. State: 0 disabled, 1 disabled by user, 2 enabled,
/// 3 disabled by policy, 4 enabled by policy.
fn packaged_items() -> Vec<StartupItem> {
    let mut out = Vec::new();
    let Some(root) = open(HKEY_CURRENT_USER, APPX, KEY_READ) else {
        return out;
    };
    for family in subkeys(&root) {
        let Some(fk) = open(HKEY_CURRENT_USER, &format!(r"{APPX}\{family}"), KEY_READ) else {
            continue;
        };
        for task in subkeys(&fk) {
            let Some(tk) = open(HKEY_CURRENT_USER, &format!(r"{APPX}\{family}\{task}"), KEY_READ) else {
                continue;
            };
            let Some(state) = dword(&tk, "State") else {
                continue;
            };
            // "Microsoft.Teams_8wekyb3d8bbwe" -> "Microsoft.Teams".
            let name = family.split('_').next().unwrap_or(&family).to_string();
            out.push(StartupItem {
                name,
                command: format!(r"shell:AppsFolder\{family}"),
                location: format!(r"{APPX_PREFIX}{family}\{task}"),
                enabled: matches!(state, 2 | 4),
                // Policy-controlled tasks cannot be changed here.
                per_user: state <= 2,
                publisher: String::new(),
            });
        }
    }
    out
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
    for (root, path, sub) in
        [(HKEY_CURRENT_USER, RUN, "Run"), (HKEY_LOCAL_MACHINE, RUN, "Run"), (HKEY_LOCAL_MACHINE, RUN32, "Run32")]
    {
        let Some(k) = open(root, path, KEY_READ) else {
            continue;
        };
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
                publisher: String::new(),
            });
        }
    }
    for (dir, root) in startup_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
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
                publisher: String::new(),
            });
        }
    }
    for it in &mut out {
        it.publisher = super::exe_publisher(&exe_of(&it.command));
    }
    out.extend(packaged_items());
    out.sort_by_cached_key(|s| s.name.to_lowercase());
    out
}

/// The program a Run command starts, with environment variables expanded (`%ProgramFiles%\x.exe /y`).
pub fn exe_of(command: &str) -> String {
    let (file, _) = crate::split_command(command);
    let mut out = String::new();
    let mut rest = file.as_str();
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) => {
                out.push_str(&std::env::var(&after[..j]).unwrap_or_default());
                rest = &after[j + 1..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn set_startup_enabled(item: &StartupItem, enabled: bool) -> io::Result<()> {
    if let Some(task) = item.location.strip_prefix(APPX_PREFIX) {
        let k = open(HKEY_CURRENT_USER, &format!(r"{APPX}\{task}"), KEY_SET_VALUE)
            .ok_or_else(|| io::Error::other("startup task not found"))?;
        let v: u32 = if enabled { 2 } else { 1 };
        // SAFETY: 4-byte REG_DWORD value.
        let r = unsafe { RegSetValueExW(k.0, wide("State").as_ptr(), 0, REG_DWORD, (&raw const v).cast(), 4) };
        return if r != ERROR_SUCCESS { Err(io::Error::from_raw_os_error(r as i32)) } else { Ok(()) };
    }
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

// ---- Replace the built-in Task Manager (Ctrl+Shift+Esc) ----

use windows_sys::Win32::System::Registry::{REG_SZ, RegDeleteKeyExW, RegDeleteValueW};

const IFEO_ROOT: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options";
const IFEO: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\taskmgr.exe";
/// Where the `Debugger` this user had before (e.g. Process Explorer's) waits to be put back.
const PREVIOUS: &str = "oxidows.PreviousDebugger";
/// The `Debugger` we wrote, to know it is ours on undo even if the exe was renamed.
const OURS: &str = "oxidows.Debugger";

fn value(k: &Key, name: &str) -> Option<String> {
    values(k).into_iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v)
}

/// The `Debugger` value Windows runs instead of taskmgr.exe, if any.
pub fn taskmgr_replacement() -> Option<String> {
    let k = open(HKEY_LOCAL_MACHINE, IFEO, KEY_READ)?;
    value(&k, "Debugger").filter(|v| !v.trim().is_empty())
}

fn set_sz(k: &Key, name: &str, data: &str) -> u32 {
    let (name, data) = (wide(name), wide(data));
    // SAFETY: REG_SZ data including its NUL, length in bytes.
    unsafe { RegSetValueExW(k.0, name.as_ptr(), 0, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32) }
}

fn delete_value(k: &Key, name: &str) -> u32 {
    let name = wide(name);
    // SAFETY: valid key and name.
    match unsafe { RegDeleteValueW(k.0, name.as_ptr()) } {
        2 => ERROR_SUCCESS, // ERROR_FILE_NOT_FOUND: not there
        r => r,
    }
}

use crate::is_oxidows_debugger as is_oxidows;

/// Points taskmgr.exe (and so Ctrl+Shift+Esc) at `exe`, or undoes that. Needs admin.
///
/// A replacement that was there before (Process Explorer, System Informer) is kept aside and
/// put back on undo; with none, undo removes the value, and the key too when nothing else is
/// in it, leaving the registry as Windows had it.
pub fn set_taskmgr_replacement(exe: Option<&str>) -> io::Result<()> {
    let path = wide(IFEO);
    let mut k = null_mut();
    // SAFETY: NUL-terminated path, out-pointer to a local.
    let r = unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_READ | KEY_WOW64_64KEY,
            null(),
            &mut k,
            null_mut(),
        )
    };
    if r != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(r as i32));
    }
    let k = Key(k);
    let current = value(&k, "Debugger").filter(|v| !v.trim().is_empty());
    let written = value(&k, OURS);
    let ours = |v: &str| is_oxidows(v) || written.as_deref() == Some(v);
    let r = match exe {
        Some(exe) => {
            let new = format!("\"{exe}\"");
            let mut r = ERROR_SUCCESS;
            if let Some(old) = current.filter(|v| !ours(v)) {
                r = set_sz(&k, PREVIOUS, &old);
            }
            for (name, data) in [("Debugger", &new), (OURS, &new)] {
                if r == ERROR_SUCCESS {
                    r = set_sz(&k, name, data);
                }
            }
            r
        }
        None => {
            let r = match value(&k, PREVIOUS).filter(|v| !v.trim().is_empty()) {
                // Only take out what is ours: another tool may have taken over since.
                _ if current.as_deref().is_some_and(|v| !ours(v)) => ERROR_SUCCESS,
                Some(old) => set_sz(&k, "Debugger", &old),
                None => delete_value(&k, "Debugger"),
            };
            [PREVIOUS, OURS].into_iter().fold(r, |r, name| if r == ERROR_SUCCESS { delete_value(&k, name) } else { r })
        }
    };
    if r != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(r as i32));
    }
    if exe.is_none() && values(&k).is_empty() && subkeys(&k).is_empty() {
        drop(k);
        let name = wide("taskmgr.exe");
        if let Some(parent) = open(HKEY_LOCAL_MACHINE, IFEO_ROOT, KEY_READ) {
            // SAFETY: valid parent key and NUL-terminated subkey name. Best effort.
            unsafe { RegDeleteKeyExW(parent.0, name.as_ptr(), KEY_WOW64_64KEY, 0) };
        }
    }
    Ok(())
}
