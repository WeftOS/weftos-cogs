//! Read-only HTTP/1.0 export on 127.0.0.1 (std only).
//!
//!   GET /           index
//!   GET /status     latest status object
//!   GET /readings   latest SenML pack, or a JSON object if there is no pack
//!   GET /guide      this cog's sensor guide (WeftOS ADR-104)
//!
//! An empty JSON array is never used for "no sample". That would look like a
//! successful pack with no records.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
pub struct State {
    pub status: Option<serde_json::Value>,
    pub pack: Option<String>,
}

pub type Shared = Arc<Mutex<State>>;

pub fn render(state: &State, method: &str, path: &str) -> (u16, &'static str, String) {
    if method != "GET" {
        return (405, "text/plain", "GET only\n".into());
    }
    match path.split('?').next().unwrap_or("") {
        "/" => (
            200,
            "text/plain",
            "bme680 export\n  /status\n  /readings\n  /guide\n".into(),
        ),
        "/status" => (
            200,
            "application/json",
            state.status.as_ref().map_or_else(
                || "{\"health\":\"starting\"}".to_string(),
                |v| v.to_string(),
            ),
        ),
        "/readings" => match &state.pack {
            Some(pack) => (200, "application/json", pack.clone()),
            None => {
                let health = state
                    .status
                    .as_ref()
                    .and_then(|s| s.get("health"))
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let error = state
                    .status
                    .as_ref()
                    .and_then(|s| s.get("error"))
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                (
                    200,
                    "application/json",
                    serde_json::json!({
                        "readings": null,
                        "health": health,
                        "error": error
                    })
                    .to_string(),
                )
            }
        },
        "/guide" => (
            200,
            "application/json",
            crate::guide::bundle_json().to_string(),
        ),
        _ => (404, "text/plain", "not found\n".into()),
    }
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
        Ok(state) => render(&state, method, path),
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
        "HTTP/1.0 {code} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Starts the export. A bind failure is reported and is not fatal: the status
/// line is still printed.
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
    fn no_pack_is_an_object_not_an_empty_array() {
        let state = State {
            status: Some(serde_json::json!({"health": "no_source", "error": "missing"})),
            pack: None,
        };
        let (code, _, body) = render(&state, "GET", "/readings");
        assert_eq!(code, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(v.is_object(), "{body}");
        assert!(v["readings"].is_null());
        assert_eq!(v["health"], "no_source");
    }

    #[test]
    fn guide_route_embeds_the_toml() {
        let (code, ctype, body) = render(&State::default(), "GET", "/guide");
        assert_eq!((code, ctype), (200, "application/json"));
        assert!(body.contains("schema = 1"));
        assert!(body.contains("\"start\""));
    }
}
