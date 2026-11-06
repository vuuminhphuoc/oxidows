//! Static hardware facts for the Performance tab: processor name, clocks, topology, caches.

use std::ptr::null_mut;

use taskmgr_core::CpuInfo;
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};
use windows_sys::Win32::System::SystemInformation::{GetLogicalProcessorInformationEx, RelationAll};
use windows_sys::Win32::System::Threading::{GetActiveProcessorCount, IsProcessorFeaturePresent};

const CPU_KEY: &str = r"HARDWARE\DESCRIPTION\System\CentralProcessor\0";
const PF_VIRT_FIRMWARE_ENABLED: u32 = 21;
const ALL_PROCESSOR_GROUPS: u16 = 0xffff;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn reg_string(key: &str, value: &str) -> Option<String> {
    let (k, v) = (wide(key), wide(value));
    let mut buf = vec![0u16; 512];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: buffer of `len` bytes.
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            k.as_ptr(),
            v.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    (r == 0).then(|| String::from_utf16_lossy(&buf[..(len as usize / 2)]).trim_end_matches('\0').trim().to_string())
}

pub fn reg_dword(key: &str, value: &str) -> Option<u32> {
    let (k, v) = (wide(key), wide(value));
    let mut d = 0u32;
    let mut len = 4u32;
    // SAFETY: 4-byte out value.
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            k.as_ptr(),
            v.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut d as *mut u32).cast(),
            &mut len,
        )
    };
    (r == 0).then_some(d)
}

pub fn base_mhz() -> u32 {
    reg_dword(CPU_KEY, "~MHz").unwrap_or(0)
}

pub fn cpu_info() -> CpuInfo {
    let mut info = CpuInfo {
        name: reg_string(CPU_KEY, "ProcessorNameString").unwrap_or_default(),
        base_mhz: base_mhz(),
        // SAFETY: plain calls.
        logical: unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) },
        virtualization: Some(unsafe { IsProcessorFeaturePresent(PF_VIRT_FIRMWARE_ENABLED) } != 0),
        ..Default::default()
    };
    let mut len = 0u32;
    // SAFETY: size query, then a u64 buffer (8-byte aligned) of at least that size.
    unsafe {
        GetLogicalProcessorInformationEx(RelationAll, null_mut(), &mut len);
        if len == 0 {
            return info;
        }
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        if GetLogicalProcessorInformationEx(RelationAll, buf.as_mut_ptr().cast(), &mut len) == 0 {
            return info;
        }
        let bytes = std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), len as usize);
        // Records: Relationship u32, Size u32, then the relationship body.
        let mut off = 0usize;
        while off + 8 <= bytes.len() {
            let rel = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap_or_default());
            let size = u32::from_le_bytes(bytes[off + 4..off + 8].try_into().unwrap_or_default()) as usize;
            if size == 0 || off + size > bytes.len() {
                break;
            }
            let body = &bytes[off + 8..off + size];
            match rel {
                0 => info.cores += 1,   // RelationProcessorCore
                3 => info.sockets += 1, // RelationProcessorPackage
                2 if body.len() >= 8 => {
                    // CACHE_RELATIONSHIP: Level u8, Associativity u8, LineSize u16, CacheSize u32, Type u32.
                    let level = body[0];
                    let cache = u64::from(u32::from_le_bytes(body[4..8].try_into().unwrap_or_default()));
                    let kind = if body.len() >= 12 {
                        u32::from_le_bytes(body[8..12].try_into().unwrap_or_default())
                    } else {
                        0
                    };
                    match level {
                        // Task Manager's L1 adds instruction and data caches.
                        1 => info.l1_bytes += cache,
                        2 if kind != 1 => info.l2_bytes += cache,
                        3 => info.l3_bytes += cache,
                        _ => {}
                    }
                }
                _ => {}
            }
            off += size;
        }
    }
    info
}
