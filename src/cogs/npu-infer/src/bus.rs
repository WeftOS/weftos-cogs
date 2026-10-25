//! The cogbus wire client (weftos ADR-122 section 4): length-prefixed JSON
//! frames on a Unix socket. This cog speaks the protocol itself instead of
//! linking the weftos crate, which keeps it a standalone package.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde_json::{json, Value};

/// Largest frame the broker accepts (64 KiB body plus header room).
pub const MAX_FRAME: usize = 64 * 1024 + 4 * 1024;

pub struct Bus {
    w: Mutex<UnixStream>,
    next: AtomicU64,
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

pub struct Connected {
    pub bus: Bus,
    pub reader: UnixStream,
    pub node_id: String,
}

impl Bus {
    /// Connect and say hello; the broker's answer names the node.
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
        Ok(Connected {
            node_id: first["result"]["node_id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            reader,
            bus: Bus {
                w: Mutex::new(w),
                next: AtomicU64::new(2),
            },
        })
    }

    fn send(&self, v: &Value) -> Result<(), String> {
        let mut w = self.w.lock().unwrap_or_else(|p| p.into_inner());
        write_frame(&mut *w, v)
    }

    fn id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    /// Become the provider of `<kind>/<source>`; returns the request id so the
    /// caller can match a refusal.
    pub fn serve(&self, kind: &str, source: &str, schema: &str) -> Result<u64, String> {
        let id = self.id();
        self.send(&json!({"op": "serve", "id": id, "topic_kind": kind, "source_id": source, "schema": schema}))?;
        Ok(id)
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

    #[test]
    fn hello_serve_reply_against_a_broker_stand_in() {
        let dir = std::env::temp_dir().join(format!("npu-infer-bus-{}", std::process::id()));
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
            let serve = read_frame(&mut s).unwrap().unwrap();
            assert_eq!(serve["op"], "serve");
            assert_eq!(serve["topic_kind"], "svc.infer");
            let reply = read_frame(&mut s).unwrap().unwrap();
            assert_eq!(
                (reply["op"].as_str(), reply["call"].as_u64()),
                (Some("reply"), Some(7))
            );
            assert_eq!(reply["body"], json!({"text": "ok"}));
            let err = read_frame(&mut s).unwrap().unwrap();
            assert_eq!(err["error"], "nope");
        });
        let c = Bus::connect(path.to_str().unwrap(), "tok").unwrap();
        assert_eq!(c.node_id, "n0");
        c.bus.serve("svc.infer", "venq-npu", "infer.v1").unwrap();
        c.bus.reply(7, Ok(json!({"text": "ok"}))).unwrap();
        c.bus.reply(8, Err("nope".into())).unwrap();
        h.join().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }
}
