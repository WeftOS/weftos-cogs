//! Cognitum Cog: vision-detect
//!
//! Finds what is in camera frames (WeftOS Ventuno cog plan 5.3 and 5.5, the see loop). It reads
//! the `video.frame-ref.v1` readings of the `camera` cog (`video.frame-ref/<source>`, default
//! `cam0`); a reading that carries the frame's `blob.v1` handle is passed, as that handle and
//! not as pixels, in a call to the node's vision model, `svc.infer/<service>` (`infer.v1` with
//! `image_blob`, the `npu-infer` cog), and the model's list of objects becomes a `detections.v1`
//! reading on `detections/<name>` (default `vis0`). One frame is in flight at a time, newer
//! frames replace older waiting ones, and calls are at least `--min-interval-ms` apart, so
//! the camera's rate never sets the NPU's load.
//!
//! Usage:
//!   cog-vision-detect                     # watch video.frame-ref/cam0 from $WEFTOS_COGBUS
//!   cog-vision-detect --model qwen3-vl-4b # another vision model of the service's catalog
//!   cog-vision-detect --once --simulate   # canned frame and canned answer, no service
//!   cog-vision-detect --once --replay F   # frame-ref bodies from a JSON-lines file, to the real service

mod bus;
mod detect;
mod export;
mod guide;
mod ingest;

use export::{round, Shared, State};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TAG: &str = "[cog-vision-detect]";
/// Per-cog id in the store vector tuple (36=tts).
pub(crate) const STORE_ID: u32 = 37;

#[derive(Debug, Clone, PartialEq)]
struct Opts {
    once: bool,
    interval: u64,
    window: u64,
    source: String,
    name: String,
    service: String,
    model: String,
    max_tokens: u32,
    priority: u8,
    min_interval_ms: u64,
    timeout_s: u64,
    bus: Option<String>,
    api_bind: String,
    simulate: bool,
    replay: Option<String>,
}

fn arg<'a>(a: &'a [String], f: &str) -> Option<&'a str> {
    a.iter()
        .position(|x| x == f)
        .and_then(|i| a.get(i + 1))
        .map(String::as_str)
}

fn num<T: std::str::FromStr + PartialOrd + Copy>(a: &[String], f: &str, d: T, lo: T, hi: T) -> T {
    match arg(a, f).and_then(|v| v.parse::<T>().ok()) {
        Some(v) if v >= lo && v <= hi => v,
        Some(_) => {
            eprintln!("{TAG} {f} out of range, using default");
            d
        }
        None => d,
    }
}

fn valid_source(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(c))
}

fn id_opt(a: &[String], flag: &str, default: &str) -> String {
    match arg(a, flag) {
        Some(s) if valid_source(s) => s.to_string(),
        Some(_) => {
            eprintln!("{TAG} {flag} must match [a-z0-9][a-z0-9._-]*, using {default}");
            default.into()
        }
        None => default.into(),
    }
}

fn parse_opts(a: &[String]) -> Opts {
    Opts {
        once: a.iter().any(|x| x == "--once"),
        interval: num(a, "--interval", 5, 1, 300),
        window: num(a, "--window", 10, 1, 120),
        source: id_opt(a, "--source", "cam0"),
        name: id_opt(a, "--name", "vis0"),
        service: id_opt(a, "--service", "venq-npu"),
        model: arg(a, "--model")
            .filter(|m| !m.is_empty() && m.len() <= 128)
            .unwrap_or("qwen3-vl-4b")
            .to_string(),
        max_tokens: num(a, "--max-tokens", 256, 16, 1024),
        priority: num(a, "--priority", 3, 0, 9),
        min_interval_ms: num(a, "--min-interval-ms", 3000, 0, 600_000),
        timeout_s: num(a, "--timeout-s", 120, 5, 600),
        bus: arg(a, "--bus")
            .map(String::from)
            .or_else(|| std::env::var("WEFTOS_COGBUS").ok()),
        api_bind: arg(a, "--api-bind").unwrap_or("127.0.0.1:8071").to_string(),
        simulate: a.iter().any(|x| x == "--simulate"),
        replay: arg(a, "--replay").map(String::from),
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// A frame reading as the cog uses it.
#[derive(Debug, Clone, PartialEq)]
struct Frame {
    hash: String,
    t_ns: u64,
    blob: Option<Value>,
}

fn frame_of(body: &Value, t_ns: u64) -> Option<Frame> {
    let hash = body["hash"].as_str().filter(|h| h.len() == 64)?.to_string();
    Some(Frame {
        hash,
        t_ns: body["t_ns"].as_u64().unwrap_or(t_ns),
        blob: body.get("blob").filter(|b| b.is_object()).cloned(),
    })
}

/// Why this frame cannot be sent, if it cannot: no handle, or the bytes have expired.
fn unusable(f: &Frame, now_ms: u64) -> Option<&'static str> {
    let Some(b) = &f.blob else {
        return Some("no blob handle in the reading (camera --no-blob, or no broker)");
    };
    match b["expires_ms"].as_u64() {
        Some(e) if e > now_ms => None,
        _ => Some("the frame's blob has expired"),
    }
}

/// The `infer.v1` request for a frame: the handle, not the pixels.
fn infer_body(o: &Opts, f: &Frame) -> Value {
    json!({
        "model": o.model, "priority": o.priority, "max_tokens": o.max_tokens,
        "system": detect::SYSTEM, "text": detect::PROMPT, "image_blob": f.blob,
    })
}

#[derive(Debug, Default, Clone)]
struct Stats {
    seen: u64,
    asked: u64,
    answered: u64,
    published: u64,
    boxes: u64,
    whole_frame: u64,
    unparsed: u64,
    unusable: u64,
    replaced: u64,
    throttled: u64,
    failures: u64,
    latency_ms: u64,
    last_labels: Vec<String>,
    last_error: Option<String>,
}

/// Something that answers a request: the service, or the canned simulator.
trait Brain {
    fn ask(&mut self, body: &Value, timeout: Duration) -> Result<String, String>;
    fn label(&self) -> String;
}

struct Canned;

impl Brain for Canned {
    fn label(&self) -> String {
        "synthetic".into()
    }

    fn ask(&mut self, body: &Value, _: Duration) -> Result<String, String> {
        if body["image_blob"].is_object() {
            Ok(r#"[{"label":"SYNTHETIC person","bbox":[0.1,0.2,0.3,0.6],"score":0.9},{"label":"SYNTHETIC chair"}]"#.into())
        } else {
            Err("the request carries no image".into())
        }
    }
}

/// One frame through the model; the boxes to publish, if the answer held a list.
fn look(
    brain: &mut dyn Brain,
    o: &Opts,
    s: &mut Stats,
    f: &Frame,
    now_ms: u64,
) -> Option<Vec<detect::Found>> {
    s.seen += 1;
    if let Some(why) = unusable(f, now_ms) {
        s.unusable += 1;
        s.last_error = Some(why.into());
        return None;
    }
    s.asked += 1;
    let t = Instant::now();
    match brain.ask(&infer_body(o, f), Duration::from_secs(o.timeout_s)) {
        Ok(text) => {
            s.latency_ms += t.elapsed().as_millis() as u64;
            match detect::parse(&text) {
                Some(found) => {
                    s.answered += 1;
                    s.boxes += found.len() as u64;
                    s.whole_frame += found.iter().filter(|b| b.whole_frame).count() as u64;
                    s.last_labels = found.iter().map(|b| b.label.clone()).collect();
                    Some(found)
                }
                None => {
                    s.unparsed += 1;
                    s.last_error = Some(format!(
                        "the answer held no list: {}",
                        text.chars().take(80).collect::<String>()
                    ));
                    None
                }
            }
        }
        Err(e) => {
            s.failures += 1;
            s.last_error = Some(e);
            None
        }
    }
}

#[derive(Debug, Default, Clone)]
struct BusState {
    connected: bool,
    subscribed: bool,
    published: u64,
    error: Option<String>,
}

struct Context<'a> {
    status: &'a str,
    model: &'a str,
    brain: &'a str,
    source: &'a str,
    bus: &'a BusState,
    error: Option<&'a str>,
    export_url: &'a Option<String>,
}

fn build_report(s: &Stats, c: &Context) -> (Value, [f64; 8]) {
    let ok = c.status != "no_source";
    let mean = if s.answered + s.unparsed > 0 {
        s.latency_ms as f64 / (s.answered + s.unparsed) as f64
    } else {
        0.0
    };
    let vector = if ok {
        [
            s.seen as f64,
            s.asked as f64,
            s.published as f64,
            s.boxes as f64,
            round(mean, 0),
            s.failures as f64,
            s.replaced as f64,
            c.bus.connected as u8 as f64,
        ]
    } else {
        [-1.0; 8]
    };
    let mut r = json!({
        "status": c.status,
        "provenance": match c.status { "simulate" => "SYNTHETIC", "replay" => "REPLAY", _ => "MEASURED" },
        "model": c.model, "brain": c.brain,
        "watching": format!("video.frame-ref/{}", c.source),
        "frames_seen": s.seen, "frames_asked": s.asked, "answers": s.answered, "detections_published": s.published,
        "boxes": s.boxes, "labels_only_boxes": s.whole_frame,
        "unusable_frames": s.unusable, "unparsed_answers": s.unparsed, "failures": s.failures,
        "replaced_while_busy": s.replaced, "throttled": s.throttled,
        "mean_latency_ms": round(mean, 0),
        "last_labels": s.last_labels, "last_error": s.last_error,
        "bus": { "connected": c.bus.connected, "subscribed": c.bus.subscribed, "published": c.bus.published, "error": c.bus.error },
        "vector_slots": ["frames_seen", "frames_asked", "published", "boxes", "mean_latency_ms", "failures", "replaced", "bus_connected"],
        "vector": vector,
        "export": c.export_url,
        "timestamp": unix_ms() / 1000,
    });
    if let Some(e) = c.error {
        r["error"] = json!(e);
    }
    (r, vector)
}

fn telemetry_items(s: &Stats, bus: &BusState) -> Vec<Value> {
    let i = |n: &str, v: f64, u: &str| json!({"name": n, "value": v, "unit": u});
    vec![
        i("frames.seen", s.seen as f64, ""),
        i("frames.asked", s.asked as f64, ""),
        i("detections", s.published as f64, ""),
        i("boxes", s.boxes as f64, ""),
        i("failures", s.failures as f64, ""),
        i(
            "latency.mean",
            if s.answered + s.unparsed > 0 {
                s.latency_ms as f64 / (s.answered + s.unparsed) as f64
            } else {
                0.0
            },
            "ms",
        ),
        i("bus.connected", bus.connected as u8 as f64, ""),
    ]
}

struct Link {
    path: Option<String>,
    source: String,
    service: String,
    bus: Option<bus::Bus>,
    msgs: Option<std::sync::mpsc::Receiver<bus::Msg>>,
    state: BusState,
    seq: u64,
    tseq: u64,
    last_try: Option<Instant>,
    /// Wait before the next connect attempt (longer after `busy`).
    retry: Duration,
}

impl Link {
    fn new(path: Option<String>, source: &str, service: &str) -> Self {
        Self {
            path,
            source: source.into(),
            service: service.into(),
            bus: None,
            msgs: None,
            state: BusState::default(),
            seq: 0,
            tseq: 0,
            last_try: None,
            retry: Duration::from_secs(2),
        }
    }

    fn connect(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        if self.bus.as_ref().is_some_and(|b| b.is_closed()) {
            self.bus = None;
            self.msgs = None;
            self.state = BusState {
                published: self.state.published,
                ..BusState::default()
            };
        }
        if self.bus.is_some() || self.last_try.is_some_and(|t| t.elapsed() < self.retry) {
            return;
        }
        self.last_try = Some(Instant::now());
        let token = std::env::var("WEFTOS_COGBUS_TOKEN").unwrap_or_default();
        let r = bus::Bus::connect(&path, &token).and_then(|b| {
            b.subscribe(
                &format!("video.frame-ref/{}", self.source),
                "video.frame-ref.v1",
            )?;
            Ok(b)
        });
        match r {
            Ok(b) => {
                eprintln!(
                    "{TAG} watching video.frame-ref/{} on {path} (node {})",
                    self.source, b.node_id
                );
                self.msgs = b.take_msgs();
                self.state.connected = true;
                self.state.subscribed = true;
                self.state.error = None;
                self.bus = Some(b);
            }
            Err(e) => {
                self.retry = bus::retry_after(&e);
                self.state.error = Some(e.clone());
                eprintln!("{TAG} bus: {e}");
            }
        }
    }

    fn next(&mut self, wait: Duration) -> Option<bus::Msg> {
        self.connect();
        match self.msgs.as_ref() {
            Some(rx) => rx.recv_timeout(wait).ok(),
            None => {
                std::thread::sleep(wait);
                None
            }
        }
    }

    /// Everything waiting, newest only; returns it and how many it replaced.
    fn latest(&mut self) -> (Option<bus::Msg>, u64) {
        let Some(rx) = self.msgs.as_ref() else {
            return (None, 0);
        };
        let mut last = None;
        let mut n = 0;
        while let Ok(m) = rx.try_recv() {
            n += last.is_some() as u64;
            last = Some(m);
        }
        (last, n)
    }

    fn publish(&mut self, name: &str, f: &Frame, model: &str, found: &[detect::Found]) {
        let Some(b) = self.bus.as_ref().filter(|b| !b.is_closed()) else {
            return;
        };
        self.seq += 1;
        let body =
            detect::detections_body(name, &b.node_id, f.t_ns, self.seq, &f.hash, model, found);
        match b.publish("detections", name, "detections.v1", f.t_ns, body) {
            Ok(()) => self.state.published += 1,
            Err(e) => {
                self.state.error = Some(e.clone());
                eprintln!("{TAG} publish: {e}");
            }
        }
    }

    fn telemetry(&mut self, name: &str, s: &Stats) {
        self.tseq += 1;
        let Some(b) = self.bus.as_ref().filter(|b| !b.is_closed()) else {
            return;
        };
        let body = json!({"source_id": name, "node_id": b.node_id, "t_ns": bus::wall_ns(), "seq": self.tseq, "items": telemetry_items(s, &self.state)});
        if let Err(e) = b.publish("telemetry", name, "telemetry.v1", bus::wall_ns(), body) {
            eprintln!("{TAG} telemetry: {e}");
        }
    }
}

/// The real model: `svc.infer/<service>` on the bus.
struct Service<'a>(&'a mut Link);

impl Brain for Service<'_> {
    fn label(&self) -> String {
        format!("svc.infer/{}", self.0.service)
    }

    fn ask(&mut self, body: &Value, timeout: Duration) -> Result<String, String> {
        self.0.connect();
        let b = self
            .0
            .bus
            .as_ref()
            .filter(|b| !b.is_closed())
            .ok_or("no broker connection")?;
        let r = b.call(
            &format!("svc.infer/{}", self.0.service),
            "infer.v1",
            body.clone(),
            timeout,
        )?;
        r["text"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| format!("the service answered without text: {r}"))
    }
}

/// A frame-ref body for `--simulate`: a handle that will not expire during the run.
fn canned_frame() -> Value {
    let hash = "ab".repeat(32);
    json!({
        "hash": hash, "t_ns": 1, "width": 1280, "height": 720,
        "blob": {"hash": hash, "size": 51200, "media_type": "image/jpeg", "expires_ms": unix_ms() + 600_000},
    })
}

fn replay_file(text: &str) -> Result<Vec<Value>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| serde_json::from_str(l).map_err(|e| format!("line {}: {e}", i + 1)))
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    cog_sensor_sources::handle_help(
        &args,
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        include_str!("../cog.toml"),
    );
    let o = parse_opts(&args);
    eprintln!(
        "{TAG} start (once={}, model={}, frames video.frame-ref/{}, detections detections/{}, simulate={}, bus={})",
        o.once,
        o.model,
        o.source,
        o.name,
        o.simulate,
        o.bus.as_deref().unwrap_or("none")
    );
    let shared: Shared = Arc::new(Mutex::new(State::default()));
    let export_url = if o.once {
        None
    } else {
        match export::serve(&o.api_bind, shared.clone()) {
            Ok(a) => Some(format!("http://{a}/status")),
            Err(e) => {
                eprintln!("{TAG} export disabled: {e}");
                None
            }
        }
    };
    let (target, warn) = ingest::target_from_env(std::env::var(ingest::ENV_URL).ok().as_deref());
    if let Some(w) = warn {
        eprintln!("{TAG} {w}");
    }
    let token = std::env::var(ingest::ENV_TOKEN).ok();
    let local = o.simulate || o.replay.is_some();
    let mut input_err: Option<String> = None;
    let mut script: Vec<Value> = if o.simulate {
        vec![canned_frame()]
    } else if let Some(f) = &o.replay {
        match std::fs::read_to_string(f)
            .map_err(|e| format!("replay {f}: {e}"))
            .and_then(|t| replay_file(&t))
        {
            Ok(v) => v,
            Err(e) => {
                input_err = Some(e);
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    script.reverse();
    let mut link = Link::new(
        if o.simulate { None } else { o.bus.clone() },
        &o.source,
        &o.service,
    );
    let mut stats = Stats::default();
    let period = Duration::from_secs(if o.once { o.window } else { o.interval });
    let mut t0 = Instant::now();
    let mut last_ask: Option<Instant> = None;
    let mut brain_label = if o.simulate {
        "synthetic".to_string()
    } else {
        format!("svc.infer/{}", o.service)
    };
    loop {
        let item: Option<Frame> = if local {
            script.pop().and_then(|b| frame_of(&b, bus::wall_ns()))
        } else {
            let first = link.next(Duration::from_millis(100));
            let (newest, replaced) = link.latest();
            stats.replaced += replaced + (first.is_some() && newest.is_some()) as u64;
            newest.or(first).and_then(|m| frame_of(&m.body, m.t_ns))
        };
        if let Some(f) = item {
            let wait = last_ask.map_or(Duration::ZERO, |t| {
                Duration::from_millis(o.min_interval_ms).saturating_sub(t.elapsed())
            });
            if !wait.is_zero() && !local {
                stats.throttled += 1;
                stats.seen += 1;
            } else {
                last_ask = Some(Instant::now());
                let found = if o.simulate {
                    look(&mut Canned, &o, &mut stats, &f, unix_ms())
                } else {
                    let mut svc = Service(&mut link);
                    brain_label = svc.label();
                    look(&mut svc, &o, &mut stats, &f, unix_ms())
                };
                if let Some(found) = found {
                    println!(
                        "{}",
                        json!({"frame": f.hash, "labels": found.iter().map(|b| b.label.clone()).collect::<Vec<_>>()})
                    );
                    link.publish(&o.name, &f, &o.model, &found);
                    stats.published += 1;
                }
            }
        }
        let done = local && script.is_empty();
        if done || t0.elapsed() >= period {
            let status = if input_err.is_some() {
                "no_source"
            } else if o.simulate {
                "simulate"
            } else if o.replay.is_some() {
                "replay"
            } else if link.state.connected {
                "ok"
            } else {
                "no_source"
            };
            let err = input_err.clone().or_else(|| {
                (status == "no_source").then(|| {
                    link.state
                        .error
                        .clone()
                        .unwrap_or_else(|| "no broker: WEFTOS_COGBUS is not set".into())
                })
            });
            let ctx = Context {
                status,
                model: &o.model,
                brain: &brain_label,
                source: &o.source,
                bus: &link.state,
                error: err.as_deref(),
                export_url: &export_url,
            };
            let (report, vector) = build_report(&stats, &ctx);
            println!("{report}");
            if let Ok(mut st) = shared.lock() {
                st.push(report.clone());
            }
            link.telemetry(&o.name, &stats);
            if report["status"] == "ok" {
                if let Err(e) = ingest::store_to_seed(&target, token.as_deref(), &vector) {
                    eprintln!("{TAG} store error: {e}");
                }
            } else if report["status"] == "no_source" {
                eprintln!("{TAG} no source: {}", report["error"]);
            }
            if o.once || done {
                return;
            }
            t0 = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        std::iter::once("cog")
            .chain(v.iter().copied())
            .map(String::from)
            .collect()
    }

    struct Fixed(Result<String, String>);
    impl Brain for Fixed {
        fn label(&self) -> String {
            "fixed".into()
        }
        fn ask(&mut self, _: &Value, _: Duration) -> Result<String, String> {
            self.0.clone()
        }
    }

    fn frame(blob: bool, expires: u64) -> Frame {
        let hash = "cd".repeat(32);
        Frame { blob: blob.then(|| json!({"hash": hash, "size": 9, "media_type": "image/jpeg", "expires_ms": expires})), hash, t_ns: 7 }
    }

    #[test]
    fn options_bounded_and_defaulted() {
        let o = parse_opts(&s(&[
            "--once",
            "--priority",
            "12",
            "--max-tokens",
            "1",
            "--name",
            "Bad",
            "--model",
            "qwen2.5-vl-7b",
        ]));
        assert!(o.once);
        assert_eq!(
            (o.priority, o.max_tokens, o.name.as_str(), o.model.as_str()),
            (3, 256, "vis0", "qwen2.5-vl-7b")
        );
        let d = parse_opts(&s(&[]));
        assert_eq!(
            (
                d.service.as_str(),
                d.source.as_str(),
                d.min_interval_ms,
                d.api_bind.as_str()
            ),
            ("venq-npu", "cam0", 3000, "127.0.0.1:8071")
        );
    }

    #[test]
    fn a_frame_without_a_live_handle_is_not_sent() {
        assert!(unusable(&frame(true, 2_000), 1_000).is_none());
        assert!(unusable(&frame(false, 0), 1_000)
            .unwrap()
            .contains("no blob handle"));
        assert!(unusable(&frame(true, 500), 1_000)
            .unwrap()
            .contains("expired"));
        let f = frame_of(&json!({"hash": "ab".repeat(32), "t_ns": 9}), 1).unwrap();
        assert_eq!((f.t_ns, f.blob), (9, None));
        assert!(frame_of(&json!({"hash": "short"}), 1).is_none());
    }

    #[test]
    fn the_request_carries_the_handle_and_no_pixels() {
        let o = parse_opts(&s(&[]));
        let f = frame(true, 2_000);
        let b = infer_body(&o, &f);
        assert_eq!(b["image_blob"], f.blob.clone().unwrap());
        assert!(b.get("image_b64").is_none() && b.get("image_mime").is_none());
        assert_eq!(
            (
                b["model"].as_str(),
                b["priority"].as_u64(),
                b["max_tokens"].as_u64()
            ),
            (Some("qwen3-vl-4b"), Some(3), Some(256))
        );
        assert!(
            b["text"].as_str().unwrap().len() < 200
                && b["system"].as_str().unwrap().contains("JSON array")
        );
        assert!(
            serde_json::to_string(&b).unwrap().len() < 4096,
            "a small request"
        );
    }

    #[test]
    fn an_answer_with_a_list_is_published_and_counted() {
        let o = parse_opts(&s(&[]));
        let mut st = Stats::default();
        let ans = r#"```json
[{"label":"person","bbox":[0.1,0.1,0.4,0.8],"score":0.8},{"label":"desk"}]
```"#;
        let found = look(
            &mut Fixed(Ok(ans.into())),
            &o,
            &mut st,
            &frame(true, u64::MAX),
            1,
        )
        .unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(
            (st.seen, st.asked, st.answered, st.boxes, st.whole_frame),
            (1, 1, 1, 2, 1)
        );
        assert_eq!(st.last_labels, ["person", "desk"]);
    }

    #[test]
    fn failures_are_counted_and_nothing_is_published() {
        let o = parse_opts(&s(&[]));
        let mut st = Stats::default();
        assert!(look(
            &mut Fixed(Err("unavailable: nobody serves svc.infer/venq-npu".into())),
            &o,
            &mut st,
            &frame(true, u64::MAX),
            1
        )
        .is_none());
        assert!(look(
            &mut Fixed(Ok("I see a desk.".into())),
            &o,
            &mut st,
            &frame(true, u64::MAX),
            1
        )
        .is_none());
        assert!(
            look(
                &mut Fixed(Ok("[]".into())),
                &o,
                &mut st,
                &frame(false, 0),
                1
            )
            .is_none(),
            "never reaches the model"
        );
        assert_eq!(
            (st.failures, st.unparsed, st.unusable, st.asked, st.seen),
            (1, 1, 1, 2, 3)
        );
        assert!(st.last_error.as_deref().unwrap().contains("no blob handle"));
        assert!(
            look(
                &mut Fixed(Ok("[]".into())),
                &o,
                &mut st,
                &frame(true, u64::MAX),
                1
            )
            .unwrap()
            .is_empty(),
            "an empty list is a published frame with no objects"
        );
    }

    #[test]
    fn the_canned_brain_needs_an_image_handle() {
        let o = parse_opts(&s(&[]));
        assert!(Canned
            .ask(&infer_body(&o, &frame(true, 1)), Duration::from_secs(1))
            .unwrap()
            .contains("SYNTHETIC person"));
        assert!(Canned
            .ask(&json!({"model": "m", "text": "t"}), Duration::from_secs(1))
            .is_err());
    }

    #[test]
    fn a_report_carries_counts_latency_and_the_vector() {
        let st = Stats {
            seen: 10,
            asked: 4,
            answered: 3,
            published: 3,
            boxes: 7,
            whole_frame: 2,
            unparsed: 1,
            unusable: 5,
            replaced: 1,
            throttled: 0,
            failures: 0,
            latency_ms: 8000,
            last_labels: vec!["a".into()],
            last_error: None,
        };
        let bus = BusState {
            connected: true,
            subscribed: true,
            published: 3,
            error: None,
        };
        let ctx = Context {
            status: "ok",
            model: "m",
            brain: "svc.infer/venq-npu",
            source: "cam0",
            bus: &bus,
            error: None,
            export_url: &None,
        };
        let (r, v) = build_report(&st, &ctx);
        assert_eq!(
            (
                r["mean_latency_ms"].as_f64(),
                r["labels_only_boxes"].as_u64()
            ),
            (Some(2000.0), Some(2))
        );
        assert_eq!(v, [10.0, 4.0, 3.0, 7.0, 2000.0, 0.0, 1.0, 1.0]);
        let (r, v) = build_report(
            &Stats::default(),
            &Context {
                status: "no_source",
                error: Some("no broker"),
                ..ctx
            },
        );
        assert_eq!((r["error"].as_str(), v), (Some("no broker"), [-1.0; 8]));
    }

    #[test]
    fn telemetry_names_are_schema_valid() {
        for it in telemetry_items(&Stats::default(), &BusState::default()) {
            let n = it["name"].as_str().unwrap();
            assert!(
                n.as_bytes()[0].is_ascii_lowercase()
                    && n.bytes().all(|c| c.is_ascii_lowercase()
                        || c.is_ascii_digit()
                        || b"_.".contains(&c)),
                "{n}"
            );
            assert!(it["unit"].as_str().unwrap().chars().count() <= 16);
        }
    }

    #[test]
    fn replay_lines_parse() {
        assert_eq!(
            replay_file("{\"hash\":\"x\"}\n\n{\"hash\":\"y\"}\n")
                .unwrap()
                .len(),
            2
        );
        assert!(replay_file("nope").is_err());
    }
}
