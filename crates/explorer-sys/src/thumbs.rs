//! Thumbnails (pictures, videos, PDFs, …) from the shell's thumbnail cache, the same ones
//! Explorer shows: `IShellItemImageFactory::GetImage`.

use std::mem::{size_of, zeroed};
use std::path::Path;

use windows::Win32::Foundation::SIZE;
use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_RESIZETOFIT};
use windows::core::{Interface, PCWSTR};
use windows_sys::Win32::Graphics::Gdi::{BITMAP, DeleteObject, GetDC, GetObjectW, ReleaseDC};

use crate::IconImage;

pub fn thumbnail(path: &Path, size: u32) -> Option<IconImage> {
    crate::windows::com_init();
    let factory: IShellItemImageFactory = if path.starts_with(explorer_core::SHELL) {
        crate::shell_ns::item(path).ok()?.cast().ok()?
    } else {
        let w: Vec<u16> = path.to_string_lossy().encode_utf16().chain(Some(0)).collect();
        unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None) }.ok()?
    };
    let s = SIZE { cx: size as i32, cy: size as i32 };
    let hbmp = unsafe { factory.GetImage(s, SIIGBF_RESIZETOFIT) }.ok()?;
    let raw = hbmp.0;
    let mut bm: BITMAP = unsafe { zeroed() };
    unsafe { GetObjectW(raw, size_of::<BITMAP>() as i32, (&mut bm as *mut BITMAP).cast()) };
    let (bw, bh) = (bm.bmWidth, bm.bmHeight.abs());
    let mut out = None;
    if bw > 0 && bh > 0 && bw <= 1024 && bh <= 1024 {
        let dc = unsafe { GetDC(std::ptr::null_mut()) };
        if let Some(mut px) = unsafe { crate::windows::bitmap_bgra(dc, raw, bw, bh) } {
            let opaque = px.as_chunks::<4>().0.iter().all(|p| p[3] == 0);
            for p in px.as_chunks_mut::<4>().0.iter_mut() {
                p.swap(0, 2);
                if opaque {
                    p[3] = 255; // 24-bit source: no alpha at all
                }
            }
            out = Some(IconImage { width: bw as usize, height: bh as usize, rgba: px });
        }
        unsafe { ReleaseDC(std::ptr::null_mut(), dc) };
    }
    unsafe { DeleteObject(raw) };
    out
}
