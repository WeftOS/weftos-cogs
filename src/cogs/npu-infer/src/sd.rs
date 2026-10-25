//! The Stable Diffusion service (`weftos-sd.service`, loopback :18700) is a
//! second NPU user. This cog does not run it and does not stop it; before a
//! language-model call it waits until the service reports itself idle, so
//! the two never hold the NPU at the same moment. The loopback listener has
//! no authentication (only the tailnet one does), and `/v1/health` is the
//! only thing this reads.
//!
//! Routing an `image.sd15` model class here is not done: a 512x512 PNG is
//! larger than a service reply may be (64 KiB) until blobs (B5) exist, and
//! the e-ink art client keeps calling the service directly.

use std::time::{Duration, Instant};

use crate::backend::Gate;
use crate::http;

pub struct SdGate {
    addr: String,
    poll: Duration,
}

impl SdGate {
    pub fn new(addr: &str) -> Self {
        Self {
            addr: addr.to_string(),
            poll: Duration::from_millis(250),
        }
    }

    #[cfg(test)]
    pub fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }
}

impl Gate for SdGate {
    fn busy(&self) -> Option<bool> {
        let r = http::request(
            &self.addr,
            "GET",
            "/v1/health",
            None,
            Duration::from_secs(2),
        )
        .ok()?;
        if r.status != 200 {
            return None;
        }
        r.json()?.get("busy")?.as_bool()
    }

    fn wait_idle(&self, deadline: Instant) -> Result<(), String> {
        loop {
            // Not running (or unreadable) is idle: there is nothing to wait for.
            if self.busy() != Some(true) {
                return Ok(());
            }
            if Instant::now() + self.poll >= deadline {
                return Err("the image service held the NPU until the deadline".into());
            }
            std::thread::sleep(self.poll);
        }
    }
}

/// Used when no image service is configured.
pub struct NoGate;

impl Gate for NoGate {
    fn busy(&self) -> Option<bool> {
        None
    }

    fn wait_idle(&self, _: Instant) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A health endpoint that answers busy for the first `busy_for` requests.
    fn health(busy_for: usize) -> (String, Arc<AtomicUsize>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        std::thread::spawn(move || {
            for mut s in l.incoming().flatten() {
                let mut buf = [0u8; 512];
                let _ = s.read(&mut buf);
                let n = h.fetch_add(1, Ordering::SeqCst);
                let body = format!(r#"{{"model_loaded":true,"busy":{}}}"#, n < busy_for);
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (addr, hits)
    }

    #[test]
    fn waits_while_the_image_service_is_busy_then_goes() {
        let (addr, hits) = health(3);
        let g = SdGate::new(&addr).with_poll(Duration::from_millis(20));
        g.wait_idle(Instant::now() + Duration::from_secs(5))
            .unwrap();
        assert!(hits.load(Ordering::SeqCst) >= 4, "polled until idle");
    }

    #[test]
    fn gives_up_at_the_deadline() {
        let (addr, _) = health(usize::MAX);
        let g = SdGate::new(&addr).with_poll(Duration::from_millis(20));
        let e = g
            .wait_idle(Instant::now() + Duration::from_millis(150))
            .unwrap_err();
        assert!(e.contains("image service"), "{e}");
    }

    #[test]
    fn a_service_that_is_not_running_is_idle() {
        let g = SdGate::new("127.0.0.1:9").with_poll(Duration::from_millis(10));
        assert_eq!(g.busy(), None);
        g.wait_idle(Instant::now() + Duration::from_millis(100))
            .unwrap();
    }
}
