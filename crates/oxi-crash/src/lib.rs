//! Keeps one panic from taking the whole app down.
//!
//! The release profile unwinds (`panic = "unwind"`), so a panic can be caught where it would
//! otherwise end the process: a frame of the UI, a callback Windows calls into us (drop target,
//! window procedures, ETW), a background thread. Every panic is written to
//! `%APPDATA%\oxidows\crash.log` by the hook [`install`] sets up, and the apps show a short
//! notice ([`take_notice`]) in their status bar.
//!
//! A panic must never unwind out of an `extern "system"` function (Rust aborts there), so each
//! of those wraps its body in [`guard_or`].

use std::any::Any;
use std::backtrace::Backtrace;
use std::fmt::Write as _;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Where the log is written and who writes it; set once by `install`.
static LOG: Mutex<Option<(PathBuf, &'static str)>> = Mutex::new(None);
/// The latest panic not yet shown to the user.
static NOTICE: Mutex<Option<String>> = Mutex::new(None);
static INSTALLED: AtomicBool = AtomicBool::new(false);
/// Times of the latest panics inside UI frames, to tell a one-off from a loop.
static FRAME_PANICS: Mutex<Vec<Instant>> = Mutex::new(Vec::new());

/// The log is cut back to nothing past this size (the older half is not worth keeping).
const MAX_LOG: u64 = 256 * 1024;
/// This many panics inside UI frames within [`FRAME_WINDOW`] mean the app cannot recover.
const FRAME_LIMIT: usize = 5;
const FRAME_WINDOW: Duration = Duration::from_secs(3);

/// `%APPDATA%\oxidows\crash.log` (the XDG config folder elsewhere).
pub fn log_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("oxidows").join("crash.log"))
}

/// Logs every panic of this process to `crash.log` and queues a [`take_notice`] text. Call once
/// at the start of `main`; the previous hook (stderr message) still runs.
pub fn install(app: &'static str) {
    if let Some(path) = log_path() {
        install_to(app, path);
    }
}

/// [`install`] with a chosen log file (tests).
pub fn install_to(app: &'static str, path: PathBuf) {
    if let Ok(mut l) = LOG.lock() {
        *l = Some((path, app));
    }
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let what = payload_text(info.payload());
        let place = info.location().map_or_else(String::new, |l| format!(" at {}:{}", l.file(), l.line()));
        let thread = std::thread::current();
        let thread = thread.name().unwrap_or("unnamed");
        // Backtraces are raw addresses in the stripped release exe; still enough to match a build.
        let trace = Backtrace::force_capture();
        record(&format!("panic in thread '{thread}': {what}{place}\n{trace}"));
        set_notice(format!("Something went wrong ({what}). Details are in crash.log."));
        previous(info);
    }));
}

/// Runs `f`; a panic inside it is caught (and logged by the hook) and gives `None`.
pub fn guard<R>(f: impl FnOnce() -> R) -> Option<R> {
    panic::catch_unwind(AssertUnwindSafe(f)).ok()
}

/// [`guard`] for callbacks that must return something: `fallback` after a panic.
pub fn guard_or<R>(fallback: R, f: impl FnOnce() -> R) -> R {
    guard(f).unwrap_or(fallback)
}

/// [`guard`] for one UI frame. A panic is caught so the window stays; if frames keep panicking
/// (the app is stuck in a state it cannot draw) the process exits instead of looping forever.
pub fn guard_frame<R>(f: impl FnOnce() -> R) -> Option<R> {
    let r = guard(f);
    if r.is_none() && frame_loop() {
        record("UI frames keep panicking; exiting");
        std::process::exit(101);
    }
    r
}

fn frame_loop() -> bool {
    let now = Instant::now();
    let Ok(mut v) = FRAME_PANICS.lock() else { return false };
    v.retain(|t| now.duration_since(*t) < FRAME_WINDOW);
    v.push(now);
    v.len() >= FRAME_LIMIT
}

/// Starts a thread whose panic is caught and logged instead of ending the thread unlogged.
pub fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name(name.to_string()).spawn(move || {
        let _ = guard(f);
    })
}

/// The latest panic's one-line description, once; for the status bar.
pub fn take_notice() -> Option<String> {
    NOTICE.lock().ok().and_then(|mut n| n.take())
}

fn set_notice(text: String) {
    if let Ok(mut n) = NOTICE.lock() {
        *n = Some(text);
    }
}

fn payload_text(p: &(dyn Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Appends an entry to the log: never fails loudly (a panic hook has nowhere to report to).
fn record(entry: &str) {
    let Ok(guard) = LOG.lock() else { return };
    let Some((path, app)) = guard.as_ref() else { return };
    let mut text = String::new();
    let _ = writeln!(text, "[{}] {app} {}: {entry}", utc_now(), env!("CARGO_PKG_VERSION"));
    append(path, &text);
}

fn append(path: &Path, text: &str) {
    use std::io::Write as _;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let too_big = std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOG);
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).write(true);
    if too_big {
        opts.truncate(true)
    } else {
        opts.append(true)
    };
    if let Ok(mut f) = opts.open(path) {
        let _ = f.write_all(text.as_bytes());
    }
}

/// `2026-11-22 03:16:08 UTC`
fn utc_now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    utc_text(secs)
}

fn utc_text(secs: u64) -> String {
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = i64::try_from(days).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(utc_text(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc_text(1_795_317_368), "2026-11-22 03:16:08 UTC");
        assert_eq!(utc_text(951_782_400), "2000-02-29 00:00:00 UTC");
    }

    #[test]
    fn guard_without_panic_returns_value() {
        assert_eq!(guard(|| 7), Some(7));
        assert_eq!(guard_or(0, || 9), 9);
    }

    // One test owns the process-wide hook and notice.
    #[test]
    fn panic_is_caught_logged_and_noticed() {
        let dir = std::env::temp_dir().join(format!("oxi-crash-test-{}", std::process::id()));
        let log = dir.join("crash.log");
        install_to("oxi-test", log.clone());

        assert_eq!(guard_or(-1, || -> i32 { panic!("boom {}", 42) }), -1);
        let text = std::fs::read_to_string(&log).expect("crash.log written");
        assert!(text.contains("oxi-test"), "{text}");
        assert!(text.contains("boom 42"), "{text}");
        assert!(text.contains("lib.rs"), "location missing: {text}");
        assert!(take_notice().is_some_and(|n| n.contains("boom 42")));
        assert!(take_notice().is_none(), "a notice is shown once");

        // A panic in a spawned thread is logged and the thread ends normally.
        spawn("worker", || panic!("worker failed")).expect("spawn").join().expect("thread survives");
        assert!(std::fs::read_to_string(&log).expect("log").contains("worker failed"));

        // A log past the limit starts over instead of growing.
        std::fs::write(&log, vec![b'x'; usize::try_from(MAX_LOG).unwrap() + 1]).unwrap();
        record("fresh");
        assert!(std::fs::metadata(&log).unwrap().len() < 4096);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
