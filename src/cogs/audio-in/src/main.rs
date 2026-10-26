//! Cognitum Cog: audio-in
//!
//! Owns the node's ALSA capture device (WeftOS Ventuno cog plan 5.2) and publishes 16 kHz
//! mono s16le PCM in 20 ms chunks (640 bytes) as the cogbus stream `audio.pcm/<source>`
//! (`pcm.s16le.v1`; ADR-122 section 3). Every other cog reads the microphone by subscribing
//! to that stream. The device is the one the host hands the owner in `WEFTOS_DEVICES`
//! (`[[devices]]` in cog.toml); nothing else is opened. Reports the level, drops and bus
//! state once per interval and posts an 8-float vector to the store.
//!
//! Usage:
//!   cog-audio-in                          # capture continuously, stream on $WEFTOS_COGBUS
//!   cog-audio-in --once                   # one <window>-second capture, report, exit
//!   cog-audio-in --once --simulate        # synthetic tone, nothing posted
//!   cog-audio-in --once --replay FILE     # play a WAV / raw s16le file through the same path
//!   cog-audio-in --device plughw:1,0      # name the ALSA device by hand (bench run)

mod alsa;
mod bus;
mod export;
mod guide;
mod ingest;

use alsa::{Arecord, CHUNK_BYTES, CHUNK_MS, RATE_HZ};
use export::{round, Shared, State};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TAG: &str = "[cog-audio-in]";
/// Per-cog id in the store vector tuple (28=board-telemetry).
pub(crate) const STORE_ID: u32 = 29;
const KIND: &str = "audio.pcm";
/// Capture level set at start: the U20CAM's microphone clips speech at 1 m at 100% (89 of 350
/// chunks in the calibration run) and peaks at -7 dBFS at 50%.
const DEFAULT_MIXER_PCT: u8 = 50;
/// Software gain applied to every chunk before it is measured and published.
const DEFAULT_GAIN_DB: f64 = 0.0;
const SCHEMA: &str = "pcm.s16le.v1";

#[derive(Debug, Clone, PartialEq)]
struct Opts {
    once: bool,
    interval: u64,
    window: u64,
    source: String,
    device: Option<String>,
    card_name: Option<String>,
    bus: Option<String>,
    api_bind: String,
    mixer_gain: Option<u8>,
    mixer_control: String,
    gain_db: f64,
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

fn parse_opts(a: &[String]) -> Opts {
    let source = match arg(a, "--source") {
        Some(s) if valid_source(s) => s.to_string(),
        Some(_) => {
            eprintln!("{TAG} --source must match [a-z0-9][a-z0-9._-]*, using mic0");
            "mic0".into()
        }
        None => "mic0".into(),
    };
    Opts {
        once: a.iter().any(|x| x == "--once"),
        interval: num(a, "--interval", 5, 1, 300),
        window: num(a, "--window", 1, 1, 30),
        source,
        device: arg(a, "--device").map(String::from),
        card_name: arg(a, "--card-name").map(String::from),
        bus: arg(a, "--bus")
            .map(String::from)
            .or_else(|| std::env::var("WEFTOS_COGBUS").ok()),
        api_bind: arg(a, "--api-bind").unwrap_or("127.0.0.1:8063").to_string(),
        mixer_gain: match arg(a, "--mixer-gain") {
            Some("off") => None,
            Some(v) => v
                .parse::<u8>()
                .ok()
                .filter(|p| *p <= 100)
                .or(Some(DEFAULT_MIXER_PCT)),
            None => Some(DEFAULT_MIXER_PCT),
        },
        mixer_control: arg(a, "--mixer-control")
            .filter(|c| {
                !c.is_empty()
                    && c.len() <= 32
                    && c.chars()
                        .all(|ch| ch.is_alphanumeric() || " _-".contains(ch))
            })
            .unwrap_or("Mic")
            .to_string(),
        gain_db: num(a, "--gain-db", DEFAULT_GAIN_DB, -40.0, 40.0),
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

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Where chunks come from.
enum Source {
    Alsa(Arecord, String),
    Synth(u64, Pacer),
    Replay(Vec<u8>, usize, Pacer),
}

/// Holds a synthetic or replayed source to real time without drifting: each chunk is due
/// one chunk after the last, whatever the sleeps overshoot.
struct Pacer(Option<Instant>);

impl Pacer {
    fn new() -> Self {
        Self(None)
    }

    fn wait(&mut self) {
        let due = self
            .0
            .map_or_else(Instant::now, |t| t + Duration::from_millis(CHUNK_MS as u64));
        let now = Instant::now();
        if due > now {
            std::thread::sleep(due - now);
        }
        self.0 = Some(due.max(now - Duration::from_millis(CHUNK_MS as u64 * 5)));
    }
}

impl Source {
    fn label(&self) -> String {
        match self {
            Source::Alsa(_, d) => format!("alsa:{d}"),
            Source::Synth(..) => "simulated".into(),
            Source::Replay(..) => "replay".into(),
        }
    }

    /// The next chunk, paced to real time for the synthetic and replayed sources.
    fn next(&mut self, buf: &mut [u8; CHUNK_BYTES]) -> Result<bool, String> {
        match self {
            Source::Alsa(a, _) => a.read_chunk(buf),
            Source::Synth(n, pace) => {
                pace.wait();
                *buf = alsa::synth_chunk(*n);
                *n += 1;
                Ok(true)
            }
            Source::Replay(data, pos, pace) => {
                if *pos + CHUNK_BYTES > data.len() {
                    return Ok(false);
                }
                pace.wait();
                buf.copy_from_slice(&data[*pos..*pos + CHUNK_BYTES]);
                *pos += CHUNK_BYTES;
                Ok(true)
            }
        }
    }
}

fn open_source(o: &Opts) -> Result<(Source, Option<String>), String> {
    if o.simulate {
        return Ok((Source::Synth(0, Pacer::new()), None));
    }
    if let Some(f) = &o.replay {
        let bytes = std::fs::read(f).map_err(|e| format!("replay {f}: {e}"))?;
        return Ok((
            Source::Replay(alsa::pcm_from_file(&bytes), 0, Pacer::new()),
            None,
        ));
    }
    let dev = match &o.device {
        Some(d) => d.clone(),
        None => {
            let handed: Vec<String> = std::env::var("WEFTOS_DEVICES")
                .unwrap_or_default()
                .split(':')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
            let cards = std::fs::read_to_string("/proc/asound/cards").unwrap_or_default();
            alsa::pick_capture(&handed, &cards, o.card_name.as_deref())?
        }
    };
    // The capture level is set before the stream starts and stays set: it is the card's
    // mixer, not this process's.
    let note = o.mixer_gain.map(|p| match alsa::card_of(&dev) {
        Some(card) => alsa::set_mixer(card, &o.mixer_control, p)
            .map_or_else(|e| format!("not set: {e}"), |s| format!("set to {p}%: {s}")),
        None => format!("not set: no card number in {dev:?}"),
    });
    Arecord::start(&dev).map(|a| (Source::Alsa(a, dev), note))
}

/// Counters over one report window.
#[derive(Debug, Default, Clone)]
struct Window {
    chunks: u64,
    power: f64,
    peak_db: f64,
    clipped: u64,
    clip_chunks: u64,
    short: u64,
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
        self.clipped += l.clipped as u64;
        self.clip_chunks += (l.clipped > 0) as u64;
    }

    fn rms_db(&self) -> f64 {
        if self.chunks == 0 {
            -120.0
        } else {
            (10.0 * (self.power / self.chunks as f64).log10()).max(-120.0)
        }
    }
}

/// What the stream writer has done so far.
#[derive(Debug, Default, Clone)]
struct BusState {
    connected: bool,
    seq: u64,
    write_errors: u64,
    topic: Option<String>,
    error: Option<String>,
}

/// What a report says besides the window's counters.
struct Context<'a> {
    gain_db: f64,
    mixer_gain: Option<u8>,
    mixer_note: Option<&'a str>,
    status: &'a str,
    source: &'a str,
    secs: f64,
    bus: &'a BusState,
    devices: &'a [String],
    error: Option<&'a str>,
    export_url: &'a Option<String>,
}

fn build_report(win: &Window, c: &Context) -> (Value, [f64; 8]) {
    let Context {
        gain_db: _,
        mixer_gain: _,
        mixer_note: _,
        status,
        source,
        secs,
        bus,
        devices,
        error,
        export_url,
    } = c;
    let (status, source, secs) = (*status, *source, *secs);
    let ok = matches!(status, "ok" | "simulate" | "replay");
    let rate = if secs > 0.0 {
        win.chunks as f64 / secs
    } else {
        0.0
    };
    let expected = 1000.0 / CHUNK_MS as f64;
    let vector = if ok {
        [
            round(win.rms_db(), 2),
            round(win.peak_db, 2),
            win.clipped as f64,
            round(rate, 2),
            win.short as f64,
            bus.connected as u8 as f64,
            bus.seq as f64,
            round((rate / expected).min(2.0), 3),
        ]
    } else {
        [-1.0; 8]
    };
    let mut r = json!({
        "status": status,
        "provenance": if status == "simulate" { "SYNTHETIC" } else if status == "replay" { "REPLAY" } else { "MEASURED" },
        "source": source,
        "rate_hz": RATE_HZ, "channels": 1, "chunk_ms": CHUNK_MS, "chunk_bytes": CHUNK_BYTES,
        "chunks": win.chunks,
        "chunks_per_s": round(rate, 2),
        "short_reads": win.short,
        "rms_dbfs": if ok { json!(round(win.rms_db(), 2)) } else { Value::Null },
        "peak_dbfs": if ok { json!(round(win.peak_db, 2)) } else { Value::Null },
        "clipped_samples": win.clipped,
        "clipped_chunks": win.clip_chunks,
        // Clipping in more than 1% of chunks distorts speech: lower the mixer or the gain.
        "clipping": win.clip_chunks * 100 > win.chunks,
        "gain_db": c.gain_db,
        "mixer_gain_pct": c.mixer_gain,
        "mixer": c.mixer_note,
        "devices": devices,
        "bus": {
            "connected": bus.connected, "topic": bus.topic, "seq": bus.seq,
            "write_errors": bus.write_errors, "error": bus.error,
        },
        "vector_slots": ["rms_dbfs", "peak_dbfs", "clipped", "chunks_per_s", "short_reads", "bus_connected", "stream_seq", "rate_ratio"],
        "vector": vector,
        "export": export_url,
        "timestamp": unix_ms() / 1000,
    });
    if let Some(e) = *error {
        r["error"] = json!(e);
    }
    (r, vector)
}

fn telemetry_items(win: &Window, bus: &BusState) -> Vec<Value> {
    let i = |n: &str, v: f64, u: &str| json!({"name": n, "value": v, "unit": u});
    vec![
        i("chunks.total", win.chunks as f64, ""),
        i("chunks.short", win.short as f64, ""),
        i("level.rms", win.rms_db(), "dBFS"),
        i("level.peak", win.peak_db, "dBFS"),
        i("level.clipped", win.clipped as f64, ""),
        i("level.clip_chunks", win.clip_chunks as f64, ""),
        i("stream.seq", bus.seq as f64, ""),
        i("bus.connected", bus.connected as u8 as f64, ""),
    ]
}

/// The broker side: connects when it can, opens the stream, writes chunks.
struct Link {
    path: Option<String>,
    source: String,
    bus: Option<bus::Bus>,
    writer: Option<bus::RingWriter>,
    state: BusState,
    seq_t: u64,
    last_try: Option<Instant>,
    /// Wait before the next connect attempt (longer after `busy`).
    retry: Duration,
}

impl Link {
    fn new(path: Option<String>, source: &str) -> Self {
        Self {
            path,
            source: source.into(),
            bus: None,
            writer: None,
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
            self.writer = None;
            self.state.connected = false;
        }
        if self.writer.is_some() || self.last_try.is_some_and(|t| t.elapsed() < self.retry) {
            return;
        }
        self.last_try = Some(Instant::now());
        let token = std::env::var("WEFTOS_COGBUS_TOKEN").unwrap_or_default();
        let r = bus::Bus::connect(&path, &token).and_then(|b| {
            let w = b.stream_writer(KIND, &self.source, SCHEMA, CHUNK_BYTES as u64, None)?;
            Ok((b, w))
        });
        match r {
            Ok((b, w)) => {
                eprintln!(
                    "{TAG} streaming {KIND}/{} on {path} (node {})",
                    self.source, b.node_id
                );
                self.state = BusState {
                    connected: true,
                    topic: Some(format!("{KIND}/{}", self.source)),
                    seq: w.last_seq(),
                    ..BusState::default()
                };
                self.bus = Some(b);
                self.writer = Some(w);
            }
            Err(e) => {
                self.retry = bus::retry_after(&e);
                self.state.error = Some(e.clone());
                eprintln!("{TAG} bus: {e}");
            }
        }
    }

    fn write(&mut self, chunk: &[u8]) {
        self.connect();
        self.seq_t += 1;
        let t = bus::wall_ns();
        if let Some(w) = self.writer.as_mut() {
            match w.write(t, chunk) {
                Ok(s) => self.state.seq = s,
                Err(e) => {
                    self.state.write_errors += 1;
                    self.state.error = Some(e);
                }
            }
        }
    }

    fn telemetry(&self, win: &Window) {
        let Some(b) = self.bus.as_ref().filter(|b| !b.is_closed()) else {
            return;
        };
        let body = json!({
            "source_id": self.source, "node_id": b.node_id, "t_ns": bus::wall_ns(),
            "seq": self.seq_t, "items": telemetry_items(win, &self.state),
        });
        if let Err(e) = b.publish(
            "telemetry",
            &self.source,
            "telemetry.v1",
            bus::wall_ns(),
            body,
        ) {
            eprintln!("{TAG} telemetry: {e}");
        }
    }
}

fn handed_devices() -> Vec<String> {
    std::env::var("WEFTOS_DEVICES")
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(String::from)
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
        "{TAG} start (once={}, interval={}s, source={}, simulate={}, bus={})",
        o.once,
        o.interval,
        o.source,
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
    let mut link = Link::new(
        if o.simulate && o.once {
            None
        } else {
            o.bus.clone()
        },
        &o.source,
    );

    let mut source: Option<Source> = None;
    let mut mixer_note: Option<String> = None;
    let mut open_err: Option<String> = None;
    let mut win = Window::default();
    let mut t0 = Instant::now();
    let mut last_raw = Instant::now() - Duration::from_secs(1);
    let mut buf = [0u8; CHUNK_BYTES];
    loop {
        if source.is_none() {
            match open_source(&o) {
                Ok((s, note)) => {
                    eprintln!(
                        "{TAG} capturing from {}{}",
                        s.label(),
                        note.as_deref()
                            .map(|n| format!(" (mixer {n})"))
                            .unwrap_or_default()
                    );
                    mixer_note = note;
                    source = Some(s);
                }
                Err(e) => {
                    open_err = Some(e);
                    std::thread::sleep(period.min(Duration::from_secs(1)));
                }
            }
        }
        if let Some(s) = source.as_mut() {
            match s.next(&mut buf) {
                Ok(true) => {
                    open_err = None;
                    alsa::apply_gain(&mut buf, o.gain_db);
                    win.add(alsa::level(&buf));
                    link.write(&buf);
                    if last_raw.elapsed() >= Duration::from_millis(250) {
                        last_raw = Instant::now();
                        if let Ok(mut st) = shared.lock() {
                            st.raw = Some(json!({
                                "rate_hz": RATE_HZ, "channels": 1, "format": "s16le",
                                "t_ms": unix_ms(), "pcm_b64": b64(&buf),
                            }));
                        }
                    }
                }
                Ok(false) => {
                    if o.replay.is_some() {
                        if win.chunks == 0 {
                            open_err = Some("the replay file holds no complete chunk".into());
                        }
                        t0 = Instant::now() - period;
                    } else {
                        open_err = Some("the capture ended".into());
                        source = None;
                        // Do not respawn a failing capture in a tight loop.
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
                Err(e) => {
                    win.short += 1;
                    open_err = Some(e);
                    source = None;
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
        if t0.elapsed() >= period {
            let status = match (&source, open_err.is_some()) {
                (_, true) if win.chunks == 0 => "no_source",
                _ if o.simulate => "simulate",
                _ if o.replay.is_some() => "replay",
                _ => "ok",
            };
            let label = source
                .as_ref()
                .map_or_else(|| "none".to_string(), Source::label);
            let ctx = Context {
                gain_db: o.gain_db,
                mixer_gain: o.mixer_gain,
                mixer_note: mixer_note.as_deref(),
                status,
                source: &label,
                secs: t0.elapsed().as_secs_f64(),
                bus: &link.state,
                devices: &devices,
                error: open_err.as_deref(),
                export_url: &export_url,
            };
            let (report, vector) = build_report(&win, &ctx);
            println!("{report}");
            if let Ok(mut st) = shared.lock() {
                st.push(report.clone());
            }
            link.telemetry(&win);
            if report["status"] == "ok" {
                if let Err(e) = ingest::store_to_seed(&target, token.as_deref(), &vector) {
                    eprintln!("{TAG} store error: {e}");
                }
            } else if report["status"] == "no_source" {
                eprintln!("{TAG} no source: {}", report["error"]);
            }
            if o.once {
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
            "99999",
            "--source",
            "Bad Name",
            "--device",
            "hw:1,0",
        ]));
        assert!(o.once);
        assert_eq!(o.interval, 5, "out of range falls back to the default");
        assert_eq!(o.source, "mic0", "a bad source id falls back");
        assert_eq!(o.device.as_deref(), Some("hw:1,0"));
        assert_eq!(parse_opts(&s(&["--mixer-gain", "off"])).mixer_gain, None);
        assert_eq!(parse_opts(&s(&["--mixer-gain", "30"])).mixer_gain, Some(30));
        assert_eq!(
            parse_opts(&s(&["--mixer-gain", "300"])).mixer_gain,
            Some(50),
            "out of range: the default"
        );
        assert_eq!(parse_opts(&s(&[])).mixer_gain, Some(50));
        assert_eq!(parse_opts(&s(&["--gain-db", "-6"])).gain_db, -6.0);
        assert_eq!(parse_opts(&s(&["--gain-db", "99"])).gain_db, 0.0);
        let d = parse_opts(&s(&[]));
        assert_eq!(
            (d.api_bind.as_str(), d.window, d.source.as_str()),
            ("127.0.0.1:8063", 1, "mic0")
        );
        assert!(!d.simulate && d.replay.is_none());
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foo"), "Zm9v");
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_window_reports_level_rate_and_the_vector() {
        let mut w = Window::default();
        for n in 0..50 {
            w.add(alsa::level(&alsa::synth_chunk(n)));
        }
        let bus = BusState {
            connected: true,
            seq: 50,
            topic: Some("audio.pcm/mic0".into()),
            ..Default::default()
        };
        let ctx = Context {
            gain_db: 0.0,
            mixer_gain: None,
            mixer_note: None,
            status: "simulate",
            source: "simulated",
            secs: 1.0,
            bus: &bus,
            devices: &[],
            error: None,
            export_url: &None,
        };
        let (r, v) = build_report(&w, &ctx);
        assert_eq!(r["status"], "simulate");
        assert_eq!(r["provenance"], "SYNTHETIC");
        assert_eq!(
            (
                r["chunks"].as_u64(),
                r["chunk_bytes"].as_u64(),
                r["rate_hz"].as_u64()
            ),
            (Some(50), Some(640), Some(16000))
        );
        assert_eq!(r["chunks_per_s"], 50.0);
        assert_eq!((v[3], v[5], v[6], v[7]), (50.0, 1.0, 50.0, 1.0));
        assert!(v[0] < -5.0 && v[0] > -30.0, "{v:?}");
        assert_eq!(r["vector"].as_array().unwrap().len(), 8);
        assert_eq!(r["bus"]["topic"], "audio.pcm/mic0");
    }

    #[test]
    fn no_source_reports_the_reason_and_an_empty_vector() {
        let ctx = Context {
            gain_db: 0.0,
            mixer_gain: None,
            mixer_note: None,
            status: "no_source",
            source: "none",
            secs: 1.0,
            bus: &BusState::default(),
            devices: &[],
            error: Some("no capture node"),
            export_url: &None,
        };
        let (r, v) = build_report(&Window::default(), &ctx);
        assert_eq!(r["status"], "no_source");
        assert_eq!(r["error"], "no capture node");
        assert!(r["rms_dbfs"].is_null());
        assert_eq!(v, [-1.0; 8]);
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

    #[test]
    fn a_replay_file_is_chunked_into_whole_20_ms_chunks() {
        let dir = std::env::temp_dir().join(format!("audio-in-replay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("t.raw");
        std::fs::write(&f, vec![0u8; CHUNK_BYTES * 3 + 100]).unwrap();
        let o = Opts {
            replay: Some(f.to_string_lossy().into()),
            ..parse_opts(&s(&[]))
        };
        let mut src = open_source(&o).unwrap();
        let mut buf = [0u8; CHUNK_BYTES];
        let mut n = 0;
        while src.0.next(&mut buf).unwrap() {
            n += 1;
        }
        assert_eq!(n, 3, "the partial tail is not a chunk");
        std::fs::remove_dir_all(&dir).ok();
    }
}
