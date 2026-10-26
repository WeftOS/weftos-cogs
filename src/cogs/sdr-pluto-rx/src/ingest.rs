//! Posts the 8-float vector to the node's ingest bridge.
//!
//! The target is `$COGNITUM_INGEST_URL` when the node sets it (the bridge may be on a loopback
//! fallback port when :80 cannot be bound), else `127.0.0.1:80`. The URL must be `http://` on a
//! loopback host; anything else is ignored with a warning, so a stray value can never send
//! readings off the machine. `$COGNITUM_COG_TOKEN`, when present, goes as a bearer token.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub const ENV_URL: &str = "COGNITUM_INGEST_URL";
pub const ENV_TOKEN: &str = "COGNITUM_COG_TOKEN";
const DEFAULT_PATH: &str = "/api/v1/store/ingest";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub path: String,
}

impl Default for Target {
    fn default() -> Self {
        Target {
            host: "127.0.0.1".into(),
            port: 80,
            path: DEFAULT_PATH.into(),
        }
    }
}

fn is_loopback(host: &str) -> bool {
    host == "localhost"
        || host == "::1"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// `http://host[:port][/path]` on a loopback host. A bare base URL gets the standard path.
pub fn parse_url(url: &str) -> Result<Target, String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("{ENV_URL} must start with http://"))?;
    if rest.contains(|c: char| c.is_whitespace() || c == '@' || c == '?' || c == '#') {
        return Err(format!(
            "{ENV_URL} must not carry credentials, a query or whitespace"
        ));
    }
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (h, after) = v6.split_once(']').ok_or("bad IPv6 host")?;
        (h, after.strip_prefix(':'))
    } else {
        match authority.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        }
    };
    if !is_loopback(host) {
        return Err(format!("{ENV_URL} host {host:?} is not loopback"));
    }
    let port = match port {
        None => 80,
        Some(p) => p
            .parse::<u16>()
            .ok()
            .filter(|p| *p > 0)
            .ok_or_else(|| format!("bad port {p:?}"))?,
    };
    let path = if path.is_empty() || path == "/" {
        DEFAULT_PATH
    } else {
        path
    };
    Ok(Target {
        host: host.into(),
        port,
        path: path.into(),
    })
}

/// The target for an env value: the default when unset or empty, the default plus a warning
/// when the value is rejected.
pub fn target_from_env(value: Option<&str>) -> (Target, Option<String>) {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None => (Target::default(), None),
        Some(v) => match parse_url(v) {
            Ok(t) => (t, None),
            Err(e) => (
                Target::default(),
                Some(format!("{e}; ignoring it and using 127.0.0.1:80")),
            ),
        },
    }
}

pub fn store_to_seed(target: &Target, token: Option<&str>, vector: &[f64]) -> Result<(), String> {
    let payload = serde_json::json!({ "vectors": [[crate::STORE_ID, vector]], "dedup": true });
    let body = serde_json::to_vec(&payload).map_err(|e| format!("json: {e}"))?;
    let mut conn = TcpStream::connect((target.host.as_str(), target.port))
        .map_err(|e| format!("connect {}:{}: {e}", target.host, target.port))?;
    conn.set_read_timeout(Some(Duration::from_secs(5))).ok();
    conn.set_write_timeout(Some(Duration::from_secs(5))).ok();
    let auth = token
        .filter(|t| !t.is_empty() && !t.contains(['\r', '\n']))
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    write!(
        conn,
        "POST {} HTTP/1.0\r\nHost: {}\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        target.path,
        target.host,
        body.len()
    )
    .map_err(|e| format!("write: {e}"))?;
    conn.write_all(&body).map_err(|e| format!("body: {e}"))?;
    // The agent answers HTTP/1.1 and keeps the socket open; stop at Content-Length, not EOF.
    let mut resp = Vec::new();
    let mut buf = [0u8; 1024];
    while !response_complete(&resp) {
        match conn.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => resp.extend_from_slice(&buf[..n]),
        }
    }
    let status = String::from_utf8_lossy(&resp)
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_string();
    if status.starts_with('2') {
        Ok(())
    } else {
        Err(format!("ingest replied {status:?}"))
    }
}

fn response_complete(resp: &[u8]) -> bool {
    let Some(end) = resp.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let head = String::from_utf8_lossy(&resp[..end]).to_ascii_lowercase();
    let len = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    resp.len() >= end + 4 + len
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn unset_or_empty_env_uses_port_80() {
        for v in [None, Some(""), Some("  ")] {
            assert_eq!(target_from_env(v), (Target::default(), None));
        }
        assert_eq!(Target::default().port, 80);
        assert_eq!(Target::default().host, "127.0.0.1");
    }

    #[test]
    fn loopback_urls_are_used() {
        let (t, w) = target_from_env(Some("http://127.0.0.1:18080"));
        assert_eq!(
            (t.host.as_str(), t.port, t.path.as_str()),
            ("127.0.0.1", 18080, DEFAULT_PATH)
        );
        assert!(w.is_none());
        let t = parse_url("http://localhost:9/custom/ingest").unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.path.as_str()),
            ("localhost", 9, "/custom/ingest")
        );
        let t = parse_url("http://[::1]:7000/").unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.path.as_str()),
            ("::1", 7000, DEFAULT_PATH)
        );
        assert_eq!(parse_url("http://127.1.2.3").unwrap().port, 80);
    }

    #[test]
    fn non_loopback_or_odd_urls_are_ignored_with_a_warning() {
        for bad in [
            "http://10.0.0.5:80",
            "http://example.com/x",
            "http://127.0.0.1.evil.com/",
            "https://127.0.0.1:443",
            "http://user@127.0.0.1:80",
            "http://127.0.0.1:99999",
            "http://127.0.0.1:0",
            "http://127.0.0.1:80/a?b=1",
            "127.0.0.1:80",
        ] {
            let (t, w) = target_from_env(Some(bad));
            assert_eq!(t, Target::default(), "{bad}");
            assert!(w.is_some_and(|w| w.contains("ignoring")), "{bad}");
        }
    }

    #[test]
    fn post_goes_to_the_env_url_with_the_bearer_token() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let mut buf = vec![0u8; 4096];
            // the head and the body can arrive in separate segments
            let mut n = 0;
            while !buf[..n].ends_with(b"}") {
                n += c.read(&mut buf[n..]).unwrap();
            }
            c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                .unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        let (t, _) = target_from_env(Some(&format!("http://127.0.0.1:{port}")));
        store_to_seed(
            &t,
            Some("tok-1"),
            &[1.0, -1.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0],
        )
        .unwrap();
        let req = server.join().unwrap();
        assert!(
            req.starts_with("POST /api/v1/store/ingest HTTP/1.0\r\n"),
            "{req}"
        );
        assert!(req.contains("Authorization: Bearer tok-1\r\n"));
        assert!(req.contains("\"vectors\":[[29,[1.0,-1.0,"));
    }

    #[test]
    fn no_token_means_no_auth_header() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let mut buf = vec![0u8; 4096];
            // the head and the body can arrive in separate segments
            let mut n = 0;
            while !buf[..n].ends_with(b"}") {
                n += c.read(&mut buf[n..]).unwrap();
            }
            c.write_all(b"HTTP/1.1 503 X\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        let t = Target {
            port,
            ..Target::default()
        };
        let err = store_to_seed(&t, None, &[0.0; 8]).unwrap_err();
        assert!(err.contains("503"));
        assert!(!server.join().unwrap().contains("Authorization"));
    }

    #[test]
    fn response_complete_stops_at_content_length() {
        assert!(!response_complete(
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n"
        ));
        assert!(response_complete(
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"
        ));
    }
}
