//! Two services on this machine talk the protocol to each other over 127.0.0.1, plus raw requests
//! shaped like the official app's (chunked upload bodies, client certificates, cancels).

use std::fs;
use std::io::{BufReader, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oxi_share::{Config, Direction, Share, State, http};

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("oxi-share-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn service(name: &str, port: u16, quick_save: bool) -> (Share, PathBuf) {
    let root = temp(name);
    let cfg = Config {
        alias: name.into(),
        device_model: Some("Test".into()),
        port,
        identity_dir: root.join("id"),
        receive_dir: root.join("in"),
        quick_save,
        multicast: false,
    };
    (Share::start(cfg, Arc::new(|| {})).unwrap(), root)
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

const LOCAL: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

#[test]
fn send_files_and_folders_both_ways() {
    let (a, root_a) = service("alice", 53417, false);
    let (b, root_b) = service("bob", 53427, true);
    a.probe(LOCAL, b.port());
    wait_for("alice to see bob", || a.snapshot().devices.iter().any(|d| d.alias == "bob"));
    // Registering also told bob about alice.
    wait_for("bob to see alice", || b.snapshot().devices.iter().any(|d| d.alias == "alice"));
    let bob = a.snapshot().devices.into_iter().find(|d| d.alias == "bob").unwrap();
    assert_eq!(bob.fingerprint, b.fingerprint());

    let src = root_a.join("out");
    fs::create_dir_all(src.join("Album/sub")).unwrap();
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    fs::write(src.join("Album/sub/big.bin"), &big).unwrap();
    fs::write(src.join("Album/Ảnh 1.jpg"), b"jpeg").unwrap();
    fs::write(src.join("empty.txt"), b"").unwrap();
    // A name already present on the receiver gets a number.
    fs::create_dir_all(root_b.join("in")).unwrap();
    fs::write(root_b.join("in/empty.txt"), b"old").unwrap();

    let id = a.send(&bob, vec![src.join("Album"), src.join("empty.txt")]);
    wait_for("the transfer", || a.snapshot().transfers.iter().any(|t| t.id == id && t.state.finished()));
    let t = a.snapshot().transfers.into_iter().find(|t| t.id == id).unwrap();
    assert_eq!(t.state, State::Done, "{t:?}");
    assert_eq!(t.done(), t.total());
    let inbox = root_b.join("in");
    assert_eq!(fs::read(inbox.join("Album/sub/big.bin")).unwrap(), big);
    assert_eq!(fs::read(inbox.join("Album/Ảnh 1.jpg")).unwrap(), b"jpeg");
    assert_eq!(fs::read(inbox.join("empty (1).txt")).unwrap(), b"");
    assert_eq!(fs::read(inbox.join("empty.txt")).unwrap(), b"old");
    wait_for("bob's record", || {
        b.snapshot().transfers.iter().any(|t| t.direction == Direction::Receive && t.state == State::Done)
    });
    // The modification time travels with the file.
    let m_src = fs::metadata(src.join("Album/Ảnh 1.jpg")).unwrap().modified().unwrap();
    let m_dst = fs::metadata(inbox.join("Album/Ảnh 1.jpg")).unwrap().modified().unwrap();
    let diff = m_src.duration_since(m_dst).unwrap_or_else(|e| e.duration());
    assert!(diff < Duration::from_secs(2), "{diff:?}");
}

#[test]
fn receiver_decides_and_messages_need_no_upload() {
    let (a, root_a) = service("carol", 53437, false);
    let (b, root_b) = service("dave", 53447, false);
    a.probe(LOCAL, b.port());
    wait_for("carol to see dave", || !a.snapshot().devices.is_empty());
    let dave = a.snapshot().devices[0].clone();
    fs::write(root_a.join("x.txt"), b"x").unwrap();

    // Declined.
    let id = a.send(&dave, vec![root_a.join("x.txt")]);
    wait_for("the request", || b.snapshot().incoming.is_some());
    let inc = b.snapshot().incoming.unwrap();
    assert_eq!(inc.alias, "carol");
    assert_eq!(inc.files.len(), 1);
    assert_eq!(inc.files[0].file_name, "x.txt");
    assert!(inc.message.is_none());
    b.respond(inc.id, false);
    wait_for("declined", || a.snapshot().transfers.iter().any(|t| t.id == id && t.state == State::Declined));
    assert!(!root_b.join("in/x.txt").exists());

    // A text message: shown, nothing saved.
    let id = a.send_text(&dave, "xin chào 👋".into());
    wait_for("the message", || b.snapshot().incoming.is_some());
    let inc = b.snapshot().incoming.unwrap();
    assert_eq!(inc.message.as_deref(), Some("xin chào 👋"));
    b.respond(inc.id, true);
    wait_for("delivered", || a.snapshot().transfers.iter().any(|t| t.id == id && t.state == State::Done));
    assert!(b.snapshot().transfers.iter().any(|t| t.message.as_deref() == Some("xin chào 👋")));
    assert!(fs::read_dir(root_b.join("in")).map(|d| d.count() == 0).unwrap_or(true));

    // The sender gives up while the receiver is still deciding: the question goes away.
    let id = a.send(&dave, vec![root_a.join("x.txt")]);
    wait_for("the request", || b.snapshot().incoming.is_some());
    a.cancel(id);
    wait_for("cancelled", || a.snapshot().transfers.iter().any(|t| t.id == id && t.state == State::Cancelled));
    wait_for("the question to close", || b.snapshot().incoming.is_none());
}

/// What the official app does as a sender: a client certificate, then upload bodies in chunked
/// transfer encoding (reqwest streams files without a length).
#[test]
fn official_app_style_requests() {
    let (b, root_b) = service("erin", 53457, true);
    let me = oxi_share::Identity::generate().unwrap();
    let addr = SocketAddr::new(LOCAL, b.port());
    let tls = oxi_share::tls::client_config(&me, Some(b.fingerprint())).unwrap();

    // A wrong pinned fingerprint fails the handshake.
    let bad = oxi_share::tls::client_config(&me, Some(&"0".repeat(64))).unwrap();
    assert!(http::connect(addr, Some(bad), Duration::from_secs(2)).is_err());

    let info = serde_json::json!({
        "alias": "Honor Magic8 Pro", "version": "2.1", "deviceModel": "HONOR", "deviceType": "mobile",
        "fingerprint": me.fingerprint, "port": 53317, "protocol": "https", "download": false
    });
    let mut s = http::connect(addr, Some(tls.clone()), Duration::from_secs(2)).unwrap();
    let body = serde_json::to_vec(&info).unwrap();
    let r = http::request(&mut s, "POST", "x", "/api/localsend/v2/register", "application/json", &mut &body[..], body.len() as u64, &mut |_| true).unwrap();
    assert_eq!(r.status, 200);
    let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(v["alias"], "erin");
    assert_eq!(v["fingerprint"], b.fingerprint());
    let d = b.snapshot().devices;
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].fingerprint, me.fingerprint, "the certificate, not the body, names the device");

    let prep = serde_json::json!({
        "info": info,
        "files": {
            "f1": {"id": "f1", "fileName": "IMG_0001.HEIC", "size": 11, "fileType": "image/heic",
                   "sha256": "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9",
                   "preview": null, "metadata": {"modified": "2025-05-01T10:00:00.000Z"}},
            "f2": {"id": "f2", "fileName": "../../evil.txt", "size": 3, "fileType": "text/plain"}
        }
    });
    let body = serde_json::to_vec(&prep).unwrap();
    let mut s = http::connect(addr, Some(tls.clone()), Duration::from_secs(2)).unwrap();
    let r = http::request(&mut s, "POST", "x", "/api/localsend/v2/prepare-upload", "application/json", &mut &body[..], body.len() as u64, &mut |_| true).unwrap();
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    let sid = v["sessionId"].as_str().unwrap().to_string();
    let t1 = v["files"]["f1"].as_str().unwrap().to_string();
    let t2 = v["files"]["f2"].as_str().unwrap().to_string();

    // A second sender is turned away while the session runs.
    let mut s = http::connect(addr, Some(tls.clone()), Duration::from_secs(2)).unwrap();
    let r = http::request(&mut s, "POST", "x", "/api/localsend/v2/prepare-upload", "application/json", &mut &body[..], body.len() as u64, &mut |_| true).unwrap();
    assert_eq!(r.status, 409);

    // A wrong token is refused.
    let mut s = http::connect(addr, Some(tls.clone()), Duration::from_secs(2)).unwrap();
    let path = format!("/api/localsend/v2/upload?sessionId={sid}&fileId=f1&token=nope");
    let r = http::request(&mut s, "POST", "x", &path, "", &mut &b"hello world"[..], 11, &mut |_| true).unwrap();
    assert_eq!(r.status, 403);

    // Chunked bodies, both files on one keep-alive connection.
    let mut s = http::connect(addr, Some(tls), Duration::from_secs(2)).unwrap();
    let chunked = |path: &str, parts: &[&[u8]]| {
        let mut raw = format!("POST {path} HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n").into_bytes();
        for p in parts {
            raw.extend(format!("{:x}\r\n", p.len()).bytes());
            raw.extend_from_slice(p);
            raw.extend(b"\r\n");
        }
        raw.extend(b"0\r\n\r\n");
        raw
    };
    s.write_all(&chunked(&format!("/api/localsend/v2/upload?sessionId={sid}&fileId=f1&token={t1}"), &[b"hello", b" world"])).unwrap();
    let mut r = BufReader::new(&mut s);
    let h = http::read_head(&mut r).unwrap().unwrap();
    assert_eq!(h.first, "200");
    let mut body = http::Body::new(&mut r, h.body_kind().unwrap());
    body.read_all(1000).unwrap();
    let s = r.into_inner();
    s.write_all(&chunked(&format!("/api/localsend/v2/upload?sessionId={sid}&fileId=f2&token={t2}"), &[b"bad"])).unwrap();
    let mut r = BufReader::new(s);
    let h = http::read_head(&mut r).unwrap().unwrap();
    assert_eq!(h.first, "200");

    let inbox = root_b.join("in");
    assert_eq!(fs::read(inbox.join("IMG_0001.HEIC")).unwrap(), b"hello world");
    assert_eq!(fs::read(inbox.join("evil.txt")).unwrap(), b"bad", "kept inside the receive folder");
    assert!(!root_b.join("evil.txt").exists());
    wait_for("done", || b.snapshot().transfers.iter().any(|t| t.state == State::Done));
}

#[test]
fn checksum_mismatch_is_rejected_and_removed() {
    let (b, root_b) = service("frank", 53467, true);
    let me = oxi_share::Identity::generate().unwrap();
    let addr = SocketAddr::new(LOCAL, b.port());
    let tls = oxi_share::tls::client_config(&me, Some(b.fingerprint())).unwrap();
    let prep = serde_json::json!({
        "info": {"alias": "p", "fingerprint": me.fingerprint, "port": 1, "protocol": "https"},
        "files": {"f": {"id": "f", "fileName": "a.bin", "size": 3, "fileType": "", "sha256": "0".repeat(64)}}
    });
    let body = serde_json::to_vec(&prep).unwrap();
    let mut s = http::connect(addr, Some(tls.clone()), Duration::from_secs(2)).unwrap();
    let r = http::request(&mut s, "POST", "x", "/api/localsend/v2/prepare-upload", "", &mut &body[..], body.len() as u64, &mut |_| true).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    let path = format!("/api/localsend/v2/upload?sessionId={}&fileId=f&token={}", v["sessionId"].as_str().unwrap(), v["files"]["f"].as_str().unwrap());
    let mut s = http::connect(addr, Some(tls), Duration::from_secs(2)).unwrap();
    let r = http::request(&mut s, "POST", "x", &path, "", &mut &b"abc"[..], 3, &mut |_| true).unwrap();
    assert_eq!(r.status, 422);
    assert!(!root_b.join("in/a.bin").exists());
}

#[test]
fn browser_gets_a_page_over_plain_http() {
    let (b, _) = service("gina", 53477, false);
    let mut s = http::connect(SocketAddr::new(LOCAL, b.port()), None, Duration::from_secs(2)).unwrap();
    s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    let mut r = BufReader::new(s);
    let h = http::read_head(&mut r).unwrap().unwrap();
    assert_eq!(h.first, "200");
    let html = http::Body::new(&mut r, h.body_kind().unwrap()).read_all(100_000).unwrap();
    assert!(String::from_utf8_lossy(&html).contains("LocalSend"));
}

/// Both on the default port: the second takes the next TCP port, and they find each other by
/// multicast alone. Needs a network interface with multicast (skipped without one).
#[test]
fn multicast_discovery() {
    if oxi_share::local_ipv4().is_empty() {
        eprintln!("no network interface; skipped");
        return;
    }
    let mk = |name: &str| {
        let root = temp(name);
        let cfg = Config {
            alias: name.into(),
            device_model: None,
            port: oxi_share::DEFAULT_PORT,
            identity_dir: root.join("id"),
            receive_dir: root.join("in"),
            quick_save: false,
            multicast: true,
        };
        Share::start(cfg, Arc::new(|| {})).unwrap()
    };
    let a = mk("mc-a");
    let b = mk("mc-b");
    assert_ne!(a.port(), b.port());
    b.refresh();
    wait_for("multicast discovery", || {
        a.snapshot().devices.iter().any(|d| d.alias == "mc-b") && b.snapshot().devices.iter().any(|d| d.alias == "mc-a")
    });
}
