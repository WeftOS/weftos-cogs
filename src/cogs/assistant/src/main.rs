//! Cognitum Cog: assistant
//!
//! Answers spoken commands (WeftOS Ventuno cog plan 5.3 and 5.5, the hear-and-answer loop). It
//! reads the commands `wake-word` publishes (`transcript/<source>`, default `wake0`), asks the
//! node's language model through the service `svc.infer/<service>` (`infer.v1`, the `npu-infer`
//! cog) and publishes the answer, cleaned for speech, as a `transcript.v1` reading on
//! `transcript/<name>` (default `reply0`), which the `tts` cog speaks. A model that cannot be
//! reached is answered with a short apology so the loop never goes silent, and counted.
//!
//! Usage:
//!   cog-assistant                          # answer transcript/wake0 from $WEFTOS_COGBUS
//!   cog-assistant --model qwen3-4b         # another model of the service's catalog
//!   cog-assistant --once --simulate        # canned commands, canned answers, no service
//!   cog-assistant --once --replay FILE     # commands from a JSON-lines file, to the real service

mod bus;
mod export;
mod guide;
mod ingest;
mod prompt;

use export::{round, Shared, State};
use prompt::History;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TAG: &str = "[cog-assistant]";
/// Per-cog id in the store vector tuple (34=wake-word).
pub(crate) const STORE_ID: u32 = 35;
const MAX_SPOKEN_CHARS: usize = 400;

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
    history: usize,
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
        source: id_opt(a, "--source", "wake0"),
        name: id_opt(a, "--name", "reply0"),
        service: id_opt(a, "--service", "venq-npu"),
        model: arg(a, "--model")
            .filter(|m| !m.is_empty() && m.len() <= 128)
            .unwrap_or("qwen3.5-0.8b")
            .to_string(),
        max_tokens: num(a, "--max-tokens", 96, 8, 1024),
        priority: num(a, "--priority", 7, 0, 9),
        history: num(a, "--history", 2, 0, 8),
        timeout_s: num(a, "--timeout-s", 90, 5, 600),
        bus: arg(a, "--bus")
            .map(String::from)
            .or_else(|| std::env::var("WEFTOS_COGBUS").ok()),
        api_bind: arg(a, "--api-bind").unwrap_or("127.0.0.1:8069").to_string(),
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

/// The `infer.v1` body for a command.
fn infer_body(o: &Opts, hist: &History, command: &str) -> Value {
    json!({
        "model": o.model, "priority": o.priority, "max_tokens": o.max_tokens,
        "system": prompt::SYSTEM, "text": hist.prompt(command),
    })
}

#[derive(Debug, Default, Clone)]
struct Stats {
    commands: u64,
    replies: u64,
    failures: u64,
    latency_ms: u64,
    last_command: Option<String>,
    last_reply: Option<String>,
    last_error: Option<String>,
}

/// Something that answers a prompt: the service, or the canned simulator.
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
        let text = body["text"].as_str().unwrap_or_default();
        let last = text
            .lines()
            .rev()
            .find(|l| l.starts_with("User:"))
            .unwrap_or("User: ")
            .trim_start_matches("User: ");
        Ok(format!("SYNTHETIC answer to: {last}."))
    }
}

/// One command through the model; returns the text to speak.
fn answer(
    brain: &mut dyn Brain,
    o: &Opts,
    hist: &mut History,
    s: &mut Stats,
    command: &str,
) -> (String, bool) {
    s.commands += 1;
    s.last_command = Some(command.to_string());
    let body = infer_body(o, hist, command);
    let t = Instant::now();
    match brain.ask(&body, Duration::from_secs(o.timeout_s)) {
        Ok(raw) => {
            let said = prompt::for_speech(&raw, MAX_SPOKEN_CHARS);
            if said.is_empty() {
                s.failures += 1;
                s.last_error = Some("the model's answer was empty".into());
                return (prompt::FALLBACK.to_string(), false);
            }
            s.replies += 1;
            s.latency_ms += t.elapsed().as_millis() as u64;
            s.last_reply = Some(said.clone());
            hist.push(command, &said);
            (said, true)
        }
        Err(e) => {
            s.failures += 1;
            s.last_error = Some(e);
            (prompt::FALLBACK.to_string(), false)
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
    history: usize,
    bus: &'a BusState,
    error: Option<&'a str>,
    export_url: &'a Option<String>,
}

fn build_report(s: &Stats, c: &Context) -> (Value, [f64; 8]) {
    let ok = c.status != "no_source";
    let mean = if s.replies > 0 {
        s.latency_ms as f64 / s.replies as f64
    } else {
        0.0
    };
    let vector = if ok {
        [
            s.commands as f64,
            s.replies as f64,
            s.failures as f64,
            round(mean, 0),
            c.history as f64,
            c.bus.published as f64,
            c.bus.connected as u8 as f64,
            round(
                if s.commands > 0 {
                    s.replies as f64 / s.commands as f64
                } else {
                    0.0
                },
                3,
            ),
        ]
    } else {
        [-1.0; 8]
    };
    let mut r = json!({
        "status": c.status,
        "provenance": match c.status { "simulate" => "SYNTHETIC", "replay" => "REPLAY", _ => "MEASURED" },
        "model": c.model, "brain": c.brain,
        "listening_to": format!("transcript/{}", c.source),
        "commands": s.commands, "replies": s.replies, "failures": s.failures,
        "mean_latency_ms": round(mean, 0),
        "remembered_turns": c.history,
        "last_command": s.last_command, "last_reply": s.last_reply, "last_error": s.last_error,
        "bus": { "connected": c.bus.connected, "subscribed": c.bus.subscribed, "published": c.bus.published, "error": c.bus.error },
        "vector_slots": ["commands", "replies", "failures", "mean_latency_ms", "remembered_turns", "published", "bus_connected", "answer_ratio"],
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
        i("commands", s.commands as f64, ""),
        i("replies", s.replies as f64, ""),
        i("failures", s.failures as f64, ""),
        i(
            "latency.mean",
            if s.replies > 0 {
                s.latency_ms as f64 / s.replies as f64
            } else {
                0.0
            },
            "ms",
        ),
        i("bus.connected", bus.connected as u8 as f64, ""),
    ]
}

fn transcript_body(name: &str, node_id: &str, seq: u64, text: &str, start_ns: u64) -> Value {
    json!({
        "source_id": name, "node_id": node_id, "t_ns": start_ns, "seq": seq,
        "text": text.chars().take(4096).collect::<String>(), "lang": "en",
        "start_ns": start_ns, "end_ns": start_ns, "confidence": 1.0,
    })
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
        }
    }

    fn connect(&mut self, subscribe: bool) {
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
        if self.bus.is_some()
            || self
                .last_try
                .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        self.last_try = Some(Instant::now());
        let token = std::env::var("WEFTOS_COGBUS_TOKEN").unwrap_or_default();
        let r = bus::Bus::connect(&path, &token).and_then(|b| {
            if subscribe {
                b.subscribe(&format!("transcript/{}", self.source), "transcript.v1")?;
            }
            Ok(b)
        });
        match r {
            Ok(b) => {
                eprintln!("{TAG} connected to {path} (node {})", b.node_id);
                self.msgs = if subscribe { b.take_msgs() } else { None };
                self.state.connected = true;
                self.state.subscribed = subscribe;
                self.state.error = None;
                self.bus = Some(b);
            }
            Err(e) => {
                self.state.error = Some(e.clone());
                eprintln!("{TAG} bus: {e}");
            }
        }
    }

    fn next(&mut self, wait: Duration) -> Option<bus::Msg> {
        self.connect(true);
        match self.msgs.as_ref() {
            Some(rx) => rx.recv_timeout(wait).ok(),
            None => {
                std::thread::sleep(wait);
                None
            }
        }
    }

    fn publish(&mut self, name: &str, text: &str, start_ns: u64) {
        let Some(b) = self.bus.as_ref().filter(|b| !b.is_closed()) else {
            return;
        };
        self.seq += 1;
        let body = transcript_body(name, &b.node_id, self.seq, text, start_ns);
        match b.publish("transcript", name, "transcript.v1", start_ns, body) {
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
        self.0.connect(true);
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

fn replay_file(text: &str) -> Result<Vec<String>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| {
            let v: Value = serde_json::from_str(l).map_err(|e| format!("line {}: {e}", i + 1))?;
            v["text"]
                .as_str()
                .map(String::from)
                .ok_or_else(|| format!("line {}: no text", i + 1))
        })
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
        "{TAG} start (once={}, model={}, from transcript/{}, to transcript/{}, simulate={}, bus={})",
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
    let mut script: Vec<String> = if o.simulate {
        vec!["what time is it".into(), "turn on the kitchen light".into()]
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
    let mut hist = History::new(o.history);
    let mut stats = Stats::default();
    let period = Duration::from_secs(if o.once { o.window } else { o.interval });
    let mut t0 = Instant::now();
    let mut brain_label = if o.simulate {
        "synthetic".to_string()
    } else {
        format!("svc.infer/{}", o.service)
    };
    loop {
        let item = if local {
            script.pop().map(|c| (c, bus::wall_ns()))
        } else {
            link.next(Duration::from_millis(100)).map(|m| {
                (
                    m.body["text"].as_str().unwrap_or_default().to_string(),
                    m.body["start_ns"].as_u64().unwrap_or(m.t_ns),
                )
            })
        };
        if let Some((cmd, start_ns)) = item.filter(|(c, _)| !c.trim().is_empty()) {
            let (said, ok) = if o.simulate {
                answer(&mut Canned, &o, &mut hist, &mut stats, &cmd)
            } else {
                let mut svc = Service(&mut link);
                brain_label = svc.label();
                answer(&mut svc, &o, &mut hist, &mut stats, &cmd)
            };
            println!("{}", json!({"command": cmd, "reply": said, "ok": ok}));
            link.publish(&o.name, &said, start_ns);
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
                history: hist.len(),
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

    #[test]
    fn options_bounded_and_defaulted() {
        let o = parse_opts(&s(&[
            "--once",
            "--max-tokens",
            "1",
            "--priority",
            "12",
            "--name",
            "Bad",
            "--model",
            "qwen3-4b",
        ]));
        assert!(o.once);
        assert_eq!(
            (o.max_tokens, o.priority, o.name.as_str(), o.model.as_str()),
            (96, 7, "reply0", "qwen3-4b")
        );
        let d = parse_opts(&s(&[]));
        assert_eq!(
            (
                d.service.as_str(),
                d.source.as_str(),
                d.history,
                d.api_bind.as_str()
            ),
            ("venq-npu", "wake0", 2, "127.0.0.1:8069")
        );
    }

    #[test]
    fn the_infer_body_matches_the_schema_rules() {
        let o = parse_opts(&s(&[]));
        let b = infer_body(&o, &History::new(2), "what time is it");
        assert_eq!(
            (
                b["model"].as_str(),
                b["priority"].as_u64(),
                b["max_tokens"].as_u64()
            ),
            (Some("qwen3.5-0.8b"), Some(7), Some(96))
        );
        assert!(b["text"]
            .as_str()
            .unwrap()
            .ends_with("User: what time is it\nAssistant:"));
        assert!(b["system"].as_str().unwrap().len() <= 32 * 1024);
        assert!(b["max_tokens"].as_u64().unwrap() <= 4096 && b["priority"].as_u64().unwrap() <= 9);
    }

    #[test]
    fn an_answer_is_cleaned_remembered_and_counted() {
        let o = parse_opts(&s(&[]));
        let (mut h, mut st) = (History::new(2), Stats::default());
        let (said, ok) = answer(
            &mut Fixed(Ok("<think>x</think> It is **noon**.".into())),
            &o,
            &mut h,
            &mut st,
            "what time is it",
        );
        assert_eq!((said.as_str(), ok), ("It is noon.", true));
        assert_eq!(
            (st.commands, st.replies, st.failures, h.len()),
            (1, 1, 0, 1)
        );
        assert!(h.prompt("again").contains("Assistant: It is noon."));
    }

    #[test]
    fn a_failure_is_an_apology_not_silence() {
        let o = parse_opts(&s(&[]));
        let (mut h, mut st) = (History::new(2), Stats::default());
        let (said, ok) = answer(
            &mut Fixed(Err("unavailable: nobody serves svc.infer/venq-npu".into())),
            &o,
            &mut h,
            &mut st,
            "hello",
        );
        assert_eq!((said.as_str(), ok), (prompt::FALLBACK, false));
        assert_eq!(
            (st.failures, h.len()),
            (1, 0),
            "a failed turn is not remembered"
        );
        assert!(st.last_error.as_deref().unwrap().contains("unavailable"));
        let (said, ok) = answer(
            &mut Fixed(Ok("<think>only thoughts</think>".into())),
            &o,
            &mut h,
            &mut st,
            "hello",
        );
        assert_eq!(
            (said.as_str(), ok, st.failures),
            (prompt::FALLBACK, false, 2)
        );
    }

    #[test]
    fn the_canned_brain_echoes_the_last_user_line() {
        let o = parse_opts(&s(&[]));
        let mut h = History::new(2);
        h.push("earlier", "ok");
        let b = infer_body(&o, &h, "turn on the light");
        assert_eq!(
            Canned.ask(&b, Duration::from_secs(1)).unwrap(),
            "SYNTHETIC answer to: turn on the light."
        );
    }

    #[test]
    fn the_published_body_has_the_schema_shape() {
        let b = transcript_body("reply0", &"a".repeat(32), 4, "It is noon.", 7);
        for k in [
            "source_id",
            "node_id",
            "t_ns",
            "seq",
            "text",
            "lang",
            "start_ns",
            "end_ns",
            "confidence",
        ] {
            assert!(b.get(k).is_some(), "{k}");
        }
        assert_eq!(
            (b["seq"].as_u64(), b["end_ns"].as_u64()),
            (Some(4), Some(7))
        );
    }

    #[test]
    fn a_report_carries_counts_latency_and_the_vector() {
        let st = Stats {
            commands: 4,
            replies: 3,
            failures: 1,
            latency_ms: 6000,
            last_command: Some("x".into()),
            last_reply: Some("y".into()),
            last_error: None,
        };
        let bus = BusState {
            connected: true,
            subscribed: true,
            published: 4,
            error: None,
        };
        let ctx = Context {
            status: "ok",
            model: "m",
            brain: "svc.infer/venq-npu",
            source: "wake0",
            history: 2,
            bus: &bus,
            error: None,
            export_url: &None,
        };
        let (r, v) = build_report(&st, &ctx);
        assert_eq!(
            (r["mean_latency_ms"].as_f64(), r["failures"].as_u64()),
            (Some(2000.0), Some(1))
        );
        assert_eq!(v, [4.0, 3.0, 1.0, 2000.0, 2.0, 4.0, 1.0, 0.75]);
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
    fn replay_lines_parse() {
        assert_eq!(
            replay_file("{\"text\":\"a\"}\n\n{\"text\":\"b\"}\n").unwrap(),
            ["a", "b"]
        );
        assert!(replay_file("{\"x\":1}").unwrap_err().contains("no text"));
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
}
