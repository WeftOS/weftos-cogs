//! Read-only HTTP export on 127.0.0.1. The latest report, a short distance
//! trace, and the guide. Person-data stays on loopback: there is no flag to
//! bind 0.0.0.0.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use crate::lines::{Reading, Stats};

pub const RING_SECONDS: u64 = 15;
#[cfg(feature = "spatial-evidence")]
const EVIDENCE_SECONDS: u64 = 30;
const MAX_READINGS: usize = 4096;

#[derive(Clone, Copy)]
pub struct Sample {
    pub t_ms: u64,
    pub distance_m: f64,
}

pub struct State {
    pub source: String,
    pub simulated: bool,
    pub readings: VecDeque<(u64, Reading)>,
    pub stats: Stats,
    pub reported: Stats,
    pub latest_report: Option<serde_json::Value>,
    #[cfg(feature = "spatial-evidence")]
    pub evidence: VecDeque<(u64, String)>,
    #[cfg(feature = "spatial-evidence")]
    pub spatial_error: Option<String>,
}

impl State {
    pub fn new() -> Self {
        Self {
            source: "none".into(),
            simulated: false,
            readings: VecDeque::new(),
            stats: Stats::default(),
            reported: Stats::default(),
            latest_report: None,
            #[cfg(feature = "spatial-evidence")]
            evidence: VecDeque::new(),
            #[cfg(feature = "spatial-evidence")]
            spatial_error: None,
        }
    }

    pub fn push_reading(&mut self, t_ms: u64, reading: Reading) {
        self.readings.push_back((t_ms, reading));
        while self.readings.len() > MAX_READINGS {
            self.readings.pop_front();
        }
    }

    /// Accepted lines whose time is inside the window ending at `now_ms`.
    /// Older lines are dropped and are not carried into this report.
    pub fn in_window(&mut self, now_ms: u64, window_s: u64) -> Vec<Reading> {
        let cutoff = now_ms.saturating_sub(window_s.saturating_mul(1000));
        while self.readings.front().is_some_and(|(t, _)| *t < cutoff) {
            self.readings.pop_front();
        }
        self.readings.iter().map(|(_, r)| *r).collect()
    }

    #[cfg(feature = "spatial-evidence")]
    pub fn push_evidence(&mut self, t_ms: u64, line: String) {
        let cutoff = t_ms.saturating_sub(EVIDENCE_SECONDS * 1000);
        self.evidence.push_back((t_ms, line));
        while self.evidence.front().is_some_and(|x| x.0 < cutoff) {
            self.evidence.pop_front();
        }
    }

    #[cfg(feature = "spatial-evidence")]
    pub fn evidence_jsonl(&self, seconds: f64) -> String {
        let newest = self.evidence.back().map_or(0, |x| x.0);
        let s = if seconds.is_finite() {
            seconds.clamp(0.0, EVIDENCE_SECONDS as f64)
        } else {
            5.0
        };
        let cutoff = newest.saturating_sub((s * 1000.0) as u64);
        let mut out = String::new();
        for (_, line) in self.evidence.iter().filter(|x| x.0 >= cutoff) {
            out.push_str(line);
            out.push('\n');
        }
        out
    }

    pub fn tail(&self, seconds: u64, now_ms: u64) -> Vec<Sample> {
        let cutoff = now_ms.saturating_sub(seconds.saturating_mul(1000));
        self.readings
            .iter()
            .filter(|(t, _)| *t >= cutoff)
            .map(|(t, r)| Sample {
                t_ms: *t,
                distance_m: r.distance_m,
            })
            .collect()
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

pub type Shared = Arc<Mutex<State>>;

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

    #[cfg(feature = "spatial-evidence")]
    if path.starts_with("/spatial") {
        let seconds = path
            .split_once("seconds=")
            .and_then(|(_, v)| v.split('&').next())
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(5.0);
        let body = shared.lock().unwrap().evidence_jsonl(seconds);
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
            body.len()
        );
        s.write_all(resp.as_bytes())?;
        s.write_all(body.as_bytes())?;
        return s.flush();
    }

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let body: String = if path.starts_with("/status") {
        let st = shared.lock().unwrap();
        st.latest_report
            .clone()
            .unwrap_or_else(|| serde_json::json!({"status": "starting"}))
            .to_string()
    } else if path.starts_with("/raw") {
        let st = shared.lock().unwrap();
        let samples: Vec<serde_json::Value> = st
            .tail(RING_SECONDS, now_ms)
            .iter()
            .map(|s| serde_json::json!({"t_ms": s.t_ms, "distance_m": s.distance_m}))
            .collect();
        serde_json::json!({
            "source": st.source,
            "simulated": st.simulated,
            "samples": samples
        })
        .to_string()
    } else if path.starts_with("/guide") {
        crate::guide::bundle_json().to_string()
    } else if path.starts_with("/healthz") {
        r#"{"ok":true}"#.to_string()
    } else {
        r#"{"error":"not found"}"#.to_string()
    };

    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(resp.as_bytes())?;
    s.write_all(body.as_bytes())?;
    s.flush()
}

#[cfg(all(test, feature = "spatial-evidence"))]
mod evidence_tests {
    use super::*;

    #[test]
    fn evidence_ring_keeps_thirty_seconds_and_serves_jsonl() {
        let mut st = State::new();
        for i in 0..400u64 {
            st.push_evidence(1_000 + i * 100, format!("{{\"i\":{i}}}"));
        }
        assert_eq!(st.evidence.len(), 301);
        let body = st.evidence_jsonl(1.0);
        assert_eq!(body.lines().count(), 11);
        assert!(body.ends_with("{\"i\":399}\n"));
        assert_eq!(st.evidence_jsonl(f64::NAN).lines().count(), 51);
    }
}
