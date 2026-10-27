//! Read-only HTTP/1.0 export on loopback (std only):
//!
//!   GET /          index
//!   GET /status    latest cog report
//!   GET /senml     latest SenML pack, or []
//!   GET /guide     this cog's sensor guide
//!   GET /spatial   spatial.evidence.v1 JSONL when `--spatial-out export` is on

#[cfg(feature = "spatial-evidence")]
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(feature = "spatial-evidence")]
pub const RING_SECONDS: u64 = 30;

#[derive(Default)]
pub struct Ring {
    pub source: String,
    pub latest_report: Option<serde_json::Value>,
    pub senml: Option<String>,
    #[cfg(feature = "spatial-evidence")]
    pub evidence: VecDeque<(u64, String)>,
}

pub type Shared = Arc<Mutex<Ring>>;

impl Ring {
    #[cfg(feature = "spatial-evidence")]
    pub fn push_evidence(&mut self, t_ms: u64, line: String) {
        let cutoff = t_ms.saturating_sub(RING_SECONDS * 1000);
        self.evidence.push_back((t_ms, line));
        while self.evidence.front().is_some_and(|x| x.0 < cutoff) {
            self.evidence.pop_front();
        }
    }
}

#[cfg(feature = "spatial-evidence")]
fn query_seconds(path: &str) -> f64 {
    path.split_once('?')
        .and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("seconds=")))
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(5.0)
}

pub fn render(ring: &Ring, method: &str, path: &str) -> (u16, &'static str, String) {
    if method != "GET" {
        return (405, "text/plain", "GET only\n".into());
    }
    match path.split('?').next().unwrap_or("") {
        "/" => (
            200,
            "text/plain",
            format!(
                "ld6004-radar export\n  /status\n  /senml\n  /guide\n{}\n",
                if cfg!(feature = "spatial-evidence") {
                    "  /spatial?seconds=N"
                } else {
                    ""
                }
            ),
        ),
        "/status" => (
            200,
            "application/json",
            ring.latest_report
                .as_ref()
                .map_or_else(|| "{\"status\":\"starting\"}".into(), |r| r.to_string()),
        ),
        "/senml" => (
            200,
            "application/json",
            ring.senml.clone().unwrap_or_else(|| "[]".into()),
        ),
        "/guide" => (
            200,
            "application/json",
            crate::guide::bundle_json().to_string(),
        ),
        "/spatial" => spatial_body(ring, path),
        _ => (404, "text/plain", "not found\n".into()),
    }
}

#[cfg(feature = "spatial-evidence")]
fn spatial_body(ring: &Ring, path: &str) -> (u16, &'static str, String) {
    let newest = ring.evidence.back().map_or(0, |x| x.0);
    let s = query_seconds(path).clamp(0.0, RING_SECONDS as f64);
    let cutoff = newest.saturating_sub((s * 1000.0) as u64);
    let mut out = String::new();
    for (_, line) in ring.evidence.iter().filter(|x| x.0 >= cutoff) {
        out.push_str(line);
        out.push('\n');
    }
    (200, "application/x-ndjson", out)
}

#[cfg(not(feature = "spatial-evidence"))]
fn spatial_body(_ring: &Ring, _path: &str) -> (u16, &'static str, String) {
    (404, "text/plain", "not found\n".into())
}

fn handle(mut conn: TcpStream, shared: &Shared) {
    conn.set_read_timeout(Some(Duration::from_secs(3))).ok();
    conn.set_write_timeout(Some(Duration::from_secs(5))).ok();
    let mut line = String::new();
    if BufReader::new(&conn).read_line(&mut line).is_err() {
        return;
    }
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
    let (code, ctype, body) = match shared.lock() {
        Ok(ring) => render(&ring, method, path),
        Err(_) => (500, "text/plain", "state poisoned\n".into()),
    };
    let reason = match code {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let _ = write!(
        conn,
        "HTTP/1.0 {code} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Bind failure is the caller's to report. The accept loop owns the listener.
pub fn serve(bind: &str, shared: Shared) -> Result<String, String> {
    let listener = TcpListener::bind(bind).map_err(|e| format!("bind {bind}: {e}"))?;
    let addr = listener
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| bind.to_string());
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            handle(conn, &shared);
        }
    });
    Ok(addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guide_and_senml_routes() {
        let mut ring = Ring {
            senml: Some("[{\"n\":\"work_mode\"}]".into()),
            ..Ring::default()
        };
        ring.latest_report = Some(serde_json::json!({"cog":"ld6004-radar"}));
        let (code, ctype, body) = render(&ring, "GET", "/guide");
        assert_eq!((code, ctype), (200, "application/json"));
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(v["toml"].as_str().unwrap().contains("schema = 1"));
        assert!(v["pages"].as_object().unwrap().contains_key("start"));
        let (code, _, body) = render(&ring, "GET", "/senml");
        assert_eq!(code, 200);
        assert!(body.starts_with('['));
        let (code, _, body) = render(&Ring::default(), "GET", "/senml");
        assert_eq!(body, "[]");
        assert_eq!(code, 200);
        let (code, _, _) = render(&ring, "PUT", "/status");
        assert_eq!(code, 405);
    }
}
