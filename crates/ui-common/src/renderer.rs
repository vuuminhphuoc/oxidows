//! Which GPU backend to draw with.
//!
//! OpenGL (glow) where the PC has a real GPU driver: measured on an RTX 3060, a window takes
//! ~120-180 MB private memory with OpenGL against ~490-500 MB with wgpu (Vulkan/DX12 driver
//! heaps), and opens as fast. wgpu where there is none (VMs, RDP, CI runners): there OpenGL is
//! only the GDI 1.1 stub, while wgpu falls back to the WARP software rasteriser.

use std::ffi::OsString;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renderer {
    /// OpenGL through glow.
    Glow,
    Wgpu,
}

/// The renderer to start with, and whether it was picked automatically (so a failure to start
/// may retry with the other one). `OXI_RENDERER=glow|wgpu` overrides.
pub fn choose() -> (Renderer, bool) {
    match std::env::var("OXI_RENDERER").as_deref() {
        Ok("glow") => (Renderer::Glow, false),
        Ok("wgpu") => (Renderer::Wgpu, false),
        _ if !cfg!(windows) || has_gpu_driver() => (Renderer::Glow, true),
        _ => (Renderer::Wgpu, true),
    }
}

/// Starts this program again with `OXI_RENDERER` set, for when the automatic choice could not
/// create its window (a window system's event loop can only be made once per process).
pub fn relaunch_with(r: Renderer) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let name = match r {
        Renderer::Glow => "glow",
        Renderer::Wgpu => "wgpu",
    };
    std::process::Command::new(exe).args(args).env("OXI_RENDERER", name).spawn().map(drop)
}

/// A desktop-attached display adapter from a GPU vendor whose drivers ship OpenGL, outside a
/// remote session.
#[cfg(windows)]
fn has_gpu_driver() -> bool {
    use windows_sys::Win32::Graphics::Gdi::{DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICEW, EnumDisplayDevicesW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_REMOTESESSION};
    // SAFETY: no arguments.
    if unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0 {
        return false;
    }
    (0..16).any(|i| {
        // SAFETY: zeroed POD with its size set, as the API requires.
        let mut d: DISPLAY_DEVICEW = unsafe { std::mem::zeroed() };
        d.cb = size_of::<DISPLAY_DEVICEW>() as u32;
        // SAFETY: null device name enumerates adapters; `d` is valid for writes.
        if unsafe { EnumDisplayDevicesW(std::ptr::null(), i, &mut d, 0) } == 0 {
            return false;
        }
        let n = d.DeviceString.iter().position(|&c| c == 0).unwrap_or(d.DeviceString.len());
        d.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP != 0
            && is_gpu_vendor(&String::from_utf16_lossy(&d.DeviceString[..n]))
    })
}

#[cfg(not(windows))]
fn has_gpu_driver() -> bool {
    true
}

/// Adapter names of vendors with real OpenGL drivers. "Microsoft Basic Display Adapter",
/// "Microsoft Remote Display Adapter", Hyper-V and other virtual adapters are not.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_gpu_vendor(device: &str) -> bool {
    let d = device.to_ascii_lowercase();
    ["nvidia", "amd ", "radeon", "intel(r)", "intel "].iter().any(|v| d.contains(v)) || d.starts_with("amd")
}

#[cfg(test)]
mod tests {
    use super::is_gpu_vendor;

    #[test]
    fn gpu_vendors() {
        assert!(is_gpu_vendor("NVIDIA GeForce RTX 3060"));
        assert!(is_gpu_vendor("AMD Radeon(TM) Graphics"));
        assert!(is_gpu_vendor("Intel(R) UHD Graphics 770"));
        assert!(!is_gpu_vendor("Microsoft Basic Display Adapter"));
        assert!(!is_gpu_vendor("Microsoft Remote Display Adapter"));
        assert!(!is_gpu_vendor("Microsoft Hyper-V Video"));
    }
}
