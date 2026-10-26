//! Read-only loopback HTTP export for a companion tool: the latest report, the last few,
//! the latest frame handle, the latest JPEG and the guide. Plain HTTP with
//! `Access-Control-Allow-Origin: *`, like the other cogs. The JPEG is served on loopback
//! only; frames never leave the node (plan 4.3, raw data stays where it is made).

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

const HISTORY: usize = 60;

#[derive(Default)]
pub struct State {
    pub latest: Option<serde_json::Value>,
    pub raw: Option<serde_json::Value>,
    pub frame: Option<Vec<u8>>,
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

/// A response body.
#[derive(Debug, PartialEq)]
pub enum Body {
    Json(String),
    Jpeg(Vec<u8>),
}

fn json(code: u16, v: String) -> (u16, Body) {
    (code, Body::Json(v))
}

/// `(status code, body)` for a request path.
pub fn route(path: &str, st: &State) -> (u16, Body) {
    let p = path.split('?').next().unwrap_or("/");
    let starting = || serde_json::json!({"status":"starting"}).to_string();
    match p {
        "/status" => json(
            200,
            st.latest.clone().map_or_else(starting, |v| v.to_string()),
        ),
        "/raw" => json(200, st.raw.clone().map_or_else(starting, |v| v.to_string())),
        "/history" => json(
            200,
            serde_json::json!({ "reports": st.history }).to_string(),
        ),
        "/frame.jpg" => match &st.frame {
            Some(b) => (200, Body::Jpeg(b.clone())),
            None => json(404, r#"{"error":"no frame yet"}"#.to_string()),
        },
        "/guide" => json(200, crate::guide::bundle_json().to_string()),
        "/healthz" => json(200, r#"{"ok":true}"#.to_string()),
        _ => json(404, r#"{"error":"not found"}"#.to_string()),
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
        Err(_) => json(500, r#"{"error":"state poisoned"}"#.to_string()),
    };
    let reason = match code {
        200 => "OK",
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    let (ctype, bytes) = match body {
        Body::Json(t) => ("application/json", t.into_bytes()),
        Body::Jpeg(b) => ("image/jpeg", b),
    };
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    s.write_all(head.as_bytes())?;
    s.write_all(&bytes)?;
    s.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(b: Body) -> String {
        match b {
            Body::Json(t) => t,
            Body::Jpeg(_) => panic!("not json"),
        }
    }

    #[test]
    fn routes_answer_and_unknown_is_404() {
        let mut st = State::default();
        assert_eq!(text(route("/status", &st).1), r#"{"status":"starting"}"#);
        assert_eq!(route("/frame.jpg", &st).0, 404);
        st.push(serde_json::json!({"status":"ok"}));
        st.raw = Some(serde_json::json!({"hash":"ab"}));
        st.frame = Some(vec![0xff, 0xd8, 0xff, 0xd9]);
        assert_eq!(text(route("/status", &st).1), r#"{"status":"ok"}"#);
        assert_eq!(text(route("/raw", &st).1), r#"{"hash":"ab"}"#);
        assert_eq!(
            route("/frame.jpg", &st),
            (200, Body::Jpeg(vec![0xff, 0xd8, 0xff, 0xd9]))
        );
        assert_eq!(
            text(route("/history?x=1", &st).1),
            r#"{"reports":[{"status":"ok"}]}"#
        );
        assert_eq!(route("/healthz", &st).0, 200);
        assert_eq!(route("/nope", &st).0, 404);
        assert!(text(route("/guide", &st).1).contains("\"pages\""));
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
