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

use crate::ring::{Geometry, Mapping, RingWriter};

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

pub struct Bus {
    w: Arc<Mutex<UnixStream>>,
    next: AtomicU64,
    pending: Arc<Pending>,
    closed: Arc<AtomicBool>,
    pub node_id: String,
}

/// Pushed `{"type":"call",...}` frames for the service this connection `serve`s.
pub type Calls = Receiver<Value>;

impl Bus {
    /// Connect and say hello; the broker's answer names the node.
    pub fn connect(path: &str, token: &str) -> Result<(Arc<Bus>, Calls), String> {
        let s = UnixStream::connect(path).map_err(|e| format!("connect {path}: {e}"))?;
        let mut rd = FdRead {
            s: s.try_clone().map_err(|e| e.to_string())?,
            fds: VecDeque::new(),
        };
        let w = Arc::new(Mutex::new(s));
        write_frame(
            &mut *w.lock().unwrap_or_else(|p| p.into_inner()),
            &json!({"op": "hello", "id": 1, "token": token}),
        )?;
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
        let (ctx, crx) = channel();
        let (p2, c2, wp) = (pending.clone(), closed.clone(), w.clone());
        std::thread::Builder::new()
            .name("bus-read".into())
            .spawn(move || {
                loop {
                    match read_frame(&mut rd) {
                        Ok(Some(f)) => dispatch(&p2, &ctx, &wp, &mut rd, f),
                        Ok(None) => break,
                        Err(e) => {
                            eprintln!("[cog-sdr-pluto-rx] bus read: {e}");
                            break;
                        }
                    }
                }
                c2.store(true, Ordering::Release);
                p2.lock().unwrap_or_else(|p| p.into_inner()).clear();
            })
            .map_err(|e| e.to_string())?;
        let bus = Bus {
            w,
            next: AtomicU64::new(2),
            pending,
            closed,
            node_id,
        };
        Ok((Arc::new(bus), crx))
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

    /// Become the provider of `<kind>/<source>`.
    pub fn serve(&self, kind: &str, source: &str, schema: &str) -> Result<(), String> {
        self.request(
            json!({"op": "serve", "topic_kind": kind, "source_id": source, "schema": schema}),
            Duration::from_secs(5),
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

    /// Open `<kind>/<source>` (a declared `stream` output) as its writer.
    pub fn stream_writer(
        &self,
        kind: &str,
        source: &str,
        schema: &str,
        max_record: u64,
        ring_bytes: Option<u64>,
        seq_base: Option<u64>,
    ) -> Result<RingWriter, String> {
        let mut req = json!({
            "op": "stream_open", "topic_kind": kind, "source_id": source,
            "schema": schema, "max_record": max_record,
        });
        if let Some(b) = ring_bytes {
            req["ring_bytes"] = json!(b);
        }
        if let Some(s) = seq_base {
            req["seq_base"] = json!(s);
        }
        let r = self.request(req, Duration::from_secs(5))?;
        let v = &r.frame["result"];
        let g = Geometry::from_json(&v["geometry"]).ok_or("stream_open answer without geometry")?;
        let fd = r
            .fds
            .into_iter()
            .next()
            .ok_or("stream_open answer without its ring")?;
        let map = Mapping::new(fd.as_fd(), g.bytes(), true).map_err(|e| e.to_string())?;
        RingWriter::attach(map, g)
    }
}

fn dispatch(
    pending: &Pending,
    calls: &Sender<Value>,
    w: &Mutex<UnixStream>,
    rd: &mut FdRead,
    f: Value,
) {
    // The broker pings a silent connection and closes it if no frame answers (cogbus gap 4).
    if f["type"] == "ping" {
        let mut g = w.lock().unwrap_or_else(|p| p.into_inner());
        let _ = write_frame(&mut *g, &json!({"op": "pong", "id": 0}));
        return;
    }
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
                    "[cog-sdr-pluto-rx] broker refused request {id}: {} {}",
                    f["code"].as_str().unwrap_or("?"),
                    f["message"].as_str().unwrap_or("")
                ),
                None => {}
            }
        }
        Some("call") => {
            let _ = calls.send(f);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring::init;
    use std::os::unix::net::UnixListener;

    #[test]
    fn frames_round_trip_and_oversize_is_refused() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &json!({"op": "x"})).unwrap();
        assert_eq!(&buf[..4], &(buf.len() as u32 - 4).to_be_bytes());
        assert_eq!(
            read_frame(&mut &buf[..]).unwrap().unwrap(),
            json!({"op": "x"})
        );
        assert!(read_frame(&mut &[][..]).unwrap().is_none());
        let huge = ((MAX_FRAME + 1) as u32).to_be_bytes();
        assert!(read_frame(&mut &huge[..]).is_err());
        assert!(write_frame(&mut Vec::new(), &json!({"t": "x".repeat(MAX_FRAME)})).is_err());
    }

    fn sock(name: &str) -> (std::path::PathBuf, UnixListener) {
        let dir = std::env::temp_dir().join(format!("sdr-bus-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.sock");
        let l = UnixListener::bind(&path).unwrap();
        (path, l)
    }

    fn hello(s: &mut UnixStream) {
        let h = read_frame(s).unwrap().unwrap();
        assert_eq!(
            (h["op"].as_str(), h["token"].as_str()),
            (Some("hello"), Some("tok"))
        );
        write_frame(
            s,
            &json!({"type": "ok", "id": 1, "result": {"instance_id": "i1", "node_id": "n0"}}),
        )
        .unwrap();
    }

    #[test]
    fn serve_call_reply_and_publish_against_a_broker_stand_in() {
        let (path, l) = sock("svc");
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            hello(&mut s);
            let serve = read_frame(&mut s).unwrap().unwrap();
            assert_eq!(
                (serve["op"].as_str(), serve["topic_kind"].as_str()),
                (Some("serve"), Some("svc.sdr"))
            );
            write_frame(
                &mut s,
                &json!({"type": "ok", "id": serve["id"], "result": {}}),
            )
            .unwrap();
            write_frame(
                &mut s,
                &json!({"type": "call", "call": 7, "body": {"method": "status"}}),
            )
            .unwrap();
            let reply = read_frame(&mut s).unwrap().unwrap();
            assert_eq!(
                (reply["op"].as_str(), reply["call"].as_u64()),
                (Some("reply"), Some(7))
            );
            assert_eq!(reply["body"], json!({"ok": true}));
            let publish = read_frame(&mut s).unwrap().unwrap();
            assert_eq!(publish["topic_kind"], "sdr.status");
            let err = read_frame(&mut s).unwrap().unwrap();
            assert_eq!(err["error"], "nope");
        });
        let (bus, calls) = Bus::connect(path.to_str().unwrap(), "tok").unwrap();
        assert_eq!(bus.node_id, "n0");
        bus.serve("svc.sdr", "venq-pluto", "sdr.v1").unwrap();
        let c = calls.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(c["body"]["method"], "status");
        bus.reply(7, Ok(json!({"ok": true}))).unwrap();
        bus.publish("sdr.status", "venq-pluto", "sdr.status.v1", 5, json!({}))
            .unwrap();
        bus.reply(8, Err("nope".into())).unwrap();
        h.join().unwrap();
    }

    /// The broker hands the writer its ring as a descriptor on the `stream_open` answer.
    #[test]
    fn stream_open_maps_the_ring_the_broker_passes() {
        let (path, l) = sock("ring");
        let g = Geometry {
            slot_count: 4,
            slot_size: 64,
            max_record: 24,
        };
        let (tx, rx) = channel();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            hello(&mut s);
            let open = read_frame(&mut s).unwrap().unwrap();
            assert_eq!(open["op"], "stream_open");
            assert_eq!(open["max_record"], 24);
            // a real shared object, passed with SCM_RIGHTS like the broker does
            let path = std::env::temp_dir().join(format!("sdr-bus-ring-{}", std::process::id()));
            let f = std::fs::File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&path)
                .unwrap();
            f.set_len(g.bytes() as u64).unwrap();
            std::fs::remove_file(&path).ok();
            let m = Mapping::new(f.as_fd(), g.bytes(), true).unwrap();
            init(&m, g, 9);
            let body = serde_json::to_vec(&json!({"type": "ok", "id": open["id"], "result": {
                "fds": 1, "ring_id": 9,
                "geometry": {"slot_count": 4, "slot_size": 64, "max_record": 24}}}))
            .unwrap();
            let mut bytes = (body.len() as u32).to_be_bytes().to_vec();
            bytes.extend_from_slice(&body);
            send_with_fd(&s, &bytes, f.as_raw_fd());
            tx.send(m).unwrap();
            std::thread::sleep(Duration::from_millis(200));
        });
        let (bus, _calls) = Bus::connect(path.to_str().unwrap(), "tok").unwrap();
        let mut w = bus
            .stream_writer("iq.cs16", "venq-pluto", "iq.cs16.v1", 24, None, None)
            .unwrap();
        assert_eq!(w.write(5, 6, &[1, 2, 3]).unwrap(), 1);
        let m = rx.recv().unwrap();
        // the stand-in's own mapping sees the committed head
        assert_eq!(m.word(OFF_HEAD_FOR_TEST).load(Ordering::Acquire), 1);
        h.join().unwrap();
    }

    const OFF_HEAD_FOR_TEST: usize = 64;

    fn send_with_fd(s: &UnixStream, bytes: &[u8], fd: RawFd) {
        // SAFETY: a single-descriptor SCM_RIGHTS message built in a local buffer.
        unsafe {
            let cap = libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) as usize;
            let mut ctl = vec![0u8; cap];
            let mut iov = libc::iovec {
                iov_base: bytes.as_ptr() as *mut _,
                iov_len: bytes.len(),
            };
            let mut msg: libc::msghdr = std::mem::zeroed();
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = ctl.as_mut_ptr().cast();
            msg.msg_controllen = cap as _;
            let c = libc::CMSG_FIRSTHDR(&msg);
            (*c).cmsg_level = libc::SOL_SOCKET;
            (*c).cmsg_type = libc::SCM_RIGHTS;
            (*c).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
            std::ptr::write_unaligned(libc::CMSG_DATA(c).cast::<RawFd>(), fd);
            assert!(libc::sendmsg(s.as_raw_fd(), &msg, 0) >= 0);
        }
    }

    #[test]
    fn a_ping_from_the_broker_is_answered_with_a_pong() {
        let (path, l) = sock("ping");
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            hello(&mut s);
            write_frame(&mut s, &json!({"type": "ping"})).unwrap();
            read_frame(&mut s).unwrap().unwrap()
        });
        let (_bus, _rx) = Bus::connect(path.to_str().unwrap(), "tok").unwrap();
        assert_eq!(h.join().unwrap(), json!({"op": "pong", "id": 0}));
    }
}
