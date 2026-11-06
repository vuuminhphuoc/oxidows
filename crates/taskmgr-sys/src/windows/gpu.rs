//! Display adapters from DXGI (name, memory sizes, LUID), to label the per-GPU pages.
//! windows-sys has no DXGI bindings, so the two COM calls go through the vtable by hand.

use std::ffi::c_void;
use std::ptr::null_mut;

use taskmgr_core::GpuAdapter;
use windows_sys::core::GUID;

#[link(name = "dxgi", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateDXGIFactory1(riid: *const GUID, factory: *mut *mut c_void) -> i32;
}

#[link(name = "d3d12", kind = "raw-dylib")]
unsafe extern "system" {
    fn D3D12CreateDevice(adapter: *mut c_void, min_level: i32, riid: *const GUID, device: *mut *mut c_void) -> i32;
}

const IID_IDXGI_FACTORY1: GUID = GUID::from_u128(0x770aae78_f26f_4dba_a829_253c83d1b387);
const IID_IDXGI_DEVICE: GUID = GUID::from_u128(0x54ec77fa_1377_44e6_8c32_88fd5f44c84c);
const IID_ID3D12_DEVICE: GUID = GUID::from_u128(0x189819f1_1db6_4b57_be54_1821339b85f7);

#[repr(C)]
struct AdapterDesc1 {
    description: [u16; 128],
    vendor_id: u32,
    device_id: u32,
    sub_sys_id: u32,
    revision: u32,
    dedicated_video_memory: usize,
    dedicated_system_memory: usize,
    shared_system_memory: usize,
    luid_low: u32,
    luid_high: i32,
    flags: u32,
}

/// Calls vtable slot `slot` of COM object `obj`.
///
/// # Safety
/// `obj` is a live COM pointer whose slot `slot` has signature `F`.
unsafe fn slot<F: Copy>(obj: *mut c_void, slot: usize) -> F {
    // SAFETY: a COM object starts with a pointer to its vtable of function pointers.
    unsafe {
        let vtbl = *obj.cast::<*const usize>();
        std::mem::transmute_copy(&*vtbl.add(slot))
    }
}

type Release = unsafe extern "system" fn(*mut c_void) -> u32;
type EnumAdapters1 = unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32;
type GetDesc1 = unsafe extern "system" fn(*mut c_void, *mut AdapterDesc1) -> i32;
type CheckInterfaceSupport = unsafe extern "system" fn(*mut c_void, *const GUID, *mut i64) -> i32;

/// The user-mode driver version DXGI reports, as Device Manager shows it.
///
/// # Safety
/// `adapter` is a live IDXGIAdapter.
unsafe fn driver_version(adapter: *mut c_void) -> String {
    let mut v = 0i64;
    // SAFETY: slot 9 of IDXGIAdapter is CheckInterfaceSupport.
    let check: CheckInterfaceSupport = unsafe { slot(adapter, 9) };
    if unsafe { check(adapter, &IID_IDXGI_DEVICE, &mut v) } < 0 {
        return String::new();
    }
    let v = v as u64;
    format!("{}.{}.{}.{}", v >> 48, (v >> 32) & 0xffff, (v >> 16) & 0xffff, v & 0xffff)
}

/// Highest Direct3D 12 feature level, asked without creating a device (a null out pointer).
///
/// # Safety
/// `adapter` is a live IDXGIAdapter.
unsafe fn directx(adapter: *mut c_void) -> String {
    for (level, name) in [(0xc200, "12.2"), (0xc100, "12.1"), (0xc000, "12.0"), (0xb100, "11.1"), (0xb000, "11.0")] {
        // SAFETY: documented probe form: S_FALSE (1) means the level is supported.
        if unsafe { D3D12CreateDevice(adapter, level, &IID_ID3D12_DEVICE, null_mut()) } >= 0 {
            return format!("12 (FL {name})");
        }
    }
    String::new()
}

/// Driver date of the display device with this PCI vendor and device id, from its driver key.
fn driver_date(vendor: u32, device: u32) -> String {
    const CLASS: &str = r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";
    let id = format!("ven_{vendor:04x}&dev_{device:04x}");
    (0..32)
        .map(|i| format!(r"{CLASS}\{i:04}"))
        .find(|k| super::hw::reg_string(k, "MatchingDeviceId").is_some_and(|m| m.to_lowercase().contains(&id)))
        .and_then(|k| super::hw::reg_string(&k, "DriverDate"))
        .unwrap_or_default()
}

/// Hardware adapters, in DXGI order (the first is the one Windows calls GPU 0).
pub fn gpu_adapters() -> Vec<GpuAdapter> {
    let mut out = Vec::new();
    let mut factory = null_mut();
    // SAFETY: standard DXGI factory creation; every object obtained is released.
    unsafe {
        if CreateDXGIFactory1(&IID_IDXGI_FACTORY1, &mut factory) < 0 || factory.is_null() {
            return out;
        }
        let enum_adapters: EnumAdapters1 = slot(factory, 12);
        for i in 0.. {
            let mut adapter = null_mut();
            if enum_adapters(factory, i, &mut adapter) < 0 || adapter.is_null() {
                break; // DXGI_ERROR_NOT_FOUND ends the list
            }
            let get_desc: GetDesc1 = slot(adapter, 10);
            let mut d: AdapterDesc1 = std::mem::zeroed();
            // Software adapters stay in the list so a VM's "Basic Render Driver" still gets its name.
            if get_desc(adapter, &mut d) >= 0 {
                let n = d.description.iter().position(|c| *c == 0).unwrap_or(128);
                out.push(GpuAdapter {
                    luid: format!("0x{:08X}_0x{:08X}", d.luid_high as u32, d.luid_low),
                    name: String::from_utf16_lossy(&d.description[..n]),
                    dedicated_total: d.dedicated_video_memory as u64,
                    shared_total: d.shared_system_memory as u64,
                    driver_version: driver_version(adapter),
                    directx: directx(adapter),
                    driver_date: driver_date(d.vendor_id, d.device_id),
                });
            }
            let release: Release = slot(adapter, 2);
            release(adapter);
        }
        let release: Release = slot(factory, 2);
        release(factory);
    }
    out
}
