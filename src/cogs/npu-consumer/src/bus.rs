//! The cogbus wire client for a caller (weftos ADR-122 sections 4 and 12):
//! length-prefixed JSON on a Unix socket. One reader thread routes answers to
//! the request that is waiting and subscribed readings to a channel.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

pub const MAX_FRAME: usize = 64 * 1024 + 4 * 1024;

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

/// What a request got back.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Ok(Value),
    /// The broker's `code` and message.
    Refused(String, String),
}

type Waiting = Arc<Mutex<HashMap<u64, Sender<Answer>>>>;

pub struct Bus {
    w: Arc<Mutex<UnixStream>>,
    next: AtomicU64,
    waiting: Waiting,
}

impl Bus {
    /// Connect, say hello, and start the reader. Subscribed readings arrive
    /// on the returned channel.
    pub fn connect(path: &str, token: &str) -> Result<(Arc<Bus>, Receiver<Value>), String> {
        let s = UnixStream::connect(path).map_err(|e| format!("connect {path}: {e}"))?;
        let mut reader = s.try_clone().map_err(|e| e.to_string())?;
        let mut w = s;
        write_frame(&mut w, &json!({"op": "hello", "id": 1, "token": token}))?;
        let first = read_frame(&mut reader)?.ok_or("the broker closed the connection")?;
        if first["type"] != "ok" {
            // `busy` here is the per-instance connection cap (8): retry later.
            return Err(format!(
                "hello refused ({}): {}",
                first["code"].as_str().unwrap_or("?"),
                first["message"].as_str().unwrap_or("?")
            ));
        }
        let w = Arc::new(Mutex::new(w));
        let pong_w = w.clone();
        let waiting: Waiting = Arc::default();
        let (msg_tx, msg_rx) = channel();
        let wt = waiting.clone();
        std::thread::spawn(move || {
            while let Ok(Some(f)) = read_frame(&mut reader) {
                match f["type"].as_str() {
                    Some("ping") => {
                        // Silent while waiting on a long call: any frame counts as life.
                        let mut w = pong_w.lock().unwrap_or_else(|p| p.into_inner());
                        if write_frame(&mut *w, &json!({"op": "pong", "id": 0})).is_err() {
                            break;
                        }
                    }
                    Some("ok") | Some("error") => {
                        let id = f["id"].as_u64().unwrap_or(0);
                        let a = if f["type"] == "ok" {
                            Answer::Ok(f["result"].clone())
                        } else {
                            Answer::Refused(
                                f["code"].as_str().unwrap_or("?").to_string(),
                                f["message"].as_str().unwrap_or("").to_string(),
                            )
                        };
                        if let Some(tx) = wt.lock().unwrap_or_else(|p| p.into_inner()).remove(&id) {
                            let _ = tx.send(a);
                        }
                    }
                    Some("msg") => {
                        let _ = msg_tx.send(f);
                    }
                    _ => {}
                }
            }
            // The connection is gone: wake every waiter with a refusal.
            for (_, tx) in wt.lock().unwrap_or_else(|p| p.into_inner()).drain() {
                let _ = tx.send(Answer::Refused("closed".into(), "connection closed".into()));
            }
        });
        Ok((
            Arc::new(Bus {
                w,
                next: AtomicU64::new(2),
                waiting,
            }),
            msg_rx,
        ))
    }

    /// Send `req` (its `id` is filled in) and wait up to `wait` for the answer.
    pub fn request(&self, mut req: Value, wait: Duration) -> Answer {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        req["id"] = json!(id);
        let (tx, rx) = channel();
        self.waiting
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, tx);
        let sent = {
            let mut w = self.w.lock().unwrap_or_else(|p| p.into_inner());
            write_frame(&mut *w, &req)
        };
        if let Err(e) = sent {
            self.waiting
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            return Answer::Refused("io".into(), e);
        }
        match rx.recv_timeout(wait) {
            Ok(a) => a,
            Err(_) => {
                self.waiting
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&id);
                Answer::Refused("timeout".into(), "no answer in time".into())
            }
        }
    }

    /// Call `topic` with `body`; the broker's own deadline is `timeout`.
    pub fn call(&self, topic: &str, body: Value, timeout: Duration) -> Answer {
        let req = json!({"op": "call", "topic": topic, "schema": "infer.v1", "body": body,
            "timeout_ms": timeout.as_millis() as u64});
        match self.request(req, timeout + Duration::from_secs(5)) {
            Answer::Ok(v) => Answer::Ok(v["body"].clone()),
            other => other,
        }
    }

    pub fn subscribe(&self, topic: &str, schema: &str) -> Answer {
        self.request(
            json!({"op": "subscribe", "topic": topic, "schema": schema}),
            Duration::from_secs(5),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn a_ping_is_answered_with_a_pong_while_a_call_waits() {
        let dir =
            std::env::temp_dir().join(format!("npu-consumer-bus-ping-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.sock");
        let l = UnixListener::bind(&path).unwrap();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            read_frame(&mut s).unwrap().unwrap();
            write_frame(&mut s, &json!({"type": "ok", "id": 1, "result": {}})).unwrap();
            let call = read_frame(&mut s).unwrap().unwrap();
            write_frame(&mut s, &json!({"type": "ping"})).unwrap();
            let pong = read_frame(&mut s).unwrap().unwrap();
            assert_eq!(
                (pong["op"].as_str(), pong["id"].as_u64()),
                (Some("pong"), Some(0))
            );
            write_frame(
                &mut s,
                &json!({"type": "ok", "id": call["id"], "result": {"body": {"text": "late"}}}),
            )
            .unwrap();
        });
        let (bus, _rx) = Bus::connect(path.to_str().unwrap(), "t").unwrap();
        assert_eq!(
            bus.call("svc.infer/venq-npu", json!({}), Duration::from_secs(5)),
            Answer::Ok(json!({"text": "late"}))
        );
        h.join().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_call_waits_for_its_own_reply_and_a_refusal_is_reported() {
        let dir = std::env::temp_dir().join(format!("npu-consumer-bus-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.sock");
        let l = UnixListener::bind(&path).unwrap();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            read_frame(&mut s).unwrap().unwrap();
            write_frame(&mut s, &json!({"type": "ok", "id": 1, "result": {}})).unwrap();
            let c1 = read_frame(&mut s).unwrap().unwrap();
            assert_eq!(
                (c1["op"].as_str(), c1["topic"].as_str()),
                (Some("call"), Some("svc.infer/venq-npu"))
            );
            write_frame(
                &mut s,
                &json!({"type": "ok", "id": c1["id"], "result": {"body": {"text": "hi"}}}),
            )
            .unwrap();
            let c2 = read_frame(&mut s).unwrap().unwrap();
            write_frame(
                &mut s,
                &json!({"type": "error", "id": c2["id"], "code": "timeout", "message": "late"}),
            )
            .unwrap();
        });
        let (bus, _rx) = Bus::connect(path.to_str().unwrap(), "t").unwrap();
        let t = Duration::from_secs(2);
        assert_eq!(
            bus.call("svc.infer/venq-npu", json!({"model": "m"}), t),
            Answer::Ok(json!({"text": "hi"}))
        );
        assert_eq!(
            bus.call("svc.infer/venq-npu", json!({}), t),
            Answer::Refused("timeout".into(), "late".into())
        );
        h.join().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }
}
