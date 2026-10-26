//! Cognitum Cog: asr
//!
//! Hears the microphone stream and publishes what was said (WeftOS Ventuno cog plan 5.3).
//! It reads `audio.pcm/<source>` (the `audio-in` cog's stream), cuts it into utterances by
//! energy, transcribes each with Parakeet on the CPU (a sherpa-onnx worker child process) and
//! publishes a `transcript.v1` reading on `transcript/<name>` (default `asr0`). Nothing is
//! recognized while the room is being learned (the first second) and the transcript, not the
//! energy, decides whether an utterance was speech: a cough comes back as empty text and is
//! dropped.
//!
//! Usage:
//!   cog-asr                               # listen on $WEFTOS_COGBUS
//!   cog-asr --once --window 10            # listen 10 s, report, exit
//!   cog-asr --once --simulate             # synthetic utterances, no model
//!   cog-asr --once --replay FILE          # run a WAV / raw 16 kHz mono file through the same path

mod bus;
mod export;
mod guide;
mod ingest;
mod segment;
mod worker;

use export::{round, Shared, State};
use segment::{Params, Segmenter, Utterance, CHUNK_BYTES, CHUNK_MS};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use worker::{Parakeet, Recognizer, Synthetic};

const TAG: &str = "[cog-asr]";
/// Per-cog id in the store vector tuple (32=mcu-bridge).
pub(crate) const STORE_ID: u32 = 33;
const AUDIO_KIND: &str = "audio.pcm";
const AUDIO_SCHEMA: &str = "pcm.s16le.v1";
const MODEL_DIR: &str = "/data/bench/models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8";
const PYTHON: &str = "/data/bench/venv/bin/python";

#[derive(Debug, Clone, PartialEq)]
struct Opts {
    once: bool,
    interval: u64,
    window: u64,
    source: String,
    name: String,
    lang: String,
    python: String,
    model_dir: String,
    threads: u32,
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
    let lang = match arg(a, "--lang") {
        Some(l)
            if l.len() <= 16
                && l.split('-').next().is_some_and(|p| {
                    (2..=3).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_lowercase())
                }) =>
        {
            l.to_string()
        }
        _ => "en".into(),
    };
    Opts {
        once: a.iter().any(|x| x == "--once"),
        interval: num(a, "--interval", 5, 1, 300),
        window: num(a, "--window", 10, 1, 120),
        source: id_opt(a, "--source", "mic0"),
        name: id_opt(a, "--name", "asr0"),
        lang,
        python: arg(a, "--python")
            .filter(|s| !s.is_empty())
            .map(String::from)
            .or_else(|| std::env::var("ASR_PYTHON").ok())
            .unwrap_or_else(|| PYTHON.into()),
        model_dir: arg(a, "--model-dir")
            .filter(|s| !s.is_empty())
            .unwrap_or(MODEL_DIR)
            .to_string(),
        threads: num(a, "--threads", 3, 1, 16),
        params: Params {
            margin_db: num(a, "--margin-db", d.margin_db, 3.0, 40.0),
            min_db: num(a, "--min-db", d.min_db, -90.0, -10.0),
            hang_ms: num(a, "--hang-ms", d.hang_ms, 100, 3000),
            min_ms: num(a, "--min-ms", d.min_ms, 60, 3000),
            max_ms: num(a, "--max-ms", d.max_ms, 1000, 60_000),
            preroll_ms: d.preroll_ms,
        },
        bus: arg(a, "--bus")
            .map(String::from)
            .or_else(|| std::env::var("WEFTOS_COGBUS").ok()),
        api_bind: arg(a, "--api-bind").unwrap_or("127.0.0.1:8067").to_string(),
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

/// What was heard in one utterance.
#[derive(Debug, Clone, PartialEq)]
pub struct Heard {
    pub text: String,
    pub start_ns: u64,
    pub end_ns: u64,
    pub audio_ms: u64,
    pub asr_ms: u64,
}

#[derive(Debug, Default, Clone)]
struct Stats {
    utterances: u64,
    heard: u64,
    empty: u64,
    errors: u64,
    audio_ms: u64,
    asr_ms: u64,
    last_text: Option<String>,
    last_error: Option<String>,
}

/// Segmenter plus recognizer.
struct Engine {
    seg: Segmenter,
    rec: Box<dyn Recognizer>,
    stats: Stats,
}

impl Engine {
    fn new(p: Params, rec: Box<dyn Recognizer>) -> Self {
        Self {
            seg: Segmenter::new(p),
            rec,
            stats: Stats::default(),
        }
    }

    fn recognize(&mut self, u: Utterance) -> Option<Heard> {
        self.stats.utterances += 1;
        self.stats.audio_ms += u.ms();
        match self.rec.transcribe(&u.pcm) {
            Ok((text, ms)) => {
                self.stats.asr_ms += ms;
                let text = text.trim().to_string();
                if text.is_empty() {
                    self.stats.empty += 1;
                    return None;
                }
                self.stats.heard += 1;
                self.stats.last_text = Some(text.clone());
                Some(Heard {
                    text,
                    start_ns: u.start_ns,
                    end_ns: u.end_ns,
                    audio_ms: u.ms(),
                    asr_ms: ms,
                })
            }
            Err(e) => {
                self.stats.errors += 1;
                self.stats.last_error = Some(e);
                None
            }
        }
    }

    /// One chunk in; a heard utterance out when one just ended.
    fn feed(&mut self, chunk: &[u8], t_ns: u64) -> Option<Heard> {
        let u = self.seg.feed(chunk, t_ns)?;
        self.recognize(u)
    }

    fn flush(&mut self) -> Option<Heard> {
        let u = self.seg.flush()?;
        self.recognize(u)
    }
}

#[derive(Debug, Default, Clone)]
struct BusState {
    connected: bool,
    streaming: bool,
    published: u64,
    skipped: u64,
    error: Option<String>,
}

/// What a report says besides the counters.
struct Context<'a> {
    status: &'a str,
    recognizer: &'a str,
    source: &'a str,
    bus: &'a BusState,
    speaking: bool,
    floor_db: f64,
    error: Option<&'a str>,
    export_url: &'a Option<String>,
}

fn build_report(s: &Stats, c: &Context) -> (Value, [f64; 8]) {
    let ok = c.status != "no_source";
    let rtf = if s.audio_ms > 0 {
        s.asr_ms as f64 / s.audio_ms as f64
    } else {
        0.0
    };
    let mean = if s.utterances > 0 {
        s.asr_ms as f64 / s.utterances as f64
    } else {
        0.0
    };
    let vector = if ok {
        [
            s.heard as f64,
            round(rtf, 3),
            round(mean, 1),
            round(c.floor_db, 1),
            round(s.audio_ms as f64 / 1000.0, 2),
            c.bus.skipped as f64,
            c.bus.connected as u8 as f64,
            s.errors as f64,
        ]
    } else {
        [-1.0; 8]
    };
    let mut r = json!({
        "status": c.status,
        "provenance": match c.status { "simulate" => "SYNTHETIC", "replay" => "REPLAY", _ => "MEASURED" },
        "recognizer": c.recognizer,
        "stream": format!("{AUDIO_KIND}/{}", c.source),
        "utterances": s.utterances, "transcripts": s.heard, "empty": s.empty, "errors": s.errors,
        "audio_s": round(s.audio_ms as f64 / 1000.0, 2),
        "asr_ms_mean": round(mean, 1),
        "real_time_factor": round(rtf, 3),
        "noise_floor_dbfs": round(c.floor_db, 1),
        "speaking": c.speaking,
        "last_text": s.last_text,
        "last_error": s.last_error,
        "bus": { "connected": c.bus.connected, "streaming": c.bus.streaming, "published": c.bus.published, "skipped_chunks": c.bus.skipped, "error": c.bus.error },
        "vector_slots": ["transcripts", "rtf", "asr_ms_mean", "noise_floor_dbfs", "audio_s", "skipped_chunks", "bus_connected", "errors"],
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
        i("utterances", s.utterances as f64, ""),
        i("transcripts", s.heard as f64, ""),
        i("errors", s.errors as f64, ""),
        i("audio", s.audio_ms as f64 / 1000.0, "s"),
        i("asr.time", s.asr_ms as f64 / 1000.0, "s"),
        i("chunks.skipped", bus.skipped as f64, ""),
        i("bus.connected", bus.connected as u8 as f64, ""),
    ]
}

/// The `transcript.v1` body.
fn transcript_body(name: &str, node_id: &str, seq: u64, lang: &str, h: &Heard) -> Value {
    json!({
        "source_id": name, "node_id": node_id, "t_ns": h.start_ns, "seq": seq,
        "text": h.text.chars().take(4096).collect::<String>(), "lang": lang,
        "start_ns": h.start_ns, "end_ns": h.end_ns.max(h.start_ns),
        // Parakeet reports no confidence; 1.0 means "not measured", not "certain".
        "confidence": 1.0,
    })
}

/// The broker side.
struct Link {
    path: Option<String>,
    source: String,
    bus: Option<bus::Bus>,
    reader: Option<bus::StreamReader>,
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
            reader: None,
            state: BusState::default(),
            seq: 0,
            tseq: 0,
            last_try: None,
        }
    }

    fn connect(&mut self, want_stream: bool) {
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
        let ready = self.bus.is_some() && (self.reader.is_some() || !want_stream);
        if ready
            || self
                .last_try
                .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        self.last_try = Some(Instant::now());
        let token = std::env::var("WEFTOS_COGBUS_TOKEN").unwrap_or_default();
        let r = bus::Bus::connect(&path, &token).and_then(|b| {
            let r = if want_stream {
                Some(b.stream_reader(&format!("{AUDIO_KIND}/{}", self.source), AUDIO_SCHEMA)?)
            } else {
                None
            };
            Ok((b, r))
        });
        match r {
            Ok((b, r)) => {
                eprintln!("{TAG} connected to {path} (node {})", b.node_id);
                self.state.connected = true;
                self.state.streaming = r.is_some();
                self.state.error = None;
                self.reader = r;
                self.bus = Some(b);
            }
            Err(e) => {
                self.state.error = Some(e.clone());
                eprintln!("{TAG} bus: {e}");
            }
        }
    }

    fn next_chunk(&mut self, wait: Duration) -> Option<bus::Record> {
        self.connect(true);
        let Some(r) = self.reader.as_mut() else {
            std::thread::sleep(wait);
            return None;
        };
        let rec = r.next(wait);
        self.state.skipped = r.skipped;
        rec
    }

    fn publish(&mut self, name: &str, lang: &str, h: &Heard) {
        self.connect(false);
        let Some(b) = self.bus.as_ref().filter(|b| !b.is_closed()) else {
            return;
        };
        self.seq += 1;
        let body = transcript_body(name, &b.node_id, self.seq, lang, h);
        match b.publish("transcript", name, "transcript.v1", h.start_ns, body) {
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

fn open_recognizer(o: &Opts, data_dir: &std::path::Path) -> Result<Box<dyn Recognizer>, String> {
    if o.simulate {
        return Ok(Box::new(Synthetic));
    }
    Parakeet::start(&o.python, &o.model_dir, o.threads, data_dir)
        .map(|p| Box::new(p) as Box<dyn Recognizer>)
}

/// A burst of tone between silences, as raw chunks, for `--simulate`.
fn synthetic_chunks() -> Vec<[u8; CHUNK_BYTES]> {
    let chunk = |amp: f64, n: usize| -> [u8; CHUNK_BYTES] {
        let mut out = [0u8; CHUNK_BYTES];
        for (i, s) in out.chunks_exact_mut(2).enumerate() {
            let v = (((n * 320 + i) as f64 * 0.2).sin() * amp) as i16;
            s.copy_from_slice(&v.to_le_bytes());
        }
        out
    };
    let mut v = Vec::new();
    for burst in 0..2 {
        v.extend((0..60).map(|n| chunk(20.0, n)));
        v.extend((0..60).map(|n| chunk(8000.0, n + burst)));
    }
    v.extend((0..50).map(|n| chunk(20.0, n)));
    v
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
        "{TAG} start (once={}, stream={AUDIO_KIND}/{}, name={}, simulate={}, bus={})",
        o.once,
        o.source,
        o.name,
        o.simulate,
        o.bus.as_deref().unwrap_or("none")
    );
    let data_dir = std::env::var("COGNITUM_COG_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join(format!("cog-asr-{}", std::process::id())));
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
    let mut link = Link::new(o.bus.clone(), &o.source);
    let mut engine: Option<Engine> = None;
    let mut open_err: Option<String> = None;
    let period = Duration::from_secs(if o.once { o.window } else { o.interval });
    let mut t0 = Instant::now();
    let mut feed: Vec<[u8; CHUNK_BYTES]> = if o.simulate {
        synthetic_chunks()
    } else if let Some(f) = &o.replay {
        match std::fs::read(f) {
            Ok(b) => alsa_pcm(&b)
                .chunks_exact(CHUNK_BYTES)
                .map(|c| c.try_into().unwrap())
                .collect(),
            Err(e) => {
                open_err = Some(format!("replay {f}: {e}"));
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    feed.reverse();
    let mut t_ns = 0u64;
    loop {
        if engine.is_none() && open_err.as_deref().is_none_or(|e| !e.starts_with("replay")) {
            match open_recognizer(&o, &data_dir) {
                Ok(r) => {
                    eprintln!("{TAG} recognizer: {}", r.label());
                    open_err = None;
                    engine = Some(Engine::new(o.params.clone(), r));
                }
                Err(e) => {
                    open_err = Some(e);
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
        let mut done = false;
        if let Some(en) = engine.as_mut() {
            let heard = if local {
                match feed.pop() {
                    Some(c) => {
                        t_ns += CHUNK_MS * 1_000_000;
                        en.feed(&c, t_ns)
                    }
                    None => {
                        done = true;
                        en.flush()
                    }
                }
            } else {
                match link.next_chunk(Duration::from_millis(100)) {
                    Some(r) if r.data.len() == CHUNK_BYTES => en.feed(&r.data, r.t_ns),
                    _ => None,
                }
            };
            if let Some(h) = heard {
                println!(
                    "{}",
                    json!({"heard": h.text, "audio_ms": h.audio_ms, "asr_ms": h.asr_ms})
                );
                link.publish(&o.name, &o.lang, &h);
            }
        } else {
            std::thread::sleep(Duration::from_millis(100));
        }
        if (local && done) || t0.elapsed() >= period {
            let stats = engine.as_ref().map(|e| e.stats.clone()).unwrap_or_default();
            let status = match (&engine, open_err.is_some()) {
                (None, _) => "no_source",
                _ if o.simulate => "simulate",
                _ if o.replay.is_some() => "replay",
                _ => "ok",
            };
            let label = engine
                .as_ref()
                .map_or_else(|| "none".to_string(), |e| e.rec.label());
            let ctx = Context {
                status,
                recognizer: &label,
                source: &o.source,
                bus: &link.state,
                speaking: engine.as_ref().is_some_and(|e| e.seg.speaking()),
                floor_db: engine.as_ref().map_or(-120.0, |e| e.seg.floor_db()),
                error: open_err.as_deref(),
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
            if o.once || (local && done) {
                return;
            }
            t0 = Instant::now();
        }
    }
}

/// Raw s16le from a WAV (header skipped to the `data` chunk) or a headerless file.
fn alsa_pcm(bytes: &[u8]) -> Vec<u8> {
    if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        let mut i = 12;
        while i + 8 <= bytes.len() {
            let n = u32::from_le_bytes([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]])
                as usize;
            if &bytes[i..i + 4] == b"data" {
                return bytes[i + 8..(i + 8 + n).min(bytes.len())].to_vec();
            }
            i += 8 + n + (n & 1);
        }
        return Vec::new();
    }
    bytes.to_vec()
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

    struct Fixed(&'static str);
    impl Recognizer for Fixed {
        fn label(&self) -> String {
            "fixed".into()
        }
        fn transcribe(&mut self, _: &[u8]) -> Result<(String, u64), String> {
            Ok((self.0.to_string(), 40))
        }
    }

    struct Broken;
    impl Recognizer for Broken {
        fn label(&self) -> String {
            "broken".into()
        }
        fn transcribe(&mut self, _: &[u8]) -> Result<(String, u64), String> {
            Err("worker died".into())
        }
    }

    fn run(rec: Box<dyn Recognizer>) -> (Vec<Heard>, Engine) {
        let mut en = Engine::new(Params::default(), rec);
        let mut out = Vec::new();
        for (i, c) in synthetic_chunks().iter().enumerate() {
            out.extend(en.feed(c, (i as u64 + 1) * 20_000_000));
        }
        out.extend(en.flush());
        (out, en)
    }

    #[test]
    fn options_bounded_and_defaulted() {
        let o = parse_opts(&s(&[
            "--once",
            "--window",
            "0",
            "--source",
            "Bad",
            "--name",
            "ears",
            "--lang",
            "EN",
            "--hang-ms",
            "5",
        ]));
        assert!(o.once);
        assert_eq!(
            (
                o.window,
                o.source.as_str(),
                o.name.as_str(),
                o.lang.as_str()
            ),
            (10, "mic0", "ears", "en")
        );
        assert_eq!(o.params.hang_ms, 600, "out of range falls back");
        let d = parse_opts(&s(&[]));
        assert_eq!(
            (d.api_bind.as_str(), d.threads, d.model_dir.as_str()),
            ("127.0.0.1:8067", 3, MODEL_DIR)
        );
    }

    #[test]
    fn two_bursts_are_two_transcripts_with_times() {
        let (heard, en) = run(Box::new(Fixed("hello there")));
        assert_eq!(heard.len(), 2, "{heard:?}");
        assert_eq!(heard[0].text, "hello there");
        assert!(heard[0].end_ns > heard[0].start_ns && heard[1].start_ns > heard[0].end_ns);
        assert_eq!(
            (en.stats.utterances, en.stats.heard, en.stats.empty),
            (2, 2, 0)
        );
        assert!(en.stats.audio_ms > 2000);
    }

    #[test]
    fn empty_text_is_not_a_transcript_and_a_worker_error_is_counted() {
        let (heard, en) = run(Box::new(Fixed("   ")));
        assert!(heard.is_empty());
        assert_eq!(
            (en.stats.utterances, en.stats.empty, en.stats.heard),
            (2, 2, 0)
        );
        let (heard, en) = run(Box::new(Broken));
        assert!(heard.is_empty());
        assert_eq!(
            (en.stats.errors, en.stats.last_error.as_deref()),
            (2, Some("worker died"))
        );
    }

    #[test]
    fn the_transcript_body_has_the_schema_shape() {
        let h = Heard {
            text: "hey arduino turn on the light".into(),
            start_ns: 5,
            end_ns: 9,
            audio_ms: 2000,
            asr_ms: 300,
        };
        let b = transcript_body("asr0", &"a".repeat(32), 3, "en", &h);
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
            (
                b["seq"].as_u64(),
                b["end_ns"].as_u64(),
                b["confidence"].as_f64()
            ),
            (Some(3), Some(9), Some(1.0))
        );
        let long = Heard {
            text: "x".repeat(5000),
            ..h
        };
        assert_eq!(
            transcript_body("a", "n", 1, "en", &long)["text"]
                .as_str()
                .unwrap()
                .len(),
            4096
        );
    }

    #[test]
    fn a_report_carries_counters_rtf_and_the_vector() {
        let (_, en) = run(Box::new(Fixed("hi")));
        let bus = BusState {
            connected: true,
            streaming: true,
            published: 2,
            ..Default::default()
        };
        let ctx = Context {
            status: "ok",
            recognizer: "fixed",
            source: "mic0",
            bus: &bus,
            speaking: false,
            floor_db: en.seg.floor_db(),
            error: None,
            export_url: &None,
        };
        let (r, v) = build_report(&en.stats, &ctx);
        assert_eq!(
            (
                r["transcripts"].as_u64(),
                r["utterances"].as_u64(),
                r["last_text"].as_str()
            ),
            (Some(2), Some(2), Some("hi"))
        );
        assert_eq!(v[0], 2.0);
        assert!(v[1] > 0.0 && v[1] < 0.1, "rtf {}", v[1]);
        let bad = Context {
            status: "no_source",
            error: Some("worker"),
            ..ctx
        };
        let (r, v) = build_report(&Stats::default(), &bad);
        assert_eq!((r["error"].as_str(), v), (Some("worker"), [-1.0; 8]));
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
