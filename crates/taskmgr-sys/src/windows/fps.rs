//! Frames per second per app, from the present calls DirectX reports through ETW (the same events
//! PresentMon and the Xbox Game Bar counter use).
//!
//! Microsoft-Windows-DXGI covers Direct3D 10/11/12, and Vulkan / OpenGL games whose driver presents
//! through a DXGI swap chain (the default for windowed and borderless games on Windows 11);
//! Microsoft-Windows-D3D9 covers older games. Only the Present_Start events are delivered (an event
//! ID filter drops everything else before it is buffered), so a game at 300 FPS costs a few hundred
//! tiny callbacks a second. Like the network trace this needs administrator rights.

use std::collections::{HashMap, VecDeque};
use std::mem::size_of;
use std::ptr::null;
use std::sync::{Mutex, OnceLock};

use taskmgr_core::fps::{FpsApi, FpsStatus, RawFps, WINDOW_SECS};
use windows_sys::Win32::System::Diagnostics::Etw::{
    CONTROLTRACE_HANDLE, ControlTraceW, ENABLE_TRACE_PARAMETERS, ENABLE_TRACE_PARAMETERS_VERSION_2,
    EVENT_CONTROL_CODE_ENABLE_PROVIDER, EVENT_FILTER_DESCRIPTOR, EVENT_FILTER_TYPE_EVENT_ID,
    EVENT_HEADER_FLAG_32_BIT_HEADER, EVENT_RECORD, EVENT_TRACE_CONTROL_STOP, EVENT_TRACE_LOGFILEW,
    EVENT_TRACE_PROPERTIES, EVENT_TRACE_REAL_TIME_MODE, EnableTraceEx2, OpenTraceW, PROCESS_TRACE_MODE_EVENT_RECORD,
    PROCESS_TRACE_MODE_REAL_TIME, PROCESSTRACE_HANDLE, ProcessTrace, StartTraceW, WNODE_FLAG_TRACED_GUID,
};
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows_sys::core::GUID;

const SESSION: &str = "oxidows-taskmgr-fps";
/// Microsoft-Windows-DXGI {CA11C036-0102-4A2D-A6AD-F03CFED5D3C9}
const DXGI: GUID = GUID::from_u128(0xca11c036_0102_4a2d_a6ad_f03cfed5d3c9);
/// Microsoft-Windows-D3D9 {783ACA0A-790E-4D7F-8451-AA850511C6B9}
const D3D9: GUID = GUID::from_u128(0x783aca0a_790e_4d7f_8451_aa850511c6b9);
/// DXGI Present_Start and PresentMultiplaneOverlay_Start.
const DXGI_PRESENT: [u16; 2] = [42, 55];
/// D3D9 Present_Start.
const D3D9_PRESENT: [u16; 1] = [1];
/// Both providers put these events under "Events" (0x2) in their Analytic channel.
const KEYWORD_EVENTS: u64 = 0x8000_0000_0000_0002;
const TRACE_LEVEL_VERBOSE: u8 = 5;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_ACCESS_DENIED: u32 = 5;
const INVALID_PROCESSTRACE_HANDLE: u64 = u64::MAX;
/// FlushTimer is in milliseconds: events reach us within ~100 ms instead of once a second.
const EVENT_TRACE_USE_MS_FLUSH_TIMER: u32 = 0x10;
const FLUSH_MS: u32 = 100;
/// Figures are taken this far in the past so frames still in ETW's buffers are not missed.
const LAG_SECS: f64 = 0.25;
/// A very fast app keeps at most this many present times per swap chain.
const MAX_TIMES: usize = 20_000;

#[derive(Default)]
struct Chain {
    api: Option<FpsApi>,
    /// QPC ticks, oldest first.
    times: VecDeque<i64>,
}

#[derive(Default)]
struct Recorder {
    /// Keyed by (PID, swap chain pointer): a process with several windows has several chains.
    chains: HashMap<(u32, u64), Chain>,
}

fn recorder() -> &'static Mutex<Recorder> {
    static R: OnceLock<Mutex<Recorder>> = OnceLock::new();
    R.get_or_init(Default::default)
}

fn qpc_freq() -> f64 {
    static F: OnceLock<f64> = OnceLock::new();
    *F.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: out-pointer to a local.
        unsafe { QueryPerformanceFrequency(&mut f) };
        if f > 0 { f as f64 } else { 10_000_000.0 }
    })
}

fn qpc_now() -> i64 {
    let mut t = 0i64;
    // SAFETY: out-pointer to a local.
    unsafe { QueryPerformanceCounter(&mut t) };
    t
}

fn same(a: &GUID, b: &GUID) -> bool {
    (a.data1, a.data2, a.data3, a.data4) == (b.data1, b.data2, b.data3, b.data4)
}

unsafe extern "system" fn on_event(rec: *mut EVENT_RECORD) {
    // SAFETY: ETW passes a valid record for the duration of the call.
    let rec = unsafe { &*rec };
    let h = &rec.EventHeader;
    let id = h.EventDescriptor.Id;
    let api = if same(&h.ProviderId, &DXGI) && DXGI_PRESENT.contains(&id) {
        FpsApi::Dxgi
    } else if same(&h.ProviderId, &D3D9) && D3D9_PRESENT.contains(&id) {
        FpsApi::D3d9
    } else {
        return;
    };
    // Both payloads start with the swap chain pointer: 4 bytes from a 32-bit process, else 8.
    let wide = u32::from(h.Flags) & EVENT_HEADER_FLAG_32_BIT_HEADER == 0;
    let need = if wide { 8 } else { 4 };
    let chain = if rec.UserData.is_null() || usize::from(rec.UserDataLength) < need {
        0
    } else if wide {
        // SAFETY: at least 8 bytes of user data, read unaligned.
        unsafe { rec.UserData.cast::<u64>().read_unaligned() }
    } else {
        // SAFETY: at least 4 bytes of user data, read unaligned.
        u64::from(unsafe { rec.UserData.cast::<u32>().read_unaligned() })
    };
    let at = h.TimeStamp;
    let keep = (WINDOW_SECS * qpc_freq()) as i64;
    if let Ok(mut r) = recorder().lock() {
        let c = r.chains.entry((h.ProcessId, chain)).or_default();
        c.api = Some(api);
        c.times.push_back(at);
        while c.times.front().is_some_and(|t| *t < at - keep) || c.times.len() > MAX_TIMES {
            c.times.pop_front();
        }
    }
}

fn wide_str(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// EVENT_TRACE_PROPERTIES followed by room for the session name, as the API requires.
#[repr(C)]
struct Props {
    p: EVENT_TRACE_PROPERTIES,
    name: [u16; 64],
}

fn props() -> Box<Props> {
    // SAFETY: plain C data; all-zero is a valid starting point.
    let mut b: Box<Props> = Box::new(unsafe { std::mem::zeroed() });
    b.p.Wnode.BufferSize = size_of::<Props>() as u32;
    b.p.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
    b.p.Wnode.ClientContext = 1; // QPC timestamps
    b.p.LogFileMode = EVENT_TRACE_REAL_TIME_MODE | EVENT_TRACE_USE_MS_FLUSH_TIMER;
    b.p.FlushTimer = FLUSH_MS;
    b.p.LoggerNameOffset = size_of::<EVENT_TRACE_PROPERTIES>() as u32;
    b
}

/// Enables one provider on the session, delivering only the listed event ids.
fn enable(handle: CONTROLTRACE_HANDLE, provider: &GUID, ids: &[u16]) -> u32 {
    // EVENT_FILTER_EVENT_ID: FilterIn (BOOLEAN), Reserved, Count (u16), then the ids.
    let mut filter: Vec<u8> = vec![1, 0];
    filter.extend_from_slice(&(ids.len() as u16).to_le_bytes());
    for id in ids {
        filter.extend_from_slice(&id.to_le_bytes());
    }
    let mut desc = EVENT_FILTER_DESCRIPTOR {
        Ptr: filter.as_ptr() as u64,
        Size: filter.len() as u32,
        Type: EVENT_FILTER_TYPE_EVENT_ID,
    };
    let params = ENABLE_TRACE_PARAMETERS {
        Version: ENABLE_TRACE_PARAMETERS_VERSION_2,
        EnableFilterDesc: &mut desc,
        FilterDescCount: 1,
        ..Default::default()
    };
    // SAFETY: valid session handle, provider GUID and parameters; `filter` outlives the call.
    unsafe {
        EnableTraceEx2(
            handle,
            provider,
            EVENT_CONTROL_CODE_ENABLE_PROVIDER,
            TRACE_LEVEL_VERBOSE,
            KEYWORD_EVENTS,
            0,
            0,
            &params,
        )
    }
}

/// Starts the trace on its own thread (once). Callable from any thread.
pub fn start() -> FpsStatus {
    static STATUS: OnceLock<FpsStatus> = OnceLock::new();
    *STATUS.get_or_init(|| {
        if std::env::var_os("OXI_TASKMGR_NO_FPS").is_some() {
            return FpsStatus::Unavailable;
        }
        if !super::procinfo::is_elevated() {
            return FpsStatus::NeedsAdmin;
        }
        let name = wide_str(SESSION);
        let mut handle: CONTROLTRACE_HANDLE = Default::default();
        let mut pr = props();
        // SAFETY: name NUL-terminated; properties buffer sized in Wnode.BufferSize.
        let mut r = unsafe { StartTraceW(&mut handle, name.as_ptr(), &mut pr.p) };
        if r == ERROR_ALREADY_EXISTS {
            // A previous run crashed and left its session behind: stop it and start fresh.
            stop();
            pr = props();
            // SAFETY: as above.
            r = unsafe { StartTraceW(&mut handle, name.as_ptr(), &mut pr.p) };
        }
        if r != 0 {
            log(&format!("StartTrace failed: {r}"));
            return if r == ERROR_ACCESS_DENIED { FpsStatus::NeedsAdmin } else { FpsStatus::Unavailable };
        }
        let dxgi = enable(handle, &DXGI, &DXGI_PRESENT);
        let d3d9 = enable(handle, &D3D9, &D3D9_PRESENT);
        log(&format!("enable DXGI: {dxgi}, D3D9: {d3d9}"));
        if dxgi != 0 && d3d9 != 0 {
            stop();
            return FpsStatus::Unavailable;
        }
        let spawned = std::thread::Builder::new()
            .name("etw-fps".into())
            .spawn(move || {
                let mut name = wide_str(SESSION);
                // SAFETY: zeroed logfile, then the fields real-time consumption needs.
                let mut log: EVENT_TRACE_LOGFILEW = unsafe { std::mem::zeroed() };
                log.LoggerName = name.as_mut_ptr();
                log.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
                log.Anonymous2.EventRecordCallback = Some(on_event);
                // SAFETY: `log` and `name` outlive the trace, which runs until the session stops.
                let h: PROCESSTRACE_HANDLE = unsafe { OpenTraceW(&mut log) };
                if h.Value == INVALID_PROCESSTRACE_HANDLE {
                    return;
                }
                // SAFETY: one valid handle; blocks delivering events until the session ends.
                unsafe { ProcessTrace(&h, 1, null(), null()) };
            })
            .is_ok();
        if spawned {
            FpsStatus::Running
        } else {
            stop();
            FpsStatus::Unavailable
        }
    })
}

fn log(msg: &str) {
    if std::env::var_os("OXI_TASKMGR_TIMING").is_some() {
        eprintln!("fps trace: {msg}");
    }
}

/// Stops the session so it does not outlive the app.
pub fn stop() {
    let name = wide_str(SESSION);
    let mut pr = props();
    // SAFETY: as in `start`.
    unsafe { ControlTraceW(Default::default(), name.as_ptr(), &mut pr.p, EVENT_TRACE_CONTROL_STOP) };
}

/// Present times per process (its busiest swap chain), dropping chains idle for the whole window.
pub fn read() -> Vec<RawFps> {
    let freq = qpc_freq();
    let now_ticks = qpc_now();
    let now = now_ticks as f64 / freq - LAG_SECS;
    let keep = (WINDOW_SECS * freq) as i64;
    let Ok(mut r) = recorder().lock() else { return Vec::new() };
    r.chains.retain(|_, c| c.times.back().is_some_and(|t| *t >= now_ticks - keep));
    let mut best: HashMap<u32, &Chain> = HashMap::new();
    let recent = |c: &Chain| c.times.iter().rev().take_while(|t| (**t as f64 / freq) > now - 1.0).count();
    for ((pid, _), c) in &r.chains {
        match best.get(pid) {
            Some(b) if (recent(b), b.times.len()) >= (recent(c), c.times.len()) => {}
            _ => {
                best.insert(*pid, c);
            }
        }
    }
    best.into_iter()
        .map(|(pid, c)| RawFps {
            pid,
            api: c.api.unwrap_or_default(),
            times: c.times.iter().map(|t| *t as f64 / freq).collect(),
            now,
        })
        .collect()
}
