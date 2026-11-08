//! Preview handlers: the same `IPreviewHandler` COM objects Explorer's Preview pane hosts
//! (Word, Excel, PowerPoint, PDF, Outlook messages, fonts, media, …). Each one draws into a
//! child window of ours that the app keeps over the pane's rectangle.
//!
//! Handlers are created out of process first (`CLSCTX_LOCAL_SERVER`, i.e. inside Windows'
//! `prevhost.exe` surrogate, as Explorer does) so a crashing handler can't take the window
//! down; in-process is the fallback for handlers that opt out of isolation.

use std::io;
use std::path::Path;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, CoCreateInstance, STGM_READ, STGM_SHARE_DENY_NONE,
};
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithFile, IInitializeWithStream};
use windows::Win32::UI::Shell::{
    ASSOCF_INIT_DEFAULTTOSTAR, ASSOCF_NOTRUNCATE, ASSOCSTR_SHELLEXTENSION, AssocQueryStringW, IInitializeWithItem,
    IPreviewHandler, IShellItem, SHCreateItemFromParsingName, SHCreateStreamOnFileEx,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GWL_STYLE, GetWindowLongPtrW, HWND_TOP, SW_HIDE, SW_SHOWNA, SWP_NOACTIVATE,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, WINDOW_EX_STYLE, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
};
use windows::core::{GUID, Interface, PCWSTR, w};

/// The shell-extension slot preview handlers register under (`{8895b1c6-…}`).
const PREVIEW_HANDLER_SLOT: PCWSTR = w!("{8895b1c6-b41f-4c1c-a562-0d564250836f}");

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn to_io(e: windows::core::Error) -> io::Error {
    io::Error::other(e.message())
}

/// Names the step that failed: "DoPreview: Not implemented" says more than the HRESULT alone.
fn step(name: &'static str) -> impl Fn(windows::core::Error) -> io::Error {
    move |e| io::Error::other(format!("{name}: {}", e.message()))
}

/// The CLSID of the preview handler registered for `ext` (".docx"), if any.
pub fn handler_for(ext: &str) -> Option<GUID> {
    if ext.is_empty() {
        return None;
    }
    let ext = wide(&format!(".{}", ext.trim_start_matches('.')));
    let mut buf = [0u16; 64];
    let mut len = buf.len() as u32;
    let flags = ASSOCF_INIT_DEFAULTTOSTAR | ASSOCF_NOTRUNCATE;
    let hr = unsafe {
        AssocQueryStringW(
            flags,
            ASSOCSTR_SHELLEXTENSION,
            PCWSTR(ext.as_ptr()),
            PREVIEW_HANDLER_SLOT,
            Some(windows::core::PWSTR(buf.as_mut_ptr())),
            &mut len,
        )
    };
    if hr.is_err() {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0)?;
    let s = String::from_utf16_lossy(&buf[..n]);
    GUID::try_from(s.trim_matches(|c| c == '{' || c == '}')).ok()
}

/// A live preview: the handler and the child window it draws in. Dropping it unloads the
/// handler and destroys the window.
pub struct PreviewHost {
    handler: IPreviewHandler,
    hwnd: HWND,
    rect: [i32; 4],
    visible: bool,
}

/// `[left, top, right, bottom]` in the parent's client pixels.
fn to_rect(r: [i32; 4]) -> RECT {
    RECT { left: r[0], top: r[1], right: r[2], bottom: r[3] }
}

/// Hands the file to the handler through whichever initializer it implements, trying the next
/// one when an initializer is there but refuses the file.
fn initialize(handler: &IPreviewHandler, path: &Path) -> windows::core::Result<()> {
    let p = wide(&path.to_string_lossy());
    let p = PCWSTR(p.as_ptr());
    let mut last = windows::core::Error::from(windows::Win32::Foundation::E_NOINTERFACE);
    if let Ok(i) = handler.cast::<IInitializeWithItem>() {
        let item: IShellItem = unsafe { SHCreateItemFromParsingName(p, None) }?;
        match unsafe { i.Initialize(&item, STGM_READ.0) } {
            Ok(()) => return Ok(()),
            Err(e) => last = e,
        }
    }
    if let Ok(i) = handler.cast::<IInitializeWithStream>() {
        let stream = unsafe { SHCreateStreamOnFileEx(p, (STGM_READ | STGM_SHARE_DENY_NONE).0, 0, false, None) }?;
        match unsafe { i.Initialize(&stream, STGM_READ.0) } {
            Ok(()) => return Ok(()),
            Err(e) => last = e,
        }
    }
    if let Ok(i) = handler.cast::<IInitializeWithFile>() {
        return unsafe { i.Initialize(p, STGM_READ.0) };
    }
    Err(last)
}

impl PreviewHost {
    /// Opens a preview of `path` with its registered handler, inside `parent` (the app's window) at
    /// `rect`. Runs on the UI thread, which is an STA.
    pub fn open(parent: isize, path: &Path, rect: [i32; 4]) -> io::Result<Self> {
        crate::windows::com_init();
        let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let clsid =
            handler_for(&ext).ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "no preview handler"))?;
        let parent = HWND(parent as *mut _);
        // Child windows must not be painted over by the parent's own drawing.
        unsafe {
            let style = GetWindowLongPtrW(parent, GWL_STYLE);
            SetWindowLongPtrW(parent, GWL_STYLE, style | WS_CLIPCHILDREN.0 as isize);
        }
        let handler: IPreviewHandler = unsafe { CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER) }
            .or_else(|_| unsafe { CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER) })
            .map_err(step("create"))?;
        initialize(&handler, path).map_err(step("initialize"))?;
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                PCWSTR::null(),
                WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                rect[0],
                rect[1],
                rect[2] - rect[0],
                rect[3] - rect[1],
                Some(parent),
                None,
                None,
                None,
            )
        }
        .map_err(to_io)?;
        let host = PreviewHost { handler, hwnd, rect, visible: false };
        let inner = to_rect([0, 0, rect[2] - rect[0], rect[3] - rect[1]]);
        unsafe { host.handler.SetWindow(hwnd, &inner) }.map_err(step("SetWindow"))?;
        unsafe { host.handler.DoPreview() }.map_err(step("DoPreview"))?;
        Ok(host)
    }

    /// Keeps the preview over `rect`, or hides it (`None`, e.g. while a menu covers the pane).
    pub fn place(&mut self, rect: Option<[i32; 4]>) {
        match rect {
            None if self.visible => {
                unsafe {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                }
                self.visible = false;
            }
            None => {}
            Some(r) => {
                if r != self.rect {
                    self.rect = r;
                    unsafe {
                        let _ = SetWindowPos(
                            self.hwnd,
                            Some(HWND_TOP),
                            r[0],
                            r[1],
                            r[2] - r[0],
                            r[3] - r[1],
                            SWP_NOACTIVATE,
                        );
                        let _ = self.handler.SetRect(&to_rect([0, 0, r[2] - r[0], r[3] - r[1]]));
                    }
                }
                if !self.visible {
                    unsafe {
                        let _ = ShowWindow(self.hwnd, SW_SHOWNA);
                    }
                    self.visible = true;
                }
            }
        }
    }
}

impl Drop for PreviewHost {
    fn drop(&mut self) {
        unsafe {
            let _ = self.handler.Unload();
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
