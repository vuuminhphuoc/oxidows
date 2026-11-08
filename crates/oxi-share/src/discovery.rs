//! Finding other devices: UDP multicast announcements on every network interface (including a
//! Wi-Fi Direct group that appears later), answered by a register request over HTTP(S); and, for
//! networks that drop multicast, a register request to every address of the local subnets.

use std::collections::HashSet;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use socket2::{Domain, SockRef, Socket, Type};

use crate::dto::{DeviceInfo, Protocol};
use crate::identity::fingerprint_of;
use crate::{Device, Inner, MULTICAST_GROUP, send};

pub(crate) struct Handle {
    sock: Arc<UdpSocket>,
    port: u16,
    joined: Mutex<HashSet<Ipv4Addr>>,
    scanning: AtomicBool,
}

fn bind(port: u16) -> io::Result<UdpSocket> {
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(socket2::Protocol::UDP))?;
    // LocalSend itself may be running on this PC and holding the port.
    s.set_reuse_address(true)?;
    #[cfg(unix)]
    let _ = s.set_reuse_port(true);
    s.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port).into())?;
    s.set_multicast_loop_v4(true)?;
    s.set_multicast_ttl_v4(4)?;
    s.set_read_timeout(Some(Duration::from_secs(1)))?;
    Ok(s.into())
}

pub(crate) fn start(inner: &Arc<Inner>) -> Handle {
    let port = inner.cfg().port;
    let sock = match bind(port) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            inner.snap().multicast_error = Some(e.to_string());
            // A socket that only sends still lets others hear us.
            Arc::new(
                UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
                    .unwrap_or_else(|_| UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("any UDP socket")),
            )
        }
    };
    let h = Handle { sock: sock.clone(), port, joined: Mutex::new(HashSet::new()), scanning: AtomicBool::new(false) };
    join_new(&h);
    let inner2 = inner.clone();
    thread::Builder::new().name("share-discovery".into()).spawn(move || receive_loop(&inner2, &sock)).ok();
    announce(inner);
    h
}

/// Joins the group on interfaces that appeared since the last call. Returns whether any did.
fn join_new(h: &Handle) -> bool {
    let mut joined = h.joined.lock().unwrap_or_else(|e| e.into_inner());
    let mut new = false;
    for (_, ip, _) in crate::local_ipv4() {
        if joined.contains(&ip) {
            continue;
        }
        // Already joined on that interface counts as success.
        let _ = h.sock.join_multicast_v4(&MULTICAST_GROUP, &ip);
        joined.insert(ip);
        new = true;
    }
    new
}

fn receive_loop(inner: &Arc<Inner>, sock: &UdpSocket) {
    let mut buf = vec![0u8; 65536];
    let mut last_check = Instant::now();
    while !inner.stop.load(Ordering::Relaxed) {
        if last_check.elapsed() > Duration::from_secs(4) {
            last_check = Instant::now();
            let new = inner.discovery.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(join_new);
            if new {
                // A new network (such as the Wi-Fi Direct group) came up: tell it we are here.
                announce(inner);
            }
        }
        let (n, src) = match sock.recv_from(&mut buf) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let Ok(msg) = serde_json::from_slice::<DeviceInfo>(&buf[..n]) else { continue };
        if msg.fingerprint.is_empty() || msg.fingerprint.eq_ignore_ascii_case(&inner.id.fingerprint) {
            continue;
        }
        let ip = src.ip();
        if msg.announce == Some(false) {
            // An answer by multicast (devices whose HTTP request to us failed).
            inner.found(device_from(&msg, ip, msg.fingerprint.to_ascii_uppercase()));
            continue;
        }
        let inner = inner.clone();
        thread::spawn(move || answer(&inner, msg, ip));
    }
}

fn device_from(msg: &DeviceInfo, ip: IpAddr, fingerprint: String) -> Device {
    Device {
        fingerprint,
        alias: msg.alias.clone(),
        device_model: msg.device_model.clone(),
        device_type: msg.device_type.unwrap_or_default(),
        ip,
        port: msg.port.unwrap_or(crate::DEFAULT_PORT),
        protocol: msg.protocol.unwrap_or(Protocol::Https),
        version: msg.version.clone(),
        last_seen: Instant::now(),
        missed: 0,
    }
}

/// Answers an announcement by registering with the announcing device, which is how it learns
/// about us. The device is listed once that worked, so every listed device is reachable.
fn answer(inner: &Arc<Inner>, msg: DeviceInfo, ip: IpAddr) {
    let protocol = msg.protocol.unwrap_or(Protocol::Https);
    let addr = SocketAddr::new(ip, msg.port.unwrap_or(crate::DEFAULT_PORT));
    let expected = (protocol == Protocol::Https).then(|| msg.fingerprint.clone());
    match register(inner, addr, protocol, expected.as_deref(), Duration::from_secs(3)) {
        Ok(_) => inner.found(device_from(&msg, ip, msg.fingerprint.to_ascii_uppercase())),
        Err(_) => {
            // Their server is unreachable from here; answer by multicast so they can still list us.
            send_multicast(inner, Some(false));
        }
    }
}

/// POST /register. Returns the peer's info and its certificate fingerprint (TLS).
fn register(
    inner: &Inner,
    addr: SocketAddr,
    protocol: Protocol,
    expected: Option<&str>,
    timeout: Duration,
) -> io::Result<(DeviceInfo, Option<String>)> {
    let body = serde_json::to_vec(&inner.info(None)).map_err(io::Error::other)?;
    let r = send::call(inner, addr, protocol, expected, "/api/localsend/v2/register", &body, timeout, timeout)?;
    if r.status != 200 {
        return Err(io::Error::other(format!("HTTP {}", r.status)));
    }
    let info: DeviceInfo = serde_json::from_slice(&r.body).map_err(io::Error::other)?;
    Ok((info, r.peer_cert.map(|c| fingerprint_of(&c))))
}

/// Registers with one address, HTTPS first and then plain HTTP. Lists the device if it answers.
pub(crate) fn probe(inner: &Inner, addr: SocketAddr) -> bool {
    probe_with(inner, addr, Duration::from_secs(3))
}

fn probe_with(inner: &Inner, addr: SocketAddr, timeout: Duration) -> bool {
    for protocol in [Protocol::Https, Protocol::Http] {
        match register(inner, addr, protocol, None, timeout) {
            Ok((info, cert_fp)) => {
                let fp = match protocol {
                    Protocol::Https => match cert_fp {
                        Some(fp) => fp,
                        None => return false,
                    },
                    Protocol::Http => info.fingerprint.to_ascii_uppercase(),
                };
                if fp.is_empty() || fp == inner.id.fingerprint {
                    return false;
                }
                let mut d = device_from(&info, addr.ip(), fp);
                d.port = addr.port();
                d.protocol = protocol;
                inner.found(d);
                return true;
            }
            // Nothing listening: no point trying plain HTTP.
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionRefused | io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                return false;
            }
            Err(_) => {}
        }
    }
    false
}

fn send_multicast(inner: &Inner, announce: Option<bool>) {
    let guard = inner.discovery.lock().unwrap_or_else(|e| e.into_inner());
    let Some(h) = guard.as_ref() else { return };
    let Ok(payload) = serde_json::to_vec(&inner.info(announce)) else { return };
    let target = SocketAddr::from((MULTICAST_GROUP, h.port));
    let ips: Vec<Ipv4Addr> = h.joined.lock().unwrap_or_else(|e| e.into_inner()).iter().copied().collect();
    for ip in ips {
        // One send per interface: the OS would otherwise use only the default route's.
        let _ = SockRef::from(&*h.sock).set_multicast_if_v4(&ip);
        let _ = h.sock.send_to(&payload, target);
    }
}

/// Announces this device: three bursts, as the official app does, since UDP may be dropped.
pub(crate) fn announce(inner: &Arc<Inner>) {
    let inner = inner.clone();
    thread::spawn(move || {
        for delay in [100, 500, 2000] {
            thread::sleep(Duration::from_millis(delay));
            if inner.stop.load(Ordering::Relaxed) {
                return;
            }
            send_multicast(&inner, Some(true));
        }
    });
}

/// Refresh rounds a device may miss in a row before it leaves the list.
const MISSES_TO_FORGET: u8 = 3;

pub(crate) fn refresh(inner: &Arc<Inner>) {
    let since = Instant::now();
    if let Some(h) = inner.discovery.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        join_new(h);
    }
    announce(inner);
    let inner = inner.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(3));
        // Multicast may not reach everyone (client isolation, a firewall blocking the answers),
        // so scan whenever a known device, or every device, stayed silent.
        let silent = {
            let s = inner.snap();
            s.devices.is_empty() || s.devices.iter().any(|d| d.last_seen < since)
        };
        if silent {
            scan(&inner);
        }
        let mut s = inner.snap();
        let before = s.devices.len();
        for d in s.devices.iter_mut().filter(|d| d.last_seen < since) {
            d.missed = d.missed.saturating_add(1);
        }
        s.devices.retain(|d| d.missed < MISSES_TO_FORGET);
        let changed = s.devices.len() != before;
        drop(s);
        if changed {
            inner.repaint();
        }
    });
}

/// Every host of each local /24 (larger subnets are narrowed to the /24 around our address).
fn scan(inner: &Arc<Inner>) {
    {
        let guard = inner.discovery.lock().unwrap_or_else(|e| e.into_inner());
        let Some(h) = guard.as_ref() else { return };
        if h.scanning.swap(true, Ordering::Relaxed) {
            return;
        }
    }
    inner.snap().scanning = true;
    inner.repaint();
    let port = inner.cfg().port;
    let mut targets = Vec::new();
    for (_, ip, prefix) in crate::local_ipv4() {
        if prefix < 16 {
            continue;
        }
        let base = u32::from(ip) & 0xFFFF_FF00;
        targets.extend((1..255).map(|h| Ipv4Addr::from(base | h)).filter(|t| *t != ip));
    }
    let targets = Arc::new(targets);
    let next = Arc::new(AtomicUsize::new(0));
    let workers: Vec<_> = (0..64)
        .map(|_| {
            let (inner, targets, next) = (inner.clone(), targets.clone(), next.clone());
            thread::spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(ip) = targets.get(i) else { break };
                    if inner.stop.load(Ordering::Relaxed) {
                        break;
                    }
                    probe_with(&inner, SocketAddr::from((*ip, port)), Duration::from_millis(600));
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
    if let Some(h) = inner.discovery.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        h.scanning.store(false, Ordering::Relaxed);
    }
    inner.snap().scanning = false;
    inner.repaint();
}
