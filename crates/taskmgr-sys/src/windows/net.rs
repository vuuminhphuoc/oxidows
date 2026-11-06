//! Per-process network bytes from the Microsoft-Windows-Kernel-Network ETW provider.
//!
//! A real-time trace session needs administrator rights (Task Manager itself gets this data from a
//! system service). Without them the Network column shows a dash.

use std::collections::HashMap;
use std::mem::size_of;
use std::ptr::null;
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::System::Diagnostics::Etw::{
    CONTROLTRACE_HANDLE, ControlTraceW, EVENT_CONTROL_CODE_ENABLE_PROVIDER, EVENT_RECORD, EVENT_TRACE_CONTROL_STOP,
    EVENT_TRACE_LOGFILEW, EVENT_TRACE_PROPERTIES, EVENT_TRACE_REAL_TIME_MODE, EnableTraceEx2, OpenTraceW,
    PROCESS_TRACE_MODE_EVENT_RECORD, PROCESS_TRACE_MODE_REAL_TIME, PROCESSTRACE_HANDLE, ProcessTrace, StartTraceW,
    WNODE_FLAG_TRACED_GUID,
};
use windows_sys::core::GUID;

const SESSION: &str = "oxidows-taskmgr-net";
/// Microsoft-Windows-Kernel-Network {7DD42A49-5329-4832-8DFD-43D979153A88}
const KERNEL_NETWORK: GUID = GUID::from_u128(0x7dd42a49_5329_4832_8dfd_43d979153a88);
const KEYWORD_IPV4: u64 = 0x10;
const KEYWORD_IPV6: u64 = 0x20;
const TRACE_LEVEL_INFORMATION: u8 = 4;
const ERROR_ALREADY_EXISTS: u32 = 183;
const INVALID_PROCESSTRACE_HANDLE: u64 = u64::MAX;

/// Cumulative bytes sent + received, by PID.
fn totals() -> &'static Mutex<HashMap<u32, u64>> {
    static T: OnceLock<Mutex<HashMap<u32, u64>>> = OnceLock::new();
    T.get_or_init(Default::default)
}

/// Send / receive event ids for TCP and UDP, IPv4 and IPv6. Each payload starts with PID u32, size u32.
fn is_transfer(id: u16) -> bool {
    matches!(id, 10 | 11 | 26 | 27 | 42 | 43 | 58 | 59)
}

unsafe extern "system" fn on_event(rec: *mut EVENT_RECORD) {
    // SAFETY: ETW passes a valid record for the duration of the call.
    let rec = unsafe { &*rec };
    if !is_transfer(rec.EventHeader.EventDescriptor.Id) || rec.UserDataLength < 8 || rec.UserData.is_null() {
        return;
    }
    // SAFETY: at least 8 bytes of user data, read unaligned.
    let (pid, size) = unsafe {
        let p = rec.UserData.cast::<u8>();
        (p.cast::<u32>().read_unaligned(), p.add(4).cast::<u32>().read_unaligned())
    };
    if let Ok(mut t) = totals().lock() {
        *t.entry(pid).or_default() += u64::from(size);
    }
}

fn wide(s: &str) -> Vec<u16> {
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
    b.p.LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
    b.p.FlushTimer = 1;
    b.p.LoggerNameOffset = size_of::<EVENT_TRACE_PROPERTIES>() as u32;
    b
}

/// Starts the trace on its own thread. Returns false when it cannot (usually: not elevated).
pub fn start() -> bool {
    static STARTED: OnceLock<bool> = OnceLock::new();
    *STARTED.get_or_init(|| {
        let name = wide(SESSION);
        let mut handle: CONTROLTRACE_HANDLE = Default::default();
        let mut pr = props();
        // SAFETY: name NUL-terminated; properties buffer sized in Wnode.BufferSize.
        let mut r = unsafe { StartTraceW(&mut handle, name.as_ptr(), &mut pr.p) };
        if r == ERROR_ALREADY_EXISTS {
            // A previous run crashed and left its session behind: stop it and start fresh.
            let mut stop = props();
            // SAFETY: as above.
            unsafe { ControlTraceW(Default::default(), name.as_ptr(), &mut stop.p, EVENT_TRACE_CONTROL_STOP) };
            pr = props();
            // SAFETY: as above.
            r = unsafe { StartTraceW(&mut handle, name.as_ptr(), &mut pr.p) };
        }
        if r != 0 {
            return false;
        }
        // SAFETY: valid session handle and provider GUID.
        let r = unsafe {
            EnableTraceEx2(
                handle,
                &KERNEL_NETWORK,
                EVENT_CONTROL_CODE_ENABLE_PROVIDER,
                TRACE_LEVEL_INFORMATION,
                KEYWORD_IPV4 | KEYWORD_IPV6,
                0,
                0,
                null(),
            )
        };
        if r != 0 {
            stop();
            return false;
        }
        std::thread::Builder::new()
            .name("etw-network".into())
            .spawn(move || {
                let mut name = wide(SESSION);
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
            .is_ok()
    })
}

/// Stops the session so it does not outlive the app.
pub fn stop() {
    let name = wide(SESSION);
    let mut pr = props();
    // SAFETY: as in `start`.
    unsafe { ControlTraceW(Default::default(), name.as_ptr(), &mut pr.p, EVENT_TRACE_CONTROL_STOP) };
}

/// Runs `f` over the cumulative bytes per PID.
pub fn with_totals<R>(f: impl FnOnce(&HashMap<u32, u64>) -> R) -> Option<R> {
    totals().lock().ok().map(|t| f(&t))
}
