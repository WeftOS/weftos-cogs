//! Cognitum Cog: wake-word
//!
//! The second stage of the wake gate (WeftOS Ventuno cog plan 5.3 and the brick spike: a
//! keyword model alone fires on ordinary speech, so confirm with the transcript). It reads the
//! `transcript.v1` readings of `asr` (`transcript/<source>`, default `asr0`) and publishes, on
//! `transcript/<name>` (default `wake0`), only the command that follows the wake phrase. A
//! transcript that is just the phrase arms the gate for a few seconds: the next transcript is
//! the command. Everything else is ignored, and counted.
//!
//! Usage:
//!   cog-wake-word                          # gate transcript/asr0 from $WEFTOS_COGBUS
//!   cog-wake-word --phrase "hey computer"  # another wake phrase
//!   cog-wake-word --once --simulate        # a canned conversation through the gate
//!   cog-wake-word --once --replay FILE     # transcripts from a JSON-lines file

mod bus;
mod export;
mod gate;
mod guide;
mod ingest;

use export::{round, Shared, State};
use gate::{Gate, Outcome};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TAG: &str = "[cog-wake-word]";
/// Per-cog id in the store vector tuple (33=asr).
pub(crate) const STORE_ID: u32 = 34;

#[derive(Debug, Clone, PartialEq)]
struct Opts {
    once: bool,
    interval: u64,
    window: u64,
    source: String,
    name: String,
    phrase: String,
    listen_s: u64,
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
        source: id_opt(a, "--source", "asr0"),
        name: id_opt(a, "--name", "wake0"),
        phrase: arg(a, "--phrase")
            .filter(|p| !p.trim().is_empty() && p.len() <= 64)
            .unwrap_or(gate::DEFAULT_PHRASE)
            .to_string(),
        listen_s: num(a, "--listen-s", 8, 1, 60),
        bus: arg(a, "--bus")
            .map(String::from)
            .or_else(|| std::env::var("WEFTOS_COGBUS").ok()),
        api_bind: arg(a, "--api-bind").unwrap_or("127.0.0.1:8068").to_string(),
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

#[derive(Debug, Default, Clone)]
struct Stats {
    seen: u64,
    commands: u64,
    bare_wakes: u64,
    ignored: u64,
    last_command: Option<String>,
}

/// One transcript through the gate; returns the command to publish, if any.
fn judge(g: &mut Gate, s: &mut Stats, text: &str, start_ns: u64, end_ns: u64) -> Option<String> {
    s.seen += 1;
    match g.on_transcript(text, start_ns, end_ns) {
        Outcome::Command(c) => {
            s.commands += 1;
            s.last_command = Some(c.clone());
            Some(c)
        }
        Outcome::BareWake => {
            s.bare_wakes += 1;
            None
        }
        Outcome::Ignored => {
            s.ignored += 1;
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
    phrase: &'a str,
    source: &'a str,
    armed: bool,
    bus: &'a BusState,
    error: Option<&'a str>,
    export_url: &'a Option<String>,
}

fn build_report(s: &Stats, c: &Context) -> (Value, [f64; 8]) {
    let ok = c.status != "no_source";
    let vector = if ok {
        [
            s.seen as f64,
            s.commands as f64,
            s.bare_wakes as f64,
            s.ignored as f64,
            c.armed as u8 as f64,
            c.bus.published as f64,
            c.bus.connected as u8 as f64,
            round(
                if s.seen > 0 {
                    s.commands as f64 / s.seen as f64
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
        "phrase": c.phrase,
        "listening_to": format!("transcript/{}", c.source),
        "transcripts_seen": s.seen, "commands": s.commands, "bare_wakes": s.bare_wakes, "ignored": s.ignored,
        "armed": c.armed,
        "last_command": s.last_command,
        "bus": { "connected": c.bus.connected, "subscribed": c.bus.subscribed, "published": c.bus.published, "error": c.bus.error },
        "vector_slots": ["seen", "commands", "bare_wakes", "ignored", "armed", "published", "bus_connected", "command_ratio"],
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
    let i = |n: &str, v: f64| json!({"name": n, "value": v, "unit": ""});
    vec![
        i("transcripts.seen", s.seen as f64),
        i("commands", s.commands as f64),
        i("wakes.bare", s.bare_wakes as f64),
        i("transcripts.ignored", s.ignored as f64),
        i("bus.connected", bus.connected as u8 as f64),
    ]
}

fn transcript_body(
    name: &str,
    node_id: &str,
    seq: u64,
    lang: &str,
    text: &str,
    start_ns: u64,
    end_ns: u64,
) -> Value {
    json!({
        "source_id": name, "node_id": node_id, "t_ns": start_ns, "seq": seq,
        "text": text.chars().take(4096).collect::<String>(), "lang": lang,
        "start_ns": start_ns, "end_ns": end_ns.max(start_ns), "confidence": 1.0,
    })
}

struct Link {
    path: Option<String>,
    source: String,
    bus: Option<bus::Bus>,
    msgs: Option<std::sync::mpsc::Receiver<bus::Msg>>,
    state: BusState,
    seq: u64,
    tseq: u64,
    last_try: Option<Instant>,
}

impl Link {
    fn new(path: Option<String>, source: &str) -> Self {
        Self {
            path,
            source: source.into(),
            bus: None,
            msgs: None,
            state: BusState::default(),
            seq: 0,
            tseq: 0,
            last_try: None,
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
            b.subscribe(&format!("transcript/{}", self.source), "transcript.v1")?;
            Ok(b)
        });
        match r {
            Ok(b) => {
                eprintln!(
                    "{TAG} listening to transcript/{} on {path} (node {})",
                    self.source, b.node_id
                );
                self.msgs = b.take_msgs();
                self.state.connected = true;
                self.state.subscribed = true;
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
        self.connect();
        match self.msgs.as_ref() {
            Some(rx) => rx.recv_timeout(wait).ok(),
            None => {
                std::thread::sleep(wait);
                None
            }
        }
    }

    fn publish(&mut self, name: &str, text: &str, start_ns: u64, end_ns: u64) {
        let Some(b) = self.bus.as_ref().filter(|b| !b.is_closed()) else {
            return;
        };
        self.seq += 1;
        let body = transcript_body(name, &b.node_id, self.seq, "en", text, start_ns, end_ns);
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

/// `(text, start_ns, end_ns)` triples for `--simulate`.
fn canned() -> Vec<(String, u64, u64)> {
    let s = 1_000_000_000u64;
    [
        ("Nice weather today", 1, 2),
        ("Hey Arduino, what time is it?", 10, 12),
        ("Hey Arduino", 20, 21),
        ("turn on the kitchen light", 23, 25),
        ("and then something else", 30, 32),
    ]
    .iter()
    .map(|(t, a, b)| (t.to_string(), a * s, b * s))
    .collect()
}

/// Transcripts from JSON lines `{"text", "start_ns", "end_ns"}` (or a transcript.v1 body).
fn replay_file(text: &str) -> Result<Vec<(String, u64, u64)>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| {
            let v: Value = serde_json::from_str(l).map_err(|e| format!("line {}: {e}", i + 1))?;
            let t = v["text"]
                .as_str()
                .ok_or_else(|| format!("line {}: no text", i + 1))?
                .to_string();
            let a = v["start_ns"].as_u64().unwrap_or(i as u64 * 10_000_000_000);
            Ok((t, a, v["end_ns"].as_u64().unwrap_or(a + 1_000_000_000)))
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
        "{TAG} start (once={}, phrase={:?}, from transcript/{}, to transcript/{}, simulate={}, bus={})",
        o.once,
        o.phrase,
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
    let mut script: Vec<(String, u64, u64)> = if o.simulate {
        canned()
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
        if o.once && local { None } else { o.bus.clone() },
        &o.source,
    );
    let mut gate = Gate::new(&o.phrase, o.listen_s);
    let mut stats = Stats::default();
    let period = Duration::from_secs(if o.once { o.window } else { o.interval });
    let mut t0 = Instant::now();
    loop {
        let item = if local {
            script.pop()
        } else {
            link.next(Duration::from_millis(100)).map(|m| {
                let t = m.body["text"].as_str().unwrap_or_default().to_string();
                (
                    t,
                    m.body["start_ns"].as_u64().unwrap_or(m.t_ns),
                    m.body["end_ns"].as_u64().unwrap_or(m.t_ns),
                )
            })
        };
        if let Some((text, a, b)) = item {
            if let Some(cmd) = judge(&mut gate, &mut stats, &text, a, b) {
                println!("{}", json!({"command": cmd, "from": text}));
                link.publish(&o.name, &cmd, a, b);
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
                phrase: &o.phrase,
                source: &o.source,
                armed: gate.armed(bus::wall_ns()),
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

    #[test]
    fn options_bounded_and_defaulted() {
        let o = parse_opts(&s(&[
            "--once",
            "--listen-s",
            "0",
            "--name",
            "Bad",
            "--phrase",
            "hey computer",
        ]));
        assert!(o.once);
        assert_eq!(
            (
                o.listen_s,
                o.name.as_str(),
                o.phrase.as_str(),
                o.source.as_str()
            ),
            (8, "wake0", "hey computer", "asr0")
        );
        let d = parse_opts(&s(&["--phrase", "  "]));
        assert_eq!(
            (d.phrase.as_str(), d.api_bind.as_str()),
            ("hey arduino", "127.0.0.1:8068")
        );
    }

    #[test]
    fn the_canned_conversation_yields_two_commands() {
        let mut g = Gate::new("hey arduino", 8);
        let mut st = Stats::default();
        let out: Vec<String> = canned()
            .iter()
            .filter_map(|(t, a, b)| judge(&mut g, &mut st, t, *a, *b))
            .collect();
        assert_eq!(out, ["what time is it", "turn on the kitchen light"]);
        assert_eq!(
            (st.seen, st.commands, st.bare_wakes, st.ignored),
            (5, 2, 1, 2)
        );
        assert_eq!(
            st.last_command.as_deref(),
            Some("turn on the kitchen light")
        );
    }

    #[test]
    fn replay_lines_parse_with_defaults_and_errors() {
        let v = replay_file(
            "{\"text\":\"hey arduino go\",\"start_ns\":5,\"end_ns\":9}\n\n{\"text\":\"again\"}\n",
        )
        .unwrap();
        assert_eq!(v[0], ("hey arduino go".to_string(), 5, 9));
        assert_eq!(v[1].0, "again");
        assert!(replay_file("{\"nope\":1}").unwrap_err().contains("no text"));
        assert!(replay_file("x").is_err());
    }

    #[test]
    fn the_published_body_has_the_schema_shape() {
        let b = transcript_body("wake0", &"a".repeat(32), 2, "en", "turn on the light", 5, 9);
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
        assert_eq!(b["end_ns"], 9);
        assert_eq!(
            transcript_body("w", "n", 1, "en", "x", 9, 5)["end_ns"],
            9,
            "end never precedes start"
        );
    }

    #[test]
    fn a_report_carries_counts_and_the_vector() {
        let st = Stats {
            seen: 5,
            commands: 2,
            bare_wakes: 1,
            ignored: 2,
            last_command: Some("go".into()),
        };
        let bus = BusState {
            connected: true,
            subscribed: true,
            published: 2,
            error: None,
        };
        let ctx = Context {
            status: "ok",
            phrase: "hey arduino",
            source: "asr0",
            armed: true,
            bus: &bus,
            error: None,
            export_url: &None,
        };
        let (r, v) = build_report(&st, &ctx);
        assert_eq!(
            (
                r["commands"].as_u64(),
                r["armed"].as_bool(),
                r["listening_to"].as_str()
            ),
            (Some(2), Some(true), Some("transcript/asr0"))
        );
        assert_eq!(v, [5.0, 2.0, 1.0, 2.0, 1.0, 2.0, 1.0, 0.4]);
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
        }
    }
}
