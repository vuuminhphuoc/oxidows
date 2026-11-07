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
/// The CLSID behind Win+E ("This PC" / Home, opened in a new window).
const WIN_E: &str = r"Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}";

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

/// Whether folders currently open in oxi-explorer.
pub fn is_default() -> bool {
    get(r"Software\Classes\Directory\shell", None).as_deref() == Some(VERB)
}

pub fn set_default(exe: &Path) -> io::Result<()> {
    let open = format!("\"{}\" \"%1\"", exe.display());
    for class in ["Directory", "Drive"] {
        let shell = format!(r"Software\Classes\{class}\shell");
        // Keep whatever default verb this user had, to put it back on undo.
        if let Some(old) = get(&shell, None).filter(|v| v != VERB) {
            set(&format!(r"{shell}\{VERB}"), Some("PreviousDefault"), &old)?;
        }
        set(&format!(r"{shell}\{VERB}"), None, "Open in oxi-explorer")?;
        set(&format!(r"{shell}\{VERB}\command"), None, &open)?;
        set(&shell, None, VERB)?;
    }
    let cmd = format!(r"{WIN_E}\shell\opennewwindow\command");
    set(&cmd, None, &format!("\"{}\"", exe.display()))?;
    // An empty DelegateExecute makes the shell run the command instead of its own handler.
    set(&cmd, Some("DelegateExecute"), "")
}

pub fn unset_default() -> io::Result<()> {
    for class in ["Directory", "Drive"] {
        let shell = format!(r"Software\Classes\{class}\shell");
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
    delete_tree(&format!(r"{WIN_E}\shell\opennewwindow"))?;
    // The parents only if nothing else of the user's is in them.
    for k in [format!(r"{WIN_E}\shell"), WIN_E.to_string()] {
        let k = w(&k);
        let _ = unsafe { windows_sys::Win32::System::Registry::RegDeleteKeyW(HKEY_CURRENT_USER as HKEY, k.as_ptr()) };
    }
    Ok(())
}
