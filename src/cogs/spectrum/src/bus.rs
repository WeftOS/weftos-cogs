//! The cogbus wire client (weftos ADR-122 section 4 and 12): length-prefixed JSON frames on a
//! Unix socket, with descriptors passed on the frames that announce them (`SCM_RIGHTS`). The
//! cog speaks the protocol itself instead of linking the weftos crate.
//!
//! One reader thread owns the read half: it answers pending requests by id, queues pushed
//! `call` frames for the service loop, and keeps the descriptors that arrive with a frame.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::os::fd::{AsFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::ring::{Geometry, Mapping, RingReader};

/// Largest frame the broker accepts (64 KiB body plus header room).
pub const MAX_FRAME: usize = 64 * 1024 + 4 * 1024;
/// Most descriptors one frame carries.
const MAX_FDS: usize = 4;

/// A read half that keeps the descriptors passed with the bytes.
struct FdRead {
    s: UnixStream,
    fds: VecDeque<OwnedFd>,
}

impl Read for FdRead {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let cap = unsafe { libc::CMSG_SPACE((MAX_FDS * std::mem::size_of::<RawFd>()) as u32) };
        let mut ctl = vec![0u8; cap as usize];
        let mut iov = libc::iovec {
            iov_base: buf.as_mut_ptr().cast(),
            iov_len: buf.len(),
        };
        #[cfg(target_os = "linux")]
        let flags = libc::MSG_CMSG_CLOEXEC;
        #[cfg(not(target_os = "linux"))]
        let flags = 0;
        // SAFETY: every pointer set below points into a live local buffer sized for what it
        // describes; the kernel writes at most those lengths, and only the control messages it
        // reports are walked.
        unsafe {
            let mut msg: libc::msghdr = std::mem::zeroed();
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = ctl.as_mut_ptr().cast();
            msg.msg_controllen = ctl.len() as _;
            let n = loop {
                let n = libc::recvmsg(self.s.as_raw_fd(), &mut msg, flags);
                if n >= 0 {
                    break n;
                }
                let e = std::io::Error::last_os_error();
                if e.kind() != std::io::ErrorKind::Interrupted {
                    return Err(e);
                }
            };
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
            Ok(n as usize)
        }
    }
}

pub fn read_frame(r: &mut impl Read) -> Result<Option<Value>, String> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(format!("read: {e}")),
    }
    let n = u32::from_be_bytes(len) as usize;
    if n > MAX_FRAME {
        return Err(format!("frame of {n} bytes"));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).map_err(|e| format!("read: {e}"))?;
    serde_json::from_slice(&buf)
        .map(Some)
        .map_err(|e| format!("bad frame: {e}"))
}

pub fn write_frame(w: &mut impl Write, v: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(v).map_err(|e| e.to_string())?;
    if body.len() > MAX_FRAME {
        return Err(format!("frame of {} bytes", body.len()));
    }
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    w.write_all(&out)
        .and_then(|_| w.flush())
        .map_err(|e| format!("write: {e}"))
}

/// A broker answer and the descriptors that came with it.
pub struct Reply {
    pub frame: Value,
    pub fds: Vec<OwnedFd>,
}

type Pending = Mutex<HashMap<u64, Sender<Reply>>>;

/// A pushed `{"type":"stream",...}` notice and its descriptors (a reader's ring being replaced).
pub type Notice = (Value, Vec<OwnedFd>);

pub struct Bus {
    w: Mutex<UnixStream>,
    next: AtomicU64,
    pending: Arc<Pending>,
    closed: Arc<AtomicBool>,
    pub node_id: String,
}

impl Bus {
    /// Connect and say hello; the broker's answer names the node.
    pub fn connect(path: &str, token: &str) -> Result<(Arc<Bus>, Receiver<Notice>), String> {
        let s = UnixStream::connect(path).map_err(|e| format!("connect {path}: {e}"))?;
        let mut rd = FdRead {
            s: s.try_clone().map_err(|e| e.to_string())?,
            fds: VecDeque::new(),
        };
        let mut w = s;
        write_frame(&mut w, &json!({"op": "hello", "id": 1, "token": token}))?;
        let first = read_frame(&mut rd)?.ok_or("the broker closed the connection")?;
        if first["type"] != "ok" {
            return Err(format!(
                "hello refused: {}",
                first["message"].as_str().unwrap_or("?")
            ));
        }
        let node_id = first["result"]["node_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let pending: Arc<Pending> = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        let (ntx, nrx) = channel();
        let (p2, c2) = (pending.clone(), closed.clone());
        std::thread::Builder::new()
            .name("bus-read".into())
            .spawn(move || {
                loop {
                    match read_frame(&mut rd) {
                        Ok(Some(f)) => dispatch(&p2, &ntx, &mut rd, f),
                        Ok(None) => break,
                        Err(e) => {
                            eprintln!("[cog-spectrum] bus read: {e}");
                            break;
                        }
                    }
                }
                c2.store(true, Ordering::Release);
                p2.lock().unwrap_or_else(|p| p.into_inner()).clear();
            })
            .map_err(|e| e.to_string())?;
        let bus = Bus {
            w: Mutex::new(w),
            next: AtomicU64::new(2),
            pending,
            closed,
            node_id,
        };
        Ok((Arc::new(bus), nrx))
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn send(&self, v: &Value) -> Result<(), String> {
        let mut w = self.w.lock().unwrap_or_else(|p| p.into_inner());
        write_frame(&mut *w, v)
    }

    fn id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    /// Send `req` (its `id` is filled in) and wait for the answer.
    pub fn request(&self, mut req: Value, wait: Duration) -> Result<Reply, String> {
        let id = self.id();
        req["id"] = json!(id);
        let (tx, rx) = channel();
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, tx);
        if let Err(e) = self.send(&req) {
            self.pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            return Err(e);
        }
        let r = rx
            .recv_timeout(wait)
            .map_err(|_| "no answer from the broker".to_string())?;
        if r.frame["type"] == "error" {
            return Err(format!(
                "{}: {}",
                r.frame["code"].as_str().unwrap_or("error"),
                r.frame["message"].as_str().unwrap_or("?")
            ));
        }
        Ok(r)
    }

    /// Publish a reading. The broker's answer is not awaited; a refusal is logged by the reader.
    pub fn publish(
        &self,
        kind: &str,
        source: &str,
        schema: &str,
        t_ns: u64,
        body: Value,
    ) -> Result<(), String> {
        let id = self.id();
        self.send(&json!({
            "op": "publish", "id": id, "topic_kind": kind, "source_id": source,
            "schema": schema, "t_ns": t_ns, "body": body,
        }))
    }

    /// Call a service (`svc.*`) and wait for its reply body.
    pub fn call(
        &self,
        topic: &str,
        schema: &str,
        body: Value,
        wait: Duration,
    ) -> Result<Value, String> {
        let r = self.request(
            json!({"op": "call", "topic": topic, "schema": schema, "body": body,
                   "timeout_ms": wait.as_millis() as u64}),
            wait + Duration::from_secs(2),
        )?;
        Ok(r.frame["result"]["body"].clone())
    }

    /// Read stream `<kind>/<source>`. Returns the subscription id and, when a writer has opened
    /// the topic already, its ring (geometry, ring id, head) with the read-only descriptor.
    pub fn stream_subscribe(
        &self,
        topic: &str,
        schema: &str,
    ) -> Result<(u64, Option<RingGrant>), String> {
        let mut r = self.request(
            json!({"op": "stream_subscribe", "topic": topic, "schema": schema}),
            Duration::from_secs(5),
        )?;
        let v = &r.frame["result"];
        let sub = v["sub"]
            .as_u64()
            .ok_or("stream_subscribe answer without sub")?;
        let ring = if v["ring"].is_null() {
            None
        } else {
            let g = Geometry::from_json(&v["ring"]["geometry"]).ok_or("bad ring geometry")?;
            if r.fds.is_empty() {
                return Err("stream_subscribe answer without its ring".into());
            }
            Some(RingGrant {
                fd: r.fds.remove(0),
                geometry: g,
                head: v["ring"]["head"].as_u64().unwrap_or(0),
            })
        };
        Ok((sub, ring))
    }
}

/// A ring handed to a reader.
pub struct RingGrant {
    pub fd: OwnedFd,
    pub geometry: Geometry,
    /// The newest committed seq when the ring was handed over; the reader starts after it.
    pub head: u64,
}

impl RingGrant {
    /// From a `{"type":"stream","notice":"ring",...}` frame and its descriptor.
    pub fn from_notice(f: &Value, mut fds: Vec<OwnedFd>) -> Option<(u64, RingGrant)> {
        if f["notice"] != "ring" || fds.is_empty() {
            return None;
        }
        Some((
            f["sub"].as_u64()?,
            RingGrant {
                fd: fds.remove(0),
                geometry: Geometry::from_json(&f["geometry"])?,
                head: f["head"].as_u64().unwrap_or(0),
            },
        ))
    }

    pub fn map(&self) -> Result<RingReader, String> {
        let m = Mapping::new(self.fd.as_fd(), self.geometry.bytes(), false)
            .map_err(|e| e.to_string())?;
        Ok(RingReader::attach(m, self.geometry))
    }
}

fn dispatch(pending: &Pending, notices: &Sender<Notice>, rd: &mut FdRead, f: Value) {
    match f["type"].as_str() {
        Some("ok") | Some("error") => {
            let n = f["result"]["fds"].as_u64().unwrap_or(0) as usize;
            let n = n.min(rd.fds.len());
            let fds: Vec<OwnedFd> = rd.fds.drain(..n).collect();
            let id = f["id"].as_u64().unwrap_or(0);
            let tx = pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            match tx {
                Some(tx) => {
                    let _ = tx.send(Reply { frame: f, fds });
                }
                None if f["type"] == "error" => eprintln!(
                    "[cog-spectrum] broker refused request {id}: {} {}",
                    f["code"].as_str().unwrap_or("?"),
                    f["message"].as_str().unwrap_or("")
                ),
                None => {}
            }
        }
        Some("stream") => {
            let n = (f["fds"].as_u64().unwrap_or(0) as usize).min(rd.fds.len());
            let fds: Vec<OwnedFd> = rd.fds.drain(..n).collect();
            let _ = notices.send((f, fds));
        }
        _ => {}
    }
}
