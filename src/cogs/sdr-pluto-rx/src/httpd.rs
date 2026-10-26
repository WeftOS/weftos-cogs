//! The cog's HTTP surface, loopback by default because it can hand out raw IQ:
//!
//! ```text
//! GET /status    the status document (the same as the `status` call)
//! GET /raw?samples=N   the newest N complex samples, interleaved little-endian i16 (octet-stream)
//! GET /guide     this cog's sensor guide bundle (WeftOS ADR-104)
//! GET /healthz   200 while blocks are arriving and the receive path is alive, else 503
//! ```

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::acquire::Shared;

pub struct Resp {
    pub status: u16,
    pub ctype: &'static str,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

fn text(status: u16, ctype: &'static str, body: impl Into<Vec<u8>>) -> Resp {
    Resp {
        status,
        ctype,
        headers: Vec::new(),
        body: body.into(),
    }
}

pub fn route(shared: &Shared, guide: &dyn Fn() -> Value, method: &str, target: &str) -> Resp {
    if method != "GET" {
        return text(405, "text/plain", "GET only\n");
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let status = || {
        shared
            .status
            .lock()
            .map(|s| s.clone())
            .unwrap_or(Value::Null)
    };
    match path {
        "/" => text(
            200,
            "text/plain",
            "sdr-pluto-rx\n  /status\n  /raw?samples=N\n  /guide\n  /healthz\n",
        ),
        "/healthz" => {
            if shared.healthy.load(Ordering::Acquire) {
                text(200, "text/plain", "ok\n")
            } else {
                let s = status();
                let why = s["error"]
                    .as_str()
                    .or(s["status"].as_str())
                    .unwrap_or("not streaming");
                text(503, "text/plain", format!("unhealthy: {why}\n"))
            }
        }
        "/status" => text(200, "application/json", status().to_string()),
        "/guide" => text(200, "application/json", guide().to_string()),
        "/raw" => {
            let want = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("samples="))
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(4096);
            let raw = shared.raw.lock().map(|r| r.clone()).unwrap_or_default();
            if raw.is_empty() {
                return text(503, "text/plain", "no samples yet\n");
            }
            let keep = (want.max(1) * 4).min(raw.len());
            let s = status();
            let mut r = text(
                200,
                "application/octet-stream",
                raw[raw.len() - keep..].to_vec(),
            );
            for (h, v) in [
                ("X-Centre-Hz", &s["band"]["centre_hz"]),
                ("X-Rate-Sps", &s["band"]["rate_sps"]),
            ] {
                if let Some(n) = v.as_u64() {
                    r.headers.push((h.into(), n.to_string()));
                }
            }
            r.headers
                .push(("X-Format".into(), "cs16-le-interleaved-iq".into()));
            r
        }
        _ => text(404, "text/plain", "not found\n"),
    }
}

fn serve_conn(mut s: TcpStream, shared: &Shared, guide: &dyn Fn() -> Value) {
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    s.set_write_timeout(Some(Duration::from_secs(5))).ok();
    let mut buf = [0u8; 4096];
    let mut n = 0;
    while n < buf.len() {
        match s.read(&mut buf[n..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => {
                n += k;
                if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
        }
    }
    let head = String::from_utf8_lossy(&buf[..n]);
    let mut it = head.lines().next().unwrap_or("").split_whitespace();
    let (method, target) = (it.next().unwrap_or(""), it.next().unwrap_or("/"));
    let r = route(shared, guide, method, target);
    let reason = match r.status {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let mut out = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        r.status,
        r.ctype,
        r.body.len()
    );
    for (h, v) in &r.headers {
        out.push_str(&format!("{h}: {v}\r\n"));
    }
    out.push_str("\r\n");
    let _ = s
        .write_all(out.as_bytes())
        .and_then(|_| s.write_all(&r.body));
}

/// Start the server on `bind`; returns the address it listens on.
pub fn serve(bind: &str, shared: Arc<Shared>, guide: fn() -> Value) -> Result<SocketAddr, String> {
    let l = TcpListener::bind(bind).map_err(|e| format!("bind {bind}: {e}"))?;
    let addr = l.local_addr().map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("http".into())
        .spawn(move || {
            for c in l.incoming().flatten() {
                let sh = shared.clone();
                std::thread::spawn(move || serve_conn(c, &sh, &guide));
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn get(addr: SocketAddr, target: &str) -> (u16, String, Vec<u8>) {
        let mut s = TcpStream::connect(addr).unwrap();
        write!(s, "GET {target} HTTP/1.0\r\nHost: x\r\n\r\n").unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).unwrap();
        let end = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&raw[..end]).into_owned();
        let code = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        (code, head, raw[end + 4..].to_vec())
    }

    #[test]
    fn the_endpoints_answer() {
        let shared = Arc::new(Shared::default());
        *shared.status.lock().unwrap() = json!({"status": "streaming", "band": {"centre_hz": 2437000000u64, "rate_sps": 5000000}});
        shared
            .raw
            .lock()
            .unwrap()
            .extend((0..64u8).collect::<Vec<_>>());
        let addr = serve(
            "127.0.0.1:0",
            shared.clone(),
            || json!({"toml": "x", "pages": {}}),
        )
        .unwrap();
        let (c, _, b) = get(addr, "/status");
        assert_eq!(c, 200);
        assert_eq!(
            serde_json::from_slice::<Value>(&b).unwrap()["status"],
            "streaming"
        );
        let (c, h, b) = get(addr, "/raw?samples=4");
        assert_eq!((c, b.len()), (200, 16));
        assert_eq!(b[0], 48);
        assert!(h.contains("X-Centre-Hz: 2437000000") && h.contains("X-Rate-Sps: 5000000"));
        assert_eq!(get(addr, "/raw?samples=1000").2.len(), 64);
        let (c, _, b) = get(addr, "/healthz");
        assert_eq!(c, 503);
        assert!(String::from_utf8_lossy(&b).contains("streaming"));
        shared.healthy.store(true, Ordering::Release);
        assert_eq!(get(addr, "/healthz").0, 200);
        assert_eq!(get(addr, "/guide").0, 200);
        assert_eq!(get(addr, "/nope").0, 404);
        let mut s = TcpStream::connect(addr).unwrap();
        write!(s, "POST /status HTTP/1.0\r\n\r\n").unwrap();
        let mut r = String::new();
        s.read_to_string(&mut r).unwrap();
        assert!(r.starts_with("HTTP/1.1 405"));
    }

    #[test]
    fn raw_is_unavailable_before_the_first_block() {
        let shared = Shared::default();
        assert_eq!(route(&shared, &|| json!({}), "GET", "/raw").status, 503);
    }
}
