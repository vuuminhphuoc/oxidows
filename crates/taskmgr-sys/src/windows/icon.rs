//! A program's small icon as RGBA pixels, for the Processes, Details and Users tables.

use std::cell::Cell;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW, HBITMAP,
    HDC, ReleaseDC,
};
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
use windows_sys::Win32::UI::Shell::{
    SHFILEINFOW, SHGFI_ICON, SHGFI_SMALLICON, SHGFI_USEFILEATTRIBUTES, SHGSI_ICON, SHGSI_SMALLICON, SHGetFileInfoW,
    SHGetStockIconInfo, SHSTOCKICONINFO, SIID_APPLICATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};

use crate::IconImage;

const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;

/// The shell wants COM on the calling thread for some icon handlers.
fn com_init() {
    thread_local!(static DONE: Cell<bool> = const { Cell::new(false) });
    DONE.with(|d| {
        if !d.get() {
            // SAFETY: plain initialisation of this thread's apartment; repeat calls are harmless.
            unsafe { CoInitializeEx(null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32) };
            d.set(true);
        }
    });
}

/// Reads a bitmap as 32-bit top-down BGRA.
///
/// # Safety
/// `dc` and `bmp` must be valid handles.
unsafe fn bitmap_bgra(dc: HDC, bmp: HBITMAP, w: i32, h: i32) -> Option<Vec<u8>> {
    // SAFETY: plain C struct; zero is a valid start.
    let mut bi: BITMAPINFO = unsafe { zeroed() };
    bi.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w,
        biHeight: -h,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        // SAFETY: as above.
        ..unsafe { zeroed() }
    };
    let mut buf = vec![0u8; (w * h * 4) as usize];
    // SAFETY: `buf` holds exactly `h` rows of `w` 32-bit pixels.
    let lines = unsafe { GetDIBits(dc, bmp, 0, h as u32, buf.as_mut_ptr().cast(), &mut bi, DIB_RGB_COLORS) };
    (lines == h).then_some(buf)
}

/// The small icon Explorer shows for `path`. An empty path (System, Registry, Idle...) or a file
/// that cannot be read gets the generic program icon, as Task Manager shows for those.
pub fn exe_icon(path: &str) -> Option<IconImage> {
    com_init();
    // SAFETY: plain C struct; zero is a valid start.
    let mut info: SHFILEINFOW = unsafe { zeroed() };
    let size = size_of::<SHFILEINFOW>() as u32;
    let mut got = 0;
    if !path.is_empty() {
        let w: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `w` is NUL-terminated and `info` is sized for the call.
        got = unsafe { SHGetFileInfoW(w.as_ptr(), 0, &mut info, size, SHGFI_ICON | SHGFI_SMALLICON) };
    }
    if got == 0 || info.hIcon.is_null() {
        let mut stock: SHSTOCKICONINFO = unsafe { zeroed() };
        stock.cbSize = size_of::<SHSTOCKICONINFO>() as u32;
        // SAFETY: sized SHSTOCKICONINFO pointer.
        if unsafe { SHGetStockIconInfo(SIID_APPLICATION, SHGSI_ICON | SHGSI_SMALLICON, &mut stock) } == 0
            && !stock.hIcon.is_null()
        {
            info.hIcon = stock.hIcon;
            got = 1;
        }
    }
    if got == 0 || info.hIcon.is_null() {
        let w: Vec<u16> = "program.exe\0".encode_utf16().collect();
        // SAFETY: as above; USEFILEATTRIBUTES never touches the disk.
        got = unsafe {
            SHGetFileInfoW(
                w.as_ptr(),
                FILE_ATTRIBUTE_NORMAL,
                &mut info,
                size,
                SHGFI_ICON | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
            )
        };
    }
    if got == 0 || info.hIcon.is_null() {
        return None;
    }
    // SAFETY: `info.hIcon` is a valid icon we own; every handle made below is freed below.
    unsafe {
        let mut ii: ICONINFO = zeroed();
        let mut out = None;
        if GetIconInfo(info.hIcon, &mut ii) != 0 && !ii.hbmColor.is_null() {
            let mut bm: BITMAP = zeroed();
            GetObjectW(ii.hbmColor, size_of::<BITMAP>() as i32, (&mut bm as *mut BITMAP).cast());
            let (bw, bh) = (bm.bmWidth, bm.bmHeight);
            if bw > 0 && bh > 0 && bw <= 256 && bh <= 256 {
                let dc = GetDC(null_mut());
                if let Some(mut px) = bitmap_bgra(dc, ii.hbmColor, bw, bh) {
                    // Old-style icons carry no alpha; transparency then comes from the AND mask.
                    if px.chunks_exact(4).all(|p| p[3] == 0) {
                        let mask = if ii.hbmMask.is_null() { None } else { bitmap_bgra(dc, ii.hbmMask, bw, bh) };
                        for (i, p) in px.chunks_exact_mut(4).enumerate() {
                            let transparent = mask.as_ref().is_some_and(|m| m[i * 4] != 0);
                            p[3] = if transparent { 0 } else { 255 };
                        }
                    }
                    for p in px.chunks_exact_mut(4) {
                        p.swap(0, 2); // BGRA -> RGBA
                    }
                    out = Some(IconImage { width: bw as usize, height: bh as usize, rgba: px });
                }
                ReleaseDC(null_mut(), dc);
            }
        }
        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor);
        }
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask);
        }
        DestroyIcon(info.hIcon);
        out
    }
}

/// The shell generic application icon, shown when a process has no path or its icon cannot be loaded.
pub fn default_app_icon() -> Option<IconImage> {
    exe_icon("")
}
