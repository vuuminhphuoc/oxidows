//! "About" and update checking, the same in every app: shows this build's version, asks GitHub
//! for the latest release, and on request downloads the app's exe from it and puts it in place of
//! the running one, keeping the same path (Ctrl+Shift+Esc and the default-app settings point at
//! that path).
//!
//! A running exe cannot be overwritten on Windows, but it can be renamed: the new exe is written
//! next to it as `<name>.new`, the running one is renamed to `<name>.old`, and the new one takes
//! its name. "Restart" then starts the new exe, which waits for this process to exit (see
//! [`startup`]) and removes the `.old` file once nothing runs it any more.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use egui::{Button, RichText, Ui};

use crate::{Icon, Palette, secondary};

/// The GitHub repository the releases come from.
pub const REPO: &str = "vuuminhphuoc/oxidows";
const LATEST_API: &str = "https://api.github.com/repos/vuuminhphuoc/oxidows/releases/latest";
/// Set on the process an update starts: the id of the process it replaces, to wait for.
const WAIT_ENV: &str = "OXI_UPDATE_WAIT_PID";

/// The app the About section is for.
#[derive(Clone, Copy, Debug)]
pub struct AppInfo {
    /// Shown as the title, e.g. "Task Manager".
    pub name: &'static str,
    /// The file name of the app's exe among the release downloads, e.g. "oxi-taskmgr.exe".
    pub asset: &'static str,
    /// This build's version (the app's `CARGO_PKG_VERSION`).
    pub version: &'static str,
}

/// Call first thing in `main`. When this process was started by an update, waits (up to 15 s)
/// for the one it replaces to exit, so the two never run at once; then removes leftover copies
/// of the previous exe in the background.
pub fn startup() {
    if let Some(pid) = std::env::var(WAIT_ENV).ok().and_then(|v| v.parse::<u32>().ok()) {
        // SAFETY: called at the top of `main`, before the app starts any thread.
        unsafe { std::env::remove_var(WAIT_ENV) };
        wait_for_exit(pid, std::time::Duration::from_secs(15));
    }
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::thread::Builder::new().name("update-cleanup".into()).spawn(move || remove_leftovers(&exe));
    }
}

/// A version as `[major, minor, patch]`, from "v0.4.0", "0.4" or "1.2.3-beta" (the suffix is
/// ignored).
pub fn parse_version(s: &str) -> Option<[u64; 3]> {
    let s = s.trim();
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    let core = s.split(['-', '+']).next()?;
    let mut out = [0u64; 3];
    let mut parts = core.split('.');
    for slot in &mut out {
        match parts.next() {
            Some(p) => *slot = p.parse().ok()?,
            None => break,
        }
    }
    if parts.next().is_some() || core.is_empty() {
        return None;
    }
    Some(out)
}

/// Whether `latest` is a newer version than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse_version(latest), parse_version(current)), (Some(l), Some(c)) if l > c)
}

/// The latest published release, as far as this app is concerned.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub tag: String,
    /// The release page on GitHub.
    pub page: String,
    /// This app's exe in the release, if it has one.
    pub asset: Option<Asset>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub url: String,
    pub size: u64,
    /// SHA-256 of the file, as GitHub reports it.
    pub sha256: Option<[u8; 32]>,
}

/// Reads GitHub's "latest release" reply, picking the download named `asset`.
pub fn parse_release(json: &[u8], asset: &str) -> Result<Release, String> {
    let v: serde_json::Value =
        serde_json::from_slice(json).map_err(|e| format!("Unexpected reply from GitHub ({e})"))?;
    let tag = v["tag_name"].as_str().ok_or_else(|| match v["message"].as_str() {
        Some(m) => format!("GitHub: {m}"),
        None => "The latest release has no version".to_string(),
    })?;
    let page =
        v["html_url"].as_str().map(str::to_string).unwrap_or_else(|| format!("https://github.com/{REPO}/releases"));
    let asset = v["assets"].as_array().and_then(|list| {
        let a = list.iter().find(|a| a["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(asset)))?;
        Some(Asset {
            url: a["browser_download_url"].as_str()?.to_string(),
            size: a["size"].as_u64().unwrap_or(0),
            sha256: a["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).and_then(parse_hex32),
        })
    });
    Ok(Release { tag: tag.to_string(), page, asset })
}

fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    let b = s.as_bytes();
    if b.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(std::str::from_utf8(&b[i * 2..i * 2 + 2]).ok()?, 16).ok()?;
    }
    Some(out)
}

/// Checks a downloaded exe before it replaces the running one.
pub fn verify(bytes: &[u8], asset: &Asset) -> Result<(), String> {
    if asset.size != 0 && bytes.len() as u64 != asset.size {
        return Err(format!("The download is incomplete ({} of {} bytes)", bytes.len(), asset.size));
    }
    if let Some(want) = asset.sha256
        && sha256(bytes) != want
    {
        return Err("The download is damaged (its SHA-256 does not match the release)".into());
    }
    if cfg!(windows) && !bytes.starts_with(b"MZ") {
        return Err("The download is not a Windows program".into());
    }
    Ok(())
}

/// Puts `bytes` in place of the exe at `exe`, which may be running: writes `<name>.new`, renames
/// `exe` out of the way and the new file in. Returns where the previous exe now is.
pub fn replace_exe(exe: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    let name = exe.file_name().ok_or_else(|| io::Error::other("no file name"))?.to_string_lossy().into_owned();
    let new = exe.with_file_name(format!("{name}.new"));
    std::fs::write(&new, bytes)?;
    // Keep the exe's permissions (on Unix, the executable bit).
    if let Ok(m) = std::fs::metadata(exe) {
        let _ = std::fs::set_permissions(&new, m.permissions());
    }
    // `.old` may still be in use by another window of the previous version; then pick a name of
    // our own. `startup` removes them all later.
    let mut old = exe.with_file_name(format!("{name}.old"));
    if old.exists() && std::fs::remove_file(&old).is_err() {
        old = exe.with_file_name(format!("{name}.old-{}", std::process::id()));
        let _ = std::fs::remove_file(&old);
    }
    if let Err(e) = std::fs::rename(exe, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&new, exe) {
        let _ = std::fs::rename(&old, exe);
        let _ = std::fs::remove_file(&new);
        return Err(e);
    }
    Ok(old)
}

/// Removes `<exe>.old*` and `<exe>.new` next to `exe`; ones still running stay until next time.
fn remove_leftovers(exe: &Path) {
    let (Some(dir), Some(name)) = (exe.parent(), exe.file_name()) else { return };
    let name = name.to_string_lossy().to_lowercase();
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let f = e.file_name().to_string_lossy().to_lowercase();
        if let Some(rest) = f.strip_prefix(&name)
            && (rest == ".new" || rest.starts_with(".old"))
        {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Starts the (now updated) exe again with the same arguments; the caller then closes its window.
pub fn restart() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    std::process::Command::new(exe).args(args).env(WAIT_ENV, std::process::id().to_string()).spawn().map(drop)
}

/// What the About section is doing.
#[derive(Clone, Debug, Default)]
enum Phase {
    #[default]
    Idle,
    Checking,
    UpToDate(String),
    Available(Release),
    Downloading {
        tag: String,
        got: u64,
        total: u64,
    },
    /// Installed; takes effect on restart.
    Installed(String),
    Failed(String),
}

/// The About section with its "Check for updates" button. Checks and downloads run on their own
/// thread; the window repaints when they finish.
pub struct Updater {
    app: AppInfo,
    phase: Arc<Mutex<Phase>>,
}

impl Updater {
    pub fn new(app: AppInfo) -> Self {
        Self { app, phase: Arc::default() }
    }

    fn phase(&self) -> Phase {
        self.phase.lock().map(|p| p.clone()).unwrap_or_default()
    }

    fn set(phase: &Mutex<Phase>, p: Phase, ctx: &egui::Context) {
        if let Ok(mut g) = phase.lock() {
            *g = p;
        }
        ctx.request_repaint();
    }

    /// Asks GitHub for the latest release.
    pub fn check(&self, ctx: &egui::Context) {
        let (phase, ctx, app) = (self.phase.clone(), ctx.clone(), self.app);
        Self::set(&phase, Phase::Checking, &ctx);
        let _ = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let r = http_get(LATEST_API, "application/vnd.github+json", &mut |_, _| {})
                .and_then(|body| parse_release(&body, app.asset));
            let p = match r {
                Ok(rel) if is_newer(&rel.tag, app.version) => Phase::Available(rel),
                Ok(rel) => Phase::UpToDate(rel.tag),
                Err(e) => Phase::Failed(e),
            };
            Self::set(&phase, p, &ctx);
        });
    }

    /// Downloads the release's exe, checks it and puts it in place of this one.
    fn install(&self, ctx: &egui::Context, rel: Release) {
        let Some(asset) = rel.asset.clone() else { return };
        let (phase, ctx) = (self.phase.clone(), ctx.clone());
        Self::set(&phase, Phase::Downloading { tag: rel.tag.clone(), got: 0, total: asset.size }, &ctx);
        let _ = std::thread::Builder::new().name("update-download".into()).spawn(move || {
            let tag = rel.tag.clone();
            let mut last = std::time::Instant::now();
            let mut progress = |got: u64, total: Option<u64>| {
                if last.elapsed().as_millis() >= 100 {
                    last = std::time::Instant::now();
                    let total = total.unwrap_or(asset.size);
                    Self::set(&phase, Phase::Downloading { tag: tag.clone(), got, total }, &ctx);
                }
            };
            let r = http_get(&asset.url, "application/octet-stream", &mut progress)
                .and_then(|bytes| verify(&bytes, &asset).map(|()| bytes))
                .and_then(|bytes| {
                    let exe = std::env::current_exe().map_err(|e| format!("Cannot find this app's exe ({e})"))?;
                    replace_exe(&exe, &bytes).map_err(|e| replace_error(&exe, &e))
                });
            let p = match r {
                Ok(_) => Phase::Installed(rel.tag),
                Err(e) => Phase::Failed(e),
            };
            Self::set(&phase, p, &ctx);
        });
    }

    /// Draws the section: name, version, the button and what it found.
    pub fn ui(&self, ui: &mut Ui) {
        let phase = self.phase();
        let app = self.app;
        ui.label(RichText::new(format!("{} (oxidows)", app.name)).strong());
        secondary(ui, format!("Version {}", app.version));
        ui.add_space(6.0);
        let busy = matches!(phase, Phase::Checking | Phase::Downloading { .. });
        let installed = matches!(phase, Phase::Installed(_));
        ui.horizontal(|ui| {
            let check = ui.add_enabled(!busy && !installed, Button::new(Icon::Refresh.with("Check for updates")));
            if check.clicked() {
                self.check(ui.ctx());
            }
            match &phase {
                Phase::Idle => {}
                Phase::Checking => {
                    ui.spinner();
                    secondary(ui, "Checking…");
                }
                Phase::UpToDate(tag) => {
                    secondary(ui, format!("You have the latest version ({tag})."));
                }
                Phase::Downloading { tag, got, total } => {
                    let frac = if *total > 0 { *got as f32 / *total as f32 } else { 0.0 };
                    ui.add(egui::ProgressBar::new(frac).desired_width(180.0).text(format!(
                        "Downloading {tag}… {:.1} / {:.1} MB",
                        mb(*got),
                        mb(*total)
                    )));
                }
                Phase::Failed(e) => {
                    ui.colored_label(Palette::of(ui).danger, e);
                }
                Phase::Available(_) | Phase::Installed(_) => {}
            }
        });
        match phase {
            Phase::Available(rel) => {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(format!("Version {} is available.", rel.tag.trim_start_matches('v')));
                    if rel.asset.is_some() {
                        let b = ui
                            .button(Icon::Downloads.with("Download and install"))
                            .on_hover_text(format!("Replaces {} with the new version", exe_display()));
                        if b.clicked() {
                            self.install(ui.ctx(), rel.clone());
                        }
                    } else {
                        secondary(ui, format!("(it has no {})", app.asset));
                    }
                    if ui.link("Release notes").clicked() {
                        open_url(&rel.page);
                    }
                });
            }
            Phase::Installed(tag) => {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(format!("Updated to {}. Restart {} to use it.", tag.trim_start_matches('v'), app.name));
                    if ui.button(Icon::Refresh.with("Restart now")).clicked() {
                        match restart() {
                            Ok(()) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                            Err(e) => {
                                Self::set(&self.phase, Phase::Failed(format!("Could not restart ({e})")), ui.ctx())
                            }
                        }
                    }
                });
            }
            _ => {}
        }
        ui.add_space(4.0);
        if ui.link(format!("github.com/{REPO}")).clicked() {
            open_url(&format!("https://github.com/{REPO}/releases"));
        }
    }
}

fn mb(b: u64) -> f64 {
    b as f64 / (1024.0 * 1024.0)
}

fn exe_display() -> String {
    std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "this app".into())
}

fn replace_error(exe: &Path, e: &io::Error) -> String {
    if e.kind() == io::ErrorKind::PermissionDenied {
        format!(
            "No permission to replace {} (run as administrator, or keep the app in a folder you own)",
            exe.display()
        )
    } else {
        format!("Could not replace {} ({e})", exe.display())
    }
}

/// Opens a web page in the default browser.
pub fn open_url(url: &str) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let (verb, url) = (wide("open"), wide(url));
        // SAFETY: null-terminated UTF-16 strings that outlive the call.
        unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                url.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Waits until process `pid` has exited, at most `max`.
#[cfg(windows)]
fn wait_for_exit(pid: u32, max: std::time::Duration) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    // SAFETY: plain calls; the handle is closed below.
    let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if h.is_null() {
        return; // Already gone.
    }
    // SAFETY: `h` is a valid process handle.
    unsafe {
        WaitForSingleObject(h, max.as_millis().min(u32::MAX as u128) as u32);
        CloseHandle(h);
    }
}

#[cfg(not(windows))]
fn wait_for_exit(pid: u32, max: std::time::Duration) {
    let t = std::time::Instant::now();
    while Path::new(&format!("/proc/{pid}")).exists() && t.elapsed() < max {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// GETs `url` over HTTPS with the system's proxy settings (WinINet), following redirects.
/// `progress` gets the bytes read so far and the length when the server sends one.
#[cfg(windows)]
fn http_get(url: &str, accept: &str, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<Vec<u8>, String> {
    use std::ffi::c_void;
    use windows_sys::Win32::Networking::WinInet::{
        HTTP_QUERY_CONTENT_LENGTH, HTTP_QUERY_FLAG_NUMBER, HTTP_QUERY_STATUS_CODE, HttpQueryInfoW,
        INTERNET_FLAG_NO_CACHE_WRITE, INTERNET_FLAG_NO_UI, INTERNET_FLAG_RELOAD, INTERNET_OPEN_TYPE_PRECONFIG,
        INTERNET_OPTION_CONNECT_TIMEOUT, INTERNET_OPTION_RECEIVE_TIMEOUT, InternetCloseHandle, InternetOpenUrlW,
        InternetOpenW, InternetReadFile, InternetSetOptionW,
    };

    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: a handle WinINet returned, closed once.
            unsafe { InternetCloseHandle(self.0) };
        }
    }
    let fail = |what: &str| net_error(what, io::Error::last_os_error());

    let agent = wide(concat!("oxidows-updater/", env!("CARGO_PKG_VERSION")));
    // SAFETY: null-terminated agent string; null proxy arguments mean the system settings.
    let net =
        unsafe { InternetOpenW(agent.as_ptr(), INTERNET_OPEN_TYPE_PRECONFIG, std::ptr::null(), std::ptr::null(), 0) };
    if net.is_null() {
        return Err(fail("Cannot use the network"));
    }
    let net = Handle(net);
    let timeout: u32 = 20_000;
    for opt in [INTERNET_OPTION_CONNECT_TIMEOUT, INTERNET_OPTION_RECEIVE_TIMEOUT] {
        // SAFETY: the option takes a u32 by pointer.
        unsafe { InternetSetOptionW(net.0, opt, (&raw const timeout).cast(), 4) };
    }
    let (url_w, headers) = (wide(url), wide(&format!("Accept: {accept}\r\n")));
    let flags = INTERNET_FLAG_RELOAD | INTERNET_FLAG_NO_CACHE_WRITE | INTERNET_FLAG_NO_UI;
    // SAFETY: null-terminated strings (header length u32::MAX = "up to the terminator").
    let req = unsafe { InternetOpenUrlW(net.0, url_w.as_ptr(), headers.as_ptr(), u32::MAX, flags, 0) };
    if req.is_null() {
        return Err(fail("Cannot reach GitHub"));
    }
    let req = Handle(req);
    let number = |level: u32| -> Option<u32> {
        let (mut v, mut len) = (0u32, 4u32);
        // SAFETY: a u32 buffer of the stated length.
        let ok = unsafe {
            HttpQueryInfoW(req.0, level | HTTP_QUERY_FLAG_NUMBER, (&raw mut v).cast(), &mut len, std::ptr::null_mut())
        };
        (ok != 0).then_some(v)
    };
    let status = number(HTTP_QUERY_STATUS_CODE).unwrap_or(0);
    let total = number(HTTP_QUERY_CONTENT_LENGTH).map(u64::from);
    let mut body = Vec::with_capacity(total.unwrap_or(64 * 1024) as usize);
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let mut n = 0u32;
        // SAFETY: `buf` is valid for `buf.len()` bytes of writes.
        let ok = unsafe { InternetReadFile(req.0, buf.as_mut_ptr().cast(), buf.len() as u32, &mut n) };
        if ok == 0 {
            return Err(fail("The download stopped"));
        }
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n as usize]);
        progress(body.len() as u64, total);
    }
    match status {
        200..=299 => Ok(body),
        // GitHub answers a JSON message ("API rate limit exceeded…", "Not Found").
        _ => match parse_release(&body, "") {
            Err(m) if m.starts_with("GitHub:") => Err(m),
            _ if status == 404 => Err("No release has been published yet".into()),
            _ => Err(format!("GitHub answered HTTP {status}")),
        },
    }
}

#[cfg(windows)]
fn net_error(what: &str, e: io::Error) -> String {
    // WinINet's own codes (12000-12175) have no text in the system message table.
    let hint = match e.raw_os_error() {
        Some(12002) => "timed out",
        Some(12007) => "server name not found; are you offline?",
        Some(12029) | Some(12030) => "could not connect; are you offline?",
        Some(12157) | Some(12175) => "secure connection failed",
        _ => "",
    };
    match (hint, e.raw_os_error()) {
        ("", Some(c)) => format!("{what} (error {c})"),
        ("", None) => format!("{what} ({e})"),
        (h, _) => format!("{what}: {h}"),
    }
}

/// Elsewhere (development on Linux), through curl.
#[cfg(not(windows))]
fn http_get(url: &str, accept: &str, _progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<Vec<u8>, String> {
    let out = std::process::Command::new("curl")
        .args(["-sSL", "--max-time", "300", "-A", "oxidows-updater", "-H"])
        .arg(format!("Accept: {accept}"))
        .arg(url)
        .output()
        .map_err(|e| format!("Cannot run curl ({e})"))?;
    if !out.status.success() {
        return Err(format!("Download failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(out.stdout)
}

/// SHA-256 of `data` (FIPS 180-4).
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] =
        [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut blocks = data.chunks_exact(64);
    for b in &mut blocks {
        sha256_block(&mut h, b);
    }
    let rest = blocks.remainder();
    let mut tail = [0u8; 128];
    tail[..rest.len()].copy_from_slice(rest);
    tail[rest.len()] = 0x80;
    let n = if rest.len() < 56 { 64 } else { 128 };
    tail[n - 8..n].copy_from_slice(&((data.len() as u64).wrapping_mul(8)).to_be_bytes());
    for b in tail[..n].chunks_exact(64) {
        sha256_block(&mut h, b);
    }
    let mut out = [0u8; 32];
    for (o, v) in out.chunks_exact_mut(4).zip(h) {
        o.copy_from_slice(&v.to_be_bytes());
    }
    out
}

fn sha256_block(h: &mut [u32; 8], block: &[u8]) {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
        0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
        0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
        0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut w = [0u32; 64];
    for (wi, c) in w.iter_mut().zip(block.chunks_exact(4)) {
        *wi = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    for (k, wi) in K.iter().zip(w) {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(*k).wrapping_add(wi);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (x, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
        *x = x.wrapping_add(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.4.0"), Some([0, 4, 0]));
        assert_eq!(parse_version("0.4"), Some([0, 4, 0]));
        assert_eq!(parse_version("1.2.3-beta.1"), Some([1, 2, 3]));
        assert_eq!(parse_version("v1.2.3.4"), None);
        assert_eq!(parse_version("latest"), None);
        assert!(is_newer("v0.5.0", "0.4.0"));
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("v1.0.0", "0.99.0"));
        assert!(!is_newer("v0.4.0", "0.4.0"));
        assert!(!is_newer("v0.3.9", "0.4.0"));
        assert!(!is_newer("nonsense", "0.4.0"));
    }

    #[test]
    fn sha256_vectors() {
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(
            hex(&sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            hex(&sha256(&vec![b'a'; 1_000_000])),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
        // Lengths around the padding boundary.
        assert_eq!(hex(&sha256(&[0u8; 64])), "f5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b");
    }

    const LATEST: &str = r#"{
        "tag_name": "v0.4.0",
        "html_url": "https://github.com/vuuminhphuoc/oxidows/releases/tag/v0.4.0",
        "assets": [
            {"name": "oxi-explorer.exe", "size": 13008896,
             "digest": "sha256:1b419138230b561f09f3365c0672845f42a8ba05dabdaebf9827f8601cf538ae",
             "browser_download_url": "https://github.com/vuuminhphuoc/oxidows/releases/download/v0.4.0/oxi-explorer.exe"},
            {"name": "oxi-taskmgr.exe", "size": 11223552, "digest": null,
             "browser_download_url": "https://github.com/vuuminhphuoc/oxidows/releases/download/v0.4.0/oxi-taskmgr.exe"}
        ]
    }"#;

    #[test]
    fn release_reply() {
        let r = parse_release(LATEST.as_bytes(), "oxi-explorer.exe").unwrap();
        assert_eq!(r.tag, "v0.4.0");
        assert!(r.page.ends_with("/tag/v0.4.0"));
        let a = r.asset.unwrap();
        assert!(a.url.ends_with("/v0.4.0/oxi-explorer.exe"));
        assert_eq!(a.size, 13008896);
        assert_eq!(hex(&a.sha256.unwrap()), "1b419138230b561f09f3365c0672845f42a8ba05dabdaebf9827f8601cf538ae");

        let t = parse_release(LATEST.as_bytes(), "OXI-TASKMGR.EXE").unwrap().asset.unwrap();
        assert_eq!(t.sha256, None);
        assert_eq!(parse_release(LATEST.as_bytes(), "oxi-notepad.exe").unwrap().asset, None);

        let limited = br#"{"message": "API rate limit exceeded for 1.2.3.4.", "documentation_url": "x"}"#;
        assert_eq!(parse_release(limited, "x").unwrap_err(), "GitHub: API rate limit exceeded for 1.2.3.4.");
        assert!(parse_release(b"<html>", "x").is_err());
    }

    #[test]
    fn verify_download() {
        let body = b"MZ new exe".to_vec();
        let ok = Asset { url: String::new(), size: body.len() as u64, sha256: Some(sha256(&body)) };
        assert_eq!(verify(&body, &ok), Ok(()));
        assert!(verify(&body[..4], &ok).is_err());
        let wrong = Asset { sha256: Some([0; 32]), ..ok.clone() };
        assert!(verify(&body, &wrong).is_err());
        let unknown = Asset { size: 0, sha256: None, ..ok };
        assert_eq!(verify(&body, &unknown), Ok(()));
    }

    #[test]
    fn replace_in_place() {
        let dir = std::env::temp_dir().join(format!("oxi-update-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("oxi-test.exe");
        std::fs::write(&exe, b"old").unwrap();
        // A leftover from an earlier update.
        std::fs::write(dir.join("oxi-test.exe.old"), b"older").unwrap();

        let old = replace_exe(&exe, b"new").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(&old).unwrap(), b"old");
        assert!(!dir.join("oxi-test.exe.new").exists());

        std::fs::write(dir.join("oxi-test.exe.old-123"), b"x").unwrap();
        std::fs::write(dir.join("other.exe.old"), b"keep").unwrap();
        remove_leftovers(&exe);
        let mut left: Vec<String> =
            std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["other.exe.old", "oxi-test.exe"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Windows does not let a running exe be overwritten; the update renames it instead. Checks
    /// that this works on a real running program (a copy of ping.exe).
    #[cfg(windows)]
    #[test]
    fn replace_running_exe() {
        let dir = std::env::temp_dir().join(format!("oxi-update-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("oxi-running.exe");
        let sys = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| "C:\\Windows".into());
        std::fs::copy(sys.join("System32").join("PING.EXE"), &exe).unwrap();
        let mut child = std::process::Command::new(&exe)
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(std::fs::write(&exe, b"MZ").is_err(), "a running exe should not be writable");
        let r = replace_exe(&exe, b"MZ replaced");
        let _ = child.kill();
        let _ = child.wait();
        let old = r.unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"MZ replaced");
        assert!(old.exists());
        remove_leftovers(&exe);
        assert!(!old.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
