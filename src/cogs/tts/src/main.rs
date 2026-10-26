//! Cognitum Cog: tts
//!
//! Speaks text on the node (WeftOS Ventuno cog plan 5.3). It reads `transcript.v1` readings
//! (`transcript/<source>`, default `reply0`, the `assistant`'s answers), synthesizes each with
//! Kokoro or Piper (a sherpa-onnx worker process, CPU), converts the speech to 16 kHz mono and
//! writes it, in real time, in 20 ms chunks to the stream `audio.pcm/<name>` (default `tts0`),
//! which the `audio-out` cog plays. Text that arrives while it is speaking waits its turn; more
//! than four waiting drops the oldest.
//!
//! Usage:
//!   cog-tts                               # speak transcript/reply0 from $WEFTOS_COGBUS
//!   cog-tts --engine kokoro               # the other voice (natural, slower than real time here)
//!   cog-tts --once --simulate             # a tone per word, no model
//!   cog-tts --once --replay FILE          # speak lines {"text": ...} from a file

mod bus;
mod export;
mod guide;
mod ingest;
mod resample;
mod worker;

use export::{round, Shared, State};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use worker::{Sherpa, Synth, Tone};

const TAG: &str = "[cog-tts]";
/// Per-cog id in the store vector tuple (35=assistant).
pub(crate) const STORE_ID: u32 = 36;
const KIND: &str = "audio.pcm";
const SCHEMA: &str = "pcm.s16le.v1";
const CHUNK_BYTES: usize = 640;
const CHUNK_MS: u64 = 20;
const MAX_TEXT: usize = 600;
const MAX_QUEUE: usize = 4;
const MODELS_ROOT: &str = "/data/bench/models";
const PYTHON: &str = "/data/bench/venv-tts/bin/python";

#[derive(Debug, Clone, PartialEq)]
struct Opts {
    once: bool,
    interval: u64,
    window: u64,
    source: String,
    name: String,
    engine: String,
    speaker: u32,
    speed: f64,
    threads: u32,
    python: String,
    models_root: String,
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
    let engine = match arg(a, "--engine") {
        Some(e @ ("kokoro" | "piper")) => e.to_string(),
        Some(_) => {
            eprintln!("{TAG} --engine must be kokoro or piper, using piper");
            "piper".into()
        }
        None => "piper".into(),
    };
    Opts {
        once: a.iter().any(|x| x == "--once"),
        interval: num(a, "--interval", 5, 1, 300),
        window: num(a, "--window", 10, 1, 120),
        source: id_opt(a, "--source", "reply0"),
        name: id_opt(a, "--name", "tts0"),
        engine,
        speaker: num(a, "--speaker", 0, 0, 100),
        speed: num(a, "--speed", 1.0, 0.5, 2.0),
        threads: num(a, "--threads", 3, 1, 16),
        python: arg(a, "--python")
            .filter(|s| !s.is_empty())
            .map(String::from)
            .or_else(|| std::env::var("TTS_PYTHON").ok())
            .unwrap_or_else(|| PYTHON.into()),
        models_root: arg(a, "--models-root")
            .filter(|s| !s.is_empty())
            .unwrap_or(MODELS_ROOT)
            .to_string(),
        bus: arg(a, "--bus")
            .map(String::from)
            .or_else(|| std::env::var("WEFTOS_COGBUS").ok()),
        api_bind: arg(a, "--api-bind").unwrap_or("127.0.0.1:8070").to_string(),
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

/// Text fit to speak: control characters become spaces, whitespace collapses, length is capped.
fn speakable(t: &str) -> String {
    let t: String = t
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    t.chars()
        .take(MAX_TEXT)
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[derive(Debug, Default, Clone)]
struct Stats {
    spoken: u64,
    dropped: u64,
    failures: u64,
    synth_ms: u64,
    audio_ms: u64,
    last_text: Option<String>,
    last_error: Option<String>,
}

/// Synthesize `text` to 16 kHz s16le bytes (a whole number of chunks, padded with silence).
fn render(synth: &mut dyn Synth, text: &str, s: &mut Stats) -> Option<Vec<u8>> {
    match synth.say(text) {
        Ok(sp) => {
            s.synth_ms += sp.ms;
            let mut pcm = resample::to_s16le(&resample::resample(&sp.samples, sp.rate, 16_000));
            if pcm.is_empty() {
                s.failures += 1;
                s.last_error = Some("the synthesizer produced no audio".into());
                return None;
            }
            pcm.resize(pcm.len().div_ceil(CHUNK_BYTES) * CHUNK_BYTES, 0);
            s.spoken += 1;
            s.audio_ms += (pcm.len() / CHUNK_BYTES) as u64 * CHUNK_MS;
            s.last_text = Some(text.to_string());
            Some(pcm)
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
    stream_open: bool,
    seq: u64,
    error: Option<String>,
}

struct Context<'a> {
    status: &'a str,
    voice: &'a str,
    source: &'a str,
    name: &'a str,
    queued: usize,
    speaking: bool,
    bus: &'a BusState,
    error: Option<&'a str>,
    export_url: &'a Option<String>,
}

fn build_report(s: &Stats, c: &Context) -> (Value, [f64; 8]) {
    let ok = c.status != "no_source";
    let rtf = if s.audio_ms > 0 {
        s.synth_ms as f64 / s.audio_ms as f64
    } else {
        0.0
    };
    let vector = if ok {
        [
            s.spoken as f64,
            round(rtf, 3),
            round(s.audio_ms as f64 / 1000.0, 2),
            s.dropped as f64,
            s.failures as f64,
            c.queued as f64,
            c.bus.connected as u8 as f64,
            c.bus.seq as f64,
        ]
    } else {
        [-1.0; 8]
    };
    let mut r = json!({
        "status": c.status,
        "provenance": match c.status { "simulate" => "SYNTHETIC", "replay" => "REPLAY", _ => "MEASURED" },
        "voice": c.voice,
        "listening_to": format!("transcript/{}", c.source),
        "stream": format!("{KIND}/{}", c.name),
        "spoken": s.spoken, "dropped": s.dropped, "failures": s.failures,
        "audio_s": round(s.audio_ms as f64 / 1000.0, 2),
        "real_time_factor": round(rtf, 3),
        "queued": c.queued, "speaking": c.speaking,
        "last_text": s.last_text, "last_error": s.last_error,
        "bus": { "connected": c.bus.connected, "stream_open": c.bus.stream_open, "seq": c.bus.seq, "error": c.bus.error },
        "vector_slots": ["spoken", "rtf", "audio_s", "dropped", "failures", "queued", "bus_connected", "stream_seq"],
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
        i("spoken", s.spoken as f64, ""),
        i("dropped", s.dropped as f64, ""),
        i("failures", s.failures as f64, ""),
        i("audio", s.audio_ms as f64 / 1000.0, "s"),
        i("synth.time", s.synth_ms as f64 / 1000.0, "ms"),
        i("stream.seq", bus.seq as f64, ""),
        i("bus.connected", bus.connected as u8 as f64, ""),
    ]
}

/// Holds the writer to real time without drifting.
struct Pacer(Option<Instant>);

impl Pacer {
    fn wait(&mut self) {
        let step = Duration::from_millis(CHUNK_MS);
        let due = self.0.map_or_else(Instant::now, |t| t + step);
        let now = Instant::now();
        if due > now {
            std::thread::sleep(due - now);
        }
        self.0 = Some(due.max(now - step * 5));
    }
}

struct Link {
    path: Option<String>,
    source: String,
    name: String,
    bus: Option<bus::Bus>,
    msgs: Option<std::sync::mpsc::Receiver<bus::Msg>>,
    writer: Option<bus::RingWriter>,
    state: BusState,
    tseq: u64,
    last_try: Option<Instant>,
}

impl Link {
    fn new(path: Option<String>, source: &str, name: &str) -> Self {
        Self {
            path,
            source: source.into(),
            name: name.into(),
            bus: None,
            msgs: None,
            writer: None,
            state: BusState::default(),
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
            self.writer = None;
            self.state = BusState {
                seq: self.state.seq,
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
            let w = b.stream_writer(KIND, &self.name, SCHEMA, CHUNK_BYTES as u64, None)?;
            Ok((b, w))
        });
        match r {
            Ok((b, w)) => {
                eprintln!(
                    "{TAG} speaking transcript/{} onto {KIND}/{} on {path} (node {})",
                    self.source, self.name, b.node_id
                );
                self.state = BusState {
                    connected: true,
                    stream_open: true,
                    seq: w.last_seq(),
                    error: None,
                };
                self.msgs = b.take_msgs();
                self.writer = Some(w);
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

    /// Write `pcm` (whole chunks) to the stream in real time. Without a stream, nothing to pace.
    fn speak(&mut self, pcm: &[u8]) {
        let Some(w) = self.writer.as_mut() else {
            return;
        };
        let mut pace = Pacer(None);
        for c in pcm.chunks_exact(CHUNK_BYTES) {
            pace.wait();
            match w.write(bus::wall_ns(), c) {
                Ok(s) => self.state.seq = s,
                Err(e) => {
                    self.state.error = Some(e);
                    return;
                }
            }
        }
    }

    fn telemetry(&mut self, s: &Stats) {
        self.tseq += 1;
        let Some(b) = self.bus.as_ref().filter(|b| !b.is_closed()) else {
            return;
        };
        let body = json!({"source_id": self.name, "node_id": b.node_id, "t_ns": bus::wall_ns(), "seq": self.tseq, "items": telemetry_items(s, &self.state)});
        if let Err(e) = b.publish(
            "telemetry",
            &self.name,
            "telemetry.v1",
            bus::wall_ns(),
            body,
        ) {
            eprintln!("{TAG} telemetry: {e}");
        }
    }
}

fn open_synth(o: &Opts, data_dir: &std::path::Path) -> Result<Box<dyn Synth>, String> {
    if o.simulate {
        return Ok(Box::new(Tone));
    }
    Sherpa::start(
        &o.python,
        &o.engine,
        &o.models_root,
        o.threads,
        o.speaker,
        o.speed,
        data_dir,
    )
    .map(|s| Box::new(s) as Box<dyn Synth>)
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
        "{TAG} start (once={}, engine={}, from transcript/{}, onto {KIND}/{}, simulate={}, bus={})",
        o.once,
        o.engine,
        o.source,
        o.name,
        o.simulate,
        o.bus.as_deref().unwrap_or("none")
    );
    let data_dir = std::env::var("COGNITUM_COG_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join(format!("cog-tts-{}", std::process::id())));
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
        vec!["This is a synthetic voice test.".into()]
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
    let mut link = Link::new(o.bus.clone(), &o.source, &o.name);
    let mut synth: Option<Box<dyn Synth>> = None;
    let mut synth_err: Option<String> = None;
    let mut synth_try: Option<Instant> = None;
    let mut stats = Stats::default();
    let mut queue: VecDeque<String> = VecDeque::new();
    let period = Duration::from_secs(if o.once { o.window } else { o.interval });
    let mut t0 = Instant::now();
    loop {
        if synth.is_none()
            && input_err.is_none()
            && synth_try.is_none_or(|t| t.elapsed() >= Duration::from_secs(5))
        {
            synth_try = Some(Instant::now());
            match open_synth(&o, &data_dir) {
                Ok(s) => {
                    eprintln!("{TAG} voice: {}", s.label());
                    synth_err = None;
                    synth = Some(s);
                }
                Err(e) => synth_err = Some(e),
            }
        }
        let text = if local {
            script.pop()
        } else {
            link.next(Duration::from_millis(100))
                .map(|m| m.body["text"].as_str().unwrap_or_default().to_string())
        };
        if let Some(t) = text.map(|t| speakable(&t)).filter(|t| !t.is_empty()) {
            queue.push_back(t);
            while queue.len() > MAX_QUEUE {
                queue.pop_front();
                stats.dropped += 1;
            }
        }
        let mut speaking = false;
        if let (Some(sy), Some(t)) = (synth.as_mut(), queue.pop_front()) {
            speaking = true;
            link.connect();
            if let Some(pcm) = render(sy.as_mut(), &t, &mut stats) {
                println!(
                    "{}",
                    json!({"speaking": t, "audio_ms": pcm.len() / CHUNK_BYTES * CHUNK_MS as usize})
                );
                link.speak(&pcm);
            }
        }
        let done = local && script.is_empty() && queue.is_empty() && !speaking;
        let failed = input_err.is_some() || (synth.is_none() && synth_err.is_some() && local);
        if done || failed || t0.elapsed() >= period {
            let status = if failed || synth.is_none() {
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
            let err = input_err.clone().or_else(|| synth_err.clone()).or_else(|| {
                (status == "no_source").then(|| {
                    link.state
                        .error
                        .clone()
                        .unwrap_or_else(|| "no broker: WEFTOS_COGBUS is not set".into())
                })
            });
            let label = synth
                .as_ref()
                .map_or_else(|| "none".to_string(), |s| s.label());
            let ctx = Context {
                status,
                voice: &label,
                source: &o.source,
                name: &o.name,
                queued: queue.len(),
                speaking,
                bus: &link.state,
                error: err.as_deref(),
                export_url: &export_url,
            };
            let (report, vector) = build_report(&stats, &ctx);
            println!("{report}");
            if let Ok(mut st) = shared.lock() {
                st.push(report.clone());
            }
            link.telemetry(&stats);
            if report["status"] == "ok" {
                if let Err(e) = ingest::store_to_seed(&target, token.as_deref(), &vector) {
                    eprintln!("{TAG} store error: {e}");
                }
            } else if report["status"] == "no_source" {
                eprintln!("{TAG} no source: {}", report["error"]);
            }
            if o.once || done || failed {
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
            "--once", "--engine", "festival", "--speed", "9", "--name", "Bad", "--source", "ans",
        ]));
        assert!(o.once);
        assert_eq!(
            (
                o.engine.as_str(),
                o.speed,
                o.name.as_str(),
                o.source.as_str()
            ),
            ("piper", 1.0, "tts0", "ans")
        );
        assert_eq!(parse_opts(&s(&["--engine", "kokoro"])).engine, "kokoro");
        let d = parse_opts(&s(&[]));
        assert_eq!(
            (d.api_bind.as_str(), d.threads, d.models_root.as_str()),
            ("127.0.0.1:8070", 3, MODELS_ROOT)
        );
    }

    #[test]
    fn text_is_made_speakable() {
        assert_eq!(
            speakable("  hello\n\tthere \u{7} world "),
            "hello there world"
        );
        assert_eq!(
            speakable(&"a ".repeat(500)).len(),
            MAX_TEXT - 1,
            "capped; the cut may end on a space"
        );
        assert_eq!(speakable("\n\n"), "");
    }

    #[test]
    fn rendering_gives_whole_16k_chunks_and_counts() {
        let mut st = Stats::default();
        let pcm = render(&mut Tone, "one two three four", &mut st).unwrap();
        assert_eq!(pcm.len() % CHUNK_BYTES, 0);
        // 4 words x 80 ms = 320 ms = 16 chunks.
        assert_eq!(pcm.len() / CHUNK_BYTES, 16);
        assert_eq!((st.spoken, st.audio_ms, st.failures), (1, 320, 0));
        assert!(
            pcm.chunks_exact(2)
                .any(|b| i16::from_le_bytes([b[0], b[1]]).abs() > 5000),
            "audible"
        );
    }

    struct Failing;
    impl Synth for Failing {
        fn say(&mut self, _: &str) -> Result<worker::Speech, String> {
            Err("worker died".into())
        }
        fn label(&self) -> String {
            "failing".into()
        }
    }

    struct Empty;
    impl Synth for Empty {
        fn say(&mut self, _: &str) -> Result<worker::Speech, String> {
            Ok(worker::Speech {
                samples: vec![],
                rate: 24_000,
                ms: 1,
            })
        }
        fn label(&self) -> String {
            "empty".into()
        }
    }

    #[test]
    fn synthesis_failures_are_counted_not_spoken() {
        let mut st = Stats::default();
        assert!(render(&mut Failing, "x", &mut st).is_none());
        assert!(render(&mut Empty, "x", &mut st).is_none());
        assert_eq!(
            (st.failures, st.spoken, st.last_error.as_deref()),
            (2, 0, Some("the synthesizer produced no audio"))
        );
    }

    #[test]
    fn a_report_carries_counts_rtf_and_the_vector() {
        let st = Stats {
            spoken: 3,
            dropped: 1,
            failures: 0,
            synth_ms: 1500,
            audio_ms: 6000,
            last_text: Some("hi".into()),
            last_error: None,
        };
        let bus = BusState {
            connected: true,
            stream_open: true,
            seq: 300,
            error: None,
        };
        let ctx = Context {
            status: "ok",
            voice: "sherpa-onnx kokoro",
            source: "reply0",
            name: "tts0",
            queued: 1,
            speaking: false,
            bus: &bus,
            error: None,
            export_url: &None,
        };
        let (r, v) = build_report(&st, &ctx);
        assert_eq!(
            (r["real_time_factor"].as_f64(), r["stream"].as_str()),
            (Some(0.25), Some("audio.pcm/tts0"))
        );
        assert_eq!(v, [3.0, 0.25, 6.0, 1.0, 0.0, 1.0, 1.0, 300.0]);
        let (r, v) = build_report(
            &Stats::default(),
            &Context {
                status: "no_source",
                error: Some("worker"),
                ..ctx
            },
        );
        assert_eq!((r["error"].as_str(), v), (Some("worker"), [-1.0; 8]));
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
