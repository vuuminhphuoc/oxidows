//! Nearby sharing with phones and PCs over the open LocalSend v2 protocol
//! (<https://github.com/localsend/protocol>), so the free LocalSend app on Android, iOS, macOS and
//! Linux can send to and receive from this PC.
//!
//! One [`Share`] runs a HTTPS server (receiving), multicast discovery and outgoing transfers on
//! plain threads. The UI reads a [`Snapshot`] each frame and calls back with decisions; a repaint
//! callback tells it when something changed.

pub mod dto;
pub mod http;
pub mod identity;
pub mod names;
pub mod time;
pub mod tls;

mod discovery;
mod send;
mod server;

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

pub use dto::{DeviceType, FileDto, Protocol};
pub use identity::Identity;

pub const DEFAULT_PORT: u16 = 53317;
pub const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 167);

#[derive(Clone, Debug)]
pub struct Config {
    /// Name shown on other devices.
    pub alias: String,
    pub device_model: Option<String>,
    /// TCP and UDP port; the official default is 53317. When the TCP port is taken (LocalSend
    /// itself running) the next free one is used and announced.
    pub port: u16,
    /// Where the certificate and key are kept.
    pub identity_dir: PathBuf,
    pub receive_dir: PathBuf,
    /// Accept every incoming transfer without asking (LocalSend's "Quick Save").
    pub quick_save: bool,
    /// Join the multicast group and announce. Off in tests that must not touch the network.
    pub multicast: bool,
}

/// A device that answered or announced itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub fingerprint: String,
    pub alias: String,
    pub device_model: Option<String>,
    pub device_type: DeviceType,
    pub ip: IpAddr,
    pub port: u16,
    pub protocol: Protocol,
    pub version: String,
    pub last_seen: Instant,
    /// Refresh rounds in a row it did not answer. A phone on flaky Wi-Fi, or with LocalSend
    /// briefly in the background, misses one now and then, so it stays listed for a few.
    pub missed: u8,
}

impl Device {
    pub fn addr(&self) -> SocketAddr {
        SocketAddr::new(self.ip, self.port)
    }
}

/// A transfer waiting for the user's yes or no.
#[derive(Clone, Debug)]
pub struct Incoming {
    pub id: u64,
    pub alias: String,
    pub device_model: Option<String>,
    pub device_type: DeviceType,
    pub ip: IpAddr,
    pub files: Vec<FileDto>,
    /// A text message instead of files: there is nothing to save, only to show.
    pub message: Option<String>,
}

impl Incoming {
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Receive,
    Send,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Sending: waiting for the receiver to accept.
    Waiting,
    Running,
    Done,
    Declined,
    Cancelled,
    Failed(String),
}

impl State {
    pub fn finished(&self) -> bool {
        !matches!(self, State::Waiting | State::Running)
    }
}

#[derive(Clone, Debug)]
pub struct TransferFile {
    pub name: String,
    pub size: u64,
    pub done: u64,
    pub state: State,
    /// Receiving: where the file was saved.
    pub path: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Transfer {
    pub id: u64,
    pub direction: Direction,
    pub peer: String,
    pub files: Vec<TransferFile>,
    pub state: State,
    pub started: Instant,
    pub finished: Option<Instant>,
    /// A received or sent text message.
    pub message: Option<String>,
}

impl Transfer {
    pub fn total(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    pub fn done(&self) -> u64 {
        self.files.iter().map(|f| f.done).sum()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub devices: Vec<Device>,
    pub incoming: Option<Incoming>,
    pub transfers: Vec<Transfer>,
    /// The TCP port receiving, or why the server could not start.
    pub listening: Option<Result<u16, String>>,
    pub multicast_error: Option<String>,
    pub scanning: bool,
    pub fingerprint: String,
    pub alias: String,
    pub receive_dir: PathBuf,
    pub quick_save: bool,
}

#[derive(Debug)]
pub(crate) enum Decision {
    Accept,
    Decline,
    /// The sender cancelled while the user was still deciding.
    Cancelled,
}

pub(crate) struct Inner {
    pub(crate) id: Identity,
    pub(crate) cfg: Mutex<Config>,
    pub(crate) snap: Mutex<Snapshot>,
    /// The decision channel of `snap.incoming`.
    pub(crate) pending: Mutex<Option<(u64, IpAddr, SyncSender<Decision>)>>,
    pub(crate) session: Mutex<Option<server::Session>>,
    pub(crate) port: AtomicU64,
    pub(crate) stop: AtomicBool,
    next_id: AtomicU64,
    /// Outgoing transfers that were asked to stop.
    pub(crate) cancelled: Mutex<HashMap<u64, Arc<AtomicBool>>>,
    repaint: Arc<dyn Fn() + Send + Sync>,
    pub(crate) discovery: Mutex<Option<discovery::Handle>>,
}

impl Inner {
    pub(crate) fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn snap(&self) -> MutexGuard<'_, Snapshot> {
        self.snap.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn cfg(&self) -> Config {
        self.cfg.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) fn repaint(&self) {
        (self.repaint)();
    }

    pub(crate) fn info(&self, announce: Option<bool>) -> dto::DeviceInfo {
        let cfg = self.cfg();
        dto::DeviceInfo {
            alias: cfg.alias,
            version: dto::PROTOCOL_VERSION.into(),
            device_model: cfg.device_model,
            device_type: Some(DeviceType::Desktop),
            fingerprint: self.id.fingerprint.clone(),
            port: Some(self.port.load(Ordering::Relaxed) as u16),
            protocol: Some(Protocol::Https),
            download: false,
            announce,
        }
    }

    /// Adds or refreshes a device, keyed by fingerprint.
    pub(crate) fn found(&self, d: Device) {
        if d.fingerprint == self.id.fingerprint {
            return;
        }
        let mut s = self.snap();
        match s.devices.iter_mut().find(|x| x.fingerprint == d.fingerprint) {
            Some(x) => *x = d,
            None => s.devices.push(d),
        }
        s.devices.sort_by_key(|d| d.alias.to_lowercase());
        drop(s);
        self.repaint();
    }

    pub(crate) fn update_transfer(&self, id: u64, f: impl FnOnce(&mut Transfer)) {
        let mut s = self.snap();
        if let Some(t) = s.transfers.iter_mut().find(|t| t.id == id) {
            f(t);
            if t.state.finished() && t.finished.is_none() {
                t.finished = Some(Instant::now());
            }
        }
    }
}

/// The running service. Dropping it stops the server and discovery.
pub struct Share {
    inner: Arc<Inner>,
}

impl Share {
    pub fn start(cfg: Config, repaint: Arc<dyn Fn() + Send + Sync>) -> io::Result<Share> {
        let id = Identity::load_or_create(&cfg.identity_dir)?;
        let snap = Snapshot {
            fingerprint: id.fingerprint.clone(),
            alias: cfg.alias.clone(),
            receive_dir: cfg.receive_dir.clone(),
            quick_save: cfg.quick_save,
            ..Default::default()
        };
        let multicast = cfg.multicast;
        let inner = Arc::new(Inner {
            id,
            port: AtomicU64::new(u64::from(cfg.port)),
            cfg: Mutex::new(cfg),
            snap: Mutex::new(snap),
            pending: Mutex::new(None),
            session: Mutex::new(None),
            stop: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            cancelled: Mutex::new(HashMap::new()),
            repaint,
            discovery: Mutex::new(None),
        });
        server::start(&inner);
        if multicast {
            let d = discovery::start(&inner);
            *inner.discovery.lock().unwrap_or_else(|e| e.into_inner()) = Some(d);
        }
        Ok(Share { inner })
    }

    pub fn snapshot(&self) -> Snapshot {
        self.inner.snap().clone()
    }

    pub fn fingerprint(&self) -> &str {
        &self.inner.id.fingerprint
    }

    /// The port actually listening (may differ from the configured one).
    pub fn port(&self) -> u16 {
        self.inner.port.load(Ordering::Relaxed) as u16
    }

    /// Announces this device again and, if nobody answers within a few seconds, asks every address
    /// of the local subnets (for networks that drop multicast, such as many hotspots).
    pub fn refresh(&self) {
        discovery::refresh(&self.inner);
    }

    /// Asks one address directly, for devices the scan cannot reach.
    pub fn probe(&self, ip: IpAddr, port: u16) {
        let inner = self.inner.clone();
        std::thread::spawn(move || discovery::probe(&inner, SocketAddr::new(ip, port)));
    }

    /// Answers the pending [`Snapshot::incoming`] request.
    pub fn respond(&self, id: u64, accept: bool) {
        let p = self.inner.pending.lock().unwrap_or_else(|e| e.into_inner()).take();
        match p {
            Some((pid, _, tx)) if pid == id => {
                let _ = tx.send(if accept { Decision::Accept } else { Decision::Decline });
            }
            other => *self.inner.pending.lock().unwrap_or_else(|e| e.into_inner()) = other,
        }
    }

    /// Sends files and folders (folders with everything inside) to a device. Returns the transfer id.
    pub fn send(&self, device: &Device, items: Vec<PathBuf>) -> u64 {
        send::start(&self.inner, device.clone(), send::Payload::Files(items))
    }

    pub fn send_text(&self, device: &Device, text: String) -> u64 {
        send::start(&self.inner, device.clone(), send::Payload::Text(text))
    }

    /// Stops a running transfer in either direction.
    pub fn cancel(&self, id: u64) {
        if let Some(flag) = self.inner.cancelled.lock().unwrap_or_else(|e| e.into_inner()).get(&id) {
            flag.store(true, Ordering::Relaxed);
        }
        server::cancel_local(&self.inner, id);
    }

    /// Removes finished transfers from the list.
    pub fn clear_finished(&self) {
        self.inner.snap().transfers.retain(|t| !t.state.finished());
    }

    pub fn set_receive_dir(&self, dir: PathBuf) {
        self.inner.cfg.lock().unwrap_or_else(|e| e.into_inner()).receive_dir = dir.clone();
        self.inner.snap().receive_dir = dir;
    }

    pub fn set_quick_save(&self, on: bool) {
        self.inner.cfg.lock().unwrap_or_else(|e| e.into_inner()).quick_save = on;
        self.inner.snap().quick_save = on;
    }

    pub fn set_alias(&self, alias: String) {
        self.inner.cfg.lock().unwrap_or_else(|e| e.into_inner()).alias = alias.clone();
        self.inner.snap().alias = alias;
        discovery::announce(&self.inner);
    }
}

impl Drop for Share {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::Relaxed);
        if let Some((_, _, tx)) = self.inner.pending.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = tx.send(Decision::Decline);
        }
        for flag in self.inner.cancelled.lock().unwrap_or_else(|e| e.into_inner()).values() {
            flag.store(true, Ordering::Relaxed);
        }
        // Wake the accept loop so it sees `stop`.
        let _ = std::net::TcpStream::connect_timeout(
            &SocketAddr::from((Ipv4Addr::LOCALHOST, self.port())),
            std::time::Duration::from_millis(200),
        );
    }
}

/// The IPv4 addresses other devices can reach this PC on (no loopback or link-local).
pub fn local_ipv4() -> Vec<(String, Ipv4Addr, u8)> {
    let mut v: Vec<_> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| i.is_oper_up())
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(a) if !a.ip.is_loopback() && !a.ip.is_link_local() => {
                Some((i.name, a.ip, a.prefixlen))
            }
            _ => None,
        })
        .collect();
    v.sort_by_key(|(_, ip, _)| *ip);
    v.dedup_by_key(|(_, ip, _)| *ip);
    v
}

/// A MIME type from the extension, as LocalSend expects in `fileType`.
pub fn mime_of(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "3gp" => "video/3gpp",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "ogg" | "opus" => "audio/ogg",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "7z" => "application/x-7z-compressed",
        "rar" => "application/vnd.rar",
        "apk" => "application/vnd.android.package-archive",
        "txt" | "log" | "md" | "csv" => "text/plain",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}

pub(crate) fn random_token() -> String {
    use ring::rand::SecureRandom;
    let mut b = [0u8; 16];
    let _ = ring::rand::SystemRandom::new().fill(&mut b);
    identity::hex_upper(&b).to_ascii_lowercase()
}

/// Random lowercase letters and digits without look-alikes (no 0/o, 1/l/i), for passwords a
/// person may have to type.
pub fn random_password(len: usize) -> String {
    use ring::rand::SecureRandom;
    const SET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut b = vec![0u8; len];
    let _ = ring::rand::SystemRandom::new().fill(&mut b);
    b.iter().map(|x| SET[*x as usize % SET.len()] as char).collect()
}
