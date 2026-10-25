//! A small blocking HTTP/1.1 client for the loopback services this cog calls
//! (GenieX, the image service). `Connection: close`, so the body ends at EOF
//! or at the chunked terminator. Only loopback addresses are accepted: GenieX
//! has no authentication, so the cog never talks to it, or lets a config
//! point it, anywhere else.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Largest response body read.
const MAX_BODY: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub struct Resp {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

/// `host:port` that resolves only to loopback addresses.
pub fn loopback_addr(hostport: &str) -> Result<SocketAddr, String> {
    let addrs: Vec<SocketAddr> = hostport
        .to_socket_addrs()
        .map_err(|e| format!("{hostport}: {e}"))?
        .collect();
    if addrs.is_empty() || addrs.iter().any(|a| !a.ip().is_loopback()) {
        return Err(format!("{hostport} is not a loopback address"));
    }
    Ok(addrs[0])
}

/// One request. `timeout` bounds connect, each read and each write.
pub fn request(
    hostport: &str,
    method: &str,
    path: &str,
    json_body: Option<&[u8]>,
    timeout: Duration,
) -> Result<Resp, String> {
    let addr = loopback_addr(hostport)?;
    let mut s =
        TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("connect {addr}: {e}"))?;
    s.set_read_timeout(Some(timeout)).ok();
    s.set_write_timeout(Some(timeout)).ok();
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {hostport}\r\nConnection: close\r\nAccept: */*\r\n"
    );
    if let Some(b) = json_body {
        head.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            b.len()
        ));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    if let Some(b) = json_body {
        s.write_all(b).map_err(|e| format!("write: {e}"))?;
    }
    let mut raw = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if raw.len() > MAX_BODY + 8192 {
                    return Err("response too large".into());
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                return Err(format!("read timed out after {timeout:?}"));
            }
            Err(e) => return Err(format!("read: {e}")),
        }
    }
    parse(&raw)
}

fn parse(raw: &[u8]) -> Result<Resp, String> {
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("no end of headers")?;
    let head = String::from_utf8_lossy(&raw[..end]).into_owned();
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or("bad status line")?;
    let chunked = lines.any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    });
    let rest = &raw[end + 4..];
    let body = if chunked {
        dechunk(rest)?
    } else {
        rest.to_vec()
    };
    Ok(Resp { status, body })
}

fn dechunk(mut b: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let eol = b
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or("bad chunk header")?;
        let size_txt = String::from_utf8_lossy(&b[..eol]);
        let size = usize::from_str_radix(size_txt.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| "bad chunk size".to_string())?;
        b = &b[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if b.len() < size + 2 {
            return Err("truncated chunk".into());
        }
        out.extend_from_slice(&b[..size]);
        b = &b[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_content_length_and_chunked_bodies() {
        let r = parse(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi").unwrap();
        assert_eq!((r.status, r.text().as_str()), (200, "hi"));
        let r = parse(
            b"HTTP/1.1 503 X\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n",
        )
        .unwrap();
        assert_eq!((r.status, r.text().as_str()), (503, "abcde"));
        assert!(parse(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nab").is_err());
    }

    #[test]
    fn only_loopback_is_reachable() {
        assert!(loopback_addr("127.0.0.1:18181").is_ok());
        assert!(loopback_addr("localhost:18181").is_ok());
        assert!(loopback_addr("0.0.0.0:18181").is_err());
        assert!(loopback_addr("192.0.2.10:18181").is_err());
        assert!(request("10.0.0.1:80", "GET", "/", None, Duration::from_millis(50)).is_err());
    }
}
