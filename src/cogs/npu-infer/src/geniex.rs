//! The real backend: GenieX's OpenAI-compatible server, one process per
//! resident model on loopback ports from `base_port` up.
//!
//! GenieX has no unload call and loads a model on its first request, so:
//! `load` starts a process, sends a one-token request to force the load and
//! measures the memory it took; `unload` kills the process. GenieX listens
//! on loopback only and has no authentication; this is the only client, and
//! [`crate::http`] refuses any other address.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::backend::{Backend, BackendError, Fault, Output, Request};
use crate::catalog::Entry;
use crate::http;
use crate::launcher::{Launcher, Proc};
use crate::mem;

struct Worker {
    port: u16,
    proc: Box<dyn Proc>,
}

pub struct GeniexBackend {
    launcher: Box<dyn Launcher>,
    base_port: u16,
    slots: u16,
    workers: HashMap<String, Worker>,
    /// Directory holding `proc/` (tests point it at a fixture tree).
    proc_root: PathBuf,
    boot_timeout: Duration,
    request_timeout: Duration,
}

impl GeniexBackend {
    pub fn new(launcher: Box<dyn Launcher>, base_port: u16, slots: u16) -> Self {
        Self {
            launcher,
            base_port,
            slots: slots.max(1),
            workers: HashMap::new(),
            proc_root: PathBuf::from("/"),
            boot_timeout: Duration::from_secs(30),
            request_timeout: Duration::from_secs(180),
        }
    }

    #[cfg(test)]
    pub fn with_timeouts(mut self, boot: Duration, request: Duration) -> Self {
        self.boot_timeout = boot;
        self.request_timeout = request;
        self
    }

    fn free_port(&self) -> Option<u16> {
        (self.base_port..self.base_port.saturating_add(self.slots))
            .find(|p| !self.workers.values().any(|w| w.port == *p))
    }

    fn addr(port: u16) -> String {
        format!("127.0.0.1:{port}")
    }

    /// Start the server for `e` and wait for it to answer.
    fn boot(&mut self, e: &Entry) -> Result<(), BackendError> {
        let port = self
            .free_port()
            .ok_or_else(|| BackendError::new(Fault::Failed, "no free GenieX slot"))?;
        let mut proc = self
            .launcher
            .spawn(port, e)
            .map_err(|m| BackendError::new(Fault::LoadFailed, m))?;
        let until = Instant::now() + self.boot_timeout;
        loop {
            if !proc.alive() {
                let why = proc.diagnostic();
                return Err(BackendError::new(
                    Fault::LoadFailed,
                    if why.is_empty() {
                        "geniex exited while starting".to_string()
                    } else {
                        format!("geniex exited while starting: {why}")
                    },
                ));
            }
            let up = http::request(
                &Self::addr(port),
                "GET",
                "/status",
                None,
                Duration::from_secs(2),
            )
            .ok()
            .and_then(|r| r.json())
            .and_then(|v| v.get("alive").and_then(Value::as_bool));
            if up == Some(true) {
                break;
            }
            if Instant::now() >= until {
                proc.kill();
                return Err(BackendError::new(
                    Fault::LoadFailed,
                    "geniex did not come up",
                ));
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        self.workers.insert(e.alias.clone(), Worker { port, proc });
        Ok(())
    }

    fn chat(&self, port: u16, e: &Entry, body: &Value) -> Result<Output, BackendError> {
        let raw = serde_json::to_vec(body)
            .map_err(|x| BackendError::new(Fault::Failed, x.to_string()))?;
        let r = http::request(
            &Self::addr(port),
            "POST",
            "/v1/chat/completions",
            Some(&raw),
            self.request_timeout,
        )
        .map_err(|m| BackendError::new(Fault::Dead, m))?;
        let text = r.text();
        if r.status != 200 {
            let low = text.to_ascii_lowercase();
            let fault = if low.contains("loading failed") || low.contains("load failed") {
                Fault::LoadFailed
            } else {
                Fault::Failed
            };
            return Err(BackendError::new(
                fault,
                format!("geniex {} for {}: {}", r.status, e.alias, clip(&text)),
            ));
        }
        let v = r.json().ok_or_else(|| {
            BackendError::new(Fault::Failed, "geniex answered something that is not JSON")
        })?;
        let choice = v.pointer("/choices/0");
        let out = choice
            .and_then(|c| c.pointer("/message/content"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                BackendError::new(Fault::Failed, format!("no content in {}", clip(&text)))
            })?;
        let tok = |k: &str| {
            v.pointer(&format!("/usage/{k}"))
                .and_then(Value::as_u64)
                .unwrap_or(0)
        };
        Ok(Output {
            text: out.to_string(),
            finish: choice
                .and_then(|c| c.get("finish_reason"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            prompt_tokens: tok("prompt_tokens"),
            completion_tokens: tok("completion_tokens"),
        })
    }
}

fn clip(s: &str) -> String {
    s.chars().take(300).collect()
}

/// The OpenAI chat body for `req`.
pub fn chat_body(e: &Entry, req: &Request) -> Value {
    let mut messages = Vec::new();
    if let Some(s) = &req.system {
        messages.push(json!({"role": "system", "content": s}));
    }
    let text = req.text.clone().unwrap_or_default();
    let content = match &req.image {
        Some((mime, b64)) => json!([
            {"type": "text", "text": text},
            {"type": "image_url", "image_url": {"url": format!("data:{mime};base64,{b64}")}},
        ]),
        None => Value::String(text),
    };
    messages.push(json!({"role": "user", "content": content}));
    json!({"model": e.geniex_id, "messages": messages, "max_tokens": req.max_tokens, "stream": false})
}

impl Backend for GeniexBackend {
    fn load(&mut self, e: &Entry) -> Result<u64, BackendError> {
        if self.workers.contains_key(&e.alias) {
            self.unload(&e.alias);
        }
        let before = mem::mem_available_mib(&self.proc_root);
        self.boot(e)?;
        let (port, pid) = self
            .workers
            .get(&e.alias)
            .map(|w| (w.port, w.proc.pid()))
            .ok_or_else(|| BackendError::new(Fault::Failed, "worker vanished"))?;
        // GenieX loads on the first request: ask for one token.
        let warm = Request {
            system: None,
            text: Some("hi".into()),
            image: None,
            image_blob: None,
            max_tokens: 1,
        };
        if let Err(err) = self.chat(port, e, &chat_body(e, &warm)) {
            self.unload(&e.alias);
            // Whatever failed, the model did not load: the caller restarts it.
            return Err(BackendError::new(
                if err.fault == Fault::Failed {
                    Fault::LoadFailed
                } else {
                    err.fault
                },
                err.msg,
            ));
        }
        let after = mem::mem_available_mib(&self.proc_root);
        let delta = match (before, after) {
            (Some(b), Some(a)) => b.saturating_sub(a),
            _ => 0,
        };
        let rss = pid
            .and_then(|p| mem::rss_mib(&self.proc_root, p))
            .unwrap_or(0);
        Ok(delta.max(rss).max(64))
    }

    fn unload(&mut self, alias: &str) {
        if let Some(mut w) = self.workers.remove(alias) {
            w.proc.kill();
        }
    }

    fn infer(&mut self, e: &Entry, req: &Request) -> Result<Output, BackendError> {
        let w = self
            .workers
            .get_mut(&e.alias)
            .ok_or_else(|| BackendError::new(Fault::Dead, format!("{} is not loaded", e.alias)))?;
        if !w.proc.alive() {
            return Err(BackendError::new(Fault::Dead, "geniex exited"));
        }
        let port = w.port;
        self.chat(port, e, &chat_body(e, req))
    }

    fn reset(&mut self, alias: &str) {
        self.unload(alias);
    }

    fn mem_available_mib(&self) -> Option<u64> {
        mem::mem_available_mib(&self.proc_root)
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! A GenieX that is a thread in the test process, so nothing real starts.

    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    pub struct Script {
        /// Fail this many first chat requests with a model-loading error.
        pub fail_chats: AtomicUsize,
        pub chats: AtomicUsize,
        pub spawns: AtomicUsize,
        pub last_body: Mutex<Option<Value>>,
    }

    pub struct FakeProc {
        alive: Arc<AtomicBool>,
        diag: String,
    }

    impl Proc for FakeProc {
        fn diagnostic(&self) -> String {
            self.diag.clone()
        }

        fn alive(&mut self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }

        fn pid(&self) -> Option<u32> {
            None
        }

        fn kill(&mut self) {
            self.alive.store(false, Ordering::SeqCst);
        }
    }

    pub struct FakeLauncher {
        pub script: Arc<Script>,
        pub refuse: bool,
        /// Start and exit at once, leaving this as the server's last words.
        pub die_with: Option<String>,
    }

    fn respond(s: &mut std::net::TcpStream, code: u16, body: &str) {
        let _ = write!(
            s,
            "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    }

    impl Launcher for FakeLauncher {
        fn spawn(&mut self, port: u16, _: &Entry) -> Result<Box<dyn Proc>, String> {
            if self.refuse {
                return Err("wrapper not found".into());
            }
            if let Some(why) = &self.die_with {
                return Ok(Box::new(FakeProc {
                    alive: Arc::new(AtomicBool::new(false)),
                    diag: why.clone(),
                }));
            }
            self.script.spawns.fetch_add(1, Ordering::SeqCst);
            // The previous server on this port may still be closing.
            let mut tries = 0;
            let l = loop {
                match TcpListener::bind(("127.0.0.1", port)) {
                    Ok(l) => break l,
                    Err(e) if tries >= 100 => return Err(e.to_string()),
                    Err(_) => {
                        tries += 1;
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
            };
            l.set_nonblocking(true).ok();
            let alive = Arc::new(AtomicBool::new(true));
            let (a, sc) = (alive.clone(), self.script.clone());
            std::thread::spawn(move || {
                while a.load(Ordering::SeqCst) {
                    let Ok((mut s, _)) = l.accept() else {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    };
                    s.set_nonblocking(false).ok();
                    s.set_read_timeout(Some(Duration::from_secs(2))).ok();
                    let mut buf = vec![0u8; 256 * 1024];
                    let n = s.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                    if req.starts_with("GET /status") {
                        respond(&mut s, 200, r#"{"alive":true,"models":[]}"#);
                    } else if req.starts_with("POST /v1/chat/completions") {
                        let body = req.split("\r\n\r\n").nth(1).unwrap_or("");
                        *sc.last_body.lock().unwrap() = serde_json::from_str(body).ok();
                        sc.chats.fetch_add(1, Ordering::SeqCst);
                        if sc.fail_chats.load(Ordering::SeqCst) > 0 {
                            sc.fail_chats.fetch_sub(1, Ordering::SeqCst);
                            respond(&mut s, 500, r#"{"error":"SDKError(Model loading failed)"}"#);
                        } else {
                            respond(
                                &mut s,
                                200,
                                r#"{"choices":[{"finish_reason":"stop","message":{"content":"hello"}}],"usage":{"prompt_tokens":3,"completion_tokens":1}}"#,
                            );
                        }
                    } else {
                        respond(&mut s, 404, "{}");
                    }
                }
            });
            Ok(Box::new(FakeProc {
                alive,
                diag: String::new(),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;
    use crate::catalog::{Catalog, Kind};
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    fn port() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    }

    fn backend(script: &Arc<Script>) -> GeniexBackend {
        GeniexBackend::new(
            Box::new(FakeLauncher {
                script: script.clone(),
                refuse: false,
                die_with: None,
            }),
            port(),
            1,
        )
        .with_timeouts(Duration::from_secs(3), Duration::from_secs(5))
    }

    fn model() -> Entry {
        Catalog::ventuno().resolve("gemma3-1b").unwrap().clone()
    }

    #[test]
    fn load_starts_a_server_forces_the_load_and_infer_uses_it() {
        let script = Arc::new(Script::default());
        let mut b = backend(&script);
        let e = model();
        assert!(b.load(&e).unwrap() >= 64);
        assert_eq!(script.spawns.load(Ordering::SeqCst), 1);
        assert_eq!(
            script.chats.load(Ordering::SeqCst),
            1,
            "one warm-up request"
        );
        let out = b
            .infer(
                &e,
                &Request {
                    system: Some("s".into()),
                    text: Some("q".into()),
                    image: None,
                    image_blob: None,
                    max_tokens: 8,
                },
            )
            .unwrap();
        assert_eq!(
            (
                out.text.as_str(),
                out.finish.as_str(),
                out.prompt_tokens,
                out.completion_tokens
            ),
            ("hello", "stop", 3, 1)
        );
        let sent = script.last_body.lock().unwrap().clone().unwrap();
        assert_eq!(sent["model"], e.geniex_id);
        assert_eq!(sent["messages"][0]["role"], "system");
        assert_eq!(sent["max_tokens"], 8);
        b.unload(&e.alias);
        assert!(matches!(
            b.infer(
                &e,
                &Request {
                    system: None,
                    text: Some("q".into()),
                    image: None,
                    image_blob: None,
                    max_tokens: 1
                }
            ),
            Err(BackendError {
                fault: Fault::Dead,
                ..
            })
        ));
    }

    #[test]
    fn a_failed_load_is_a_load_failure_and_leaves_no_process() {
        let script = Arc::new(Script::default());
        script.fail_chats.store(1, Ordering::SeqCst);
        let mut b = backend(&script);
        let e = model();
        let err = b.load(&e).unwrap_err();
        assert_eq!(err.fault, Fault::LoadFailed);
        assert!(err.msg.contains("loading failed"), "{}", err.msg);
        // the slot is free again: a second try starts a fresh process and works
        assert!(b.load(&e).is_ok());
        assert_eq!(script.spawns.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_wrapper_that_will_not_start_is_a_load_failure() {
        let script = Arc::new(Script::default());
        let mut b = GeniexBackend::new(
            Box::new(FakeLauncher {
                script,
                refuse: true,
                die_with: None,
            }),
            port(),
            1,
        );
        assert_eq!(b.load(&model()).unwrap_err().fault, Fault::LoadFailed);
    }

    #[test]
    fn a_server_that_dies_at_start_tells_why() {
        let script = Arc::new(Script::default());
        let l = FakeLauncher {
            script,
            refuse: false,
            die_with: Some("fatal error: failed to reserve page summary memory".into()),
        };
        let mut b = GeniexBackend::new(Box::new(l), port(), 1);
        let e = b.load(&model()).unwrap_err();
        assert_eq!(e.fault, Fault::LoadFailed);
        assert!(
            e.msg
                .contains("exited while starting: fatal error: failed to reserve"),
            "{}",
            e.msg
        );
    }

    #[test]
    fn a_vlm_request_carries_the_image_as_a_data_uri() {
        let e = Catalog::ventuno().resolve("qwen3-vl-4b").unwrap().clone();
        assert_eq!(e.kind, Kind::Vlm);
        let body = chat_body(
            &e,
            &Request {
                system: None,
                text: Some("what?".into()),
                image: Some(("image/png".into(), "AAAA".into())),
                image_blob: None,
                max_tokens: 4,
            },
        );
        assert_eq!(
            body["messages"][0]["content"][1]["image_url"]["url"],
            "data:image/png;base64,AAAA"
        );
    }

    #[test]
    fn a_process_that_died_is_reported_dead() {
        let script = Arc::new(Script::default());
        let mut b = backend(&script);
        let e = model();
        b.load(&e).unwrap();
        b.workers.get_mut(&e.alias).unwrap().proc.kill();
        let err = b
            .infer(
                &e,
                &Request {
                    system: None,
                    text: Some("q".into()),
                    image: None,
                    image_blob: None,
                    max_tokens: 1,
                },
            )
            .unwrap_err();
        assert_eq!(err.fault, Fault::Dead);
    }
}
