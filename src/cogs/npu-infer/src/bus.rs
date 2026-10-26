//! The cogbus wire client (weftos ADR-122 sections 4, 12 and 13): length-
//! prefixed JSON on a Unix socket. This cog speaks the protocol itself
//! instead of linking the weftos crate, which keeps it a standalone package.
//!
//! One reader thread routes each answer to the request waiting for it and
//! hands `call` frames to the channel the main loop reads, so the worker
//! thread can ask the broker for a blob while the main loop keeps taking calls.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::b64;

/// Largest frame the broker accepts (64 KiB body plus header room).
pub const MAX_FRAME: usize = 64 * 1024 + 4 * 1024;
/// Largest slice of a blob one `blob_get` returns.
const SLICE: u32 = 32 * 1024;

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

/// What a request got back: the result, or the broker's `code: message`.
pub type Answer = Result<Value, String>;

type Waiting = Arc<Mutex<HashMap<u64, Sender<Answer>>>>;

pub struct Bus {
    w: Mutex<UnixStream>,
    next: AtomicU64,
    waiting: Waiting,
}

pub struct Connected {
    pub bus: Arc<Bus>,
    /// `call` frames, until the connection ends.
    pub calls: Receiver<Value>,
    pub node_id: String,
}

impl Bus {
    /// Connect, say hello (the answer names the node) and start the reader.
    pub fn connect(path: &str, token: &str) -> Result<Connected, String> {
        let s = UnixStream::connect(path).map_err(|e| format!("connect {path}: {e}"))?;
        let mut reader = s.try_clone().map_err(|e| e.to_string())?;
        let mut w = s;
        write_frame(&mut w, &json!({"op": "hello", "id": 1, "token": token}))?;
        let first = read_frame(&mut reader)?.ok_or("the broker closed the connection")?;
        if first["type"] != "ok" {
            return Err(format!(
                "hello refused: {}",
                first["message"].as_str().unwrap_or("?")
            ));
        }
        let waiting: Waiting = Arc::default();
        let (call_tx, calls) = channel();
        let wt = waiting.clone();
        std::thread::spawn(move || {
            while let Ok(Some(f)) = read_frame(&mut reader) {
                let kind = f["type"].as_str().map(String::from);
                if kind.as_deref() == Some("call") {
                    if call_tx.send(f).is_err() {
                        break;
                    }
                    continue;
                }
                if let Some(t @ ("ok" | "error")) = kind.as_deref() {
                    let id = f["id"].as_u64().unwrap_or(0);
                    let a = if t == "ok" {
                        Ok(f["result"].clone())
                    } else {
                        Err(format!(
                            "{}: {}",
                            f["code"].as_str().unwrap_or("?"),
                            f["message"].as_str().unwrap_or("")
                        ))
                    };
                    // An answer nobody waits for (a reply's or a publish's ack) is dropped.
                    if let Some(tx) = wt.lock().unwrap_or_else(|p| p.into_inner()).remove(&id) {
                        let _ = tx.send(a);
                    }
                }
            }
            // The connection is gone: wake every waiter.
            for (_, tx) in wt.lock().unwrap_or_else(|p| p.into_inner()).drain() {
                let _ = tx.send(Err("closed: connection closed".into()));
            }
        });
        Ok(Connected {
            node_id: first["result"]["node_id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            calls,
            bus: Arc::new(Bus {
                w: Mutex::new(w),
                next: AtomicU64::new(2),
                waiting,
            }),
        })
    }

    fn id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    fn send(&self, v: &Value) -> Result<(), String> {
        let mut w = self.w.lock().unwrap_or_else(|p| p.into_inner());
        write_frame(&mut *w, v)
    }

    /// Send `req` with a fresh id and wait up to `wait` for its answer.
    pub fn request(&self, mut req: Value, wait: Duration) -> Answer {
        let id = self.id();
        req["id"] = json!(id);
        let (tx, rx) = channel();
        self.waiting
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, tx);
        if let Err(e) = self.send(&req) {
            self.waiting
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            return Err(format!("io: {e}"));
        }
        rx.recv_timeout(wait).unwrap_or_else(|_| {
            self.waiting
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            Err("timeout: no answer in time".into())
        })
    }

    /// Become the provider of `<kind>/<source>`; `Err` is the broker's refusal.
    pub fn serve(&self, kind: &str, source: &str, schema: &str) -> Result<(), String> {
        self.request(
            json!({"op": "serve", "topic_kind": kind, "source_id": source, "schema": schema}),
            Duration::from_secs(10),
        )
        .map(|_| ())
    }

    pub fn reply(&self, call: u64, r: Result<Value, String>) -> Result<(), String> {
        let id = self.id();
        match r {
            Ok(body) => self.send(&json!({"op": "reply", "id": id, "call": call, "body": body})),
            Err(m) => self.send(&json!({"op": "reply", "id": id, "call": call, "error": m})),
        }
    }

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

    /// The bytes of blob `hash` (`size` bytes, from its handle), read in
    /// slices. The broker checks that this instance may read it.
    pub fn blob_get(&self, hash: &str, size: u64) -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(size as usize);
        loop {
            let r = self.request(
                json!({"op": "blob_get", "hash": hash, "offset": out.len(), "len": SLICE}),
                Duration::from_secs(10),
            )?;
            let chunk = b64::decode(r["data"].as_str().ok_or("blob_get answer without data")?)?;
            if chunk.is_empty() && r["eof"] != true {
                return Err("blob_get returned nothing before the end".into());
            }
            out.extend_from_slice(&chunk);
            if r["eof"] == true || out.len() as u64 >= size {
                break;
            }
        }
        if out.len() as u64 != size {
            return Err(format!(
                "blob is {} bytes, its handle says {size}",
                out.len()
            ));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn stand_in(
        name: &str,
        script: impl FnOnce(&mut UnixStream) + Send + 'static,
    ) -> (String, std::thread::JoinHandle<()>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("npu-infer-bus-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.sock");
        let l = UnixListener::bind(&path).unwrap();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let hello = read_frame(&mut s).unwrap().unwrap();
            assert_eq!(
                (hello["op"].as_str(), hello["token"].as_str()),
                (Some("hello"), Some("tok"))
            );
            write_frame(
                &mut s,
                &json!({"type": "ok", "id": 1, "result": {"instance_id": "i1", "node_id": "n0"}}),
            )
            .unwrap();
            script(&mut s);
        });
        (path.to_str().unwrap().to_string(), h, dir)
    }

    #[test]
    fn serve_is_answered_replies_are_sent_and_calls_arrive_on_the_channel() {
        let (path, h, dir) = stand_in("serve", |s| {
            let serve = read_frame(s).unwrap().unwrap();
            assert_eq!(
                (serve["op"].as_str(), serve["topic_kind"].as_str()),
                (Some("serve"), Some("svc.infer"))
            );
            write_frame(s, &json!({"type": "ok", "id": serve["id"], "result": {}})).unwrap();
            write_frame(
                s,
                &json!({"type": "call", "call": 7, "timeout_ms": 1000, "body": {"model": "m"}}),
            )
            .unwrap();
            let reply = read_frame(s).unwrap().unwrap();
            assert_eq!(
                (reply["op"].as_str(), reply["call"].as_u64()),
                (Some("reply"), Some(7))
            );
            assert_eq!(reply["body"], json!({"text": "ok"}));
            assert_eq!(read_frame(s).unwrap().unwrap()["error"], "nope");
        });
        let c = Bus::connect(&path, "tok").unwrap();
        assert_eq!(c.node_id, "n0");
        c.bus.serve("svc.infer", "venq-npu", "infer.v1").unwrap();
        let call = c.calls.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(call["call"], 7);
        c.bus.reply(7, Ok(json!({"text": "ok"}))).unwrap();
        c.bus.reply(8, Err("nope".into())).unwrap();
        h.join().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_refused_serve_is_an_error_with_the_brokers_words() {
        let (path, h, dir) = stand_in("refuse", |s| {
            let serve = read_frame(s).unwrap().unwrap();
            write_frame(
                s,
                &json!({"type": "error", "id": serve["id"], "code": "denied", "message": "closed"}),
            )
            .unwrap();
        });
        let c = Bus::connect(&path, "tok").unwrap();
        assert_eq!(
            c.bus
                .serve("svc.infer", "venq-npu", "infer.v1")
                .unwrap_err(),
            "denied: closed"
        );
        h.join().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_blob_is_read_in_slices_and_checked_against_its_handle() {
        let data: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
        let served = data.clone();
        let (path, h, dir) = stand_in("blob", move |s| {
            loop {
                let Ok(Some(req)) = read_frame(s) else { return };
                assert_eq!(req["op"], "blob_get");
                let off = req["offset"].as_u64().unwrap() as usize;
                let end = (off + req["len"].as_u64().unwrap() as usize).min(served.len());
                let result = json!({"offset": off, "data": b64::encode(&served[off..end]), "eof": end == served.len()});
                write_frame(s, &json!({"type": "ok", "id": req["id"], "result": result})).unwrap();
                if end == served.len() {
                    // a second read of a lying handle
                    let req = read_frame(s).unwrap().unwrap();
                    let result =
                        json!({"offset": 0, "data": b64::encode(&served[..10]), "eof": true});
                    write_frame(s, &json!({"type": "ok", "id": req["id"], "result": result}))
                        .unwrap();
                    return;
                }
            }
        });
        let c = Bus::connect(&path, "tok").unwrap();
        assert_eq!(c.bus.blob_get(&"a".repeat(64), 70_000).unwrap(), data);
        let e = c.bus.blob_get(&"a".repeat(64), 20).unwrap_err();
        assert!(e.contains("handle says"), "{e}");
        h.join().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }
}
