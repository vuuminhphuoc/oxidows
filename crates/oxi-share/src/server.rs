//! The receiving side: a HTTP(S) server with the LocalSend v2 routes. TLS and plain HTTP share the
//! port (a TLS connection starts with byte 0x16), so a browser can open `http://<pc>:53317`.

use std::collections::HashMap;
use std::fs::{self, File, FileTimes};
use std::io::{self, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use crate::dto::{DeviceInfo, PrepareUploadRequest, PrepareUploadResponse, Protocol};
use crate::http::{self, Body, Head, Stream};
use crate::identity::fingerprint_of;
use crate::{Decision, Device, DeviceType, Direction, Incoming, Inner, State, Transfer, TransferFile, names};

/// How long a transfer request waits for the user before it is declined.
const DECISION_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Connections served at once.
const MAX_CONNECTIONS: usize = 64;
/// A session no file arrived for in this long no longer blocks new ones (the sender vanished).
const SESSION_IDLE: Duration = Duration::from_secs(3 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileState {
    Pending,
    InProgress,
    Done,
    Failed,
}

pub(crate) struct SessionFile {
    dto: crate::FileDto,
    token: String,
    /// Index in the transfer's file list.
    index: usize,
    state: FileState,
    path: Option<PathBuf>,
}

pub(crate) struct Session {
    id: String,
    transfer: u64,
    sender: IpAddr,
    files: HashMap<String, SessionFile>,
    cancelled: Arc<AtomicBool>,
    last_activity: Instant,
}

impl Session {
    fn busy(&self) -> bool {
        self.files.values().any(|f| f.state == FileState::InProgress) || self.last_activity.elapsed() < SESSION_IDLE
    }
}

pub(crate) fn start(inner: &Arc<Inner>) {
    let want = inner.port.load(Ordering::Relaxed) as u16;
    let mut bound = None;
    let mut err = String::new();
    for port in want..want.saturating_add(10) {
        match TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))) {
            Ok(l) => {
                bound = Some((l, port));
                break;
            }
            Err(e) => err = e.to_string(),
        }
    }
    let Some((listener, port)) = bound else {
        inner.snap().listening = Some(Err(err));
        return;
    };
    inner.port.store(u64::from(port), Ordering::Relaxed);
    inner.snap().listening = Some(Ok(port));
    let tls = match crate::tls::server_config(&inner.id) {
        Ok(c) => c,
        Err(e) => {
            inner.snap().listening = Some(Err(e.to_string()));
            return;
        }
    };
    let inner = inner.clone();
    thread::Builder::new()
        .name("share-accept".into())
        .spawn(move || {
            let open = Arc::new(AtomicUsize::new(0));
            for conn in listener.incoming() {
                if inner.stop.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(tcp) = conn else { continue };
                // Bounded, so a misbehaving device on the network cannot pile up threads.
                if open.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                    continue;
                }
                open.fetch_add(1, Ordering::Relaxed);
                let (inner, tls, held) = (inner.clone(), tls.clone(), open.clone());
                let spawned = thread::Builder::new().name("share-conn".into()).spawn(move || {
                    let _ = serve(&inner, tcp, tls);
                    held.fetch_sub(1, Ordering::Relaxed);
                });
                if spawned.is_err() {
                    open.fetch_sub(1, Ordering::Relaxed);
                }
            }
        })
        .ok();
}

fn serve(inner: &Arc<Inner>, tcp: TcpStream, tls: Arc<rustls::ServerConfig>) -> io::Result<()> {
    let peer = tcp.peer_addr()?.ip();
    let peer = match peer {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(peer),
        v4 => v4,
    };
    tcp.set_nodelay(true)?;
    tcp.set_read_timeout(Some(Duration::from_secs(120)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(60)))?;
    let mut first = [0u8; 1];
    if tcp.peek(&mut first)? == 0 {
        return Ok(());
    }
    let stream = if first[0] == 0x16 {
        let conn = rustls::ServerConnection::new(tls).map_err(io::Error::other)?;
        let mut s = rustls::StreamOwned::new(conn, tcp);
        while s.conn.is_handshaking() {
            s.conn.complete_io(&mut s.sock)?;
        }
        Stream::TlsServer(Box::new(s))
    } else {
        Stream::Plain(tcp)
    };
    let cert_fp = stream.peer_cert().map(|c| fingerprint_of(&c));
    let tls_on = !matches!(stream, Stream::Plain(_));
    let mut r = BufReader::with_capacity(256 * 1024, stream);
    while !inner.stop.load(Ordering::Relaxed) {
        let Some(head) = http::read_head(&mut r)? else { return Ok(()) };
        let kind = head.body_kind()?;
        let mut body = Body::new(&mut r, kind);
        let reply = route(inner, &head, &mut body, peer, cert_fp.as_deref(), tls_on);
        let finished = body.is_done() || body.drain(1 << 20);
        let keep = finished && head.keep_alive();
        let w = r.get_mut();
        match reply {
            Reply::Json(code, v) => {
                http::write_response(w, code, "application/json", &[], v.to_string().as_bytes(), keep)?
            }
            Reply::Empty(code) => http::write_response(w, code, "", &[], b"", keep)?,
            Reply::Html(html) => http::write_response(w, 200, "text/html; charset=utf-8", &[], html.as_bytes(), keep)?,
            Reply::Error(code, msg) => {
                let v = serde_json::json!({ "message": msg });
                http::write_response(w, code, "application/json", &[], v.to_string().as_bytes(), keep)?
            }
        }
        if !keep {
            return Ok(());
        }
    }
    Ok(())
}

enum Reply {
    Json(u16, serde_json::Value),
    Empty(u16),
    Html(String),
    Error(u16, &'static str),
}

fn route<R: io::BufRead>(
    inner: &Arc<Inner>,
    head: &Head,
    body: &mut Body<'_, R>,
    peer: IpAddr,
    cert_fp: Option<&str>,
    tls: bool,
) -> Reply {
    let method = head.first.as_str();
    match (method, head.path()) {
        ("POST", "/api/localsend/v2/register") => register(inner, body, peer, cert_fp, tls),
        ("GET" | "POST", "/api/localsend/v2/info") => Reply::Json(200, info_json(inner)),
        ("POST", "/api/localsend/v2/prepare-upload") => prepare_upload(inner, body, peer, cert_fp),
        ("POST", "/api/localsend/v2/upload") => upload(inner, head, body, peer),
        ("POST", "/api/localsend/v2/cancel") => cancel(inner, head, peer),
        ("POST", "/api/localsend/v2/prepare-download") | ("GET", "/api/localsend/v2/download") => {
            Reply::Error(403, "Rejected")
        }
        ("GET", "/" | "/index.html") => Reply::Html(landing_page(inner)),
        (_, p) if p.starts_with("/api/localsend/") => Reply::Error(404, "Not found"),
        _ => Reply::Error(404, "Not found"),
    }
}

fn info_json(inner: &Inner) -> serde_json::Value {
    let mut i = inner.info(None);
    i.port = None;
    i.protocol = None;
    serde_json::to_value(i).unwrap_or_default()
}

fn read_json<T: serde::de::DeserializeOwned, R: io::BufRead>(body: &mut Body<'_, R>) -> Option<T> {
    let bytes = body.read_all(http::MAX_JSON).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn register<R: io::BufRead>(
    inner: &Arc<Inner>,
    body: &mut Body<'_, R>,
    peer: IpAddr,
    cert_fp: Option<&str>,
    tls: bool,
) -> Reply {
    let Some(info) = read_json::<DeviceInfo, _>(body) else { return Reply::Error(400, "Invalid body") };
    // Over TLS the certificate is the identity; a body that claims another one is ignored.
    let fingerprint = match cert_fp {
        Some(fp) if tls => fp.to_string(),
        _ => info.fingerprint.to_ascii_uppercase(),
    };
    let claimed_ok = cert_fp.is_none_or(|fp| info.fingerprint.is_empty() || info.fingerprint.eq_ignore_ascii_case(fp));
    if claimed_ok && !fingerprint.is_empty() {
        inner.found(Device {
            fingerprint,
            alias: info.alias.clone(),
            device_model: info.device_model.clone(),
            device_type: info.device_type.unwrap_or_default(),
            ip: peer,
            port: info.port.unwrap_or(crate::DEFAULT_PORT),
            protocol: info.protocol.unwrap_or(Protocol::Https),
            version: info.version.clone(),
            last_seen: Instant::now(),
        });
    }
    Reply::Json(200, info_json(inner))
}

fn prepare_upload<R: io::BufRead>(
    inner: &Arc<Inner>,
    body: &mut Body<'_, R>,
    peer: IpAddr,
    cert_fp: Option<&str>,
) -> Reply {
    let Some(req) = read_json::<PrepareUploadRequest, _>(body) else { return Reply::Error(400, "Invalid body") };
    if req.files.is_empty() {
        return Reply::Error(400, "No files provided");
    }
    {
        let session = inner.session.lock().unwrap_or_else(|e| e.into_inner());
        if session.as_ref().is_some_and(Session::busy) {
            return Reply::Error(409, "Blocked by another session");
        }
    }
    // A sender we have not seen yet becomes a known device too, so the user can answer back.
    if let Some(fp) = cert_fp {
        inner.found(Device {
            fingerprint: fp.to_string(),
            alias: req.info.alias.clone(),
            device_model: req.info.device_model.clone(),
            device_type: req.info.device_type.unwrap_or_default(),
            ip: peer,
            port: req.info.port.unwrap_or(crate::DEFAULT_PORT),
            protocol: req.info.protocol.unwrap_or(Protocol::Https),
            version: req.info.version.clone(),
            last_seen: Instant::now(),
        });
    }

    let mut files: Vec<crate::FileDto> = req.files.into_values().collect();
    files.sort_by_key(|f| f.file_name.to_lowercase());
    let message = match files.as_slice() {
        [f] if f.file_type.starts_with("text/") => f.preview.clone(),
        _ => None,
    };
    let id = inner.next_id();
    let incoming = Incoming {
        id,
        alias: req.info.alias.clone(),
        device_model: req.info.device_model.clone(),
        device_type: req.info.device_type.unwrap_or(DeviceType::Mobile),
        ip: peer,
        files: files.clone(),
        message: message.clone(),
    };

    let decision = if inner.cfg().quick_save && message.is_none() {
        Decision::Accept
    } else {
        let (tx, rx) = sync_channel(1);
        {
            // One question at a time: checked and taken under the same lock.
            let mut p = inner.pending.lock().unwrap_or_else(|e| e.into_inner());
            if p.is_some() {
                return Reply::Error(409, "Blocked by another session");
            }
            *p = Some((id, peer, tx));
        }
        inner.snap().incoming = Some(incoming);
        inner.repaint();
        let deadline = Instant::now() + DECISION_TIMEOUT;
        let d = loop {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(d) => break d,
                Err(RecvTimeoutError::Disconnected) => break Decision::Decline,
                Err(RecvTimeoutError::Timeout) if inner.stop.load(Ordering::Relaxed) || Instant::now() > deadline => {
                    break Decision::Decline;
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        };
        {
            let mut p = inner.pending.lock().unwrap_or_else(|e| e.into_inner());
            if p.as_ref().is_some_and(|(pid, _, _)| *pid == id) {
                *p = None;
            }
        }
        let mut s = inner.snap();
        if s.incoming.as_ref().is_some_and(|i| i.id == id) {
            s.incoming = None;
        }
        drop(s);
        inner.repaint();
        d
    };

    match decision {
        Decision::Decline | Decision::Cancelled => Reply::Error(403, "Rejected"),
        Decision::Accept if message.is_some() => {
            inner.snap().transfers.push(Transfer {
                id,
                direction: Direction::Receive,
                peer: req.info.alias,
                files: Vec::new(),
                state: State::Done,
                started: Instant::now(),
                finished: Some(Instant::now()),
                message,
            });
            inner.repaint();
            // "Read, nothing to upload."
            Reply::Empty(204)
        }
        Decision::Accept => {
            let session_id = crate::random_token();
            let mut tokens = HashMap::new();
            let mut sfiles = HashMap::new();
            let mut tfiles = Vec::new();
            for (index, f) in files.into_iter().enumerate() {
                let token = crate::random_token();
                tokens.insert(f.id.clone(), token.clone());
                tfiles.push(TransferFile {
                    name: f.file_name.clone(),
                    size: f.size,
                    done: 0,
                    state: State::Waiting,
                    path: None,
                });
                sfiles
                    .insert(f.id.clone(), SessionFile { dto: f, token, index, state: FileState::Pending, path: None });
            }
            let cancelled = Arc::new(AtomicBool::new(false));
            let stale = {
                let mut slot = inner.session.lock().unwrap_or_else(|e| e.into_inner());
                // Another transfer may have been accepted meanwhile (Quick Save takes no question).
                if slot.as_ref().is_some_and(Session::busy) {
                    return Reply::Error(409, "Blocked by another session");
                }
                slot.replace(Session {
                    id: session_id.clone(),
                    transfer: id,
                    sender: peer,
                    files: sfiles,
                    cancelled: cancelled.clone(),
                    last_activity: Instant::now(),
                })
            };
            // A session whose sender vanished ends as failed.
            if let Some(old) = stale {
                old.cancelled.store(true, Ordering::Relaxed);
                inner.update_transfer(old.transfer, |t| {
                    if !t.state.finished() {
                        t.state = State::Failed("The sender stopped".into());
                    }
                });
            }
            inner.cancelled.lock().unwrap_or_else(|e| e.into_inner()).insert(id, cancelled);
            inner.snap().transfers.push(Transfer {
                id,
                direction: Direction::Receive,
                peer: req.info.alias,
                files: tfiles,
                state: State::Running,
                started: Instant::now(),
                finished: None,
                message: None,
            });
            inner.repaint();
            let resp = PrepareUploadResponse { session_id, files: tokens };
            Reply::Json(200, serde_json::to_value(resp).unwrap_or_default())
        }
    }
}

struct Slot {
    transfer: u64,
    index: usize,
    path: PathBuf,
    size: u64,
    sha256: Option<String>,
    times: FileTimes,
    cancelled: Arc<AtomicBool>,
}

fn upload<R: io::BufRead>(inner: &Arc<Inner>, head: &Head, body: &mut Body<'_, R>, peer: IpAddr) -> Reply {
    let q = head.query();
    let (Some(sid), Some(fid), Some(token)) = (q.get("sessionId"), q.get("fileId"), q.get("token")) else {
        return Reply::Error(400, "Missing parameters");
    };
    let receive_dir = inner.cfg().receive_dir;
    let slot = {
        let mut guard = inner.session.lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = guard.as_mut() else { return Reply::Error(403, "Invalid token or IP address") };
        if s.id != *sid || s.sender != peer {
            return Reply::Error(403, "Invalid token or IP address");
        }
        let taken: Vec<PathBuf> = s.files.values().filter_map(|f| f.path.clone()).collect();
        let Some(f) = s.files.get_mut(fid) else { return Reply::Error(403, "Invalid token or IP address") };
        if f.token != *token || !matches!(f.state, FileState::Pending | FileState::Failed) {
            return Reply::Error(403, "Invalid token or IP address");
        }
        let want = receive_dir.join(names::safe_relative(&f.dto.file_name));
        let path = names::unique_path(&want, &|p: &Path| taken.iter().any(|t| t == p));
        f.state = FileState::InProgress;
        f.path = Some(path.clone());
        s.last_activity = Instant::now();
        let mut times = FileTimes::new();
        if let Some(m) = &f.dto.metadata {
            if let Some(t) = m.modified.as_deref().and_then(crate::time::parse) {
                times = times.set_modified(t);
            }
            if let Some(t) = m.accessed.as_deref().and_then(crate::time::parse) {
                times = times.set_accessed(t);
            }
        }
        Slot {
            transfer: s.transfer,
            index: f.index,
            path,
            size: f.dto.size,
            sha256: f.dto.sha256.clone().filter(|h| h.len() == 64),
            times,
            cancelled: s.cancelled.clone(),
        }
    };
    let (t, i) = (slot.transfer, slot.index);
    inner.update_transfer(t, |tr| {
        if let Some(f) = tr.files.get_mut(i) {
            f.state = State::Running;
            f.path = Some(slot.path.clone());
        }
    });
    inner.repaint();

    let result = save(inner, body, &slot);
    let ok = result.is_ok();
    if !ok {
        let _ = fs::remove_file(&slot.path);
    }
    inner.update_transfer(t, |tr| {
        if let Some(f) = tr.files.get_mut(i) {
            f.state = match &result {
                Ok(()) => State::Done,
                Err(e) => State::Failed(e.to_string()),
            };
            if ok {
                f.done = f.size;
            }
        }
    });
    finish_file(inner, sid, fid, ok);
    inner.repaint();
    match result {
        Ok(()) => Reply::Empty(200),
        Err(e) if e.kind() == io::ErrorKind::InvalidData => Reply::Error(422, "Checksum mismatch"),
        Err(_) => Reply::Error(500, "Could not save the file"),
    }
}

fn save<R: io::BufRead>(inner: &Inner, body: &mut Body<'_, R>, slot: &Slot) -> io::Result<()> {
    if let Some(parent) = slot.path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = File::create_new(&slot.path)?;
    let mut hash = slot.sha256.as_ref().map(|_| ring::digest::Context::new(&ring::digest::SHA256));
    let mut buf = vec![0u8; 256 * 1024];
    let mut got = 0u64;
    let mut last = Instant::now();
    loop {
        if slot.cancelled.load(Ordering::Relaxed) || inner.stop.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = body.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if got + n as u64 > slot.size {
            return Err(io::Error::other("more data than announced"));
        }
        file.write_all(&buf[..n])?;
        if let Some(h) = hash.as_mut() {
            h.update(&buf[..n]);
        }
        got += n as u64;
        if last.elapsed() > Duration::from_millis(100) {
            last = Instant::now();
            inner.update_transfer(slot.transfer, |t| {
                if let Some(f) = t.files.get_mut(slot.index) {
                    f.done = got;
                }
            });
            inner.repaint();
        }
    }
    if got < slot.size {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the sender stopped before the end"));
    }
    if let (Some(h), Some(want)) = (hash, &slot.sha256)
        && !crate::identity::hex_upper(h.finish().as_ref()).eq_ignore_ascii_case(want)
    {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "checksum mismatch"));
    }
    let _ = file.set_times(slot.times);
    file.sync_all().or(Ok(()))
}

/// Marks a file done and closes the session (and transfer) after its last file.
fn finish_file(inner: &Inner, sid: &str, fid: &str, ok: bool) {
    let mut guard = inner.session.lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = guard.as_mut().filter(|s| s.id == sid) else { return };
    if let Some(f) = s.files.get_mut(fid) {
        f.state = if ok { FileState::Done } else { FileState::Failed };
    }
    s.last_activity = Instant::now();
    if s.files.values().all(|f| matches!(f.state, FileState::Done | FileState::Failed)) {
        let failed = s.files.values().filter(|f| f.state == FileState::Failed).count();
        let cancelled = s.cancelled.load(Ordering::Relaxed);
        let t = s.transfer;
        *guard = None;
        drop(guard);
        inner.cancelled.lock().unwrap_or_else(|e| e.into_inner()).remove(&t);
        inner.update_transfer(t, |tr| {
            if !tr.state.finished() {
                tr.state = match (cancelled, failed) {
                    (true, _) => State::Cancelled,
                    (_, 0) => State::Done,
                    (_, n) => State::Failed(format!("{n} file(s) failed")),
                };
            }
        });
    }
}

fn cancel(inner: &Arc<Inner>, head: &Head, peer: IpAddr) -> Reply {
    let sid = head.query().get("sessionId").cloned();
    // Still waiting for the user: the sender has no session id yet.
    {
        let mut p = inner.pending.lock().unwrap_or_else(|e| e.into_inner());
        if p.as_ref().is_some_and(|(_, ip, _)| *ip == peer)
            && let Some((_, _, tx)) = p.take()
        {
            let _ = tx.send(Decision::Cancelled);
        }
    }
    let mut guard = inner.session.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = guard.as_ref()
        && s.sender == peer
        && sid.as_deref().is_none_or(|id| id == s.id)
    {
        s.cancelled.store(true, Ordering::Relaxed);
        let t = s.transfer;
        *guard = None;
        drop(guard);
        inner.update_transfer(t, |tr| tr.state = State::Cancelled);
        inner.repaint();
    }
    Reply::Empty(200)
}

/// The user stopped an incoming transfer.
pub(crate) fn cancel_local(inner: &Inner, transfer: u64) {
    let mut guard = inner.session.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = guard.as_ref().filter(|s| s.transfer == transfer) {
        s.cancelled.store(true, Ordering::Relaxed);
        *guard = None;
        drop(guard);
        inner.update_transfer(transfer, |tr| {
            if !tr.state.finished() {
                tr.state = State::Cancelled;
            }
        });
        inner.repaint();
    }
}

fn landing_page(inner: &Inner) -> String {
    let alias = inner.cfg().alias;
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width'>\
         <title>{a}</title><body style='font-family:system-ui;margin:2em'><h2>{a}</h2>\
         <p>This PC receives files with the free <b>LocalSend</b> app (localsend.org). \
         Open LocalSend on your phone; this PC appears under <i>Send</i>.</p>",
        a = esc(&alias)
    )
}
