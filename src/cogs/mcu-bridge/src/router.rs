//! Calls to the WeftOS sketch through the board's arduino-router (msgpack-RPC on a Unix
//! socket). One connection per call, like the spike's client: a call takes 1.5 to 5 ms, and
//! a second router client on the same socket is fine (the MCU serialises).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::msgpack;

pub const DEFAULT_SOCKET: &str = "/run/arduino-router.sock";

#[derive(Debug, Clone, PartialEq)]
pub enum RpcError {
    /// The socket could not be opened or died.
    Unavailable(String),
    /// The router or the sketch answered with an error.
    Remote(Value),
    /// The answer was not msgpack-RPC.
    Protocol(String),
    Timeout,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::Unavailable(m) => write!(f, "router unavailable: {m}"),
            RpcError::Remote(v) => write!(
                f,
                "{}",
                v.as_str().map_or_else(|| v.to_string(), String::from)
            ),
            RpcError::Protocol(m) => write!(f, "bad router answer: {m}"),
            RpcError::Timeout => f.write_str("the MCU did not answer in time"),
        }
    }
}

pub trait Router {
    fn call(
        &mut self,
        method: &str,
        params: &[Value],
        timeout: Duration,
    ) -> Result<Value, RpcError>;
    fn label(&self) -> String;
}

/// The real router.
pub struct SocketRouter {
    pub path: String,
    msgid: u32,
}

impl SocketRouter {
    pub fn new(path: &str) -> Self {
        Self {
            path: path.into(),
            msgid: 0,
        }
    }
}

impl Router for SocketRouter {
    fn label(&self) -> String {
        format!("router:{}", self.path)
    }

    fn call(
        &mut self,
        method: &str,
        params: &[Value],
        timeout: Duration,
    ) -> Result<Value, RpcError> {
        self.msgid = self.msgid.wrapping_add(1).max(1);
        let id = self.msgid;
        let req = msgpack::request(id, method, params).map_err(RpcError::Protocol)?;
        let mut s = UnixStream::connect(&self.path)
            .map_err(|e| RpcError::Unavailable(format!("{}: {e}", self.path)))?;
        s.set_write_timeout(Some(timeout)).ok();
        s.write_all(&req)
            .map_err(|e| RpcError::Unavailable(e.to_string()))?;
        let end = Instant::now() + timeout;
        let (mut buf, mut chunk) = (Vec::new(), [0u8; 1024]);
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(RpcError::Timeout);
            }
            s.set_read_timeout(Some(left)).ok();
            match s.read(&mut chunk) {
                Ok(0) => {
                    return Err(RpcError::Unavailable(
                        "the router closed the connection".into(),
                    ))
                }
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Err(RpcError::Timeout)
                }
                Err(e) => return Err(RpcError::Unavailable(e.to_string())),
            }
            while !buf.is_empty() {
                match msgpack::decode(&buf) {
                    Ok((msg, n)) => {
                        buf.drain(..n);
                        if let Some(r) = response_for(&msg, id)? {
                            return r;
                        }
                    }
                    Err(e) if e == "truncated" => break,
                    Err(e) => return Err(RpcError::Protocol(e)),
                }
            }
        }
    }
}

/// `[1, msgid, error, result]` for `id`: its outcome; anything else is not ours.
fn response_for(msg: &Value, id: u32) -> Result<Option<Result<Value, RpcError>>, RpcError> {
    let a = msg
        .as_array()
        .ok_or_else(|| RpcError::Protocol("not an array".into()))?;
    if a.first().and_then(Value::as_u64) != Some(1)
        || a.get(1).and_then(Value::as_u64) != Some(id as u64)
    {
        return Ok(None);
    }
    let (err, res) = (
        a.get(2).cloned().unwrap_or(Value::Null),
        a.get(3).cloned().unwrap_or(Value::Null),
    );
    Ok(Some(if err.is_null() {
        Ok(res)
    } else {
        Err(RpcError::Remote(err))
    }))
}

/// A router with canned answers: `--simulate` and `--replay`, and the tests. `weftos_calls`
/// counts up like the sketch does.
pub struct FakeRouter {
    answers: HashMap<String, Value>,
    calls: u64,
    label: String,
}

impl FakeRouter {
    pub fn simulated() -> Self {
        Self {
            answers: HashMap::from([("weftos_ping".to_string(), json!(42))]),
            calls: 0,
            label: "simulated".into(),
        }
    }

    /// Answers from JSON lines `{"method": "...", "result": ...}`.
    pub fn replay(text: &str) -> Result<Self, String> {
        let mut answers = HashMap::new();
        for (i, l) in text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
        {
            let v: Value = serde_json::from_str(l).map_err(|e| format!("line {}: {e}", i + 1))?;
            let m = v["method"]
                .as_str()
                .ok_or_else(|| format!("line {}: no method", i + 1))?;
            answers.insert(m.to_string(), v["result"].clone());
        }
        Ok(Self {
            answers,
            calls: 0,
            label: "replay".into(),
        })
    }
}

impl Router for FakeRouter {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn call(
        &mut self,
        method: &str,
        _params: &[Value],
        _timeout: Duration,
    ) -> Result<Value, RpcError> {
        if method == "weftos_calls" {
            self.calls += 1;
            return Ok(json!(self.calls));
        }
        self.answers
            .get(method)
            .cloned()
            .ok_or_else(|| RpcError::Remote(json!(format!("method {method} not available"))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    fn serve(
        answer: impl Fn(&Value) -> Vec<u8> + Send + 'static,
    ) -> (String, std::thread::JoinHandle<()>) {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mcu-router-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("r.sock");
        let l = UnixListener::bind(&path).unwrap();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = [0u8; 256];
            let n = s.read(&mut buf).unwrap();
            let (req, _) = msgpack::decode(&buf[..n]).unwrap();
            s.write_all(&answer(&req)).unwrap();
        });
        (path.to_string_lossy().into(), h)
    }

    fn reply(id: &Value, err: Value, res: Value) -> Vec<u8> {
        let mut o = Vec::new();
        msgpack::encode(&json!([1, id, err, res]), &mut o).unwrap();
        o
    }

    #[test]
    fn a_call_reaches_the_router_and_the_result_comes_back() {
        let (path, h) = serve(|req| {
            assert_eq!(req[0], 0);
            assert_eq!(
                (req[2].as_str(), req[3].clone()),
                (Some("weftos_ping"), json!([]))
            );
            // A notification first, then a response for another id, then ours.
            let mut o = Vec::new();
            msgpack::encode(&json!([2, "log", ["hi"]]), &mut o).unwrap();
            o.extend(reply(&json!(999), Value::Null, json!(0)));
            o.extend(reply(&req[1], Value::Null, json!(42)));
            o
        });
        let mut r = SocketRouter::new(&path);
        assert_eq!(
            r.call("weftos_ping", &[], Duration::from_secs(2)).unwrap(),
            42
        );
        h.join().unwrap();
    }

    #[test]
    fn a_sketch_error_is_a_remote_error() {
        let (path, h) = serve(|req| reply(&req[1], json!("method not found"), Value::Null));
        let e = SocketRouter::new(&path)
            .call("nope", &[], Duration::from_secs(2))
            .unwrap_err();
        assert_eq!(e, RpcError::Remote(json!("method not found")));
        assert_eq!(e.to_string(), "method not found");
        h.join().unwrap();
    }

    #[test]
    fn a_missing_socket_is_unavailable_and_a_silent_router_times_out() {
        let e = SocketRouter::new("/nonexistent/r.sock")
            .call("x", &[], Duration::from_millis(100))
            .unwrap_err();
        assert!(matches!(e, RpcError::Unavailable(_)), "{e:?}");
        let (path, h) = serve(|_| Vec::new());
        let e = SocketRouter::new(&path)
            .call("x", &[], Duration::from_millis(100))
            .unwrap_err();
        assert!(
            matches!(e, RpcError::Unavailable(_) | RpcError::Timeout),
            "{e:?}"
        );
        h.join().unwrap();
    }

    #[test]
    fn the_fake_router_counts_calls_like_the_sketch_and_replays_answers() {
        let mut f = FakeRouter::simulated();
        let t = Duration::from_secs(1);
        assert_eq!(f.call("weftos_ping", &[], t).unwrap(), 42);
        assert_eq!(
            (
                f.call("weftos_calls", &[], t).unwrap(),
                f.call("weftos_calls", &[], t).unwrap()
            ),
            (json!(1), json!(2))
        );
        assert!(matches!(f.call("other", &[], t), Err(RpcError::Remote(_))));
        let mut r = FakeRouter::replay("{\"method\":\"weftos_ping\",\"result\":7}\n\n{\"method\":\"adc.read\",\"result\":[1,2]}\n").unwrap();
        assert_eq!(
            (
                r.call("weftos_ping", &[], t).unwrap(),
                r.call("adc.read", &[], t).unwrap()
            ),
            (json!(7), json!([1, 2]))
        );
        assert!(FakeRouter::replay("not json").is_err());
    }
}
