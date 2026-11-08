//! Display adapters from DXGI (name, memory sizes, LUID), to label the per-GPU pages.
//! windows-sys has no DXGI bindings, so the two COM calls go through the vtable by hand.

use std::ffi::c_void;
use std::mem::size_of;
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

/// Device Manager's "Location" of the display device with this vendor and device id.
fn location(vendor: u32, device: u32) -> String {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        DIGCF_PRESENT, GUID_DEVCLASS_DISPLAY, SP_DEVINFO_DATA, SPDRP_HARDWAREID, SPDRP_LOCATION_INFORMATION,
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDeviceRegistryPropertyW,
    };
    let id = format!("ven_{vendor:04x}&dev_{device:04x}");
    // SAFETY: device list destroyed below; buffers sized as passed.
    unsafe {
        let set = SetupDiGetClassDevsW(&GUID_DEVCLASS_DISPLAY, std::ptr::null(), null_mut(), DIGCF_PRESENT);
        if set == -1 {
            return String::new();
        }
        let prop = |d: &SP_DEVINFO_DATA, p: u32| {
            let mut buf = [0u16; 512];
            let ok = SetupDiGetDeviceRegistryPropertyW(
                set,
                d,
                p,
                null_mut(),
                buf.as_mut_ptr().cast(),
                (buf.len() * 2) as u32,
                null_mut(),
            );
            if ok == 0 {
                return String::new();
            }
            // REG_MULTI_SZ for hardware ids: the first string is enough.
            let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
            String::from_utf16_lossy(&buf[..n])
        };
        let mut out = String::new();
        for i in 0.. {
            let mut d = SP_DEVINFO_DATA { cbSize: size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            if SetupDiEnumDeviceInfo(set, i, &mut d) == 0 {
                break;
            }
            if prop(&d, SPDRP_HARDWAREID).to_lowercase().contains(&id) {
                out = prop(&d, SPDRP_LOCATION_INFORMATION);
                break;
            }
        }
        SetupDiDestroyDeviceInfoList(set);
        out
    }
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
                    location: location(d.vendor_id, d.device_id),
                    npu: false,
                });
            }
            let release: Release = slot(adapter, 2);
            release(adapter);
        }
        let release: Release = slot(factory, 2);
        release(factory);
    }
    // NPUs are not display adapters, so DXGI leaves them out; DXCore lists them.
    for n in npus() {
        if !out.iter().any(|a| a.luid == n.luid) {
            out.push(n);
        }
    }
    out
}

const IID_DXCORE_FACTORY: GUID = GUID::from_u128(0x78ee5945_c36e_4b13_a669_005dd11c0f06);
const IID_DXCORE_LIST: GUID = GUID::from_u128(0x526c7776_40e9_459b_b711_f32ad76dfc28);
const IID_DXCORE_ADAPTER: GUID = GUID::from_u128(0xf0db4c7f_fe5a_42a2_bd62_f2a6cf6fc83e);
const DXCORE_GENERIC_ML: GUID = GUID::from_u128(0xb71b0d41_1088_422f_a27c_0250b7d3a988);
const DXCORE_D3D12_GRAPHICS: GUID = GUID::from_u128(0x0c9ece4d_2f6e_4f01_8c96_e89e331b47b1);
// DXCoreAdapterProperty values.
const PROP_LUID: u32 = 0;
const PROP_DRIVER_VERSION: u32 = 1;
const PROP_DESCRIPTION: u32 = 2;
const PROP_DEDICATED: u32 = 7;
const PROP_SHARED: u32 = 9;

type CreateFactory = unsafe extern "system" fn(*const GUID, *mut *mut c_void) -> i32;
type CreateList = unsafe extern "system" fn(*mut c_void, u32, *const GUID, *const GUID, *mut *mut c_void) -> i32;
type GetAdapter = unsafe extern "system" fn(*mut c_void, u32, *const GUID, *mut *mut c_void) -> i32;
type GetCount = unsafe extern "system" fn(*mut c_void) -> u32;
type IsAttr = unsafe extern "system" fn(*mut c_void, *const GUID) -> u8;
type GetProp = unsafe extern "system" fn(*mut c_void, u32, usize, *mut c_void) -> i32;
type GetPropSize = unsafe extern "system" fn(*mut c_void, u32, *mut usize) -> i32;

/// Machine-learning adapters without graphics: the NPUs. dxcore.dll is loaded on demand, so
/// Windows versions without it just have no NPU entries.
fn npus() -> Vec<GpuAdapter> {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    let mut out = Vec::new();
    let dll: Vec<u16> = "dxcore.dll".encode_utf16().chain(Some(0)).collect();
    // SAFETY: the vtable slots follow dxcore_interface.h; every object obtained is released.
    unsafe {
        let lib = LoadLibraryW(dll.as_ptr());
        if lib.is_null() {
            return out;
        }
        let Some(create) = GetProcAddress(lib, c"DXCoreCreateAdapterFactory".as_ptr().cast()) else {
            return out;
        };
        let create: CreateFactory = std::mem::transmute(create);
        let mut factory = null_mut();
        if create(&IID_DXCORE_FACTORY, &mut factory) < 0 || factory.is_null() {
            return out;
        }
        let mut list = null_mut();
        let create_list: CreateList = slot(factory, 3);
        if create_list(factory, 1, &DXCORE_GENERIC_ML, &IID_DXCORE_LIST, &mut list) >= 0 && !list.is_null() {
            let count: GetCount = slot(list, 4);
            let get: GetAdapter = slot(list, 3);
            for i in 0..count(list) {
                let mut a = null_mut();
                if get(list, i, &IID_DXCORE_ADAPTER, &mut a) < 0 || a.is_null() {
                    continue;
                }
                let is_attr: IsAttr = slot(a, 4);
                if is_attr(a, &DXCORE_D3D12_GRAPHICS) == 0 {
                    out.push(npu_info(a));
                }
                let release: Release = slot(a, 2);
                release(a);
            }
            let release: Release = slot(list, 2);
            release(list);
        }
        let release: Release = slot(factory, 2);
        release(factory);
    }
    out
}

/// # Safety
/// `a` is a live IDXCoreAdapter.
unsafe fn npu_info(a: *mut c_void) -> GpuAdapter {
    // SAFETY: GetProperty / GetPropertySize are slots 6 and 7; buffers sized as passed.
    unsafe {
        let get: GetProp = slot(a, 6);
        let size: GetPropSize = slot(a, 7);
        let u64_prop = |p| {
            let mut v = 0u64;
            if get(a, p, 8, (&raw mut v).cast()) < 0 { 0 } else { v }
        };
        let mut luid = [0u32; 2];
        get(a, PROP_LUID, 8, luid.as_mut_ptr().cast());
        let mut n = 0usize;
        let mut name = String::new();
        if size(a, PROP_DESCRIPTION, &mut n) >= 0 && n > 0 && n < 4096 {
            let mut buf = vec![0u8; n];
            if get(a, PROP_DESCRIPTION, n, buf.as_mut_ptr().cast()) >= 0 {
                name = String::from_utf8_lossy(&buf).trim_end_matches('\0').to_string();
            }
        }
        let v = u64_prop(PROP_DRIVER_VERSION);
        GpuAdapter {
            luid: format!("0x{:08X}_0x{:08X}", luid[1], luid[0]),
            name,
            dedicated_total: u64_prop(PROP_DEDICATED),
            shared_total: u64_prop(PROP_SHARED),
            driver_version: if v == 0 {
                String::new()
            } else {
                format!("{}.{}.{}.{}", v >> 48, (v >> 32) & 0xffff, (v >> 16) & 0xffff, v & 0xffff)
            },
            npu: true,
            ..Default::default()
        }
    }
}

#[repr(C)]
struct OpenAdapterFromLuid {
    luid_low: u32,
    luid_high: i32,
    adapter: u32,
}

#[repr(C)]
struct QueryAdapterInfo {
    adapter: u32,
    kind: u32,
    data: *mut c_void,
    size: u32,
}

/// D3DKMT_ADAPTER_PERFDATA (Windows 10 2004+).
#[repr(C)]
#[derive(Default)]
struct AdapterPerfData {
    physical_index: u32,
    memory_frequency: u64,
    max_memory_frequency: u64,
    max_memory_frequency_oc: u64,
    memory_bandwidth: u64,
    pcie_bandwidth: u64,
    fan_rpm: u32,
    power: u32,
    /// Deci-degrees Celsius.
    temperature: u32,
    power_state_override: u8,
}

const KMTQAITYPE_ADAPTERPERFDATA: u32 = 62;

type KmtOpen = unsafe extern "system" fn(*mut OpenAdapterFromLuid) -> i32;
type KmtQuery = unsafe extern "system" fn(*const QueryAdapterInfo) -> i32;
type KmtClose = unsafe extern "system" fn(*const u32) -> i32;

/// GPU temperature in °C, as Task Manager shows it, for the adapter with this PDH-style LUID
/// ("0x00000000_0x0000D5E1"). None when the driver does not report one (WARP, most VMs).
pub fn gpu_temperature(luid: &str) -> Option<f32> {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    let (hi, lo) = luid.split_once('_')?;
    let hex = |s: &str| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok();
    let (hi, lo) = (hex(hi)?, hex(lo)?);
    let dll: Vec<u16> = "gdi32.dll".encode_utf16().chain(Some(0)).collect();
    // SAFETY: documented D3DKMT thunks with their C layouts; the adapter handle is closed below.
    unsafe {
        let lib = LoadLibraryW(dll.as_ptr());
        if lib.is_null() {
            return None;
        }
        let open: KmtOpen = std::mem::transmute(GetProcAddress(lib, c"D3DKMTOpenAdapterFromLuid".as_ptr().cast())?);
        let query: KmtQuery = std::mem::transmute(GetProcAddress(lib, c"D3DKMTQueryAdapterInfo".as_ptr().cast())?);
        let close: KmtClose = std::mem::transmute(GetProcAddress(lib, c"D3DKMTCloseAdapter".as_ptr().cast())?);
        let mut o = OpenAdapterFromLuid { luid_low: lo, luid_high: hi as i32, adapter: 0 };
        if open(&mut o) < 0 {
            return None;
        }
        let mut perf = AdapterPerfData::default();
        let q = QueryAdapterInfo {
            adapter: o.adapter,
            kind: KMTQAITYPE_ADAPTERPERFDATA,
            data: (&raw mut perf).cast(),
            size: size_of::<AdapterPerfData>() as u32,
        };
        let r = query(&q);
        close(&o.adapter);
        (r >= 0 && perf.temperature > 0).then(|| perf.temperature as f32 / 10.0)
    }
}
