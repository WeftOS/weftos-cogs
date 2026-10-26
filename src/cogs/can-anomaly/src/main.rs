//! Cognitum Cog: can-anomaly
//!
//! Learns what a CAN bus normally looks like and says when it stops looking like that (WeftOS
//! Ventuno cog plan 5.1). It reads the stream `can.frame/<source>` of the `can-fd` cog (default
//! `can0`), spends its first `--learn-s` seconds counting which ids send and how often, then
//! publishes an `event.v1` reading on `event/<name>` (default `can-anomaly0`) for: an id that was
//! not there (`can.new_id`), a length code it never used (`can.dlc_change`), a periodic id that
//! went quiet or came back (`can.id_missing`, `can.id_back`), an id much faster or slower than
//! before (`can.rate_high`, `can.rate_low`) and a flooded bus (`can.flood`). Counts only: no
//! model, no NPU, and no reading of the data bytes.
//!
//! Usage:
//!   cog-can-anomaly                          # watch can.frame/can0 from $WEFTOS_COGBUS
//!   cog-can-anomaly --learn-s 60             # learn for a minute
//!   cog-can-anomaly --once --simulate        # a synthetic bus with injected faults (virtual time)
//!   cog-can-anomaly --once --replay FILE     # a `candump -L` log through the same engine

mod anomaly;
mod bus;
mod export;
mod frame;
mod guide;
mod ingest;

use anomaly::{Engine, Event, Params, Phase};
use export::{round, Shared, State};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TAG: &str = "[cog-can-anomaly]";
/// Per-cog id in the store vector tuple (38=can-fd).
pub(crate) const STORE_ID: u32 = 39;

#[derive(Debug, Clone, PartialEq)]
struct Opts {
    once: bool,
    interval: u64,
    window: u64,
    source: String,
    name: String,
    params: Params,
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
    let d = Params::default();
    let s = 1_000_000_000u64;
    Opts {
        once: a.iter().any(|x| x == "--once"),
        interval: num(a, "--interval", 5, 1, 300),
        window: num(a, "--window", 60, 1, 3600),
        source: id_opt(a, "--source", "can0"),
        name: id_opt(a, "--name", "can-anomaly0"),
        params: Params {
            learn_ns: num(a, "--learn-s", d.learn_ns / s, 2, 3600) * s,
            window_ns: num(a, "--window-s", d.window_ns / s, 1, 600) * s,
            rate_high_x: num(a, "--rate-high-x", d.rate_high_x, 1.5, 100.0),
            rate_high_min_hz: d.rate_high_min_hz,
            rate_low_frac: num(a, "--rate-low-frac", d.rate_low_frac, 0.01, 0.9),
            missing_windows: d.missing_windows,
            cooldown_ns: num(a, "--cooldown-s", d.cooldown_ns / s, 1, 3600) * s,
        },
        bus: arg(a, "--bus")
            .map(String::from)
            .or_else(|| std::env::var("WEFTOS_COGBUS").ok()),
        api_bind: arg(a, "--api-bind").unwrap_or("127.0.0.1:8073").to_string(),
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

/// The `event.v1` body.
fn event_body(name: &str, node_id: &str, seq: u64, e: &Event) -> Value {
    let mut v = json!({
        "source_id": name, "node_id": node_id, "t_ns": e.t_ns, "seq": seq,
        "kind": e.kind, "severity": e.severity, "subject": e.subject,
        "detail": e.detail.chars().take(256).collect::<String>(),
    });
    if let Some(x) = e.value.filter(|x| x.is_finite()) {
        v["value"] = json!(x);
    }
    v
}

#[derive(Debug, Default, Clone)]
struct BusState {
    connected: bool,
    subscribed: bool,
    published: u64,
    skipped: u64,
    error: Option<String>,
}

struct Context<'a> {
    status: &'a str,
    source: &'a str,
    engine: &'a Engine,
    last_events: &'a [Event],
    bus: &'a BusState,
    error: Option<&'a str>,
    export_url: &'a Option<String>,
}

fn build_report(c: &Context) -> (Value, [f64; 8]) {
    let ok = c.status != "no_source";
    let e = c.engine;
    let total: u64 = e
        .counts
        .iter()
        .filter(|(k, _)| **k != "can.baseline")
        .map(|(_, n)| *n)
        .sum();
    let vector = if ok {
        [
            e.frames as f64,
            e.baseline_ids() as f64,
            total as f64,
            *e.counts.get("can.new_id").unwrap_or(&0) as f64,
            *e.counts.get("can.id_missing").unwrap_or(&0) as f64,
            (*e.counts.get("can.rate_high").unwrap_or(&0)
                + *e.counts.get("can.rate_low").unwrap_or(&0)) as f64,
            *e.counts.get("can.flood").unwrap_or(&0) as f64,
            (e.phase() == Phase::Monitoring) as u8 as f64,
        ]
    } else {
        [-1.0; 8]
    };
    let counts: BTreeMap<&str, u64> = e.counts.iter().map(|(k, v)| (*k, *v)).collect();
    let mut r = json!({
        "status": c.status,
        "provenance": match c.status { "simulate" => "SYNTHETIC", "replay" => "REPLAY", _ => "MEASURED" },
        "watching": format!("can.frame/{}", c.source),
        "phase": match e.phase() { Phase::Learning => "learning", Phase::Monitoring => "monitoring" },
        "frames_seen": e.frames, "baseline_ids": e.baseline_ids(), "baseline_frames_per_s": round(e.learned_hz(), 1),
        "events": total, "events_by_kind": counts,
        "last_events": c.last_events.iter().rev().take(5).map(|x| json!({"kind": x.kind, "subject": x.subject, "detail": x.detail})).collect::<Vec<_>>(),
        "bus": { "connected": c.bus.connected, "subscribed": c.bus.subscribed, "published": c.bus.published, "skipped_records": c.bus.skipped, "error": c.bus.error },
        "vector_slots": ["frames_seen", "baseline_ids", "events", "new_ids", "missing_ids", "rate_events", "floods", "monitoring"],
        "vector": vector,
        "export": c.export_url,
        "timestamp": unix_ms() / 1000,
    });
    if let Some(x) = c.error {
        r["error"] = json!(x);
    }
    (r, vector)
}

fn telemetry_items(e: &Engine, bus: &BusState) -> Vec<Value> {
    let i = |n: &str, v: f64| json!({"name": n, "value": v, "unit": ""});
    vec![
        i("frames.seen", e.frames as f64),
        i("baseline.ids", e.baseline_ids() as f64),
        i(
            "events",
            e.counts
                .iter()
                .filter(|(k, _)| **k != "can.baseline")
                .map(|(_, n)| *n)
                .sum::<u64>() as f64,
        ),
        i("monitoring", (e.phase() == Phase::Monitoring) as u8 as f64),
        i("records.skipped", bus.skipped as f64),
        i("bus.connected", bus.connected as u8 as f64),
    ]
}

struct Link {
    path: Option<String>,
    source: String,
    bus: Option<bus::Bus>,
    reader: Option<bus::StreamReader>,
    state: BusState,
    seq: u64,
    tseq: u64,
    last_try: Option<Instant>,
    retry: Duration,
}

impl Link {
    fn new(path: Option<String>, source: &str) -> Self {
        Self {
            path,
            source: source.into(),
            bus: None,
            reader: None,
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
            self.reader = None;
            self.state = BusState {
                published: self.state.published,
                ..BusState::default()
            };
        }
        if self.reader.is_some() || self.last_try.is_some_and(|t| t.elapsed() < self.retry) {
            return;
        }
        self.last_try = Some(Instant::now());
        let token = std::env::var("WEFTOS_COGBUS_TOKEN").unwrap_or_default();
        let r = bus::Bus::connect(&path, &token).and_then(|b| {
            let r = b.stream_reader(&format!("can.frame/{}", self.source), "can.frame.v1")?;
            Ok((b, r))
        });
        match r {
            Ok((b, r)) => {
                eprintln!(
                    "{TAG} watching can.frame/{} on {path} (node {})",
                    self.source, b.node_id
                );
                self.state.connected = true;
                self.state.subscribed = true;
                self.state.error = None;
                self.bus = Some(b);
                self.reader = Some(r);
            }
            Err(e) => {
                self.retry = bus::retry_after(&e);
                self.state.error = Some(e.clone());
                eprintln!("{TAG} bus: {e}");
            }
        }
    }

    fn next(&mut self, wait: Duration) -> Option<bus::Record> {
        self.connect();
        let Some(r) = self.reader.as_mut() else {
            std::thread::sleep(wait);
            return None;
        };
        let rec = r.next(wait);
        self.state.skipped = r.skipped;
        rec
    }

    fn publish(&mut self, name: &str, e: &Event) {
        let Some(b) = self.bus.as_ref().filter(|b| !b.is_closed()) else {
            return;
        };
        self.seq += 1;
        let body = event_body(name, &b.node_id, self.seq, e);
        match b.publish("event", name, "event.v1", e.t_ns, body) {
            Ok(()) => self.state.published += 1,
            Err(x) => {
                self.state.error = Some(x.clone());
                eprintln!("{TAG} publish: {x}");
            }
        }
    }

    fn telemetry(&mut self, name: &str, e: &Engine) {
        self.tseq += 1;
        let Some(b) = self.bus.as_ref().filter(|b| !b.is_closed()) else {
            return;
        };
        let body = json!({"source_id": name, "node_id": b.node_id, "t_ns": bus::wall_ns(), "seq": self.tseq, "items": telemetry_items(e, &self.state)});
        if let Err(x) = b.publish("telemetry", name, "telemetry.v1", bus::wall_ns(), body) {
            eprintln!("{TAG} telemetry: {x}");
        }
    }
}

/// `(t_ns, id, ext, dlc)`.
type Rec = (u64, u32, bool, u8);

/// Learn 6 s of a three-id bus (virtual time), then inject: a new id, a silent id, a burst.
fn synthetic() -> Vec<Rec> {
    let s = 1_000_000_000u64;
    let mut v = Vec::new();
    for ms in (0..30_000u64).step_by(10) {
        let t = ms * 1_000_000;
        if ms % 20 == 0 {
            v.push((t, 0x100, false, 8));
        }
        // 0x200 goes silent after 12 s.
        if ms % 100 == 0 && ms < 12_000 {
            v.push((t, 0x200, false, 4));
        }
        if ms % 1000 == 0 {
            v.push((t, 0x7e8, false, 8));
        }
        // A new id from 8 s on; a burst of 0x100 from 16 s to 20 s.
        if ms >= 8_000 && ms % 500 == 0 {
            v.push((t, 0x1a4, false, 8));
        }
        if (16_000..20_000).contains(&ms) && ms % 10 == 0 {
            v.push((t + 3_000_000, 0x100, false, 8));
            v.push((t + 6_000_000, 0x100, false, 8));
        }
    }
    let _ = s;
    v.sort_by_key(|r| r.0);
    v
}

fn replay_file(text: &str) -> Result<Vec<Rec>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| {
            let (t, f) =
                frame::parse_candump_line(l).map_err(|e| format!("line {}: {e}", i + 1))?;
            Ok((t, f.id, f.ext, frame::dlc_of(f.data.len()).unwrap_or(0)))
        })
        .collect()
}

/// One stream record (the `can.frame.v1` JSON `can-fd` writes) as a [`Rec`].
fn rec_of(r: &bus::Record) -> Option<Rec> {
    let v: Value = serde_json::from_slice(&r.data).ok()?;
    Some((
        r.t_ns,
        u32::try_from(v["id"].as_u64()?).ok()?,
        v["ext"].as_bool()?,
        u8::try_from(v["dlc"].as_u64()?).ok()?,
    ))
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
    eprintln!("{TAG} start (once={}, watching can.frame/{}, events event/{}, learn {} s, simulate={}, bus={})", o.once, o.source, o.name, o.params.learn_ns / 1_000_000_000, o.simulate, o.bus.as_deref().unwrap_or("none"));
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
    let mut script: Vec<Rec> = if o.simulate {
        synthetic()
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
    let mut params = o.params.clone();
    if o.simulate {
        // The synthetic bus is 30 s long: learn for 6 s of it, look in 2 s windows.
        params = Params {
            learn_ns: 6_000_000_000,
            window_ns: 2_000_000_000,
            cooldown_ns: 4_000_000_000,
            ..params
        };
    }
    script.reverse();
    let mut link = Link::new(
        if local {
            o.bus.clone().filter(|_| o.replay.is_some() || o.simulate)
        } else {
            o.bus.clone()
        },
        &o.source,
    );
    let mut engine = Engine::new(params);
    let mut recent: Vec<Event> = Vec::new();
    let period = Duration::from_secs(if o.once { o.window } else { o.interval });
    let mut t0 = Instant::now();
    loop {
        let mut events = Vec::new();
        if local {
            for _ in 0..512 {
                match script.pop() {
                    Some((t, id, ext, dlc)) => events.extend(engine.feed(t, id, ext, dlc)),
                    None => break,
                }
            }
        } else {
            let mut got = false;
            if let Some(r) = link.next(Duration::from_millis(100)) {
                got = true;
                match rec_of(&r) {
                    Some((t, id, ext, dlc)) => events.extend(engine.feed(t, id, ext, dlc)),
                    None => link.state.error = Some("a record was not a can.frame.v1 body".into()),
                }
                while let Some(r) = link.reader.as_mut().and_then(|x| x.next(Duration::ZERO)) {
                    if let Some((t, id, ext, dlc)) = rec_of(&r) {
                        events.extend(engine.feed(t, id, ext, dlc));
                    }
                }
            }
            if !got {
                events.extend(engine.advance(bus::wall_ns()));
            }
        }
        for e in &events {
            println!(
                "{}",
                json!({"event": e.kind, "severity": e.severity, "subject": e.subject, "detail": e.detail})
            );
            link.publish(&o.name, e);
        }
        recent.extend(events);
        if recent.len() > 50 {
            recent.drain(..recent.len() - 50);
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
                source: &o.source,
                engine: &engine,
                last_events: &recent,
                bus: &link.state,
                error: err.as_deref(),
                export_url: &export_url,
            };
            let (report, vector) = build_report(&ctx);
            println!("{report}");
            if let Ok(mut st) = shared.lock() {
                st.push(report.clone());
            }
            link.telemetry(&o.name, &engine);
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

    #[test]
    fn options_bounded_and_defaulted() {
        let o = parse_opts(&s(&[
            "--once",
            "--learn-s",
            "0",
            "--rate-high-x",
            "1",
            "--name",
            "Bad",
            "--source",
            "vcan0",
        ]));
        assert!(o.once);
        assert_eq!(
            (
                o.params.learn_ns,
                o.params.rate_high_x,
                o.name.as_str(),
                o.source.as_str()
            ),
            (30_000_000_000, 3.0, "can-anomaly0", "vcan0")
        );
        assert_eq!(
            parse_opts(&s(&["--learn-s", "60"])).params.learn_ns,
            60_000_000_000
        );
        assert_eq!(parse_opts(&s(&[])).api_bind, "127.0.0.1:8073");
    }

    #[test]
    fn the_event_body_has_the_schema_shape() {
        let e = Event {
            t_ns: 9,
            kind: "can.new_id",
            severity: "warn",
            subject: "0x1a4".into(),
            detail: "d".repeat(400),
            value: Some(2.5),
        };
        let b = event_body("can-anomaly0", &"a".repeat(32), 3, &e);
        for k in [
            "source_id",
            "node_id",
            "t_ns",
            "seq",
            "kind",
            "severity",
            "subject",
            "detail",
            "value",
        ] {
            assert!(b.get(k).is_some(), "{k}");
        }
        assert_eq!(
            b["detail"].as_str().unwrap().len(),
            256,
            "detail is capped at the schema's limit"
        );
        let none = Event { value: None, ..e };
        assert!(event_body("a", "n", 1, &none).get("value").is_none());
    }

    fn run(recs: Vec<Rec>, p: Params) -> (Engine, Vec<Event>) {
        let mut e = Engine::new(p);
        let mut out = Vec::new();
        for (t, id, ext, dlc) in recs {
            out.extend(e.feed(t, id, ext, dlc));
        }
        (e, out)
    }

    #[test]
    fn the_synthetic_bus_shows_each_injected_fault() {
        let p = Params {
            learn_ns: 6_000_000_000,
            window_ns: 2_000_000_000,
            cooldown_ns: 4_000_000_000,
            ..Params::default()
        };
        let (e, ev) = run(synthetic(), p);
        let kinds: Vec<&str> = ev.iter().map(|x| x.kind).collect();
        for k in [
            "can.baseline",
            "can.new_id",
            "can.id_missing",
            "can.rate_high",
        ] {
            assert!(kinds.contains(&k), "{k} in {kinds:?}");
        }
        assert!(ev
            .iter()
            .any(|x| x.kind == "can.new_id" && x.subject == "0x1a4"));
        assert!(ev
            .iter()
            .any(|x| x.kind == "can.id_missing" && x.subject == "0x200"));
        assert!(ev
            .iter()
            .any(|x| x.kind == "can.rate_high" && x.subject == "0x100"));
        assert_eq!(e.phase(), Phase::Monitoring);
    }

    #[test]
    fn replay_lines_become_records() {
        let v = replay_file("(1.000000) can0 123#DEADBEEF\n\n(1.5) can0 18FF0001#0102\n").unwrap();
        assert_eq!(
            v,
            vec![
                (1_000_000_000, 0x123, false, 4),
                (1_500_000_000, 0x18ff0001, true, 2)
            ]
        );
        assert!(replay_file("(1.0) can0 123#00\nnope")
            .unwrap_err()
            .starts_with("line 2"));
    }

    #[test]
    fn a_stream_record_is_read_back() {
        let r = bus::Record { seq: 1, t_ns: 77, w_ns: 78, data: br#"{"source_id":"can0","id":420,"ext":false,"fd":false,"dlc":8,"data":"0000000000000000"}"#.to_vec() };
        assert_eq!(rec_of(&r), Some((77, 420, false, 8)));
        assert_eq!(
            rec_of(&bus::Record {
                data: b"nope".to_vec(),
                ..r
            }),
            None
        );
    }

    #[test]
    fn a_report_carries_phase_counts_and_the_vector() {
        let p = Params {
            learn_ns: 6_000_000_000,
            window_ns: 2_000_000_000,
            cooldown_ns: 4_000_000_000,
            ..Params::default()
        };
        let (e, ev) = run(synthetic(), p);
        let bus = BusState {
            connected: true,
            subscribed: true,
            published: ev.len() as u64,
            ..Default::default()
        };
        let ctx = Context {
            status: "ok",
            source: "can0",
            engine: &e,
            last_events: &ev,
            bus: &bus,
            error: None,
            export_url: &None,
        };
        let (r, v) = build_report(&ctx);
        assert_eq!(
            (r["phase"].as_str(), r["baseline_ids"].as_u64()),
            (Some("monitoring"), Some(3))
        );
        assert!(r["events_by_kind"]["can.new_id"].as_u64().unwrap() >= 1);
        assert_eq!(v[7], 1.0);
        assert!(v[2] >= 3.0 && v[3] >= 1.0 && v[4] >= 1.0);
        let (r, v) = build_report(&Context {
            status: "no_source",
            error: Some("no broker"),
            ..ctx
        });
        assert_eq!((r["error"].as_str(), v), (Some("no broker"), [-1.0; 8]));
    }

    #[test]
    fn telemetry_names_are_schema_valid() {
        for it in telemetry_items(&Engine::new(Params::default()), &BusState::default()) {
            let n = it["name"].as_str().unwrap();
            assert!(
                n.as_bytes()[0].is_ascii_lowercase()
                    && n.bytes().all(|c| c.is_ascii_lowercase()
                        || c.is_ascii_digit()
                        || b"_.".contains(&c)),
                "{n}"
            );
        }
    }
}
