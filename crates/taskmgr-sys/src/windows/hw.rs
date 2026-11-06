//! Static hardware facts for the Performance tab: processor name, clocks, topology, caches.

use std::ptr::null_mut;

use taskmgr_core::CpuInfo;
use windows_sys::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_NOEXPAND, RRF_RT_REG_DWORD, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW,
};
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
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND,
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

// ---- Disks: model, capacity, SSD / HDD, bus ----

use std::io;

use taskmgr_core::{DiskInfo, MemoryInfo};
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::SystemInformation::GetSystemFirmwareTable;

const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
const IOCTL_DISK_GET_LENGTH_INFO: u32 = 0x0007_405C;
const STORAGE_DEVICE_PROPERTY: u32 = 0;
const STORAGE_SEEK_PENALTY_PROPERTY: u32 = 7;

fn ioctl(h: *mut core::ffi::c_void, code: u32, input: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut got = 0u32;
    // SAFETY: buffers with their true lengths; synchronous handle.
    let ok = unsafe {
        DeviceIoControl(
            h,
            code,
            input.as_ptr().cast(),
            input.len() as u32,
            out.as_mut_ptr().cast(),
            out.len() as u32,
            &mut got,
            null_mut(),
        )
    };
    (ok != 0).then_some(got as usize)
}

fn query(property: u32) -> [u8; 12] {
    let mut q = [0u8; 12];
    q[..4].copy_from_slice(&property.to_le_bytes()); // QueryType 0 = PropertyStandardQuery
    q
}

/// Facts about physical disk `number` (as in PDH's "0 C:"). Opening the disk needs no rights.
pub fn disk_info(number: u32) -> io::Result<DiskInfo> {
    let path: Vec<u16> = format!(r"\\.\PhysicalDrive{number}").encode_utf16().chain(Some(0)).collect();
    // SAFETY: zero access rights: only metadata IOCTLs are sent.
    let h = unsafe {
        CreateFileW(path.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, null_mut(), OPEN_EXISTING, 0, null_mut())
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let mut info = DiskInfo::default();
    let mut buf = vec![0u8; 1024];
    if let Some(n) = ioctl(h, IOCTL_STORAGE_QUERY_PROPERTY, &query(STORAGE_DEVICE_PROPERTY), &mut buf) {
        let u32_at = |o: usize| buf.get(o..o + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])) as usize;
        let text = |off: usize| {
            if off == 0 || off >= n {
                return String::new();
            }
            let end = buf[off..n].iter().position(|c| *c == 0).map_or(n, |p| off + p);
            String::from_utf8_lossy(&buf[off..end]).trim().to_string()
        };
        let (vendor, product) = (text(u32_at(12)), text(u32_at(16)));
        info.model = if vendor.is_empty() { product } else { format!("{vendor} {product}") };
        info.bus = match u32_at(28) {
            7 => "USB",
            8 => "RAID",
            10 => "SAS",
            11 => "SATA",
            12 => "SD",
            17 => "NVMe",
            14 | 15 => "Virtual",
            _ => "",
        }
        .into();
    }
    let mut pen = [0u8; 12];
    if ioctl(h, IOCTL_STORAGE_QUERY_PROPERTY, &query(STORAGE_SEEK_PENALTY_PROPERTY), &mut pen).is_some_and(|n| n >= 9) {
        info.kind = if pen[8] == 0 { "SSD" } else { "HDD" }.into();
    }
    let mut len = [0u8; 8];
    if ioctl(h, IOCTL_DISK_GET_LENGTH_INFO, &[], &mut len) == Some(8) {
        info.capacity = u64::from_le_bytes(len);
    }
    // SAFETY: handle opened above.
    unsafe { CloseHandle(h) };
    Ok(info)
}

// ---- Memory modules from SMBIOS type 17 ----

pub fn memory_info() -> MemoryInfo {
    const RSMB: u32 = u32::from_be_bytes(*b"RSMB");
    let mut info = MemoryInfo::default();
    // SAFETY: size query, then a buffer of that size.
    let size = unsafe { GetSystemFirmwareTable(RSMB, 0, null_mut(), 0) };
    if size == 0 {
        return info;
    }
    let mut buf = vec![0u8; size as usize];
    // SAFETY: buffer of `size` bytes.
    if unsafe { GetSystemFirmwareTable(RSMB, 0, buf.as_mut_ptr(), size) } == 0 {
        return info;
    }
    // RawSMBIOSData: 4 bytes of version info, u32 length, then the structure table.
    let table = &buf[8.min(buf.len())..];
    let mut off = 0;
    let mut speeds = Vec::new();
    while off + 4 <= table.len() {
        let (kind, len) = (table[off], table[off + 1] as usize);
        if len < 4 || off + len > table.len() {
            break;
        }
        let s = &table[off..off + len];
        let u16_at = |o: usize| s.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
        if kind == 17 {
            info.slots_total += 1;
            let size = u16_at(0x0C).unwrap_or(0);
            if size != 0 && size != 0xFFFF {
                info.slots_used += 1;
                if info.form_factor.is_empty() {
                    info.form_factor = match s.get(0x0E) {
                        Some(0x09) => "DIMM",
                        Some(0x0D) => "SODIMM",
                        Some(0x0F) => "FB-DIMM",
                        Some(0x0B) => "Row of chips",
                        Some(_) => "Other",
                        None => "",
                    }
                    .into();
                }
                // Configured speed (SMBIOS 2.7+) when set, else the module's rated speed.
                let speed = u16_at(0x20).filter(|v| *v != 0).or(u16_at(0x15)).unwrap_or(0);
                if speed != 0 && speed != 0xFFFF {
                    speeds.push(u32::from(speed));
                }
            }
        }
        if kind == 127 {
            break; // end of table
        }
        // Skip the formatted part, then the string set ending in a double NUL.
        let mut p = off + len;
        while p + 1 < table.len() && !(table[p] == 0 && table[p + 1] == 0) {
            p += 1;
        }
        off = p + 2;
    }
    info.speed_mts = speeds.into_iter().min().unwrap_or(0);
    info
}
