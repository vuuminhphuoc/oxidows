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

const IID_IDXGI_FACTORY1: GUID = GUID::from_u128(0x770aae78_f26f_4dba_a829_253c83d1b387);
const DXGI_ADAPTER_FLAG_SOFTWARE: u32 = 2;

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
            if get_desc(adapter, &mut d) >= 0 && d.flags & DXGI_ADAPTER_FLAG_SOFTWARE == 0 {
                let n = d.description.iter().position(|c| *c == 0).unwrap_or(128);
                out.push(GpuAdapter {
                    luid: format!("0x{:08X}_0x{:08X}", d.luid_high as u32, d.luid_low),
                    name: String::from_utf16_lossy(&d.description[..n]),
                    dedicated_total: d.dedicated_video_memory as u64,
                    shared_total: d.shared_system_memory as u64,
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
