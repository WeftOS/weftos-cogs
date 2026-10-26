//! The cogbus wire client (WeftOS ADR-122 sections 4, 6, 12): length-prefixed JSON
//! frames on a Unix socket, descriptors over `SCM_RIGHTS`, stream rings in shared
//! memory. This cog speaks the protocol itself instead of linking the WeftOS crate,
//! which keeps it a standalone package. The same file is carried by every cog that
//! uses the bus; each uses part of it.
//!
//! Ring layout (one writer, many readers; `weftos-cogbus` `stream/ring.rs`):
//! a 128-byte header (magic `COGRING1`, head seq at 64) then `slot_count` slots of
//! `slot_size` bytes: `seq`, `len`, `t_ns`, `w_ns`, payload. Every word is accessed
//! through atomics and a slot is a seqlock.
//!
//! This writer never waits for `block` readers (it does not map their cursors), so a
//! slow `block` reader loses records like a `drop-oldest` one.
#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{fence, AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Largest frame the broker accepts (64 KiB body plus header room).
pub const MAX_FRAME: usize = 64 * 1024 + 4 * 1024;
const MAX_FDS: usize = 4;
const HEADER: usize = 128;
const SLOT_HEADER: usize = 32;
const OFF_HEAD: usize = 64;
const BUSY: u64 = u64::MAX;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

// ---- frames and descriptors ----

/// One frame from the broker with the descriptors that came with it.
pub struct Frame {
    pub v: Value,
    pub fds: Vec<OwnedFd>,
}

/// Buffered frame reader over `recvmsg`, keeping passed descriptors in order.
pub struct FrameReader {
    sock: UnixStream,
    buf: Vec<u8>,
    fds: VecDeque<OwnedFd>,
}

fn cmsg_space() -> usize {
    // SAFETY: CMSG_SPACE is a pure size computation.
    unsafe { libc::CMSG_SPACE((MAX_FDS * std::mem::size_of::<RawFd>()) as u32) as usize }
}

impl FrameReader {
    pub fn new(sock: UnixStream) -> Self {
        Self {
            sock,
            buf: Vec::new(),
            fds: VecDeque::new(),
        }
    }

    fn fill(&mut self) -> std::io::Result<usize> {
        let mut chunk = [0u8; 8192];
        let mut ctl = vec![0u8; cmsg_space()];
        let mut iov = libc::iovec {
            iov_base: chunk.as_mut_ptr().cast(),
            iov_len: chunk.len(),
        };
        #[cfg(target_os = "linux")]
        let flags = libc::MSG_CMSG_CLOEXEC;
        #[cfg(not(target_os = "linux"))]
        let flags = 0;
        // SAFETY: the kernel writes at most the lengths passed; only control messages
        // it reports are walked, and each received descriptor is wrapped exactly once.
        let n = unsafe {
            let mut msg: libc::msghdr = std::mem::zeroed();
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = ctl.as_mut_ptr().cast();
            msg.msg_controllen = ctl.len() as _;
            let n = libc::recvmsg(self.sock.as_raw_fd(), &mut msg, flags);
            if n < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut c = libc::CMSG_FIRSTHDR(&msg);
            while !c.is_null() {
                if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                    let data = libc::CMSG_DATA(c);
                    let bytes = (*c).cmsg_len as usize - (data as usize - c as usize);
                    for i in 0..bytes / std::mem::size_of::<RawFd>() {
                        let fd = std::ptr::read_unaligned(data.cast::<RawFd>().add(i));
                        #[cfg(not(target_os = "linux"))]
                        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                        self.fds.push_back(OwnedFd::from_raw_fd(fd));
                    }
                }
                c = libc::CMSG_NXTHDR(&msg, c);
            }
            n as usize
        };
        self.buf.extend_from_slice(&chunk[..n]);
        Ok(n)
    }

    /// The next frame; `Ok(None)` at a clean end of stream.
    pub fn next(&mut self) -> Result<Option<Frame>, String> {
        loop {
            if self.buf.len() >= 4 {
                let n = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]])
                    as usize;
                if n > MAX_FRAME {
                    return Err(format!("frame of {n} bytes"));
                }
                if self.buf.len() >= 4 + n {
                    let body: Vec<u8> = self.buf.drain(..4 + n).skip(4).collect();
                    let v: Value =
                        serde_json::from_slice(&body).map_err(|e| format!("bad frame: {e}"))?;
                    let want = v["result"]["fds"]
                        .as_u64()
                        .or_else(|| v["fds"].as_u64())
                        .unwrap_or(0) as usize;
                    let take = want.min(self.fds.len());
                    return Ok(Some(Frame {
                        v,
                        fds: self.fds.drain(..take).collect(),
                    }));
                }
            }
            match self.fill() {
                Ok(0) => return Ok(None),
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(format!("read: {e}")),
            }
        }
    }
}

pub fn encode_frame(v: &Value) -> Result<Vec<u8>, String> {
    let body = serde_json::to_vec(v).map_err(|e| e.to_string())?;
    if body.len() > MAX_FRAME {
        return Err(format!("frame of {} bytes", body.len()));
    }
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

pub fn write_frame(w: &mut impl Write, v: &Value) -> Result<(), String> {
    let out = encode_frame(v)?;
    w.write_all(&out)
        .and_then(|_| w.flush())
        .map_err(|e| format!("write: {e}"))
}

/// Send `bytes` with `fds` attached (a broker stand-in for tests).
#[cfg(test)]
pub fn send_with_fds(sock: &UnixStream, bytes: &[u8], fds: &[RawFd]) -> std::io::Result<()> {
    let mut ctl = vec![0u8; cmsg_space()];
    let mut iov = libc::iovec {
        iov_base: bytes.as_ptr() as *mut libc::c_void,
        iov_len: bytes.len(),
    };
    // SAFETY: every pointer set below points into a live local buffer sized for it.
    let rc = unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        if !fds.is_empty() {
            msg.msg_control = ctl.as_mut_ptr().cast();
            let len = std::mem::size_of_val(fds) as u32;
            msg.msg_controllen = libc::CMSG_SPACE(len) as _;
            let c = libc::CMSG_FIRSTHDR(&msg);
            (*c).cmsg_level = libc::SOL_SOCKET;
            (*c).cmsg_type = libc::SCM_RIGHTS;
            (*c).cmsg_len = libc::CMSG_LEN(len) as _;
            std::ptr::copy_nonoverlapping(
                fds.as_ptr(),
                libc::CMSG_DATA(c).cast::<RawFd>(),
                fds.len(),
            );
        }
        libc::sendmsg(sock.as_raw_fd(), &msg, 0)
    };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    assert_eq!(rc as usize, bytes.len());
    Ok(())
}

// ---- shared memory ----

/// A mapped shared-memory object.
pub struct Map {
    ptr: *mut u8,
    len: usize,
    writable: bool,
}

// SAFETY: the mapping is only accessed through atomics.
unsafe impl Send for Map {}
unsafe impl Sync for Map {}

impl Map {
    pub fn new(fd: &OwnedFd, len: usize, writable: bool) -> Result<Self, String> {
        let prot = libc::PROT_READ | if writable { libc::PROT_WRITE } else { 0 };
        // SAFETY: a fresh shared mapping of a descriptor we hold; the result is checked.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                prot,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if p == libc::MAP_FAILED {
            return Err(format!("mmap: {}", std::io::Error::last_os_error()));
        }
        Ok(Self {
            ptr: p.cast(),
            len,
            writable,
        })
    }

    pub fn word(&self, off: usize) -> &AtomicU64 {
        assert!(
            off.is_multiple_of(8) && off + 8 <= self.len,
            "word out of the mapping"
        );
        // SAFETY: in bounds and 8-aligned (mmap is page-aligned); the memory lives as long as `self`.
        unsafe { &*self.ptr.add(off).cast::<AtomicU64>() }
    }
}

impl Drop for Map {
    fn drop(&mut self) {
        // SAFETY: unmapping exactly what `new` mapped.
        unsafe { libc::munmap(self.ptr.cast(), self.len) };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub slot_count: u64,
    pub slot_size: u64,
    pub max_record: u64,
}

impl Geometry {
    pub fn from_json(v: &Value) -> Result<Self, String> {
        let g = Self {
            slot_count: v["slot_count"].as_u64().ok_or("geometry.slot_count")?,
            slot_size: v["slot_size"].as_u64().ok_or("geometry.slot_size")?,
            max_record: v["max_record"].as_u64().ok_or("geometry.max_record")?,
        };
        let ok = g.slot_count >= 2
            && g.slot_size.is_multiple_of(64)
            && g.slot_size >= SLOT_HEADER as u64 + g.max_record.div_ceil(8) * 8
            && g.slot_count
                .checked_mul(g.slot_size)
                .is_some_and(|b| b < (1 << 32));
        ok.then_some(g)
            .ok_or_else(|| "bad ring geometry".to_string())
    }

    pub fn bytes(&self) -> usize {
        HEADER + (self.slot_count * self.slot_size) as usize
    }

    fn slot(&self, seq: u64) -> usize {
        HEADER + (((seq - 1) % self.slot_count) * self.slot_size) as usize
    }
}

/// The writer's side of a ring.
pub struct RingWriter {
    map: Map,
    g: Geometry,
    head: u64,
}

impl RingWriter {
    pub fn attach(map: Map, g: Geometry) -> Result<Self, String> {
        if map.len < g.bytes() || !map.writable {
            return Err("ring geometry does not match its mapping".into());
        }
        let head = map.word(OFF_HEAD).load(Ordering::Acquire);
        Ok(Self { map, g, head })
    }

    pub fn geometry(&self) -> Geometry {
        self.g
    }

    pub fn last_seq(&self) -> u64 {
        self.head
    }

    /// Commit one record; returns its seq.
    pub fn write(&mut self, t_ns: u64, data: &[u8]) -> Result<u64, String> {
        if data.len() as u64 > self.g.max_record {
            return Err(format!(
                "record of {} bytes exceeds max_record {}",
                data.len(),
                self.g.max_record
            ));
        }
        let seq = self.head + 1;
        let base = self.g.slot(seq);
        let m = &self.map;
        m.word(base).store(BUSY, Ordering::Relaxed);
        fence(Ordering::Release);
        m.word(base + 8).store(data.len() as u64, Ordering::Relaxed);
        m.word(base + 16).store(t_ns, Ordering::Relaxed);
        m.word(base + 24).store(wall_ns(), Ordering::Relaxed);
        for (i, chunk) in data.chunks(8).enumerate() {
            let mut w = [0u8; 8];
            w[..chunk.len()].copy_from_slice(chunk);
            m.word(base + SLOT_HEADER + i * 8)
                .store(u64::from_le_bytes(w), Ordering::Relaxed);
        }
        m.word(base).store(seq, Ordering::Release);
        m.word(OFF_HEAD).store(seq, Ordering::Release);
        self.head = seq;
        Ok(seq)
    }
}

/// One record read from a ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub seq: u64,
    pub t_ns: u64,
    pub w_ns: u64,
    pub data: Vec<u8>,
}

/// A reader's side of a ring (read-only mapping).
pub struct RingReader {
    map: Map,
    g: Geometry,
}

impl RingReader {
    pub fn attach(map: Map, g: Geometry) -> Result<Self, String> {
        if map.len < g.bytes() {
            return Err("ring geometry does not match its mapping".into());
        }
        Ok(Self { map, g })
    }

    pub fn head(&self) -> u64 {
        self.map.word(OFF_HEAD).load(Ordering::Acquire)
    }

    /// The record at `*next`, or the oldest still safe to read, advancing `*next` past it.
    pub fn read(&self, next: &mut u64) -> Option<Record> {
        let n = self.g.slot_count;
        for _ in 0..(2 * n + 16) {
            let head = self.head();
            if head < *next {
                return None;
            }
            let oldest = (head + 2).saturating_sub(n).max(1);
            if *next < oldest {
                *next = oldest;
            }
            let base = self.g.slot(*next);
            let m = &self.map;
            let s1 = m.word(base).load(Ordering::Acquire);
            if s1 != *next {
                if s1 < *next {
                    *next += 1;
                }
                continue;
            }
            let len = m.word(base + 8).load(Ordering::Relaxed);
            let t_ns = m.word(base + 16).load(Ordering::Relaxed);
            let w_ns = m.word(base + 24).load(Ordering::Relaxed);
            let len = len.min(self.g.max_record) as usize;
            let mut data = Vec::with_capacity(len.div_ceil(8) * 8);
            for i in 0..len.div_ceil(8) {
                data.extend_from_slice(
                    &m.word(base + SLOT_HEADER + i * 8)
                        .load(Ordering::Relaxed)
                        .to_le_bytes(),
                );
            }
            data.truncate(len);
            fence(Ordering::Acquire);
            if m.word(base).load(Ordering::Relaxed) != s1 {
                continue;
            }
            let seq = *next;
            *next += 1;
            return Some(Record {
                seq,
                t_ns,
                w_ns,
                data,
            });
        }
        None
    }
}

pub fn wall_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

// ---- the bus ----

/// A service call pushed to the provider.
#[derive(Debug, Clone)]
pub struct Call {
    pub call: u64,
    pub from: String,
    pub timeout_ms: u64,
    pub body: Value,
}

/// A reading delivered to a subscription.
#[derive(Debug, Clone)]
pub struct Msg {
    pub topic: String,
    pub source_id: String,
    pub t_ns: u64,
    pub seq: u64,
    pub body: Value,
}

type Pending = Arc<Mutex<HashMap<u64, Sender<Frame>>>>;

/// A ring a stream reader is waiting for or reading.
#[derive(Default)]
struct RingSlot {
    ring: Mutex<Option<(RingReader, u64)>>,
}

struct Orphan {
    geometry: Value,
    head: u64,
    fd: OwnedFd,
}

#[derive(Default)]
struct Subs {
    slots: HashMap<u64, Arc<RingSlot>>,
    orphans: HashMap<u64, Orphan>,
}

pub struct Bus {
    w: Mutex<UnixStream>,
    next: AtomicU64,
    pending: Pending,
    calls: Mutex<Option<Receiver<Call>>>,
    msgs: Mutex<Option<Receiver<Msg>>>,
    subs: Arc<Mutex<Subs>>,
    closed: Arc<AtomicBool>,
    pub node_id: String,
}

fn attach_ring(slot: &RingSlot, geometry: &Value, head: u64, fd: &OwnedFd) -> Result<(), String> {
    let g = Geometry::from_json(geometry)?;
    let map = Map::new(fd, g.bytes(), false)?;
    *lock(&slot.ring) = Some((RingReader::attach(map, g)?, head + 1));
    Ok(())
}

impl Bus {
    /// Connect and say hello; the answer names the node.
    pub fn connect(path: &str, token: &str) -> Result<Bus, String> {
        let s = UnixStream::connect(path).map_err(|e| format!("connect {path}: {e}"))?;
        let mut reader = FrameReader::new(s.try_clone().map_err(|e| e.to_string())?);
        let mut w = s;
        write_frame(&mut w, &json!({"op": "hello", "id": 1, "token": token}))?;
        let first = reader.next()?.ok_or("the broker closed the connection")?;
        if first.v["type"] != "ok" {
            return Err(format!(
                "hello refused: {}",
                first.v["message"].as_str().unwrap_or("?")
            ));
        }
        let node_id = first.v["result"]["node_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let pending: Pending = Arc::default();
        let subs: Arc<Mutex<Subs>> = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        let (ctx, crx) = channel();
        let (mtx, mrx) = channel();
        let (p2, s2, c2) = (pending.clone(), subs.clone(), closed.clone());
        std::thread::spawn(move || {
            while let Ok(Some(f)) = reader.next() {
                dispatch(f, &p2, &s2, &ctx, &mtx);
            }
            c2.store(true, Ordering::Release);
            lock(&p2).clear();
        });
        Ok(Bus {
            w: Mutex::new(w),
            next: AtomicU64::new(2),
            pending,
            calls: Mutex::new(Some(crx)),
            msgs: Mutex::new(Some(mrx)),
            subs,
            closed,
            node_id,
        })
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn send(&self, v: &Value) -> Result<(), String> {
        write_frame(&mut *lock(&self.w), v)
    }

    fn id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    /// Send `req` (its `id` is set here) and wait for the answer.
    fn request(&self, req: Value) -> Result<Frame, String> {
        self.request_for(req, REQUEST_TIMEOUT)
    }

    fn request_for(&self, mut req: Value, wait: Duration) -> Result<Frame, String> {
        let id = self.id();
        req["id"] = json!(id);
        let (tx, rx) = channel();
        lock(&self.pending).insert(id, tx);
        if let Err(e) = self.send(&req) {
            lock(&self.pending).remove(&id);
            return Err(e);
        }
        match rx.recv_timeout(wait) {
            Ok(f) if f.v["type"] == "error" => Err(format!(
                "{}: {}",
                f.v["code"].as_str().unwrap_or("error"),
                f.v["message"].as_str().unwrap_or("")
            )),
            Ok(f) => Ok(f),
            Err(RecvTimeoutError::Timeout) => {
                lock(&self.pending).remove(&id);
                Err("the broker did not answer".into())
            }
            Err(RecvTimeoutError::Disconnected) => Err("the broker closed the connection".into()),
        }
    }

    /// Publish a reading on `<kind>/<source>`.
    pub fn publish(
        &self,
        kind: &str,
        source: &str,
        schema: &str,
        t_ns: u64,
        body: Value,
    ) -> Result<(), String> {
        self.request(json!({
            "op": "publish", "topic_kind": kind, "source_id": source,
            "schema": schema, "t_ns": t_ns, "body": body,
        }))
        .map(|_| ())
    }

    /// Become the provider of service `<kind>/<source>`.
    pub fn serve(&self, kind: &str, source: &str, schema: &str) -> Result<(), String> {
        self.request(
            json!({"op": "serve", "topic_kind": kind, "source_id": source, "schema": schema}),
        )
        .map(|_| ())
    }

    /// Answer a pushed call.
    pub fn reply(&self, call: u64, r: Result<Value, String>) -> Result<(), String> {
        let id = self.id();
        match r {
            Ok(body) => self.send(&json!({"op": "reply", "id": id, "call": call, "body": body})),
            Err(m) => self.send(&json!({"op": "reply", "id": id, "call": call, "error": m})),
        }
    }

    /// Read `<kind>/<source>` or `<kind>/*`; the deliveries arrive on [`Bus::take_msgs`].
    pub fn subscribe(&self, topic: &str, schema: &str) -> Result<u64, String> {
        let f = self.request(json!({"op": "subscribe", "topic": topic, "schema": schema}))?;
        f.v["result"]["sub"]
            .as_u64()
            .ok_or_else(|| "subscribe answered without a sub".to_string())
    }

    /// Call service `<kind>/<source>`; returns the reply body.
    pub fn call(
        &self,
        topic: &str,
        schema: &str,
        body: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let req = json!({
            "op": "call", "topic": topic, "schema": schema, "body": body,
            "timeout_ms": timeout.as_millis() as u64,
        });
        self.request_for(req, timeout + Duration::from_secs(2))
            .map(|f| f.v["result"]["body"].clone())
    }

    /// The delivered readings of every subscription; available once.
    pub fn take_msgs(&self) -> Option<Receiver<Msg>> {
        lock(&self.msgs).take()
    }

    /// The pushed-call queue; available once.
    pub fn take_calls(&self) -> Option<Receiver<Call>> {
        lock(&self.calls).take()
    }

    /// Open `<kind>/<source>` as its stream writer.
    pub fn stream_writer(
        &self,
        kind: &str,
        source: &str,
        schema: &str,
        max_record: u64,
        ring_bytes: Option<u64>,
    ) -> Result<RingWriter, String> {
        let mut req = json!({
            "op": "stream_open", "topic_kind": kind, "source_id": source,
            "schema": schema, "max_record": max_record,
        });
        if let Some(b) = ring_bytes {
            req["ring_bytes"] = json!(b);
        }
        let f = self.request(req)?;
        let g = Geometry::from_json(&f.v["result"]["geometry"])?;
        let fd = f
            .fds
            .first()
            .ok_or("stream_open answered without its ring")?;
        RingWriter::attach(Map::new(fd, g.bytes(), true)?, g)
    }

    /// Read stream `<kind>/<source>`.
    pub fn stream_reader(&self, topic: &str, schema: &str) -> Result<StreamReader, String> {
        let f =
            self.request(json!({"op": "stream_subscribe", "topic": topic, "schema": schema}))?;
        let sub = f.v["result"]["sub"]
            .as_u64()
            .ok_or("stream_subscribe answered without a sub")?;
        let slot: Arc<RingSlot> = Arc::default();
        let ring = &f.v["result"]["ring"];
        if ring.is_object() {
            let fd = f
                .fds
                .first()
                .ok_or("stream_subscribe answered without its ring")?;
            attach_ring(
                &slot,
                &ring["geometry"],
                ring["head"].as_u64().unwrap_or(0),
                fd,
            )?;
        } else {
            let mut s = lock(&self.subs);
            if let Some(o) = s.orphans.remove(&sub) {
                attach_ring(&slot, &o.geometry, o.head, &o.fd)?;
            }
            s.slots.insert(sub, slot.clone());
        }
        Ok(StreamReader {
            slot,
            next: 0,
            skipped: 0,
            last_seq: 0,
        })
    }
}

fn dispatch(
    f: Frame,
    pending: &Pending,
    subs: &Arc<Mutex<Subs>>,
    calls: &Sender<Call>,
    msgs: &Sender<Msg>,
) {
    match f.v["type"].as_str() {
        Some("ok") | Some("error") => {
            let id = f.v["id"].as_u64().unwrap_or(0);
            if let Some(tx) = lock(pending).remove(&id) {
                let _ = tx.send(f);
            }
        }
        Some("call") => {
            let _ = calls.send(Call {
                call: f.v["call"].as_u64().unwrap_or(0),
                from: f.v["from"].as_str().unwrap_or_default().to_string(),
                timeout_ms: f.v["timeout_ms"].as_u64().unwrap_or(30_000),
                body: f.v["body"].clone(),
            });
        }
        Some("msg") => {
            let _ = msgs.send(Msg {
                topic: f.v["topic"].as_str().unwrap_or_default().to_string(),
                source_id: f.v["source_id"].as_str().unwrap_or_default().to_string(),
                t_ns: f.v["t_ns"].as_u64().unwrap_or(0),
                seq: f.v["seq"].as_u64().unwrap_or(0),
                body: f.v["body"].clone(),
            });
        }
        Some("stream") if f.v["notice"] == "ring" => {
            let (Some(sub), Some(fd)) = (f.v["sub"].as_u64(), f.fds.into_iter().next()) else {
                return;
            };
            let head = f.v["head"].as_u64().unwrap_or(0);
            let mut s = lock(subs);
            match s.slots.get(&sub).cloned() {
                Some(slot) => {
                    let _ = attach_ring(&slot, &f.v["geometry"], head, &fd);
                }
                None => {
                    s.orphans.insert(
                        sub,
                        Orphan {
                            geometry: f.v["geometry"].clone(),
                            head,
                            fd,
                        },
                    );
                }
            }
        }
        _ => {}
    }
}

/// Reads one stream topic.
pub struct StreamReader {
    slot: Arc<RingSlot>,
    next: u64,
    pub skipped: u64,
    pub last_seq: u64,
}

impl StreamReader {
    /// The next record, waiting up to `wait` for one (and for the writer to open the ring).
    pub fn next(&mut self, wait: Duration) -> Option<Record> {
        let end = Instant::now() + wait;
        loop {
            if let Some((ring, start)) = lock(&self.slot.ring).as_ref() {
                if self.next == 0 {
                    self.next = *start;
                }
                if let Some(r) = ring.read(&mut self.next) {
                    if self.last_seq != 0 && r.seq > self.last_seq + 1 {
                        self.skipped += r.seq - self.last_seq - 1;
                    }
                    self.last_seq = r.seq;
                    return Some(r);
                }
            }
            if Instant::now() >= end {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// A shared-memory object any process on a dev machine can map: an unlinked temp file.
    pub fn shm(len: usize) -> OwnedFd {
        let mut p = *b"/tmp/cogring.XXXXXX\0";
        // SAFETY: `p` is a writable NUL-terminated template; the file is unlinked at once.
        let fd = unsafe { libc::mkstemp(p.as_mut_ptr().cast()) };
        assert!(fd >= 0);
        // SAFETY: path and fd are ours.
        unsafe {
            libc::unlink(p.as_ptr().cast());
            assert_eq!(libc::ftruncate(fd, len as libc::off_t), 0);
            OwnedFd::from_raw_fd(fd)
        }
    }

    pub fn init_ring(fd: &OwnedFd, g: Geometry) {
        let m = Map::new(fd, g.bytes(), true).unwrap();
        for (off, v) in [
            (0, u64::from_le_bytes(*b"COGRING1")),
            (8, 1),
            (16, g.slot_count),
            (24, g.slot_size),
            (32, g.max_record),
            (40, 7),
        ] {
            m.word(off).store(v, Ordering::Relaxed);
        }
    }

    pub fn geometry() -> Geometry {
        let slot_size = (SLOT_HEADER as u64 + 640u64.div_ceil(8) * 8).div_ceil(64) * 64;
        Geometry {
            slot_count: 8,
            slot_size,
            max_record: 640,
        }
    }

    #[test]
    fn ring_round_trips_in_order_and_a_slow_reader_jumps_forward() {
        let g = geometry();
        let fd = shm(g.bytes());
        init_ring(&fd, g);
        let mut w = RingWriter::attach(Map::new(&fd, g.bytes(), true).unwrap(), g).unwrap();
        let r = RingReader::attach(Map::new(&fd, g.bytes(), false).unwrap(), g).unwrap();
        for i in 0..5u8 {
            assert_eq!(w.write(100 + i as u64, &[i; 640]).unwrap(), i as u64 + 1);
        }
        let mut next = 1;
        for i in 0..5u8 {
            let rec = r.read(&mut next).unwrap();
            assert_eq!(
                (rec.seq, rec.t_ns, rec.data.len(), rec.data[0]),
                (i as u64 + 1, 100 + i as u64, 640, i)
            );
        }
        assert!(r.read(&mut next).is_none());
        for i in 5..30u8 {
            w.write(0, &[i; 8]).unwrap();
        }
        let mut next = 1;
        let rec = r.read(&mut next).unwrap();
        assert!(
            rec.seq > 20,
            "a reader a ring behind jumps forward, got {}",
            rec.seq
        );
        assert!(w.write(0, &[0; 641]).is_err(), "over max_record is refused");
    }

    /// A broker stand-in: hello, one stream_open, one stream_subscribe whose ring arrives
    /// in a later notice, and one pushed call.
    #[test]
    fn writer_and_reader_meet_through_a_broker_stand_in() {
        let dir = std::env::temp_dir().join(format!("cogbus-client-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.sock");
        let l = UnixListener::bind(&path).unwrap();
        let g = geometry();
        let ring = shm(g.bytes());
        init_ring(&ring, g);
        let gj = json!({"slot_count": g.slot_count, "slot_size": g.slot_size, "max_record": g.max_record});
        let (gj2, ring_fd) = (gj.clone(), ring.as_raw_fd());
        let srv = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut rd = FrameReader::new(s.try_clone().unwrap());
            let hello = rd.next().unwrap().unwrap().v;
            assert_eq!(hello["op"], "hello");
            write_frame(
                &mut s,
                &json!({"type": "ok", "id": 1, "result": {"node_id": "n0"}}),
            )
            .unwrap();
            let open = rd.next().unwrap().unwrap().v;
            assert_eq!(
                (open["op"].as_str(), open["max_record"].as_u64()),
                (Some("stream_open"), Some(640))
            );
            let ok = json!({"type": "ok", "id": open["id"], "result": {"ring_id": 7, "geometry": gj2, "fds": 1}});
            send_with_fds(&s, &encode_frame(&ok).unwrap(), &[ring_fd]).unwrap();
            let sub = rd.next().unwrap().unwrap().v;
            assert_eq!(sub["op"], "stream_subscribe");
            let ok = json!({"type": "ok", "id": sub["id"], "result": {"sub": 5, "ring": null, "fds": 0}});
            write_frame(&mut s, &ok).unwrap();
            let notice = json!({"type": "stream", "notice": "ring", "sub": 5, "ring_id": 7, "head": 0, "geometry": gj2, "fds": 1});
            send_with_fds(&s, &encode_frame(&notice).unwrap(), &[ring_fd]).unwrap();
            write_frame(&mut s, &json!({"type": "call", "call": 9, "topic": "svc.x/n", "schema": "s", "from": "i", "node_id": "n0", "recv_ns": 0, "timeout_ms": 500, "body": {"a": 1}})).unwrap();
            let reply = rd.next().unwrap().unwrap().v;
            assert_eq!(
                (
                    reply["op"].as_str(),
                    reply["call"].as_u64(),
                    reply["body"].clone()
                ),
                (Some("reply"), Some(9), json!({"ok": true}))
            );
        });
        let bus = Bus::connect(path.to_str().unwrap(), "tok").unwrap();
        assert_eq!(bus.node_id, "n0");
        let mut w = bus
            .stream_writer("audio.pcm", "mic0", "pcm.s16le.v1", 640, None)
            .unwrap();
        let mut r = bus.stream_reader("audio.pcm/mic0", "pcm.s16le.v1").unwrap();
        w.write(11, &[1; 640]).unwrap();
        w.write(12, &[2; 640]).unwrap();
        let a = r.next(Duration::from_secs(2)).expect("first record");
        let b = r.next(Duration::from_secs(2)).expect("second record");
        assert_eq!(
            (a.seq, a.t_ns, a.data[0], b.seq, b.data[0]),
            (1, 11, 1, 2, 2)
        );
        assert_eq!(r.skipped, 0);
        let call = bus
            .take_calls()
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert_eq!(
            (call.call, call.from.as_str(), call.body.clone()),
            (9, "i", json!({"a": 1}))
        );
        bus.reply(call.call, Ok(json!({"ok": true}))).unwrap();
        srv.join().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn subscribe_delivers_readings_and_call_returns_the_reply_body() {
        let dir = std::env::temp_dir().join(format!("cogbus-client-sub-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.sock");
        let l = UnixListener::bind(&path).unwrap();
        let srv = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut rd = FrameReader::new(s.try_clone().unwrap());
            let h = rd.next().unwrap().unwrap().v;
            assert_eq!(h["op"], "hello");
            write_frame(
                &mut s,
                &json!({"type": "ok", "id": 1, "result": {"node_id": "n0"}}),
            )
            .unwrap();
            let sub = rd.next().unwrap().unwrap().v;
            assert_eq!(
                (sub["op"].as_str(), sub["topic"].as_str()),
                (Some("subscribe"), Some("transcript/asr0"))
            );
            write_frame(
                &mut s,
                &json!({"type": "ok", "id": sub["id"], "result": {"sub": 3}}),
            )
            .unwrap();
            write_frame(&mut s, &json!({"type": "msg", "sub": 3, "topic": "transcript/asr0", "schema": "transcript.v1",
                "source_id": "asr0", "node_id": "n0", "t_ns": 5, "seq": 1, "recv_ns": 6, "body": {"text": "hi"}})).unwrap();
            let call = rd.next().unwrap().unwrap().v;
            assert_eq!(
                (call["op"].as_str(), call["timeout_ms"].as_u64()),
                (Some("call"), Some(2000))
            );
            write_frame(
                &mut s,
                &json!({"type": "ok", "id": call["id"], "result": {"body": {"text": "yo"}}}),
            )
            .unwrap();
        });
        let bus = Bus::connect(path.to_str().unwrap(), "tok").unwrap();
        assert_eq!(
            bus.subscribe("transcript/asr0", "transcript.v1").unwrap(),
            3
        );
        let m = bus
            .take_msgs()
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert_eq!(
            (m.source_id.as_str(), m.seq, m.body["text"].clone()),
            ("asr0", 1, json!("hi"))
        );
        let r = bus
            .call(
                "svc.infer/venq-npu",
                "infer.v1",
                json!({"model": "m"}),
                Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(r, json!({"text": "yo"}));
        srv.join().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn frames_round_trip_and_oversize_is_refused() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &json!({"op": "x"})).unwrap();
        assert_eq!(&buf[..4], &(buf.len() as u32 - 4).to_be_bytes());
        assert!(write_frame(&mut Vec::new(), &json!({"t": "x".repeat(MAX_FRAME)})).is_err());
        let (a, b) = UnixStream::pair().unwrap();
        (&a).write_all(&((MAX_FRAME + 1) as u32).to_be_bytes())
            .unwrap();
        assert!(FrameReader::new(b).next().is_err());
    }
}
