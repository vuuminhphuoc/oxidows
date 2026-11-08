//! Just enough HTTP/1.1 for the LocalSend API: request and response heads, bodies with
//! `Content-Length` or chunked transfer encoding, keep-alive. Both ends use it.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

const MAX_HEAD: usize = 64 * 1024;
/// JSON bodies (prepare-upload lists every file) are read into memory; cap them.
pub const MAX_JSON: u64 = 64 * 1024 * 1024;

/// A plain or TLS connection.
pub enum Stream {
    Plain(TcpStream),
    TlsServer(Box<rustls::StreamOwned<rustls::ServerConnection, TcpStream>>),
    TlsClient(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Stream {
    pub fn tcp(&self) -> &TcpStream {
        match self {
            Stream::Plain(s) => s,
            Stream::TlsServer(s) => &s.sock,
            Stream::TlsClient(s) => &s.sock,
        }
    }

    /// The certificate the peer presented, if any.
    pub fn peer_cert(&self) -> Option<Vec<u8>> {
        let certs = match self {
            Stream::Plain(_) => None,
            Stream::TlsServer(s) => s.conn.peer_certificates(),
            Stream::TlsClient(s) => s.conn.peer_certificates(),
        };
        certs.and_then(|c| c.first()).map(|c| c.to_vec())
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::TlsServer(s) => s.read(buf),
            Stream::TlsClient(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::TlsServer(s) => s.write(buf),
            Stream::TlsClient(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::TlsServer(s) => s.flush(),
            Stream::TlsClient(s) => s.flush(),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Head {
    /// Request: method. Response: status code as text.
    pub first: String,
    /// Request: path with query. Response: reason phrase.
    pub second: String,
    pub headers: Vec<(String, String)>,
}

impl Head {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn path(&self) -> &str {
        self.second.split('?').next().unwrap_or("")
    }

    pub fn query(&self) -> HashMap<String, String> {
        parse_query(self.second.split_once('?').map(|(_, q)| q).unwrap_or(""))
    }

    pub fn body_kind(&self) -> io::Result<BodyKind> {
        if self.header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked")) {
            return Ok(BodyKind::Chunked);
        }
        match self.header("content-length") {
            Some(v) => v.trim().parse().map(BodyKind::Length).map_err(|_| bad("bad Content-Length")),
            None => Ok(BodyKind::Length(0)),
        }
    }

    pub fn keep_alive(&self) -> bool {
        !self.header("connection").is_some_and(|v| v.eq_ignore_ascii_case("close"))
    }
}

pub fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Reads a request or response head. `Ok(None)` when the peer closed the connection before
/// sending anything, which ends a keep-alive connection normally.
pub fn read_head<R: BufRead>(r: &mut R) -> io::Result<Option<Head>> {
    let mut line = String::new();
    let mut total = 0;
    let mut head = Head::default();
    let mut first = true;
    loop {
        line.clear();
        let n = r.take((MAX_HEAD - total) as u64 + 1).read_line(&mut line)?;
        if n == 0 {
            return if first { Ok(None) } else { Err(bad("connection closed in the head")) };
        }
        total += n;
        if total > MAX_HEAD {
            return Err(bad("head too large"));
        }
        let l = line.trim_end_matches(['\r', '\n']);
        if first {
            // Tolerate stray blank lines between keep-alive requests.
            if l.is_empty() {
                continue;
            }
            let mut parts = l.splitn(3, ' ');
            let a = parts.next().unwrap_or("");
            let b = parts.next().unwrap_or("");
            let c = parts.next().unwrap_or("");
            if a.starts_with("HTTP/") {
                head.first = b.to_string();
                head.second = c.to_string();
            } else {
                head.first = a.to_string();
                head.second = b.to_string();
            }
            first = false;
            continue;
        }
        if l.is_empty() {
            return Ok(Some(head));
        }
        if let Some((k, v)) = l.split_once(':') {
            head.headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyKind {
    Length(u64),
    Chunked,
}

/// Reads one message body and stops at its end, so the connection can carry the next message.
pub struct Body<'a, R: BufRead> {
    r: &'a mut R,
    kind: BodyKind,
    /// Bytes left in the body (Length) or in the current chunk (Chunked).
    left: u64,
    done: bool,
}

impl<'a, R: BufRead> Body<'a, R> {
    pub fn new(r: &'a mut R, kind: BodyKind) -> Self {
        let (left, done) = match kind {
            BodyKind::Length(n) => (n, n == 0),
            BodyKind::Chunked => (0, false),
        };
        Self { r, kind, left, done }
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Reads and drops the rest, up to `limit` bytes. Returns whether the body ended.
    pub fn drain(&mut self, limit: u64) -> bool {
        let mut sink = io::sink();
        let _ = io::copy(&mut self.by_ref().take(limit), &mut sink);
        self.done
    }

    pub fn read_all(&mut self, limit: u64) -> io::Result<Vec<u8>> {
        let mut v = Vec::new();
        self.by_ref().take(limit + 1).read_to_end(&mut v)?;
        if v.len() as u64 > limit {
            return Err(bad("body too large"));
        }
        Ok(v)
    }

    fn next_chunk(&mut self) -> io::Result<()> {
        let mut line = String::new();
        self.r.take(1024).read_line(&mut line)?;
        let l = line.trim();
        if l.is_empty() {
            // The CRLF after the previous chunk's data.
            line.clear();
            self.r.take(1024).read_line(&mut line)?;
        }
        let size = line.trim().split(';').next().unwrap_or("").trim();
        let n = u64::from_str_radix(size, 16).map_err(|_| bad("bad chunk size"))?;
        if n == 0 {
            // Trailers until the blank line.
            loop {
                line.clear();
                if self.r.take(8192).read_line(&mut line)? == 0 || line.trim().is_empty() {
                    break;
                }
            }
            self.done = true;
        }
        self.left = n;
        Ok(())
    }
}

impl<R: BufRead> Read for Body<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            match self.kind {
                BodyKind::Length(_) => {
                    self.done = true;
                    return Ok(0);
                }
                BodyKind::Chunked => {
                    self.next_chunk()?;
                    if self.done {
                        return Ok(0);
                    }
                }
            }
        }
        let max = buf.len().min(self.left.min(usize::MAX as u64) as usize);
        let n = self.r.read(&mut buf[..max])?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed in the body"));
        }
        self.left -= n as u64;
        if self.left == 0 && self.kind != BodyKind::Chunked {
            self.done = true;
        }
        Ok(n)
    }
}

pub fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        _ => "Internal Server Error",
    }
}

pub fn write_response<W: Write>(
    w: &mut W,
    code: u16,
    content_type: &str,
    extra: &[(&str, String)],
    body: &[u8],
    keep_alive: bool,
) -> io::Result<()> {
    let mut h = format!("HTTP/1.1 {code} {}\r\nContent-Length: {}\r\n", reason(code), body.len());
    if !content_type.is_empty() {
        h += &format!("Content-Type: {content_type}\r\n");
    }
    for (k, v) in extra {
        h += &format!("{k}: {v}\r\n");
    }
    h += if keep_alive { "Connection: keep-alive\r\n\r\n" } else { "Connection: close\r\n\r\n" };
    w.write_all(h.as_bytes())?;
    w.write_all(body)?;
    w.flush()
}

pub fn parse_query(q: &str) -> HashMap<String, String> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect()
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let hex = |c: u8| (c as char).to_digit(16);
                match (hex(b[i + 1]), hex(b[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out += &format!("%{b:02X}");
        }
    }
    out
}

/// Opens a connection to a peer, with TLS when `tls` is given.
pub fn connect(addr: SocketAddr, tls: Option<Arc<rustls::ClientConfig>>, timeout: Duration) -> io::Result<Stream> {
    let tcp = TcpStream::connect_timeout(&addr, timeout)?;
    tcp.set_nodelay(true)?;
    tcp.set_read_timeout(Some(timeout.max(Duration::from_secs(5))))?;
    tcp.set_write_timeout(Some(Duration::from_secs(30)))?;
    match tls {
        None => Ok(Stream::Plain(tcp)),
        Some(cfg) => {
            let name = rustls::pki_types::ServerName::IpAddress(addr.ip().into());
            let conn = rustls::ClientConnection::new(cfg, name).map_err(io::Error::other)?;
            let mut s = rustls::StreamOwned::new(conn, tcp);
            // Handshake now, so a wrong certificate fails here rather than mid-request.
            while s.conn.is_handshaking() {
                s.conn.complete_io(&mut s.sock)?;
            }
            Ok(Stream::TlsClient(Box::new(s)))
        }
    }
}

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    /// The server's certificate (TLS only).
    pub peer_cert: Option<Vec<u8>>,
}

/// Sends one request on `stream` and reads the whole response. `body` is copied with
/// `Content-Length: len`, calling `progress` with the bytes sent so far.
#[allow(clippy::too_many_arguments)]
pub fn request(
    stream: &mut Stream,
    method: &str,
    host: &str,
    path: &str,
    content_type: &str,
    body: &mut dyn Read,
    len: u64,
    progress: &mut dyn FnMut(u64) -> bool,
) -> io::Result<Response> {
    let mut h = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {len}\r\nConnection: close\r\n");
    if !content_type.is_empty() {
        h += &format!("Content-Type: {content_type}\r\n");
    }
    h += "User-Agent: oxi-explorer\r\n\r\n";
    stream.write_all(h.as_bytes())?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut sent = 0u64;
    while sent < len {
        let want = buf.len().min((len - sent) as usize);
        let n = body.read(&mut buf[..want])?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "file shorter than announced"));
        }
        stream.write_all(&buf[..n])?;
        sent += n as u64;
        if !progress(sent) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
    }
    stream.flush()?;
    let peer_cert = stream.peer_cert();
    let mut r = BufReader::new(stream);
    let head = read_head(&mut r)?.ok_or_else(|| bad("no response"))?;
    let status: u16 = head.first.parse().map_err(|_| bad("bad status line"))?;
    let kind = match head.body_kind()? {
        // A response without length runs until the connection closes.
        BodyKind::Length(0) if head.header("content-length").is_none() && status != 204 => BodyKind::Length(MAX_JSON),
        k => k,
    };
    let mut b = Body::new(&mut r, kind);
    let mut body = Vec::new();
    match b.by_ref().take(MAX_JSON).read_to_end(&mut body) {
        Ok(_) => {}
        // Read-until-close: EOF is the end.
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof && kind == BodyKind::Length(MAX_JSON) => {}
        Err(e) if matches!(e.kind(), io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted) && !body.is_empty() => {}
        Err(e) => return Err(e),
    }
    Ok(Response { status, body, peer_cert })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunked_body_then_next_request() {
        let raw = b"POST /a HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6;x=y\r\n world\r\n0\r\n\r\nGET /b?x=%C3%A9&y=a+b HTTP/1.1\r\nContent-Length: 2\r\n\r\nok";
        let mut r = BufReader::new(&raw[..]);
        let h = read_head(&mut r).unwrap().unwrap();
        assert_eq!((h.first.as_str(), h.path()), ("POST", "/a"));
        let mut b = Body::new(&mut r, h.body_kind().unwrap());
        assert_eq!(b.read_all(100).unwrap(), b"hello world");
        assert!(b.is_done());
        let h = read_head(&mut r).unwrap().unwrap();
        assert_eq!(h.path(), "/b");
        let q = h.query();
        assert_eq!(q["x"], "é");
        assert_eq!(q["y"], "a b");
        let mut b = Body::new(&mut r, h.body_kind().unwrap());
        assert_eq!(b.read_all(100).unwrap(), b"ok");
        assert!(read_head(&mut r).unwrap().is_none());
    }

    #[test]
    fn length_body_is_bounded() {
        let raw = b"PUT / HTTP/1.1\r\ncontent-length: 3\r\n\r\nabcdef";
        let mut r = BufReader::new(&raw[..]);
        let h = read_head(&mut r).unwrap().unwrap();
        let mut b = Body::new(&mut r, h.body_kind().unwrap());
        assert!(b.read_all(2).is_err());
    }

    #[test]
    fn percent_round_trip() {
        let s = "Ảnh 1/2 & co.jpg";
        assert_eq!(percent_decode(&percent_encode(s)), s);
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }
}
