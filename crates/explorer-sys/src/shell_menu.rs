//! The shell's own right-click menu (`IContextMenu`): every handler installed on the machine
//! (Open with, Send to, 7-Zip, Git, Run as administrator, Share, …) shows up as in Explorer.

use std::cell::RefCell;
use std::ffi::c_void;
use std::io;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    CMF_CANRENAME, CMF_EXPLORE, CMF_EXTENDEDVERBS, CMF_NORMAL, CMIC_MASK_PTINVOKE, CMIC_MASK_SHIFT_DOWN,
    CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, DefSubclassProc, GCS_VERBW, IContextMenu, IContextMenu2, IContextMenu3,
    ILFree, IShellFolder, RemoveWindowSubclass, SHBindToParent, SHGetDesktopFolder, SHParseDisplayName,
    SetWindowSubclass,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GetCursorPos, SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx,
    WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR,
};
use windows::core::{Interface, PCSTR, PCWSTR};

use crate::MenuOutcome;

const CMIC_MASK_UNICODE: u32 = 0x4000;
const FIRST_ID: u32 = 1;
const LAST_ID: u32 = 0x7FFF;
const SUBCLASS_ID: usize = 0x0F1E;

thread_local! {
    /// The menu being shown, so the subclassed window can forward owner-draw and submenu
    /// messages to it ("Send to" and "Open with" fill themselves in on WM_INITMENUPOPUP).
    static ACTIVE: RefCell<Option<IContextMenu>> = const { RefCell::new(None) };
}

unsafe extern "system" fn forward_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR) {
        let handled = ACTIVE.with(|a| {
            let a = a.borrow();
            let cm = a.as_ref()?;
            if let Ok(cm3) = cm.cast::<IContextMenu3>() {
                let mut result = LRESULT(0);
                if unsafe { cm3.HandleMenuMsg2(msg, wparam, lparam, Some(&mut result)) }.is_ok() {
                    return Some(result);
                }
            } else if let Ok(cm2) = cm.cast::<IContextMenu2>()
                && unsafe { cm2.HandleMenuMsg(msg, wparam, lparam) }.is_ok()
            {
                return Some(LRESULT(0));
            }
            None
        });
        if let Some(r) = handled {
            return r;
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// A path, or for `::shell\\…` items the shell parsing name, as UTF-16.
fn wide(s: &Path) -> Vec<u16> {
    explorer_core::shell_parsing(s).encode_utf16().chain(Some(0)).collect()
}

fn to_io(e: windows::core::Error) -> io::Error {
    io::Error::other(e.message())
}

/// Absolute PIDLs, freed on drop.
struct Pidls(Vec<*mut ITEMIDLIST>);
impl Drop for Pidls {
    fn drop(&mut self) {
        for &p in &self.0 {
            unsafe { ILFree(Some(p)) };
        }
    }
}

fn parse(paths: &[PathBuf]) -> io::Result<Pidls> {
    let mut out = Pidls(Vec::with_capacity(paths.len()));
    for p in paths {
        let w = wide(p);
        let mut pidl = std::ptr::null_mut();
        unsafe { SHParseDisplayName(PCWSTR(w.as_ptr()), None, &mut pidl, 0, None) }.map_err(to_io)?;
        out.0.push(pidl);
    }
    Ok(out)
}

/// Menu for `items` (all in one folder), or for the folder `dir` itself when `items` is empty
/// (the background menu: New, Paste, Properties…). Shown at the mouse pointer; blocks until it
/// closes. Verbs the host must handle itself are returned instead of run.
fn context_menu(hwnd: HWND, dir: &Path, items: &[PathBuf]) -> io::Result<IContextMenu> {
    crate::windows::com_init();
    Ok(unsafe {
        if items.is_empty() {
            let pidls = parse(&[dir.to_path_buf()])?;
            let desktop: IShellFolder = SHGetDesktopFolder().map_err(to_io)?;
            let folder: IShellFolder = desktop.BindToObject(pidls.0[0], None).map_err(to_io)?;
            folder.CreateViewObject(hwnd).map_err(to_io)?
        } else {
            let pidls = parse(items)?;
            let mut child = std::ptr::null_mut();
            let parent: IShellFolder = SHBindToParent(pidls.0[0], Some(&mut child)).map_err(to_io)?;
            // Every item's last ID, relative to the shared parent.
            let children: Vec<*const ITEMIDLIST> = pidls
                .0
                .iter()
                .map(|&p| {
                    let mut c = std::ptr::null_mut();
                    let _: windows::core::Result<IShellFolder> = SHBindToParent(p, Some(&mut c));
                    c as *const ITEMIDLIST
                })
                .collect();
            let cm = parent.GetUIObjectOf(hwnd, &children, None).map_err(to_io)?;
            drop(pidls);
            cm
        }
    })
}

fn verb_of(cm: &IContextMenu, offset: usize) -> String {
    let mut verb = [0u16; 128];
    unsafe {
        cm.GetCommandString(offset, GCS_VERBW, None, windows::core::PSTR(verb.as_mut_ptr().cast()), verb.len() as u32)
    }
    .map(|()| {
        let n = verb.iter().position(|&c| c == 0).unwrap_or(0);
        String::from_utf16_lossy(&verb[..n]).to_ascii_lowercase()
    })
    .unwrap_or_default()
}

/// Canonical verbs the menu would offer (for tests and diagnostics; nothing is shown).
pub fn verbs(dir: &Path, items: &[PathBuf]) -> io::Result<Vec<String>> {
    use windows::Win32::UI::WindowsAndMessaging::{GetMenuItemCount, GetMenuItemID};
    let cm = context_menu(HWND::default(), dir, items)?;
    let menu = unsafe { CreatePopupMenu() }.map_err(to_io)?;
    unsafe { cm.QueryContextMenu(menu, 0, FIRST_ID, LAST_ID, CMF_NORMAL | CMF_EXPLORE) }.ok().map_err(to_io)?;
    let n = unsafe { GetMenuItemCount(Some(menu)) };
    let mut out = Vec::new();
    for i in 0..n.max(0) {
        let id = unsafe { GetMenuItemID(menu, i) };
        if id != u32::MAX && id >= FIRST_ID {
            let v = verb_of(&cm, (id - FIRST_ID) as usize);
            if !v.is_empty() {
                out.push(v);
            }
        }
    }
    unsafe {
        let _ = DestroyMenu(menu);
    }
    Ok(out)
}

pub fn show(hwnd: isize, dir: &Path, items: &[PathBuf], extended: bool) -> io::Result<MenuOutcome> {
    let hwnd = HWND(hwnd as *mut c_void);
    let cm = context_menu(hwnd, dir, items)?;

    let menu = unsafe { CreatePopupMenu() }.map_err(to_io)?;
    let flags = CMF_NORMAL | CMF_EXPLORE | CMF_CANRENAME | if extended { CMF_EXTENDEDVERBS } else { 0 };
    unsafe { cm.QueryContextMenu(menu, 0, FIRST_ID, LAST_ID, flags) }.ok().map_err(to_io)?;

    let mut pt = POINT::default();
    let _ = unsafe { GetCursorPos(&mut pt) };
    ACTIVE.with(|a| *a.borrow_mut() = Some(cm.clone()));
    unsafe {
        let _ = SetWindowSubclass(hwnd, Some(forward_proc), SUBCLASS_ID, 0);
    }
    let cmd = unsafe { TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, pt.x, pt.y, hwnd, None) }.0 as u32;
    unsafe {
        let _ = RemoveWindowSubclass(hwnd, Some(forward_proc), SUBCLASS_ID);
    }
    ACTIVE.with(|a| *a.borrow_mut() = None);

    let outcome = if cmd >= FIRST_ID {
        let offset = (cmd - FIRST_ID) as usize;
        let verb = verb_of(&cm, offset);
        // Rename needs an in-place editor; opening a folder should stay in this window.
        let host = verb == "rename"
            || ((verb == "open" || verb == "explore")
                && !items.is_empty()
                && items
                    .iter()
                    .all(|p| p.is_dir() || (p.starts_with(explorer_core::SHELL) && crate::shell_ns::is_folder(p))));
        if host {
            MenuOutcome::Host(verb)
        } else {
            let dir_w = wide(dir);
            let ex = CMINVOKECOMMANDINFOEX {
                cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
                fMask: CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE | if extended { CMIC_MASK_SHIFT_DOWN } else { 0 },
                hwnd,
                // MAKEINTRESOURCE(offset): the verb as a command number.
                lpVerb: PCSTR(offset as *const u8),
                lpVerbW: PCWSTR(offset as *const u16),
                lpDirectoryW: PCWSTR(dir_w.as_ptr()),
                nShow: SW_SHOWNORMAL.0,
                ptInvoke: pt,
                ..Default::default()
            };
            unsafe { cm.InvokeCommand(&ex as *const CMINVOKECOMMANDINFOEX as *const CMINVOKECOMMANDINFO) }
                .map_err(to_io)?;
            MenuOutcome::Invoked(verb)
        }
    } else {
        MenuOutcome::Dismissed
    };
    unsafe {
        let _ = DestroyMenu(menu);
    }
    Ok(outcome)
}

/// The shell's data object for `items` (all in one folder): what Explorer puts on the
/// clipboard or in a drag, with every format other programs expect (CF_HDROP, shell IDs,
/// file contents for virtual targets…).
fn data_object(hwnd: HWND, items: &[PathBuf]) -> io::Result<windows::Win32::System::Com::IDataObject> {
    crate::windows::com_init();
    let pidls = parse(items)?;
    let parent: IShellFolder = unsafe { SHBindToParent(pidls.0[0], None) }.map_err(to_io)?;
    let children: Vec<*const ITEMIDLIST> = pidls
        .0
        .iter()
        .map(|&p| {
            let mut c = std::ptr::null_mut();
            let _: windows::core::Result<IShellFolder> = unsafe { SHBindToParent(p, Some(&mut c)) };
            c as *const ITEMIDLIST
        })
        .collect();
    unsafe { parent.GetUIObjectOf(hwnd, &children, None) }.map_err(to_io)
}

/// Drags `items` out with OLE drag and drop, like dragging from Explorer: any window that
/// accepts files (Explorer, the desktop, mail, chat apps, editors…) can take them. Blocks
/// until the drop or cancel. Returns what the target did.
pub fn drag_out(hwnd: isize, items: &[PathBuf]) -> io::Result<crate::DropResult> {
    use windows::Win32::System::Ole::{DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, OleInitialize};
    use windows::Win32::UI::Shell::SHDoDragDrop;
    let hwnd = HWND(hwnd as *mut c_void);
    // Already done by winit for its drop target; a second call on the same thread is harmless.
    let _ = unsafe { OleInitialize(None) };
    let data = data_object(hwnd, items)?;
    // No drop source of our own: the shell supplies the standard one (drag image, cursors,
    // Esc to cancel, right-button drag menu).
    let effect = unsafe {
        SHDoDragDrop(
            Some(hwnd),
            &data,
            None::<&windows::Win32::System::Ole::IDropSource>,
            DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK,
        )
    };
    match effect {
        Ok(e) if e.contains(DROPEFFECT_MOVE) => Ok(crate::DropResult::Moved),
        Ok(e) if e.contains(DROPEFFECT_COPY) => Ok(crate::DropResult::Copied),
        Ok(e) if e.contains(DROPEFFECT_LINK) => Ok(crate::DropResult::Linked),
        Ok(_) => Ok(crate::DropResult::None),
        // DRAGDROP_S_CANCEL is a success code; failures are real errors.
        Err(e) => Err(to_io(e)),
    }
}

/// Target of a `.lnk` shortcut (files and folders only; shortcuts to virtual items give `None`).
pub fn resolve_link(lnk: &Path) -> Option<PathBuf> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    crate::windows::com_init();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        let w = wide(lnk);
        file.Load(PCWSTR(w.as_ptr()), STGM_READ).ok()?;
        let mut buf = [0u16; 1024];
        // SLGP_RAWPATH = 4: don't expand or resolve; fast, no disk access to the target.
        link.GetPath(&mut buf, std::ptr::null_mut(), 4).ok()?;
        let n = buf.iter().position(|&c| c == 0)?;
        (n > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n])))
    }
}

/// Runs one canonical verb ("copy", "cut", "paste", "delete", "properties", "rename"…) of the
/// shell's menu for `items` (or for the folder `dir` when empty), without showing the menu.
/// Used for keyboard shortcuts in shell namespace folders (a phone), where the shell itself
/// has to do the work.
pub fn invoke_verb(hwnd: isize, dir: &Path, items: &[PathBuf], verb: &str) -> io::Result<()> {
    let hwnd = HWND(hwnd as *mut c_void);
    let cm = context_menu(hwnd, dir, items)?;
    let menu = unsafe { CreatePopupMenu() }.map_err(to_io)?;
    let r = (|| {
        unsafe { cm.QueryContextMenu(menu, 0, FIRST_ID, LAST_ID, CMF_NORMAL | CMF_EXPLORE) }.ok().map_err(to_io)?;
        let verb_a: Vec<u8> = verb.bytes().chain(Some(0)).collect();
        let verb_w: Vec<u16> = verb.encode_utf16().chain(Some(0)).collect();
        let dir_w = wide(dir);
        let ex = CMINVOKECOMMANDINFOEX {
            cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
            fMask: CMIC_MASK_UNICODE,
            hwnd,
            lpVerb: PCSTR(verb_a.as_ptr()),
            lpVerbW: PCWSTR(verb_w.as_ptr()),
            lpDirectoryW: PCWSTR(dir_w.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        unsafe { cm.InvokeCommand(&ex as *const CMINVOKECOMMANDINFOEX as *const CMINVOKECOMMANDINFO) }.map_err(to_io)
    })();
    unsafe {
        let _ = DestroyMenu(menu);
    }
    r
}
