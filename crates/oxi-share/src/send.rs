//! The sending side: prepare-upload, wait for the receiver's answer, then upload the accepted files
//! (a few at a time, each on its own connection).

use std::fs::{self, File};
use std::io::{self, Read};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::dto::{FileDto, FileMetadata, PrepareUploadRequest, PrepareUploadResponse, Protocol};
use crate::http;
use crate::{Device, Direction, Inner, State, Transfer, TransferFile};

/// Files uploaded at the same time.
const PARALLEL: usize = 3;

pub(crate) enum Payload {
    Files(Vec<PathBuf>),
    Text(String),
}

/// One request to a peer on a fresh connection.
#[allow(clippy::too_many_arguments)]
pub(crate) fn call(
    inner: &Inner,
    addr: SocketAddr,
    protocol: Protocol,
    expected: Option<&str>,
    path: &str,
    body: &[u8],
    connect_timeout: Duration,
    read_timeout: Duration,
) -> io::Result<http::Response> {
    let tls = match protocol {
        Protocol::Https => Some(crate::tls::client_config(&inner.id, expected).map_err(io::Error::other)?),
        Protocol::Http => None,
    };
    let mut s = http::connect(addr, tls, connect_timeout)?;
    s.tcp().set_read_timeout(Some(read_timeout))?;
    let host = addr.to_string();
    http::request(&mut s, "POST", &host, path, "application/json", &mut &body[..], body.len() as u64, &mut |_| true)
}

struct Item {
    dto: FileDto,
    /// `None` for a text message, whose bytes are the preview.
    path: Option<PathBuf>,
}

/// Every file under the selection, with names relative to the selection's folder ("Album/1.jpg").
fn collect(paths: &[PathBuf]) -> Vec<Item> {
    fn walk(path: &Path, rel: String, out: &mut Vec<Item>) {
        let Ok(meta) = fs::metadata(path) else { return };
        if meta.is_dir() {
            let Ok(rd) = fs::read_dir(path) else { return };
            let mut children: Vec<_> = rd.flatten().collect();
            children.sort_by_key(|e| e.file_name());
            for c in children {
                let name = c.file_name().to_string_lossy().into_owned();
                walk(&c.path(), format!("{rel}/{name}"), out);
            }
            return;
        }
        let metadata = FileMetadata {
            modified: meta.modified().ok().map(crate::time::format),
            accessed: meta.accessed().ok().map(crate::time::format),
        };
        out.push(Item {
            dto: FileDto {
                id: crate::random_token(),
                file_type: crate::mime_of(&rel).into(),
                file_name: rel,
                size: meta.len(),
                sha256: None,
                preview: None,
                metadata: Some(metadata),
            },
            path: Some(path.to_path_buf()),
        });
    }
    let mut out = Vec::new();
    for p in paths {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        walk(p, name, &mut out);
    }
    out
}

pub(crate) fn start(inner: &Arc<Inner>, device: Device, payload: Payload) -> u64 {
    let id = inner.next_id();
    let (items, message) = match payload {
        Payload::Files(paths) => (collect(&paths), None),
        Payload::Text(text) => {
            let dto = FileDto {
                id: crate::random_token(),
                file_name: format!("{}.txt", crate::random_token()),
                size: text.len() as u64,
                file_type: "text/plain".into(),
                sha256: None,
                preview: Some(text.clone()),
                metadata: None,
            };
            (vec![Item { dto, path: None }], Some(text))
        }
    };
    let files = items
        .iter()
        .map(|i| TransferFile {
            name: i.dto.file_name.clone(),
            size: i.dto.size,
            done: 0,
            state: State::Waiting,
            path: i.path.clone(),
        })
        .collect();
    let cancel = Arc::new(AtomicBool::new(false));
    inner.cancelled.lock().unwrap_or_else(|e| e.into_inner()).insert(id, cancel.clone());
    inner.snap().transfers.push(Transfer {
        id,
        direction: Direction::Send,
        peer: device.alias.clone(),
        files,
        state: State::Waiting,
        started: Instant::now(),
        finished: None,
        message,
    });
    inner.repaint();
    let inner = inner.clone();
    thread::Builder::new()
        .name("share-send".into())
        .spawn(move || {
            let state = if items.is_empty() {
                State::Failed("Nothing to send".into())
            } else {
                match run(&inner, &device, id, items, &cancel) {
                    Ok(s) => s,
                    Err(_) if cancel.load(Ordering::Relaxed) => State::Cancelled,
                    Err(e) => State::Failed(e.to_string()),
                }
            };
            inner.update_transfer(id, |t| {
                t.state = state;
                for f in &mut t.files {
                    if !f.state.finished() {
                        f.state = t.state.clone();
                    }
                }
            });
            inner.cancelled.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            inner.repaint();
        })
        .ok();
    id
}

fn run(inner: &Arc<Inner>, device: &Device, id: u64, items: Vec<Item>, cancel: &Arc<AtomicBool>) -> io::Result<State> {
    let expected = (device.protocol == Protocol::Https).then_some(device.fingerprint.as_str());
    let req = PrepareUploadRequest {
        info: inner.info(None),
        files: items.iter().map(|i| (i.dto.id.clone(), i.dto.clone())).collect(),
    };
    let body = serde_json::to_vec(&req).map_err(io::Error::other)?;

    // The receiver's user decides while this request is open; a cancel here closes the socket.
    let tls = match device.protocol {
        Protocol::Https => Some(crate::tls::client_config(&inner.id, expected).map_err(io::Error::other)?),
        Protocol::Http => None,
    };
    let mut s = http::connect(device.addr(), tls, Duration::from_secs(5))?;
    s.tcp().set_read_timeout(Some(Duration::from_secs(1)))?;
    let tcp = s.tcp().try_clone()?;
    let done = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (cancel, done, inner2) = (cancel.clone(), done.clone(), inner.clone());
        thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                if cancel.load(Ordering::Relaxed) || inner2.stop.load(Ordering::Relaxed) {
                    let _ = tcp.shutdown(std::net::Shutdown::Both);
                    return;
                }
                thread::sleep(Duration::from_millis(200));
            }
        })
    };
    let host = device.addr().to_string();
    // Long waits are fine: the read timeout only paces the cancel check through retries below.
    s.tcp().set_read_timeout(Some(Duration::from_secs(15 * 60)))?;
    let resp = http::request(
        &mut s,
        "POST",
        &host,
        "/api/localsend/v2/prepare-upload",
        "application/json",
        &mut &body[..],
        body.len() as u64,
        &mut |_| true,
    );
    done.store(true, Ordering::Relaxed);
    let _ = watcher.join();
    if cancel.load(Ordering::Relaxed) {
        // Tell the receiver to close its dialog.
        let _ = call(
            inner,
            device.addr(),
            device.protocol,
            expected,
            "/api/localsend/v2/cancel",
            b"",
            Duration::from_secs(3),
            Duration::from_secs(3),
        );
        return Ok(State::Cancelled);
    }
    let resp = resp?;
    match resp.status {
        200 => {}
        204 => {
            // Nothing to upload (a text message was read).
            inner.update_transfer(id, |t| {
                t.files.iter_mut().for_each(|f| {
                    f.done = f.size;
                    f.state = State::Done;
                })
            });
            return Ok(State::Done);
        }
        401 => return Err(io::Error::other("The receiver asks for a PIN, which is not supported yet")),
        403 => return Ok(State::Declined),
        409 => return Err(io::Error::other("The receiver is busy with another transfer")),
        429 => return Err(io::Error::other("Too many requests, try again shortly")),
        c => return Err(io::Error::other(format!("The receiver answered HTTP {c}"))),
    }
    let accepted: PrepareUploadResponse = serde_json::from_slice(&resp.body).map_err(io::Error::other)?;
    inner.update_transfer(id, |t| t.state = State::Running);
    inner.repaint();

    // Files the receiver did not pick are skipped.
    let jobs: Vec<(usize, Item, String)> = items
        .into_iter()
        .enumerate()
        .filter_map(|(i, it)| accepted.files.get(&it.dto.id).cloned().map(|tok| (i, it, tok)))
        .collect();
    let accepted_idx: Vec<usize> = jobs.iter().map(|(i, _, _)| *i).collect();
    inner.update_transfer(id, |t| {
        for (i, f) in t.files.iter_mut().enumerate() {
            if !accepted_idx.contains(&i) {
                f.state = State::Declined;
            }
        }
    });
    let jobs = Arc::new(Mutex::new(jobs.into_iter()));
    let failures = Arc::new(Mutex::new(Vec::<String>::new()));
    let workers: Vec<_> = (0..PARALLEL)
        .map(|_| {
            let (inner, device, jobs, failures, cancel) =
                (inner.clone(), device.clone(), jobs.clone(), failures.clone(), cancel.clone());
            let session = accepted.session_id.clone();
            thread::spawn(move || {
                loop {
                    let next = jobs.lock().unwrap_or_else(|e| e.into_inner()).next();
                    let Some((index, item, token)) = next else { break };
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let r = upload_one(&inner, &device, &session, id, index, &item, &token, &cancel);
                    inner.update_transfer(id, |t| {
                        if let Some(f) = t.files.get_mut(index) {
                            f.state = match &r {
                                Ok(()) => State::Done,
                                Err(e) => State::Failed(e.to_string()),
                            };
                        }
                    });
                    inner.repaint();
                    if let Err(e) = r {
                        failures.lock().unwrap_or_else(|e| e.into_inner()).push(e.to_string());
                    }
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
    if cancel.load(Ordering::Relaxed) {
        let path = format!("/api/localsend/v2/cancel?sessionId={}", http::percent_encode(&accepted.session_id));
        let _ = call(
            inner,
            device.addr(),
            device.protocol,
            expected,
            &path,
            b"",
            Duration::from_secs(3),
            Duration::from_secs(3),
        );
        return Ok(State::Cancelled);
    }
    let failures = failures.lock().unwrap_or_else(|e| e.into_inner());
    Ok(match failures.len() {
        0 => State::Done,
        1 => State::Failed(failures[0].clone()),
        n => State::Failed(format!("{n} files failed: {}", failures[0])),
    })
}

#[allow(clippy::too_many_arguments)]
fn upload_one(
    inner: &Inner,
    device: &Device,
    session: &str,
    id: u64,
    index: usize,
    item: &Item,
    token: &str,
    cancel: &AtomicBool,
) -> io::Result<()> {
    inner.update_transfer(id, |t| {
        if let Some(f) = t.files.get_mut(index) {
            f.state = State::Running;
        }
    });
    let path = format!(
        "/api/localsend/v2/upload?sessionId={}&fileId={}&token={}",
        http::percent_encode(session),
        http::percent_encode(&item.dto.id),
        http::percent_encode(token)
    );
    let mut reader: Box<dyn Read> = match &item.path {
        Some(p) => Box::new(File::open(p)?),
        None => Box::new(io::Cursor::new(item.dto.preview.clone().unwrap_or_default().into_bytes())),
    };
    let tls = match device.protocol {
        Protocol::Https => {
            Some(crate::tls::client_config(&inner.id, Some(&device.fingerprint)).map_err(io::Error::other)?)
        }
        Protocol::Http => None,
    };
    let mut s = http::connect(device.addr(), tls, Duration::from_secs(5))?;
    s.tcp().set_read_timeout(Some(Duration::from_secs(120)))?;
    let host = device.addr().to_string();
    let mut last = Instant::now();
    let resp = http::request(
        &mut s,
        "POST",
        &host,
        &path,
        "application/octet-stream",
        &mut reader,
        item.dto.size,
        &mut |sent| {
            if last.elapsed() > Duration::from_millis(100) {
                last = Instant::now();
                inner.update_transfer(id, |t| {
                    if let Some(f) = t.files.get_mut(index) {
                        f.done = sent;
                    }
                });
                inner.repaint();
            }
            !cancel.load(Ordering::Relaxed) && !inner.stop.load(Ordering::Relaxed)
        },
    )?;
    match resp.status {
        200 | 204 => {
            inner.update_transfer(id, |t| {
                if let Some(f) = t.files.get_mut(index) {
                    f.done = f.size;
                }
            });
            Ok(())
        }
        422 => Err(io::Error::other("The file arrived damaged (checksum mismatch)")),
        403 => Err(io::Error::other("The receiver stopped the transfer")),
        c => Err(io::Error::other(format!("The receiver answered HTTP {c}"))),
    }
}
