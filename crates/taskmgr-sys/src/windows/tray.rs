//! Notification-area icon that shows CPU usage as a bar, like Task Manager's.
//! Left click shows the window; right click offers Show and Exit.

use std::mem::size_of;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS, DeleteObject,
};
use windows_sys::Win32::UI::Shell::{
    DefSubclassProc, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    RemoveWindowSubclass, SetWindowSubclass, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos, HICON, ICONINFO,
    MF_STRING, PostMessageW, SW_HIDE, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP, WM_CLOSE, WM_LBUTTONUP, WM_RBUTTONUP,
};

const CALLBACK: u32 = WM_APP + 0x51;
const ICON_ID: u32 = 1;
const SUBCLASS_ID: usize = 0x1d0;
const MENU_SHOW: usize = 1;
const MENU_EXIT: usize = 2;
const SIZE: i32 = 16;

static HWND_V: AtomicUsize = AtomicUsize::new(0);
static ICON: AtomicUsize = AtomicUsize::new(0);

fn hwnd() -> HWND {
    HWND_V.load(Ordering::Relaxed) as HWND
}

fn data(h: HWND) -> NOTIFYICONDATAW {
    // SAFETY: plain C struct; zero is a valid start.
    let mut d: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    d.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    d.hWnd = h;
    d.uID = ICON_ID;
    d
}

/// A 16x16 icon: dark background, a bar `pct` high in Task Manager green.
fn cpu_icon(pct: f32) -> HICON {
    let h = ((pct.clamp(0.0, 100.0) / 100.0) * SIZE as f32).round() as i32;
    // SAFETY: a top-down 32-bit DIB we fill in place, plus an all-zero (opaque) mask; both are
    // copied by CreateIconIndirect and deleted here.
    unsafe {
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = SIZE;
        bi.bmiHeader.biHeight = -SIZE;
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;
        let mut bits = null_mut();
        let color = CreateDIBSection(null_mut(), &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if color.is_null() || bits.is_null() {
            return null_mut();
        }
        let px = std::slice::from_raw_parts_mut(bits.cast::<u32>(), (SIZE * SIZE) as usize);
        for y in 0..SIZE {
            for x in 0..SIZE {
                let edge = x == 0 || y == 0 || x == SIZE - 1 || y == SIZE - 1;
                let filled = y >= SIZE - h && x > 2 && x < SIZE - 3;
                // 0xAARRGGBB
                px[(y * SIZE + x) as usize] = if edge {
                    0xFF60_6060
                } else if filled {
                    0xFF4C_C24C
                } else {
                    0xFF10_1010
                };
            }
        }
        let mask = CreateBitmap(SIZE, SIZE, 1, 1, null());
        let ii = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&ii);
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}

fn show_window(h: HWND) {
    // SAFETY: plain window calls on our own window.
    unsafe {
        ShowWindow(h, SW_SHOW);
        ShowWindow(h, SW_RESTORE);
        SetForegroundWindow(h);
    }
}

unsafe extern "system" fn subclass(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    if msg == CALLBACK {
        match (lp & 0xffff) as u32 {
            WM_LBUTTONUP => show_window(h),
            WM_RBUTTONUP => {
                // SAFETY: a menu we create and destroy here; TPM_RETURNCMD returns the chosen id.
                unsafe {
                    let menu = CreatePopupMenu();
                    let show: Vec<u16> = "Show Task Manager\0".encode_utf16().collect();
                    let exit: Vec<u16> = "Exit\0".encode_utf16().collect();
                    AppendMenuW(menu, MF_STRING, MENU_SHOW, show.as_ptr());
                    AppendMenuW(menu, MF_STRING, MENU_EXIT, exit.as_ptr());
                    let mut pt = POINT { x: 0, y: 0 };
                    GetCursorPos(&mut pt);
                    SetForegroundWindow(h);
                    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, h, null()) as usize;
                    DestroyMenu(menu);
                    match cmd {
                        MENU_SHOW => show_window(h),
                        MENU_EXIT => {
                            show_window(h);
                            PostMessageW(h, WM_CLOSE, 0, 0);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        return 0;
    }
    // SAFETY: forwarding to the next handler in the chain.
    unsafe { DefSubclassProc(h, msg, wp, lp) }
}

/// Adds the icon for window `h` (a raw HWND). Safe to call more than once.
pub fn install(h: isize) -> bool {
    if HWND_V.load(Ordering::Relaxed) != 0 {
        return true;
    }
    let h = h as HWND;
    // SAFETY: subclassing our own window from its UI thread; the icon data is fully initialised.
    unsafe {
        if SetWindowSubclass(h, Some(subclass), SUBCLASS_ID, 0) == 0 {
            return false;
        }
        let mut d = data(h);
        d.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        d.uCallbackMessage = CALLBACK;
        d.hIcon = cpu_icon(0.0);
        let tip: Vec<u16> = "Task Manager (oxidows)".encode_utf16().collect();
        d.szTip[..tip.len()].copy_from_slice(&tip);
        if Shell_NotifyIconW(NIM_ADD, &d) == 0 {
            RemoveWindowSubclass(h, Some(subclass), SUBCLASS_ID);
            DestroyIcon(d.hIcon);
            return false;
        }
        ICON.store(d.hIcon as usize, Ordering::Relaxed);
    }
    HWND_V.store(h as usize, Ordering::Relaxed);
    true
}

/// Redraws the icon for `cpu_pct` and sets its tooltip. Callable from any thread.
pub fn update(cpu_pct: f32, tip: &str) {
    let h = hwnd();
    if h.is_null() {
        return;
    }
    let mut d = data(h);
    d.uFlags = NIF_ICON | NIF_TIP;
    d.hIcon = cpu_icon(cpu_pct);
    let tip: Vec<u16> = tip.encode_utf16().take(127).collect();
    d.szTip[..tip.len()].copy_from_slice(&tip);
    // SAFETY: modifying our own icon; the previous HICON is destroyed after the swap.
    unsafe {
        Shell_NotifyIconW(NIM_MODIFY, &d);
        let old = ICON.swap(d.hIcon as usize, Ordering::Relaxed);
        if old != 0 {
            DestroyIcon(old as HICON);
        }
    }
}

/// Hides the window (it stays reachable from the icon).
pub fn hide_window() {
    let h = hwnd();
    if !h.is_null() {
        // SAFETY: our own window.
        unsafe { ShowWindow(h, SW_HIDE) };
    }
}

pub fn remove() {
    let h = hwnd();
    if h.is_null() {
        return;
    }
    let d = data(h);
    // SAFETY: removing our own icon and subclass.
    unsafe {
        Shell_NotifyIconW(NIM_DELETE, &d);
        RemoveWindowSubclass(h, Some(subclass), SUBCLASS_ID);
        let old = ICON.swap(0, Ordering::Relaxed);
        if old != 0 {
            DestroyIcon(old as HICON);
        }
    }
    HWND_V.store(0, Ordering::Relaxed);
}
