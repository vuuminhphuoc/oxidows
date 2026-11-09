//! Windows reader: NT native API for processes and per-core CPU, Win32 for memory and network,
//! PDH for disk and GPU rates.

mod adapters;
mod gpu;
mod hw;
mod net;
mod procinfo;
mod services;
mod sessions;
mod srum;
mod startup;
pub mod tray;
mod waitchain;

pub use adapters::adapter_info;
pub use gpu::gpu_temperature;
pub use hw::{cpu_info, disk_info, disk_temperature, memory_info};
pub use waitchain::{set_uac_virtualization, uac_virtualization, wait_chain};

pub fn app_history() -> io::Result<taskmgr_core::AppHistory> {
    srum::app_history(f64::from(hw::base_mhz()) * 1e6)
}
pub use procinfo::{
    affinity, create_dump, efficiency_mode, file_properties, is_elevated, is_running, open_url, restart_elevated,
    run_new_task, run_self_elevated, set_affinity, set_efficiency_mode, set_priority, static_info, switch_to,
};
pub use services::{list_services, service_control};
pub use sessions::{disconnect_session, list_sessions, logoff_session};

/// Firmware ("BIOS") time of the last boot in milliseconds, as Startup apps shows it.
pub fn last_bios_ms() -> Option<u32> {
    hw::reg_dword(r"SYSTEM\CurrentControlSet\Control\Session Manager\Power", "FwPOSTTime").filter(|v| *v > 0)
}

pub fn exe_publisher(path: &str) -> String {
    procinfo::version_strings(path).1
}
pub use startup::{
    exe_of as startup_exe, set_startup_enabled, set_taskmgr_replacement, startup_items, taskmgr_replacement,
};

use std::ffi::c_void;
use std::io;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use taskmgr_core::{CpuTimes, RawProcess, RawSample, RawSystem};
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
use windows_sys::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows_sys::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA, PdhAddEnglishCounterW,
    PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows_sys::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
use windows_sys::Win32::System::SystemInformation::{
    GetPhysicallyInstalledSystemMemory, GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX,
};
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
            let status = unsafe { NtQuerySystemInformation(class, self.buf.as_mut_ptr().cast(), len, &mut needed) };
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
    gpu_dedicated: *mut c_void,
    gpu_shared: *mut c_void,
    cpu_perf: *mut c_void,
    disks_time: *mut c_void,
    disks_read: *mut c_void,
    disks_write: *mut c_void,
    disks_resp: *mut c_void,
    item_buf: Vec<u64>,
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
            gpu: add(r"\GPU Engine(*)\Utilization Percentage"),
            gpu_dedicated: add(r"\GPU Adapter Memory(*)\Dedicated Usage"),
            gpu_shared: add(r"\GPU Adapter Memory(*)\Shared Usage"),
            cpu_perf: add(r"\Processor Information(_Total)\% Processor Performance"),
            disks_time: add(r"\PhysicalDisk(*)\% Disk Time"),
            disks_read: add(r"\PhysicalDisk(*)\Disk Read Bytes/sec"),
            disks_write: add(r"\PhysicalDisk(*)\Disk Write Bytes/sec"),
            disks_resp: add(r"\PhysicalDisk(*)\Avg. Disk sec/Transfer"),
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
            (st == 0 && v.CStatus == 0).then_some(v.Anonymous.doubleValue)
        }
    }

    /// Every instance of a wildcard counter as (instance name, value).
    fn items(&mut self, counter: *mut c_void) -> Option<Vec<(String, f64)>> {
        if counter.is_null() {
            return None;
        }
        let mut size = (self.item_buf.len() * 8) as u32;
        let mut count = 0u32;
        loop {
            // SAFETY: u64 buffer (8-byte aligned, as the item struct needs) valid for `size` bytes.
            let st = unsafe {
                PdhGetFormattedCounterArrayW(
                    counter,
                    PDH_FMT_DOUBLE | PDH_FMT_NOCAP100,
                    &mut size,
                    &mut count,
                    if self.item_buf.is_empty() { null_mut() } else { self.item_buf.as_mut_ptr().cast() },
                )
            };
            if st == PDH_MORE_DATA {
                self.item_buf.resize((size as usize).div_ceil(8) + 128, 0);
                size = (self.item_buf.len() * 8) as u32;
                continue;
            }
            if st != 0 {
                return None;
            }
            break;
        }
        let items = self.item_buf.as_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            // SAFETY: PDH wrote `count` items at the start of the buffer; names point inside it.
            unsafe {
                let item = &*items.add(i);
                if item.FmtValue.CStatus != 0 || item.szName.is_null() {
                    continue;
                }
                let mut n = 0;
                while *item.szName.add(n) != 0 {
                    n += 1;
                }
                let name = String::from_utf16_lossy(std::slice::from_raw_parts(item.szName, n));
                out.push((name, item.FmtValue.Anonymous.doubleValue));
            }
        }
        Some(out)
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
    base_mhz: u32,
    /// The ETW network session is running.
    net: bool,
    gpus: Vec<taskmgr_core::GpuAdapter>,
}

// The PDH handles are only ever used from the thread that owns the collector.
unsafe impl Send for Collector {}

fn filetime(ft: FILETIME) -> u64 {
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}

/// SYSTEM_THREAD_INFORMATION (x64): 0x50 bytes; ThreadState at 0x44, WaitReason at 0x48.
const THREAD_INFO_SIZE: usize = 0x50;
const THREAD_STATE_WAITING: u32 = 5;
const WAIT_REASON_SUSPENDED: u32 = 5;

/// A process is "Suspended" when every one of its threads waits because it was suspended.
fn all_threads_suspended(bytes: &[u8], offset: usize, p: &SystemProcessInformation) -> bool {
    let n = p.number_of_threads as usize;
    let start = offset + size_of::<SystemProcessInformation>();
    if n == 0 || start + n * THREAD_INFO_SIZE > bytes.len() {
        return false;
    }
    let u32_at = |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
    (0..n).all(|i| {
        let t = start + i * THREAD_INFO_SIZE;
        u32_at(t + 0x44) == THREAD_STATE_WAITING && u32_at(t + 0x48) == WAIT_REASON_SUSPENDED
    })
}

/// Stops background tracing started by the collector.
pub fn shutdown() {
    net::stop();
    tray::remove();
}

impl Collector {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            procs: NtBuffer::new(512 * 1024),
            cores: NtBuffer::new(4096),
            pdh: Pdh::open(),
            base_mhz: hw::base_mhz(),
            // OXI_TASKMGR_NO_NET=1 turns per-process network off (diagnostics).
            net: std::env::var_os("OXI_TASKMGR_NO_NET").is_none() && procinfo::is_elevated() && net::start(),
            gpus: gpu::gpu_adapters(),
        })
    }

    pub fn collect(&mut self) -> io::Result<RawSample> {
        let at = Instant::now();
        let mut processes = self.read_processes()?;
        let windows = procinfo::window_pids();
        for p in &mut processes {
            p.has_window = windows.contains(&p.pid);
            p.efficiency = procinfo::efficiency_mode(p.pid).unwrap_or(false);
        }
        if self.net {
            net::with_totals(|t| {
                for p in &mut processes {
                    p.net_bytes = t.get(&p.pid).copied().unwrap_or(0);
                }
            });
        }
        let mut system = self.read_system()?;
        system.per_process_net = self.net;
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
                    net_bytes: 0,
                    suspended: all_threads_suspended(bytes, offset, p),
                    efficiency: false,
                    counters: taskmgr_core::ProcCounters {
                        working_set: p.working_set_size as u64,
                        peak_working_set: p.peak_working_set_size as u64,
                        commit: p.pagefile_usage as u64,
                        paged_pool: p.quota_paged_pool_usage as u64,
                        nonpaged_pool: p.quota_non_paged_pool_usage as u64,
                        page_faults: p.page_fault_count,
                        hard_faults: p.hard_fault_count,
                        read_ops: p.read_operation_count as u64,
                        write_ops: p.write_operation_count as u64,
                        other_ops: p.other_operation_count as u64,
                        other_bytes: p.other_transfer_count as u64,
                    },
                    threads: p.number_of_threads,
                    handles: p.handle_count,
                    session_id: p.session_id,
                    has_window: false,
                    base_priority: p.base_priority,
                    info: Default::default(),
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
            // The kernel figure includes idle time.
            let (i, k) = (filetime(idle), filetime(kernel));
            sys.cpu = CpuTimes { idle: i, total: k + filetime(user), kernel: k.saturating_sub(i) };
        }

        let bytes = self.cores.query(SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION_CLASS)?;
        let n = bytes.len() / size_of::<ProcessorPerformance>();
        let ptr = bytes.as_ptr().cast::<ProcessorPerformance>();
        for i in 0..n {
            // SAFETY: `n` whole entries fit in the returned length.
            let c = unsafe { *ptr.add(i) };
            sys.cores.push(CpuTimes {
                idle: c.idle_time as u64,
                total: (c.kernel_time + c.user_time) as u64,
                kernel: (c.kernel_time - c.idle_time).max(0) as u64,
            });
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

            let mut pi: PERFORMANCE_INFORMATION = zeroed();
            pi.cb = size_of::<PERFORMANCE_INFORMATION>() as u32;
            if GetPerformanceInfo(&mut pi, pi.cb) != 0 {
                let page = pi.PageSize as u64;
                sys.mem_cached = Some(pi.SystemCache as u64 * page);
                sys.mem_paged_pool = Some(pi.KernelPaged as u64 * page);
                sys.mem_nonpaged_pool = Some(pi.KernelNonpaged as u64 * page);
            }
            let mut installed_kb = 0u64;
            if GetPhysicallyInstalledSystemMemory(&mut installed_kb) != 0 {
                sys.mem_hw_reserved = Some((installed_kb * 1024).saturating_sub(sys.mem_total));
            }
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
                        let text = |w: &[u16]| {
                            let n = w.iter().position(|c| *c == 0).unwrap_or(w.len());
                            String::from_utf16_lossy(&w[..n])
                        };
                        sys.adapters.push(taskmgr_core::RawAdapter {
                            name: text(&r.Alias),
                            description: text(&r.Description),
                            kind: match r.Type {
                                6 => "Ethernet",
                                71 => "Wi-Fi",
                                243 | 244 => "Cellular",
                                _ => "Network",
                            }
                            .into(),
                            rx_bytes: r.InOctets,
                            tx_bytes: r.OutOctets,
                            link_bps: r.ReceiveLinkSpeed.max(r.TransmitLinkSpeed),
                        });
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
                let (gpu, ded, shared) = (pdh.gpu, pdh.gpu_dedicated, pdh.gpu_shared);
                let ded_items = pdh.items(ded);
                let shared_items = pdh.items(shared);
                sys.gpu_dedicated_used = ded_items.as_ref().map(|v| v.iter().map(|(_, b)| *b as u64).sum());
                sys.gpu_shared_used = shared_items.as_ref().map(|v| v.iter().map(|(_, b)| *b as u64).sum());
                if let Some(engines) = pdh.items(gpu) {
                    sys.gpus = taskmgr_core::split_gpus(
                        &self.gpus,
                        &engines,
                        ded_items.as_deref().unwrap_or_default(),
                        shared_items.as_deref().unwrap_or_default(),
                    );
                    let taskmgr_core::GpuUsage { total, per_pid, engines: by_engine, engine_per_pid } =
                        taskmgr_core::gpu_from_engines(&engines);
                    sys.gpu_engine_per_pid = engine_per_pid;
                    sys.gpu_pct = Some(total);
                    sys.gpu_per_pid = per_pid;
                    sys.gpu_engines = by_engine;
                }
                if self.base_mhz > 0 {
                    sys.cpu_mhz = Pdh::value(pdh.cpu_perf).map(|p| (p * f64::from(self.base_mhz) / 100.0) as f32);
                }
                let (t, r, w, resp) = (pdh.disks_time, pdh.disks_read, pdh.disks_write, pdh.disks_resp);
                if let Some(times) = pdh.items(t) {
                    let reads = pdh.items(r).unwrap_or_default();
                    let writes = pdh.items(w).unwrap_or_default();
                    let resps = pdh.items(resp).unwrap_or_default();
                    let find = |v: &[(String, f64)], n: &str| v.iter().find(|(k, _)| k == n).map_or(0.0, |(_, x)| *x);
                    let mut disks: Vec<taskmgr_core::DiskRates> = times
                        .iter()
                        .filter(|(n, _)| n != "_Total")
                        .map(|(n, pct)| taskmgr_core::DiskRates {
                            name: n.clone(),
                            active_pct: pct.clamp(0.0, 100.0) as f32,
                            read_bps: find(&reads, n),
                            write_bps: find(&writes, n),
                            response_ms: (find(&resps, n) * 1000.0) as f32,
                        })
                        .collect();
                    disks.sort_by(|a, b| a.name.cmp(&b.name));
                    sys.disks = disks;
                }
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
