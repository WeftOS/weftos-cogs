//! Read-only loopback HTTP export for a companion tool: the latest report, the last few,
//! the raw latest sample and the guide. Plain HTTP with `Access-Control-Allow-Origin: *`,
//! like the other cogs.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

const HISTORY: usize = 60;

#[derive(Default)]
pub struct State {
    pub latest: Option<serde_json::Value>,
    pub raw: Option<serde_json::Value>,
    pub history: VecDeque<serde_json::Value>,
}

impl State {
    pub fn push(&mut self, report: serde_json::Value) {
        self.history.push_back(report.clone());
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
        self.latest = Some(report);
    }
}

pub type Shared = Arc<Mutex<State>>;

pub fn round(x: f64, places: i32) -> f64 {
    let f = 10f64.powi(places);
    (x * f).round() / f
}

/// `(status code, JSON body)` for a request path.
pub fn route(path: &str, st: &State) -> (u16, String) {
    let p = path.split('?').next().unwrap_or("/");
    match p {
        "/status" => (
            200,
            st.latest
                .clone()
                .unwrap_or_else(|| serde_json::json!({"status":"starting"}))
                .to_string(),
        ),
        "/raw" => (
            200,
            st.raw
                .clone()
                .unwrap_or_else(|| serde_json::json!({"status":"starting"}))
                .to_string(),
        ),
        "/history" => (
            200,
            serde_json::json!({ "reports": st.history }).to_string(),
        ),
        "/guide" => (200, crate::guide::bundle_json().to_string()),
        "/healthz" => (200, r#"{"ok":true}"#.to_string()),
        _ => (404, r#"{"error":"not found"}"#.to_string()),
    }
}

pub fn serve(bind: &str, shared: Shared) -> Result<SocketAddr, String> {
    let listener = TcpListener::bind(bind).map_err(|e| format!("bind {bind}: {e}"))?;
    let addr = listener.local_addr().map_err(|e| format!("addr: {e}"))?;
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = handle(stream, &shared);
        }
    });
    Ok(addr)
}

fn handle(mut s: TcpStream, shared: &Shared) -> std::io::Result<()> {
    s.set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .ok();
    let mut buf = [0u8; 1024];
    let n = s.read(&mut buf)?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let path = req
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .nth(1)
        .unwrap_or("/");
    let (code, body) = match shared.lock() {
        Ok(st) => route(path, &st),
        Err(_) => (500, r#"{"error":"state poisoned"}"#.to_string()),
    };
    let reason = match code {
        200 => "OK",
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes())?;
    s.write_all(body.as_bytes())?;
    s.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_answer_and_unknown_is_404() {
        let mut st = State::default();
        assert_eq!(route("/status", &st).1, r#"{"status":"starting"}"#);
        assert_eq!(route("/raw", &st).1, r#"{"status":"starting"}"#);
        st.push(serde_json::json!({"status":"ok"}));
        st.raw = Some(serde_json::json!({"rate_hz":16000}));
        assert_eq!(route("/status", &st).1, r#"{"status":"ok"}"#);
        assert_eq!(route("/raw", &st).1, r#"{"rate_hz":16000}"#);
        assert_eq!(
            route("/history?x=1", &st).1,
            r#"{"reports":[{"status":"ok"}]}"#
        );
        assert_eq!(route("/healthz", &st).0, 200);
        assert_eq!(route("/nope", &st).0, 404);
        assert!(route("/guide", &st).1.contains("\"pages\""));
    }

    #[test]
    fn history_is_bounded() {
        let mut st = State::default();
        for i in 0..(HISTORY + 5) {
            st.push(serde_json::json!({ "i": i }));
        }
        assert_eq!(st.history.len(), HISTORY);
        assert_eq!(st.history.front().unwrap()["i"], 5);
    }
}
