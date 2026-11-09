//! Explorer's "New" menu items and file creation.
//!
//! Windows lists file types registered with a `ShellNew` key. The user's order comes from
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Discardable\PostSetup\ShellNew\Classes`.
//! When missing, registered extensions starting with `.` in `HKEY_CLASSES_ROOT` are enumerated.
//! Creating a new item follows the ShellNew values: `NullFile`, `Data`, `FileName` template, and
//! optional `Command`.

use std::collections::HashSet;
use std::ffi::{OsString, c_void};
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, KEY_READ, REG_BINARY, REG_EXPAND_SZ, REG_SZ, RRF_RT_REG_EXPAND_SZ,
    RRF_RT_REG_MULTI_SZ, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, RegQueryValueExW,
};
use windows_sys::Win32::System::Threading::{CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW};
use windows_sys::Win32::UI::Shell::{
    FOLDERID_Templates, SHFILEINFOW, SHGFI_TYPENAME, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW, SHGetKnownFolderPath,
};
use windows_sys::core::GUID;

use crate::NewItem;

struct AutoKey(HKEY);

impl Drop for AutoKey {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: self.0 is an open registry key handle.
            unsafe { RegCloseKey(self.0) };
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn open_key(parent: HKEY, subkey: &str) -> Option<AutoKey> {
    let wide_key = wide(subkey);
    let mut h: HKEY = std::ptr::null_mut();
    // SAFETY: parent is valid handle or predefined root; subkey is null-terminated UTF-16; h receives the new key.
    let r = unsafe { RegOpenKeyExW(parent, wide_key.as_ptr(), 0, KEY_READ, &mut h) };
    if r == ERROR_SUCCESS { Some(AutoKey(h)) } else { None }
}

fn get_string_value(hkey: HKEY, name: Option<&str>) -> Option<String> {
    let wide_name = name.map(wide);
    let name_ptr = wide_name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr());
    let mut len = 0u32;
    // SAFETY: hkey is open; query size first.
    let r = unsafe {
        RegGetValueW(
            hkey,
            std::ptr::null(),
            name_ptr,
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut len,
        )
    };
    if r != ERROR_SUCCESS || len == 0 {
        return None;
    }
    let mut buf = vec![0u16; (len as usize).div_ceil(2)];
    // SAFETY: buf is allocated with the reported size; null-terminated string is returned.
    let r = unsafe {
        RegGetValueW(
            hkey,
            std::ptr::null(),
            name_ptr,
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if r != ERROR_SUCCESS {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]))
}

fn has_value(hkey: HKEY, name: &str) -> bool {
    let wide_name = wide(name);
    let mut val_type = 0u32;
    // SAFETY: hkey is open; query value existence and type.
    let r = unsafe {
        RegQueryValueExW(
            hkey,
            wide_name.as_ptr(),
            std::ptr::null_mut(),
            &mut val_type,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    r == ERROR_SUCCESS
}

fn get_data_bytes(hkey: HKEY) -> Option<Vec<u8>> {
    let wide_name = wide("Data");
    let mut val_type = 0u32;
    let mut len = 0u32;
    // SAFETY: query Data value type and length.
    let r = unsafe {
        RegQueryValueExW(hkey, wide_name.as_ptr(), std::ptr::null_mut(), &mut val_type, std::ptr::null_mut(), &mut len)
    };
    if r != ERROR_SUCCESS {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    // SAFETY: buf is sized to len; reads the raw bytes.
    let r = unsafe {
        RegQueryValueExW(hkey, wide_name.as_ptr(), std::ptr::null_mut(), &mut val_type, buf.as_mut_ptr(), &mut len)
    };
    if r != ERROR_SUCCESS {
        return None;
    }
    buf.truncate(len as usize);
    if val_type == REG_BINARY {
        Some(buf)
    } else if val_type == REG_SZ || val_type == REG_EXPAND_SZ {
        // String value in registry is UTF-16 code units; convert to string bytes.
        let u16_len = len as usize / 2;
        let u16_slice: &[u16] = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast(), u16_len) };
        let end = u16_slice.iter().position(|&c| c == 0).unwrap_or(u16_slice.len());
        let s = String::from_utf16_lossy(&u16_slice[..end]);
        Some(s.into_bytes())
    } else {
        Some(buf)
    }
}

fn find_shell_new_key(ext: &str) -> Option<AutoKey> {
    // 1. Direct: HKCR\<ext>\ShellNew
    let direct = format!("{ext}\\ShellNew");
    if let Some(k) = open_key(HKEY_CLASSES_ROOT as HKEY, &direct) {
        return Some(k);
    }
    // 2. ProgID: HKCR\<ext>\<ProgID>\ShellNew where ProgID is default value of HKCR\<ext>
    if let Some(k_ext) = open_key(HKEY_CLASSES_ROOT as HKEY, ext)
        && let Some(prog_id) = get_string_value(k_ext.0, None)
    {
        let prog_id = prog_id.trim();
        if !prog_id.is_empty() {
            let prog_key = format!("{ext}\\{prog_id}\\ShellNew");
            if let Some(k) = open_key(HKEY_CLASSES_ROOT as HKEY, &prog_key) {
                return Some(k);
            }
        }
    }
    None
}

fn type_name_of(ext: &str) -> Option<String> {
    let dummy = wide(&format!("x{ext}"));
    let mut info: SHFILEINFOW = unsafe { std::mem::zeroed() };
    // SAFETY: SHGetFileInfoW with SHGFI_USEFILEATTRIBUTES looks up the type name without disk access.
    let r = unsafe {
        SHGetFileInfoW(
            dummy.as_ptr(),
            FILE_ATTRIBUTE_NORMAL,
            &mut info,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES,
        )
    };
    if r == 0 {
        return None;
    }
    let end = info.szTypeName.iter().position(|&c| c == 0).unwrap_or(info.szTypeName.len());
    let name = String::from_utf16_lossy(&info.szTypeName[..end]).trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

fn get_classes_multi_sz() -> Option<Vec<String>> {
    let subkey = wide(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Discardable\PostSetup\ShellNew");
    let value_name = wide("Classes");
    let mut len = 0u32;
    // SAFETY: query size of Classes REG_MULTI_SZ.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER as HKEY,
            subkey.as_ptr(),
            value_name.as_ptr(),
            RRF_RT_REG_MULTI_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut len,
        )
    };
    if r != ERROR_SUCCESS || len == 0 {
        return None;
    }
    let mut buf = vec![0u16; (len as usize).div_ceil(2)];
    // SAFETY: read multi-sz into allocated buffer.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER as HKEY,
            subkey.as_ptr(),
            value_name.as_ptr(),
            RRF_RT_REG_MULTI_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if r != ERROR_SUCCESS {
        return None;
    }
    let mut list = Vec::new();
    let mut current = Vec::new();
    for &c in &buf {
        if c == 0 {
            if !current.is_empty() {
                list.push(String::from_utf16_lossy(&current));
                current.clear();
            }
        } else {
            current.push(c);
        }
    }
    Some(list)
}

fn enumerate_hkcr_extensions() -> Vec<String> {
    let mut exts = Vec::new();
    let mut index = 0u32;
    let mut name_buf = [0u16; 256];
    loop {
        let mut name_len = name_buf.len() as u32;
        // SAFETY: enumerates HKCR subkeys sequentially until ERROR_NO_MORE_ITEMS.
        let r = unsafe {
            RegEnumKeyExW(
                HKEY_CLASSES_ROOT as HKEY,
                index,
                name_buf.as_mut_ptr(),
                &mut name_len,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if r == ERROR_NO_MORE_ITEMS {
            break;
        }
        if r != ERROR_SUCCESS {
            break;
        }
        index += 1;
        let name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
        if name.starts_with('.') {
            exts.push(name);
        }
    }
    exts
}

pub fn new_items() -> Vec<NewItem> {
    let mut candidates = if let Some(classes) = get_classes_multi_sz() {
        classes.into_iter().filter(|e| !e.eq_ignore_ascii_case("folder") && e.starts_with('.')).collect()
    } else {
        enumerate_hkcr_extensions()
    };

    if !candidates.iter().any(|e| e.eq_ignore_ascii_case(".txt")) && find_shell_new_key(".txt").is_some() {
        candidates.push(".txt".into());
    }

    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for ext in candidates {
        if !seen.insert(ext.to_ascii_lowercase()) {
            continue;
        }
        if find_shell_new_key(&ext).is_none() {
            continue;
        }
        if let Some(name) = type_name_of(&ext) {
            items.push(NewItem { ext, name });
        }
    }
    // Explorer lists the types by name, whatever order the registry keeps them in.
    items.sort_by_key(|i| i.name.to_lowercase());
    items
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    let mut p = std::ptr::null_mut();
    // SAFETY: id is valid GUID; p receives CoTaskMemAlloc allocated path string on success.
    let hr = unsafe { SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut p) };
    let out = if hr >= 0 && !p.is_null() {
        let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
        Some(PathBuf::from(OsString::from_wide(unsafe { std::slice::from_raw_parts(p, len) })))
    } else {
        None
    };
    unsafe { CoTaskMemFree(p as *const c_void) };
    out
}

fn resolve_template(filename: &str) -> Option<PathBuf> {
    let p = Path::new(filename);
    if p.is_absolute() {
        if p.is_file() {
            return Some(p.to_path_buf());
        }
        return None;
    }
    // %WINDIR%\ShellNew
    if let Some(windir) = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")) {
        let candidate = PathBuf::from(windir).join("ShellNew").join(filename);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    // FOLDERID_Templates
    if let Some(templates_dir) = known_folder(&FOLDERID_Templates) {
        let candidate = templates_dir.join(filename);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn expand_env_strings(s: &str) -> String {
    let w_in = wide(s);
    // SAFETY: query required buffer length.
    let len = unsafe { ExpandEnvironmentStringsW(w_in.as_ptr(), std::ptr::null_mut(), 0) };
    if len == 0 {
        return s.to_string();
    }
    let mut buf = vec![0u16; len as usize];
    // SAFETY: expand environment strings into sized buffer.
    let r = unsafe { ExpandEnvironmentStringsW(w_in.as_ptr(), buf.as_mut_ptr(), len) };
    if r == 0 {
        return s.to_string();
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn run_command(cmd_template: &str, file_path: &Path) -> io::Result<()> {
    let expanded = expand_env_strings(cmd_template);
    let full_path = std::path::absolute(file_path).unwrap_or_else(|_| file_path.to_path_buf());
    let path_str = full_path.to_string_lossy();
    let cmd =
        if expanded.contains("%1") { expanded.replace("%1", &path_str) } else { format!("{expanded} \"{path_str}\"") };

    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let mut cmd_w = wide(&cmd);
    // SAFETY: CreateProcessW launches child process asynchronously; handles closed immediately.
    let ok = unsafe {
        CreateProcessW(
            std::ptr::null(),
            cmd_w.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            0,
            std::ptr::null(),
            std::ptr::null(),
            &si,
            &mut pi,
        )
    };
    if ok != 0 {
        unsafe {
            CloseHandle(pi.hProcess);
            CloseHandle(pi.hThread);
        }
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub fn create_new(dir: &Path, item: &NewItem) -> io::Result<PathBuf> {
    let key = find_shell_new_key(&item.ext)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no ShellNew handler for {}", item.ext)))?;

    let filename = crate::unique_new_name(&item.name, &item.ext, |name| dir.join(name).exists());
    let path = dir.join(filename);

    let has_file_name = get_string_value(key.0, Some("FileName"));
    let template_path = has_file_name.as_deref().and_then(resolve_template);
    let has_null_file = has_value(key.0, "NullFile");
    let data_bytes = get_data_bytes(key.0);
    let command = get_string_value(key.0, Some("Command"));

    if let Some(src) = template_path {
        std::fs::copy(&src, &path)?;
    } else if has_null_file {
        std::fs::File::create(&path)?;
    } else if let Some(bytes) = data_bytes {
        std::fs::write(&path, bytes)?;
    } else if let Some(f) = &has_file_name {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("template file not found: {f}")));
    } else {
        // Only Command exists, or no other values: create empty file first.
        std::fs::File::create(&path)?;
    }

    if let Some(cmd) = command
        && !cmd.trim().is_empty()
    {
        run_command(&cmd, &path)?;
    }

    Ok(path)
}
