//! Cognitum Cog: audio-out
//!
//! Owns the node's ALSA playback device (WeftOS Ventuno cog plan 5.2) and plays the 16 kHz
//! mono s16le PCM stream it is sent: the cogbus stream `audio.pcm/<source>` (`pcm.s16le.v1`,
//! 20 ms chunks; ADR-122 section 3). A speech-synthesis cog writes that stream; this cog is
//! the only one that opens the speaker. The device is the one the host hands the owner in
//! `WEFTOS_DEVICES`. Reports chunks played, drops (gaps in the stream's seq), latency and
//! level once per interval.
//!
//! Usage:
//!   cog-audio-out                         # play audio.pcm/tts0 from $WEFTOS_COGBUS
//!   cog-audio-out --source tts1           # play another writer's stream
//!   cog-audio-out --once --simulate       # a one-second tone into a null sink, report, exit
//!   cog-audio-out --once --replay FILE    # play a WAV / raw s16le file through the device
//!   cog-audio-out --device plughw:0,1     # name the ALSA device by hand (bench run)

mod alsa;
mod bus;
mod export;
mod guide;
mod ingest;

use alsa::{Sink, CHUNK_BYTES, CHUNK_MS, RATE_HZ};
use export::{round, Shared, State};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TAG: &str = "[cog-audio-out]";
/// Per-cog id in the store vector tuple (29=audio-in).
pub(crate) const STORE_ID: u32 = 30;
const KIND: &str = "audio.pcm";
const SCHEMA: &str = "pcm.s16le.v1";

#[derive(Debug, Clone, PartialEq)]
struct Opts {
    once: bool,
    interval: u64,
    window: u64,
    source: String,
    name: String,
    device: Option<String>,
    card_name: Option<String>,
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
        window: num(a, "--window", 1, 1, 30),
        source: id_opt(a, "--source", "tts0"),
        name: id_opt(a, "--name", "spk0"),
        device: arg(a, "--device").map(String::from),
        card_name: arg(a, "--card-name").map(String::from),
        bus: arg(a, "--bus")
            .map(String::from)
            .or_else(|| std::env::var("WEFTOS_COGBUS").ok()),
        api_bind: arg(a, "--api-bind").unwrap_or("127.0.0.1:8064").to_string(),
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

/// Holds a synthetic source to real time without drifting.
struct Pacer(Option<Instant>);

impl Pacer {
    fn wait(&mut self) {
        let step = Duration::from_millis(CHUNK_MS as u64);
        let due = self.0.map_or_else(Instant::now, |t| t + step);
        let now = Instant::now();
        if due > now {
            std::thread::sleep(due - now);
        }
        self.0 = Some(due.max(now - step * 5));
    }
}

fn open_sink(o: &Opts) -> Result<Sink, String> {
    if o.simulate {
        return Ok(Sink::Null);
    }
    let dev = match &o.device {
        Some(d) => d.clone(),
        None => {
            let cards = std::fs::read_to_string("/proc/asound/cards").unwrap_or_default();
            alsa::pick_playback(&handed_devices(), &cards, o.card_name.as_deref())?
        }
    };
    Sink::aplay(&dev)
}

fn handed_devices() -> Vec<String> {
    std::env::var("WEFTOS_DEVICES")
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

/// Counters over one report window.
#[derive(Debug, Default, Clone)]
struct Window {
    chunks: u64,
    power: f64,
    peak_db: f64,
    drops: u64,
    write_errors: u64,
    latency_ms_sum: f64,
    latency_n: u64,
}

impl Window {
    fn add(&mut self, l: alsa::Level) {
        self.chunks += 1;
        self.power += 10f64.powf(l.rms_dbfs / 10.0);
        self.peak_db = if self.chunks == 1 {
            l.peak_dbfs
        } else {
            self.peak_db.max(l.peak_dbfs)
        };
    }

    fn rms_db(&self) -> f64 {
        if self.chunks == 0 {
            -120.0
        } else {
            (10.0 * (self.power / self.chunks as f64).log10()).max(-120.0)
        }
    }

    fn latency_ms(&self) -> Option<f64> {
        (self.latency_n > 0).then(|| self.latency_ms_sum / self.latency_n as f64)
    }
}

#[derive(Debug, Default, Clone)]
struct BusState {
    connected: bool,
    writer_open: bool,
    last_seq: u64,
    error: Option<String>,
}

/// What a report says besides the window's counters.
struct Context<'a> {
    status: &'a str,
    sink: &'a str,
    stream: &'a str,
    secs: f64,
    bus: &'a BusState,
    devices: &'a [String],
    error: Option<&'a str>,
    export_url: &'a Option<String>,
}

fn build_report(win: &Window, c: &Context) -> (Value, [f64; 8]) {
    let ok = matches!(c.status, "ok" | "simulate" | "replay");
    let rate = if c.secs > 0.0 {
        win.chunks as f64 / c.secs
    } else {
        0.0
    };
    let vector = if ok {
        [
            round(win.rms_db(), 2),
            round(win.peak_db, 2),
            round(rate, 2),
            win.drops as f64,
            win.write_errors as f64,
            c.bus.connected as u8 as f64,
            win.latency_ms().map_or(-1.0, |l| round(l, 1)),
            (win.chunks > 0) as u8 as f64,
        ]
    } else {
        [-1.0; 8]
    };
    let mut r = json!({
        "status": c.status,
        "provenance": match c.status { "simulate" => "SYNTHETIC", "replay" => "REPLAY", _ => "MEASURED" },
        "sink": c.sink,
        "stream": c.stream,
        "rate_hz": RATE_HZ, "channels": 1, "chunk_ms": CHUNK_MS, "chunk_bytes": CHUNK_BYTES,
        "chunks_played": win.chunks,
        "chunks_per_s": round(rate, 2),
        "dropped_chunks": win.drops,
        "write_errors": win.write_errors,
        "latency_ms": win.latency_ms().map(|l| round(l, 1)),
        "rms_dbfs": if win.chunks > 0 { json!(round(win.rms_db(), 2)) } else { Value::Null },
        "peak_dbfs": if win.chunks > 0 { json!(round(win.peak_db, 2)) } else { Value::Null },
        "idle": ok && win.chunks == 0,
        "devices": c.devices,
        "bus": {
            "connected": c.bus.connected, "stream_open": c.bus.writer_open,
            "last_seq": c.bus.last_seq, "error": c.bus.error,
        },
        "vector_slots": ["rms_dbfs", "peak_dbfs", "chunks_per_s", "dropped", "write_errors", "bus_connected", "latency_ms", "active"],
        "vector": vector,
        "export": c.export_url,
        "timestamp": unix_ms() / 1000,
    });
    if let Some(e) = c.error {
        r["error"] = json!(e);
    }
    (r, vector)
}

fn telemetry_items(win: &Window, bus: &BusState) -> Vec<Value> {
    let i = |n: &str, v: f64, u: &str| json!({"name": n, "value": v, "unit": u});
    vec![
        i("chunks.played", win.chunks as f64, ""),
        i("chunks.dropped", win.drops as f64, ""),
        i("write.errors", win.write_errors as f64, ""),
        i("latency", win.latency_ms().unwrap_or(0.0), "ms"),
        i("level.rms", win.rms_db(), "dBFS"),
        i("stream.seq", bus.last_seq as f64, ""),
        i("bus.connected", bus.connected as u8 as f64, ""),
    ]
}

/// The broker side: connects when it can and reads the stream.
struct Link {
    path: Option<String>,
    stream: String,
    bus: Option<bus::Bus>,
    reader: Option<bus::StreamReader>,
    state: BusState,
    seq_t: u64,
    last_try: Option<Instant>,
    /// Wait before the next connect attempt (longer after `busy`).
    retry: Duration,
}

impl Link {
    fn new(path: Option<String>, stream: String) -> Self {
        Self {
            path,
            stream,
            bus: None,
            reader: None,
            state: BusState::default(),
            seq_t: 0,
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
            self.state.connected = false;
            self.state.writer_open = false;
        }
        if self.reader.is_some() || self.last_try.is_some_and(|t| t.elapsed() < self.retry) {
            return;
        }
        self.last_try = Some(Instant::now());
        let token = std::env::var("WEFTOS_COGBUS_TOKEN").unwrap_or_default();
        let r = bus::Bus::connect(&path, &token).and_then(|b| {
            let r = b.stream_reader(&self.stream, SCHEMA)?;
            Ok((b, r))
        });
        match r {
            Ok((b, r)) => {
                eprintln!(
                    "{TAG} reading {} on {path} (node {})",
                    self.stream, b.node_id
                );
                self.state = BusState {
                    connected: true,
                    writer_open: true,
                    ..BusState::default()
                };
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

    /// The next record of the stream, waiting up to `wait`.
    fn next(&mut self, wait: Duration) -> Option<bus::Record> {
        self.connect();
        let Some(r) = self.reader.as_mut() else {
            std::thread::sleep(wait);
            return None;
        };
        let before = r.skipped;
        let rec = r.next(wait);
        if let Some(rec) = &rec {
            self.state.last_seq = rec.seq;
            self.state.error = None;
            let _ = before;
        }
        rec
    }

    fn skipped(&self) -> u64 {
        self.reader.as_ref().map_or(0, |r| r.skipped)
    }

    fn telemetry(&mut self, name: &str, win: &Window) {
        self.seq_t += 1;
        let Some(b) = self.bus.as_ref().filter(|b| !b.is_closed()) else {
            return;
        };
        let body = json!({
            "source_id": name, "node_id": b.node_id, "t_ns": bus::wall_ns(),
            "seq": self.seq_t, "items": telemetry_items(win, &self.state),
        });
        if let Err(e) = b.publish("telemetry", name, "telemetry.v1", bus::wall_ns(), body) {
            eprintln!("{TAG} telemetry: {e}");
        }
    }
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
    let stream = format!("{KIND}/{}", o.source);
    eprintln!(
        "{TAG} start (once={}, interval={}s, stream={stream}, simulate={}, bus={})",
        o.once,
        o.interval,
        o.simulate,
        o.bus.as_deref().unwrap_or("none")
    );
    let shared: Shared = Arc::new(Mutex::new(State::default()));
    let export_url = if o.once {
        None
    } else {
        match export::serve(&o.api_bind, shared.clone()) {
            Ok(addr) => Some(format!("http://{addr}/status")),
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
    let devices = handed_devices();
    let period = Duration::from_secs(if o.once { o.window } else { o.interval });
    let local = o.simulate || o.replay.is_some();
    let replay: Option<Vec<u8>> = o.replay.as_ref().map(|f| match std::fs::read(f) {
        Ok(b) => alsa::pcm_from_file(&b),
        Err(e) => {
            eprintln!("{TAG} replay {f}: {e}");
            Vec::new()
        }
    });
    let mut link = Link::new(if local { None } else { o.bus.clone() }, stream.clone());

    let mut sink: Option<Sink> = None;
    let mut open_err: Option<String> = None;
    let mut win = Window::default();
    let mut t0 = Instant::now();
    let mut skipped_seen = 0u64;
    let mut pace = Pacer(None);
    let (mut n, mut pos) = (0u64, 0usize);
    loop {
        if sink.is_none() {
            match open_sink(&o) {
                Ok(s) => {
                    eprintln!("{TAG} playing to {}", s.label());
                    open_err = None;
                    sink = Some(s);
                }
                Err(e) => {
                    open_err = Some(e);
                    std::thread::sleep(period.min(Duration::from_secs(1)));
                }
            }
        }
        if let Some(s) = sink.as_mut() {
            let mut play =
                |chunk: &[u8], win: &mut Window, latency_ms: Option<f64>| match s.write(chunk) {
                    Ok(()) => {
                        win.add(alsa::level(chunk));
                        if let Some(l) = latency_ms {
                            win.latency_ms_sum += l;
                            win.latency_n += 1;
                        }
                        true
                    }
                    Err(e) => {
                        win.write_errors += 1;
                        open_err = Some(e);
                        false
                    }
                };
            let ok = if let Some(data) = &replay {
                if pos + CHUNK_BYTES <= data.len() {
                    let c = &data[pos..pos + CHUNK_BYTES];
                    pos += CHUNK_BYTES;
                    if o.simulate {
                        pace.wait()
                    }
                    play(c, &mut win, None)
                } else {
                    t0 = Instant::now() - period;
                    true
                }
            } else if o.simulate {
                pace.wait();
                n += 1;
                play(&alsa::tone_chunk(n), &mut win, None)
            } else {
                match link.next(Duration::from_millis(100)) {
                    Some(rec) => {
                        let lat = bus::wall_ns().saturating_sub(rec.w_ns) as f64 / 1e6;
                        let c = rec.data.as_slice();
                        if c.len() == CHUNK_BYTES {
                            play(c, &mut win, Some(lat))
                        } else {
                            true
                        }
                    }
                    None => true,
                }
            };
            if !ok {
                sink = None;
                // Do not respawn a failing player in a tight loop.
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        let skipped = link.skipped();
        win.drops += skipped.saturating_sub(skipped_seen);
        skipped_seen = skipped;
        if t0.elapsed() >= period {
            let status = match (&sink, open_err.is_some()) {
                (_, true) if win.chunks == 0 => "no_source",
                _ if o.simulate => "simulate",
                _ if o.replay.is_some() => "replay",
                _ => "ok",
            };
            let label = sink
                .as_ref()
                .map_or_else(|| "none".to_string(), Sink::label);
            let mut err = open_err.clone();
            if err.is_none() && !local && !link.state.connected {
                err = link
                    .state
                    .error
                    .clone()
                    .or_else(|| Some("no broker: WEFTOS_COGBUS is not set".into()));
            }
            let ctx = Context {
                status: if status == "ok"
                    && err.is_some()
                    && win.chunks == 0
                    && !link.state.connected
                {
                    "no_source"
                } else {
                    status
                },
                sink: &label,
                stream: &stream,
                secs: t0.elapsed().as_secs_f64(),
                bus: &link.state,
                devices: &devices,
                error: err.as_deref(),
                export_url: &export_url,
            };
            let (report, vector) = build_report(&win, &ctx);
            println!("{report}");
            if let Ok(mut st) = shared.lock() {
                st.push(report.clone());
            }
            link.telemetry(&o.name, &win);
            if report["status"] == "ok" {
                if let Err(e) = ingest::store_to_seed(&target, token.as_deref(), &vector) {
                    eprintln!("{TAG} store error: {e}");
                }
            } else if report["status"] == "no_source" {
                eprintln!("{TAG} no source: {}", report["error"]);
            }
            if o.once {
                if let Some(s) = sink.take() {
                    s.finish();
                }
                return;
            }
            win = Window::default();
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
            "--interval",
            "0",
            "--source",
            "Bad Name",
            "--name",
            "spk1",
        ]));
        assert!(o.once);
        assert_eq!(o.interval, 5);
        assert_eq!(o.source, "tts0", "a bad source id falls back");
        assert_eq!(o.name, "spk1");
        let d = parse_opts(&s(&[]));
        assert_eq!(
            (d.api_bind.as_str(), d.window, d.source.as_str()),
            ("127.0.0.1:8064", 1, "tts0")
        );
    }

    #[test]
    fn a_window_reports_level_latency_drops_and_the_vector() {
        let mut w = Window::default();
        for n in 0..50 {
            w.add(alsa::level(&alsa::tone_chunk(n)));
            w.latency_ms_sum += 4.0;
            w.latency_n += 1;
        }
        w.drops = 2;
        let bus = BusState {
            connected: true,
            writer_open: true,
            last_seq: 52,
            error: None,
        };
        let ctx = Context {
            status: "ok",
            sink: "alsa:plughw:0,1",
            stream: "audio.pcm/tts0",
            secs: 1.0,
            bus: &bus,
            devices: &[],
            error: None,
            export_url: &None,
        };
        let (r, v) = build_report(&w, &ctx);
        assert_eq!(
            (r["status"].as_str(), r["provenance"].as_str()),
            (Some("ok"), Some("MEASURED"))
        );
        assert_eq!(
            (r["chunks_played"].as_u64(), r["dropped_chunks"].as_u64()),
            (Some(50), Some(2))
        );
        assert_eq!(r["latency_ms"], 4.0);
        assert_eq!((v[2], v[3], v[5], v[6], v[7]), (50.0, 2.0, 1.0, 4.0, 1.0));
        assert_eq!(r["idle"], false);
    }

    #[test]
    fn an_idle_sink_is_ok_but_marked_idle_and_no_source_has_an_empty_vector() {
        let bus = BusState {
            connected: true,
            writer_open: true,
            ..Default::default()
        };
        let ctx = Context {
            status: "ok",
            sink: "null",
            stream: "audio.pcm/tts0",
            secs: 5.0,
            bus: &bus,
            devices: &[],
            error: None,
            export_url: &None,
        };
        let (r, _) = build_report(&Window::default(), &ctx);
        assert_eq!(
            (r["idle"].clone(), r["rms_dbfs"].clone()),
            (json!(true), Value::Null)
        );
        let ctx = Context {
            status: "no_source",
            error: Some("no playback node"),
            ..ctx
        };
        let (r, v) = build_report(&Window::default(), &ctx);
        assert_eq!(
            (r["error"].as_str(), v),
            (Some("no playback node"), [-1.0; 8])
        );
    }

    #[test]
    fn telemetry_names_are_schema_valid() {
        for it in telemetry_items(&Window::default(), &BusState::default()) {
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
