//! Windows 11 Mica backdrop support via Desktop Window Manager (DWM).

/// `DWMWA_USE_IMMERSIVE_DARK_MODE` attribute (Windows 10 20H1+ / Windows 11).
pub const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;

/// `DWMWA_SYSTEMBACKDROP_TYPE` attribute (Windows 11 build 22621+).
pub const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;

/// Backdrop type: None (default opaque window).
pub const DWMSBT_NONE: u32 = 1;

/// Backdrop type: MainWindow (Mica).
pub const DWMSBT_MAINWINDOW: u32 = 2;

#[cfg(windows)]
#[repr(C)]
struct OsVersionInfoW {
    dw_os_version_info_size: u32,
    dw_major_version: u32,
    dw_minor_version: u32,
    dw_build_number: u32,
    dw_platform_id: u32,
    sz_csd_version: [u16; 128],
}

#[cfg(windows)]
unsafe extern "system" {
    fn RtlGetVersion(version_info: *mut OsVersionInfoW) -> i32;
}

#[cfg(windows)]
fn windows_build_number() -> u32 {
    let mut info = OsVersionInfoW {
        dw_os_version_info_size: std::mem::size_of::<OsVersionInfoW>() as u32,
        dw_major_version: 0,
        dw_minor_version: 0,
        dw_build_number: 0,
        dw_platform_id: 0,
        sz_csd_version: [0; 128],
    };
    // SAFETY: OsVersionInfoW buffer matches size field.
    if unsafe { RtlGetVersion(&mut info) } == 0 { info.dw_build_number } else { 0 }
}

/// Whether the system supports Mica backdrops (Windows 11 build 22621+, or forced via `OXI_MICA=1`).
pub fn is_supported() -> bool {
    if std::env::var_os("OXI_MICA").is_some_and(|v| v == "1") {
        return true;
    }
    #[cfg(windows)]
    {
        windows_build_number() >= 22621
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Applies or removes the Mica backdrop and sets immersive dark mode on `hwnd`.
pub fn apply(hwnd: isize, enable: bool, dark: bool) {
    if hwnd == 0 || !is_supported() {
        return;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::HWND;
        use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;

        let dark_val: i32 = if dark { 1 } else { 0 };
        let backdrop: u32 = if enable { DWMSBT_MAINWINDOW } else { DWMSBT_NONE };

        // SAFETY: DwmSetWindowAttribute receives a valid HWND and pointers to properly-sized attribute payloads.
        unsafe {
            let _ = DwmSetWindowAttribute(
                hwnd as HWND,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                (&raw const dark_val).cast(),
                std::mem::size_of::<i32>() as u32,
            );
            let _ = DwmSetWindowAttribute(
                hwnd as HWND,
                DWMWA_SYSTEMBACKDROP_TYPE,
                (&raw const backdrop).cast(),
                std::mem::size_of::<u32>() as u32,
            );
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (hwnd, enable, dark);
    }
}

/// Modifies `color` to have ~0.75 alpha (191) when `transparent` is true.
pub fn apply_transparency(color: egui::Color32, transparent: bool) -> egui::Color32 {
    if transparent {
        egui::Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            ((color.a() as f32) * 0.75).round() as u8,
        )
    } else {
        color
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparency_alpha_scaling() {
        let opaque = egui::Color32::from_rgb(100, 150, 200);
        let trans = apply_transparency(opaque, true);
        assert_eq!(trans.a(), 191);
        let unmult = trans.to_srgba_unmultiplied();
        assert_eq!(unmult[0], 100);
        assert_eq!(unmult[1], 150);
        assert_eq!(unmult[2], 200);
        assert_eq!(unmult[3], 191);

        let unchanged = apply_transparency(opaque, false);
        assert_eq!(unchanged, opaque);
    }
}
