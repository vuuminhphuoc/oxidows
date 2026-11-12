//! Opt-in: make oxi-explorer open folders instead of the built-in Explorer, for this user only
//! (HKCU), and undo it. Covers double-clicking folders and drives (the `Directory` and
//! `Drive` verbs, used by every program that "opens" a folder) and Win+E (the
//! `opennewwindow` verb of This PC's CLSID). Nothing outside `HKCU\Software\Classes` changes.

use std::io;
use std::path::Path;
use std::ptr::null;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW, RegSetKeyValueW,
};

const VERB: &str = "oxi-explorer";
pub const DEFAULT_ROOT: &str = r"Software\Classes";
const WIN_E_SUBKEY: &str = r"CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}";

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn check(code: u32) -> io::Result<()> {
    if code == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(code as i32)) }
}

fn set(key: &str, name: Option<&str>, value: &str) -> io::Result<()> {
    let (k, v) = (w(key), w(value));
    let n = name.map(w);
    check(unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER as HKEY,
            k.as_ptr(),
            n.as_ref().map_or(null(), |n| n.as_ptr()),
            REG_SZ,
            v.as_ptr().cast(),
            (v.len() * 2) as u32,
        )
    })
}

fn get(key: &str, name: Option<&str>) -> Option<String> {
    let k = w(key);
    let n = name.map(w);
    let mut buf = [0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER as HKEY,
            k.as_ptr(),
            n.as_ref().map_or(null(), |n| n.as_ptr()),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    (r == ERROR_SUCCESS).then(|| {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(0);
        String::from_utf16_lossy(&buf[..end])
    })
}

fn delete_tree(key: &str) -> io::Result<()> {
    let k = w(key);
    match unsafe { RegDeleteTreeW(HKEY_CURRENT_USER as HKEY, k.as_ptr()) } {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => {}
        e => return Err(io::Error::from_raw_os_error(e as i32)),
    }
    let _ = unsafe { windows_sys::Win32::System::Registry::RegDeleteKeyW(HKEY_CURRENT_USER as HKEY, k.as_ptr()) };
    Ok(())
}

/// Deletes `key` when it holds no values and no subkeys (one we may have created), so undo leaves
/// the user's classes as they were.
fn delete_if_empty(key: &str) {
    use windows_sys::Win32::System::Registry::{KEY_READ, RegCloseKey, RegOpenKeyExW, RegQueryInfoKeyW};
    let k = w(key);
    let mut h: HKEY = std::ptr::null_mut();
    // SAFETY: NUL-terminated name, out-pointer to a local; the handle is closed below.
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER as HKEY, k.as_ptr(), 0, KEY_READ, &mut h) } != ERROR_SUCCESS {
        return;
    }
    let (mut subkeys, mut values) = (0u32, 0u32);
    let none = std::ptr::null_mut::<u32>;
    // SAFETY: open key; unused outputs are null.
    let r = unsafe {
        RegQueryInfoKeyW(
            h,
            std::ptr::null_mut(),
            none(),
            null(),
            &mut subkeys,
            none(),
            none(),
            &mut values,
            none(),
            none(),
            none(),
            std::ptr::null_mut(),
        )
    };
    // SAFETY: the key opened above.
    unsafe { RegCloseKey(h) };
    if r == ERROR_SUCCESS && subkeys == 0 && values == 0 {
        // SAFETY: NUL-terminated name.
        unsafe { windows_sys::Win32::System::Registry::RegDeleteKeyW(HKEY_CURRENT_USER as HKEY, k.as_ptr()) };
    }
}

/// Whether folders currently open in oxi-explorer.
pub fn is_default() -> bool {
    is_default_in(DEFAULT_ROOT)
}

/// Internal variant that scopes registry checks to `root`.
pub fn is_default_in(root: &str) -> bool {
    get(&format!(r"{root}\Directory\shell"), None).as_deref() == Some(VERB)
}

/// Verbs Windows itself uses as a folder's default; a user value of one of these is no takeover.
const BUILT_IN_VERBS: [&str; 5] = ["", "none", "open", "explore", "opennewwindow"];

/// Who opens folders now: `Ours(exe)` when our verb is the default (with the exe it starts),
/// `Other(command)` when some other program's verb is, `BuiltIn` otherwise.
pub fn handler() -> crate::FolderHandler {
    handler_in(DEFAULT_ROOT)
}

/// Internal variant that checks folder handler under `root`.
pub fn handler_in(root: &str) -> crate::FolderHandler {
    let shell = format!(r"{root}\Directory\shell");
    let Some(verb) = get(&shell, None) else { return crate::FolderHandler::BuiltIn };
    let command = get(&format!(r"{shell}\{verb}\command"), None).unwrap_or_default();
    if verb == VERB {
        crate::FolderHandler::Ours(crate::command_exe(&command))
    } else if BUILT_IN_VERBS.iter().any(|v| v.eq_ignore_ascii_case(&verb)) {
        crate::FolderHandler::BuiltIn
    } else {
        crate::FolderHandler::Other(if command.is_empty() { verb } else { crate::command_exe(&command) })
    }
}

/// Where a Win+E command another program had goes while ours is in, to be put back on undo.
const PREVIOUS: &str = "oxidows.Previous";

/// Whether a Win+E command starts oxi-explorer: by name, or `exe` (ours, however it is named).
fn is_ours(command: &str, exe: Option<&str>) -> bool {
    command.to_ascii_lowercase().contains("oxi-explorer")
        || exe.is_some_and(|e| !e.is_empty() && crate::command_exe(command).eq_ignore_ascii_case(e))
}

pub fn set_default(exe: &Path) -> io::Result<()> {
    set_default_in(exe, DEFAULT_ROOT)
}

/// Internal variant that writes association keys under `root`.
pub fn set_default_in(exe: &Path, root: &str) -> io::Result<()> {
    let open = format!("\"{}\" \"%1\"", exe.display());
    let win_e = format!(r"{root}\{WIN_E_SUBKEY}");
    for class in ["Directory", "Drive"] {
        let shell = format!(r"{root}\{class}\shell");
        // Keep whatever default verb this user had, to put it back on undo.
        if let Some(old) = get(&shell, None).filter(|v| v != VERB) {
            set(&format!(r"{shell}\{VERB}"), Some("PreviousDefault"), &old)?;
        }
        set(&format!(r"{shell}\{VERB}"), None, "Open in oxi-explorer")?;
        set(&format!(r"{shell}\{VERB}\command"), None, &open)?;
        set(&shell, None, VERB)?;
    }
    let cmd = format!(r"{win_e}\shell\opennewwindow\command");
    // Another file manager's Win+E (Files, Directory Opus) comes back on undo.
    let me = exe.display().to_string();
    if let Some(old) = get(&cmd, None).filter(|v| !is_ours(v, Some(&me))) {
        set(&cmd, Some(PREVIOUS), &old)?;
    }
    set(&cmd, None, &format!("\"{}\"", exe.display()))?;
    // An empty DelegateExecute makes the shell run the command instead of its own handler.
    set(&cmd, Some("DelegateExecute"), "")
}

pub fn unset_default() -> io::Result<()> {
    unset_default_in(DEFAULT_ROOT)
}

/// Internal variant that cleans up association keys under `root`.
pub fn unset_default_in(root: &str) -> io::Result<()> {
    let win_e = format!(r"{root}\{WIN_E_SUBKEY}");
    // The exe we set, to know our Win+E command below even if the exe was renamed.
    let our_exe = get(&format!(r"{root}\Directory\shell\oxi-explorer\command"), None).map(|c| crate::command_exe(&c));
    for class in ["Directory", "Drive"] {
        let shell = format!(r"{root}\{class}\shell");
        let previous = get(&format!(r"{shell}\{VERB}"), Some("PreviousDefault")).filter(|v| !v.is_empty());
        delete_tree(&format!(r"{shell}\{VERB}"))?;
        if get(&shell, None).as_deref() == Some(VERB) {
            match previous {
                Some(old) => set(&shell, None, &old)?,
                None => {
                    let k = w(&shell);
                    match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER as HKEY, k.as_ptr(), null()) } {
                        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => {}
                        e => return Err(io::Error::from_raw_os_error(e as i32)),
                    }
                }
            }
        }
    }
    let cmd = format!(r"{win_e}\shell\opennewwindow\command");
    let ours = get(&cmd, None).is_some_and(|v| is_ours(&v, our_exe.as_deref()));
    match get(&cmd, Some(PREVIOUS)).filter(|v| !v.is_empty()) {
        Some(old) if ours => {
            set(&cmd, None, &old)?;
            let (k, n) = (w(&cmd), w(PREVIOUS));
            // SAFETY: NUL-terminated key and value names. Best effort.
            let _ = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER as HKEY, k.as_ptr(), n.as_ptr()) };
        }
        // Someone else's since: leave it.
        Some(_) => {}
        None if ours => delete_tree(&format!(r"{win_e}\shell\opennewwindow"))?,
        None => {}
    }
    for class in ["Directory", "Drive"] {
        delete_if_empty(&format!(r"{root}\{class}\shell"));
        delete_if_empty(&format!(r"{root}\{class}"));
    }
    // The parents only if nothing else of the user's is in them.
    for k in [format!(r"{win_e}\shell"), win_e] {
        delete_if_empty(&k);
    }
    Ok(())
}
