//! A local HTTP/1.1 range server with fault modes, and a strict range client
//! implementing `ReadAt`. Validation rules follow imazen/zencodec f5a7fd46
//! perf/spatial-validation/src/http_source.rs and http_snapshot.py (Imazen-owned):
//! 206 only, exact Content-Range and Content-Length, one strong ETag kept with
//! If-Match after the first response, no Content-Encoding, and no reuse after
//! any failure. Protocol qualification on loopback, not a performance claim.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use zencodec::source::ReadAt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    None,
    /// From this request on, the object (and its ETag) is replaced.
    ChangeFrom(u32),
    /// From this request on, the body is one byte short.
    ShortFrom(u32),
    /// From this request on, the server ignores Range and answers 200.
    FullFrom(u32),
}

#[derive(Clone, Debug)]
pub struct Logged {
    pub call: u32,
    pub range: Option<(u64, u64)>,
    pub if_match: Option<String>,
    pub status: u16,
    pub body: usize,
}

pub struct Server {
    pub port: u16,
    pub log: Arc<Mutex<Vec<Logged>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

fn etag(data: &[u8]) -> String {
    // FNV-1a: a strong validator for the test object, not a security hash.
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("\"{h:016x}\"")
}

impl Server {
    pub fn start(object: Arc<[u8]>, fault: Fault) -> Self {
        let listener = (3000..4000)
            .find_map(|port| TcpListener::bind(("127.0.0.1", port)).ok())
            .expect("a free loopback port in 3000..3999");
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicU32::new(0));
        let (log2, stop2) = (log.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
                serve(stream, &object, fault, call, &log2);
            }
        });
        Self {
            port,
            log,
            stop,
            thread: Some(thread),
        }
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/object", self.port)
    }

    pub fn requests(&self) -> Vec<Logged> {
        self.log.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).ok()? == 0 || head.len() > 16 * 1024 {
            return None;
        }
        head.push(byte[0]);
    }
    String::from_utf8(head).ok()
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then_some(v.trim())
    })
}

fn serve(mut stream: TcpStream, object: &[u8], fault: Fault, call: u32, log: &Mutex<Vec<Logged>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let Some(head) = read_head(&mut stream) else {
        return;
    };
    let changed;
    let data: &[u8] = match fault {
        Fault::ChangeFrom(n) if call >= n => {
            changed = object.iter().map(|b| b ^ 0x5a).collect::<Vec<_>>();
            &changed
        }
        _ => object,
    };
    let tag = etag(data);
    let range = header(&head, "Range")
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|r| r.split_once('-'))
        .and_then(|(a, b)| Some((a.parse::<u64>().ok()?, b.parse::<u64>().ok()?)));
    let if_match = header(&head, "If-Match").map(str::to_owned);
    let mut status = 206;
    let mut body: Vec<u8> = Vec::new();
    let mut content_range = None;
    match range {
        Some((a, b)) if a <= b && (b as usize) < data.len() => {
            body.extend_from_slice(&data[a as usize..=b as usize]);
            content_range = Some(format!("bytes {a}-{b}/{}", data.len()));
        }
        _ => status = 416,
    }
    if if_match.as_ref().is_some_and(|m| *m != tag) {
        status = 412;
        body.clear();
        content_range = None;
    }
    if matches!(fault, Fault::FullFrom(n) if call >= n) && status == 206 {
        status = 200;
        body = data.to_vec();
        content_range = None;
    }
    let mut length = body.len();
    if matches!(fault, Fault::ShortFrom(n) if call >= n) && status == 206 && !body.is_empty() {
        body.pop();
        // The header still announces the full length: a truncated transfer.
        length = body.len() + 1;
    }
    log.lock().unwrap().push(Logged {
        call,
        range,
        if_match,
        status,
        body: body.len(),
    });
    let reason = match status {
        206 => "Partial Content",
        200 => "OK",
        412 => "Precondition Failed",
        _ => "Range Not Satisfiable",
    };
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\nETag: {tag}\r\nContent-Length: {length}\r\nConnection: close\r\n"
    );
    if let Some(cr) = content_range {
        response.push_str(&format!("Content-Range: {cr}\r\n"));
    }
    response.push_str("\r\n");
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    Bounds,
    Status(u16),
    ContentRange,
    ContentLength,
    Encoding,
    Validator,
    Transport,
    Body,
    Poisoned,
}

#[derive(Debug, Clone)]
pub struct HttpFailure {
    pub kind: FailureKind,
}

impl std::fmt::Display for HttpFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "strict HTTP range source: {:?}", self.kind)
    }
}

impl std::error::Error for HttpFailure {}

fn fail(kind: FailureKind) -> HttpFailure {
    HttpFailure { kind }
}

/// Strict single-object range client. One request per range; no retries.
pub struct StrictHttp {
    port: u16,
    length: Option<u64>,
    etag: Option<String>,
    failed: Option<HttpFailure>,
    pub requests: Arc<AtomicU32>,
}

impl StrictHttp {
    /// Open with a one-byte probe that establishes length and validator.
    pub fn open(port: u16) -> Result<Self, HttpFailure> {
        let mut source = Self {
            port,
            length: None,
            etag: None,
            failed: None,
            requests: Arc::new(AtomicU32::new(0)),
        };
        source.read_exact_at(0, &mut [0])?;
        Ok(source)
    }

    fn fetch(&mut self, offset: u64, out: &mut [u8]) -> Result<(), HttpFailure> {
        if out.is_empty() {
            return Ok(());
        }
        let end = offset
            .checked_add(out.len() as u64 - 1)
            .filter(|&end| self.length.is_none_or(|len| end < len))
            .ok_or(fail(FailureKind::Bounds))?;
        let mut stream = TcpStream::connect(("127.0.0.1", self.port))
            .map_err(|_| fail(FailureKind::Transport))?;
        let mut request = format!(
            "GET /object HTTP/1.1\r\nHost: 127.0.0.1\r\nRange: bytes={offset}-{end}\r\nAccept-Encoding: identity\r\nConnection: close\r\n"
        );
        if let Some(tag) = &self.etag {
            request.push_str(&format!("If-Match: {tag}\r\n"));
        }
        request.push_str("\r\n");
        self.requests.fetch_add(1, Ordering::SeqCst);
        stream
            .write_all(request.as_bytes())
            .map_err(|_| fail(FailureKind::Transport))?;
        let head = read_head(&mut stream).ok_or(fail(FailureKind::Transport))?;
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or(fail(FailureKind::Transport))?;
        if status != 206 {
            return Err(fail(FailureKind::Status(status)));
        }
        if header(&head, "Content-Encoding").is_some() {
            return Err(fail(FailureKind::Encoding));
        }
        let length: usize = header(&head, "Content-Length")
            .and_then(|v| v.parse().ok())
            .ok_or(fail(FailureKind::ContentLength))?;
        if length != out.len() {
            return Err(fail(FailureKind::ContentLength));
        }
        let total = header(&head, "Content-Range")
            .and_then(|v| v.strip_prefix(&format!("bytes {offset}-{end}/")))
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&t| t > end && self.length.is_none_or(|old| old == t))
            .ok_or(fail(FailureKind::ContentRange))?;
        let tag = header(&head, "ETag").ok_or(fail(FailureKind::Validator))?;
        if tag.len() < 2
            || !tag.starts_with('"')
            || !tag.ends_with('"')
            || self.etag.as_deref().is_some_and(|prior| prior != tag)
        {
            return Err(fail(FailureKind::Validator));
        }
        let tag = tag.to_owned();
        stream
            .read_exact(out)
            .map_err(|_| fail(FailureKind::Body))?;
        let mut extra = [0u8; 1];
        if stream
            .read(&mut extra)
            .map_err(|_| fail(FailureKind::Body))?
            != 0
        {
            return Err(fail(FailureKind::ContentLength));
        }
        // Commit representation facts only after the complete response.
        self.length = Some(total);
        self.etag = Some(tag);
        Ok(())
    }
}

impl ReadAt for StrictHttp {
    type Error = HttpFailure;

    fn len(&mut self) -> Result<u64, HttpFailure> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        self.length.ok_or(fail(FailureKind::Bounds))
    }

    fn len_hint(&self) -> Option<u64> {
        self.failed.is_none().then_some(self.length).flatten()
    }

    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), HttpFailure> {
        if self.failed.is_some() {
            return Err(fail(FailureKind::Poisoned));
        }
        let result = self.fetch(offset, out);
        if let Err(error) = &result {
            // No implicit reopen or adoption of a new validator after a failure.
            self.failed = Some(error.clone());
        }
        result
    }
}
