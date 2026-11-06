//! Windows reader: NT native API for processes and per-core CPU, Win32 for memory and network,
//! PDH for disk and GPU rates.

use std::ffi::c_void;
use std::io;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use tm_core::{CpuTimes, RawProcess, RawSample, RawSystem};
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
use windows_sys::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows_sys::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA,
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows_sys::Win32::System::SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{GetSystemTimes, OpenProcess, PROCESS_TERMINATE, TerminateProcess};

const SYSTEM_PROCESS_INFORMATION_CLASS: u32 = 5;
const SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION_CLASS: u32 = 8;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
/// Let PDH report sums above 100 % (multi-engine GPU); we clamp later.
const PDH_FMT_NOCAP100: u32 = 0x0000_8000;
const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
const IF_OPER_STATUS_UP: i32 = 1;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, buf: *mut c_void, len: u32, ret_len: *mut u32) -> i32;
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *const u16,
}

/// Full layout of `SYSTEM_PROCESS_INFORMATION` (the SDK header only documents part of it).
#[repr(C)]
struct SystemProcessInformation {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name: UnicodeString,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_page_count: usize,
    read_operation_count: i64,
    write_operation_count: i64,
    other_operation_count: i64,
    read_transfer_count: i64,
    write_transfer_count: i64,
    other_transfer_count: i64,
    // Followed by `number_of_threads` SYSTEM_THREAD_INFORMATION entries we do not read.
}

// Sizes documented for x64 Windows; a mismatch here would mean a wrong field offset.
#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<SystemProcessInformation>() == 0x100);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<ProcessorPerformance>() == 48);

#[repr(C)]
#[derive(Clone, Copy)]
struct ProcessorPerformance {
    idle_time: i64,
    /// Includes idle time.
    kernel_time: i64,
    user_time: i64,
    reserved1: [i64; 2],
    reserved2: u32,
}

/// Buffer for one NtQuerySystemInformation class, grown on demand and reused between calls.
struct NtBuffer {
    buf: Vec<u64>,
}

impl NtBuffer {
    fn new(bytes: usize) -> Self {
        Self { buf: vec![0; bytes / 8] }
    }

    fn query(&mut self, class: u32) -> io::Result<&[u8]> {
        loop {
            let len = (self.buf.len() * 8) as u32;
            let mut needed = 0u32;
            // SAFETY: the buffer is valid for `len` bytes and 8-byte aligned.
            let status =
                unsafe { NtQuerySystemInformation(class, self.buf.as_mut_ptr().cast(), len, &mut needed) };
            if status == STATUS_INFO_LENGTH_MISMATCH {
                // Processes appear between calls: leave headroom.
                let want = (needed as usize).max(len as usize * 2) + 64 * 1024;
                self.buf.resize(want.div_ceil(8), 0);
                continue;
            }
            if status < 0 {
                return Err(io::Error::other(format!("NtQuerySystemInformation({class}) = {status:#x}")));
            }
            // SAFETY: u64 buffer reinterpreted as bytes, same allocation.
            let bytes = unsafe { std::slice::from_raw_parts(self.buf.as_ptr().cast::<u8>(), self.buf.len() * 8) };
            let used = if needed == 0 { bytes.len() } else { (needed as usize).min(bytes.len()) };
            return Ok(&bytes[..used]);
        }
    }
}

struct Pdh {
    query: *mut c_void,
    disk_time: *mut c_void,
    disk_read: *mut c_void,
    disk_write: *mut c_void,
    gpu: *mut c_void,
    item_buf: Vec<u8>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

impl Pdh {
    fn open() -> Option<Self> {
        let mut query = null_mut();
        // SAFETY: out-pointer to a local.
        if unsafe { PdhOpenQueryW(null(), 0, &mut query) } != 0 {
            return None;
        }
        let add = |path: &str| -> *mut c_void {
            let mut counter = null_mut();
            let path = wide(path);
            // SAFETY: valid query handle, NUL-terminated path, out-pointer to a local.
            let ok = unsafe { PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) } == 0;
            if ok { counter } else { null_mut() }
        };
        let pdh = Self {
            query,
            disk_time: add(r"\PhysicalDisk(_Total)\% Disk Time"),
            disk_read: add(r"\PhysicalDisk(_Total)\Disk Read Bytes/sec"),
            disk_write: add(r"\PhysicalDisk(_Total)\Disk Write Bytes/sec"),
            gpu: add(r"\GPU Engine(*engtype_3D)\Utilization Percentage"),
            item_buf: Vec::new(),
        };
        // Rate counters need a first sample to diff against.
        // SAFETY: valid query handle.
        unsafe { PdhCollectQueryData(query) };
        Some(pdh)
    }

    fn value(counter: *mut c_void) -> Option<f64> {
        if counter.is_null() {
            return None;
        }
        // SAFETY: zeroed POD out-struct; valid counter handle.
        unsafe {
            let mut v: PDH_FMT_COUNTERVALUE = zeroed();
            let st = PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE | PDH_FMT_NOCAP100, null_mut(), &mut v);
            (st == 0 && v.CStatus == 0).then(|| v.Anonymous.doubleValue)
        }
    }

    /// Sum over every instance of a wildcard counter (all processes' 3D engine usage).
    fn sum(&mut self, counter: *mut c_void) -> Option<f64> {
        if counter.is_null() {
            return None;
        }
        let mut size = self.item_buf.len() as u32;
        let mut count = 0u32;
        loop {
            // SAFETY: buffer valid for `size` bytes (or null when 0) and aligned for the item type
            // because Vec<u8> from the global allocator is at least 8-byte aligned for these sizes.
            let st = unsafe {
                PdhGetFormattedCounterArrayW(
                    counter,
                    PDH_FMT_DOUBLE | PDH_FMT_NOCAP100,
                    &mut size,
                    &mut count,
                    if self.item_buf.is_empty() { null_mut() } else { self.item_buf.as_mut_ptr().cast() },
                )
            };
            if st == PDH_MORE_DATA as u32 {
                self.item_buf.resize(size as usize + 1024, 0);
                size = self.item_buf.len() as u32;
                continue;
            }
            if st != 0 {
                return None;
            }
            break;
        }
        let items = self.item_buf.as_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        let mut total = 0.0;
        for i in 0..count as usize {
            // SAFETY: PDH wrote `count` items at the start of the buffer.
            let item = unsafe { &*items.add(i) };
            if item.FmtValue.CStatus == 0 {
                // SAFETY: requested PDH_FMT_DOUBLE.
                total += unsafe { item.FmtValue.Anonymous.doubleValue };
            }
        }
        Some(total)
    }
}

impl Drop for Pdh {
    fn drop(&mut self) {
        // SAFETY: handle from PdhOpenQueryW.
        unsafe { PdhCloseQuery(self.query) };
    }
}

pub struct Collector {
    procs: NtBuffer,
    cores: NtBuffer,
    pdh: Option<Pdh>,
}

// The PDH handles are only ever used from the thread that owns the collector.
unsafe impl Send for Collector {}

fn filetime(ft: FILETIME) -> u64 {
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}

impl Collector {
    pub fn new() -> io::Result<Self> {
        Ok(Self { procs: NtBuffer::new(512 * 1024), cores: NtBuffer::new(4096), pdh: Pdh::open() })
    }

    pub fn collect(&mut self) -> io::Result<RawSample> {
        let at = Instant::now();
        let processes = self.read_processes()?;
        let system = self.read_system()?;
        Ok(RawSample { at, processes, system })
    }

    fn read_processes(&mut self) -> io::Result<Vec<RawProcess>> {
        let bytes = self.procs.query(SYSTEM_PROCESS_INFORMATION_CLASS)?;
        let mut out = Vec::with_capacity(400);
        let mut offset = 0usize;
        loop {
            if offset + size_of::<SystemProcessInformation>() > bytes.len() {
                break;
            }
            // SAFETY: the kernel lays out 8-byte aligned entries; bounds checked above.
            let p = unsafe { &*bytes.as_ptr().add(offset).cast::<SystemProcessInformation>() };
            let pid = p.unique_process_id as u32;
            if pid != 0 {
                let name = if p.image_name.buffer.is_null() {
                    if pid == 4 { "System".to_string() } else { format!("PID {pid}") }
                } else {
                    // SAFETY: buffer points into the same query buffer, `length` bytes long.
                    let s = unsafe {
                        std::slice::from_raw_parts(p.image_name.buffer, usize::from(p.image_name.length) / 2)
                    };
                    String::from_utf16_lossy(s)
                };
                out.push(RawProcess {
                    pid,
                    parent_pid: p.inherited_from_unique_process_id as u32,
                    start_time: p.create_time as u64,
                    name,
                    cpu_time: (p.user_time + p.kernel_time) as u64,
                    memory: p.working_set_private_size as u64,
                    io_read_bytes: p.read_transfer_count as u64,
                    io_write_bytes: p.write_transfer_count as u64,
                    threads: p.number_of_threads,
                    handles: p.handle_count,
                });
            }
            if p.next_entry_offset == 0 {
                break;
            }
            offset += p.next_entry_offset as usize;
        }
        Ok(out)
    }

    fn read_system(&mut self) -> io::Result<RawSystem> {
        let mut sys = RawSystem::default();

        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        // SAFETY: out-pointers to locals.
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } != 0 {
            sys.cpu = CpuTimes { idle: filetime(idle), total: filetime(kernel) + filetime(user) };
        }

        let bytes = self.cores.query(SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION_CLASS)?;
        let n = bytes.len() / size_of::<ProcessorPerformance>();
        let ptr = bytes.as_ptr().cast::<ProcessorPerformance>();
        for i in 0..n {
            // SAFETY: `n` whole entries fit in the returned length.
            let c = unsafe { *ptr.add(i) };
            sys.cores.push(CpuTimes { idle: c.idle_time as u64, total: (c.kernel_time + c.user_time) as u64 });
        }

        // SAFETY: POD out-struct with its length set, as the API requires.
        unsafe {
            let mut m: MEMORYSTATUSEX = zeroed();
            m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
            if GlobalMemoryStatusEx(&mut m) != 0 {
                sys.mem_total = m.ullTotalPhys;
                sys.mem_available = m.ullAvailPhys;
                sys.commit_limit = m.ullTotalPageFile;
                sys.commit_used = m.ullTotalPageFile.saturating_sub(m.ullAvailPageFile);
            }
            sys.uptime = Duration::from_millis(GetTickCount64());
        }

        // SAFETY: GetIfTable2 allocates the table; we free it with FreeMibTable.
        unsafe {
            let mut table: *mut MIB_IF_TABLE2 = null_mut();
            if GetIfTable2(&mut table) == 0 && !table.is_null() {
                let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
                for r in rows {
                    let hardware = r.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
                    if hardware && r.Type != IF_TYPE_SOFTWARE_LOOPBACK && r.OperStatus == IF_OPER_STATUS_UP {
                        sys.net_rx_bytes += r.InOctets;
                        sys.net_tx_bytes += r.OutOctets;
                    }
                }
                FreeMibTable(table.cast());
            }
        }

        if let Some(pdh) = self.pdh.as_mut() {
            // SAFETY: valid query handle.
            if unsafe { PdhCollectQueryData(pdh.query) } == 0 {
                sys.disk_active_pct = Pdh::value(pdh.disk_time).map(|v| v as f32);
                sys.disk_read_bps = Pdh::value(pdh.disk_read);
                sys.disk_write_bps = Pdh::value(pdh.disk_write);
                let gpu = pdh.gpu;
                sys.gpu_pct = pdh.sum(gpu).map(|v| v as f32);
            }
        }
        Ok(sys)
    }
}

pub fn end_process(pid: u32) -> io::Result<()> {
    // SAFETY: plain Win32 calls; handle closed on every path.
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if h.is_null() {
            return Err(io::Error::last_os_error());
        }
        let ok = TerminateProcess(h, 1) != 0;
        let err = io::Error::last_os_error();
        CloseHandle(h);
        if ok { Ok(()) } else { Err(err) }
    }
}
