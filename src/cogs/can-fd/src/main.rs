//! Cognitum Cog: can-fd
//!
//! Owns the node's CAN interface (WeftOS Ventuno cog plan 5.1) and publishes what is on the bus:
//! every data frame, classic or CAN FD, as a `can.frame.v1` record in the cogbus stream
//! `can.frame/<source>` (default the interface name), with counters for error frames, bus-off,
//! the controller's error counters and the bus load. Receive only: it reads a raw SocketCAN
//! socket and never writes to it, and it never configures the interface (bringing one up, and
//! its bit rate, are the operator's; the cog reports what the kernel says and whether the bit
//! rate is the one configured). Transmitting on a bus is WeftOS ADR-123, which is not accepted.
//!
//! Usage:
//!   cog-can-fd --iface can0 --bitrate 500000     # stream can.frame/can0 on $WEFTOS_COGBUS
//!   cog-can-fd --once --iface vcan0              # listen for --window seconds, report, exit
//!   cog-can-fd --once --simulate                 # synthetic traffic, no interface
//!   cog-can-fd --once --replay FILE              # a `candump -L` log through the same path

mod bus;
mod export;
mod frame;
mod guide;
mod iface;
mod ingest;
mod socket;

use export::{round, Shared, State};
use frame::{Frame, Rx};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TAG: &str = "[cog-can-fd]";
/// Per-cog id in the store vector tuple (37=vision-detect).
pub(crate) const STORE_ID: u32 = 38;
const KIND: &str = "can.frame";
const SCHEMA: &str = "can.frame.v1";
const MAX_RECORD: u64 = 512;

#[derive(Debug, Clone, PartialEq)]
struct Opts {
    once: bool,
    interval: u64,
    window: u64,
    iface: Option<String>,
    bitrate: u64,
    source: Option<String>,
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

fn valid_iface(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 15
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
}

fn parse_opts(a: &[String]) -> Opts {
    Opts {
        once: a.iter().any(|x| x == "--once"),
        interval: num(a, "--interval", 5, 1, 300),
        window: num(a, "--window", 10, 1, 600),
        iface: arg(a, "--iface")
            .filter(|i| valid_iface(i))
            .map(String::from),
        bitrate: num(a, "--bitrate", 500_000, 10_000, 1_000_000),
        source: arg(a, "--source")
            .filter(|s| valid_source(s))
            .map(String::from),
        bus: arg(a, "--bus")
            .map(String::from)
            .or_else(|| std::env::var("WEFTOS_COGBUS").ok()),
        api_bind: arg(a, "--api-bind").unwrap_or("127.0.0.1:8072").to_string(),
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

/// The interface to listen on: `--iface`, else the first interface name the host handed the owner
/// (`WEFTOS_DEVICES` entries that do not start with `/`).
fn pick_iface(o: &Opts, handed: &[String]) -> Result<String, String> {
    if let Some(i) = &o.iface {
        return Ok(i.clone());
    }
    handed
        .iter()
        .find(|d| !d.starts_with('/') && valid_iface(d))
        .cloned()
        .ok_or_else(|| {
            "no CAN interface in WEFTOS_DEVICES (the host hands the owner net:can*) and no --iface"
                .to_string()
        })
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
    frames: u64,
    fd_frames: u64,
    ext_frames: u64,
    remote: u64,
    bits: u64,
    errors: BTreeMap<&'static str, u64>,
    error_frames: u64,
    bus_off: u64,
    read_errors: u64,
    top: BTreeMap<u32, u64>,
}

impl Window {
    fn count(&mut self, rx: &Rx) {
        match rx {
            Rx::Data(f) => {
                self.frames += 1;
                self.fd_frames += f.fd as u64;
                self.ext_frames += f.ext as u64;
                self.bits += frame::wire_bits(f);
                *self.top.entry(f.id).or_default() += 1;
            }
            Rx::Remote => self.remote += 1,
            Rx::Error(e) => {
                self.error_frames += 1;
                for n in frame::class_names(e.class) {
                    *self.errors.entry(n).or_default() += 1;
                }
                self.bus_off += (e.class & frame::ERR_BUSOFF != 0) as u64;
            }
        }
    }

    fn top_ids(&self, n: usize) -> Vec<Value> {
        let mut v: Vec<(&u32, &u64)> = self.top.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        v.into_iter()
            .take(n)
            .map(|(id, c)| json!({"id": format!("{id:#x}"), "frames": c}))
            .collect()
    }
}

/// Bus load in percent: bits on the wire over what the bit rate carries in `secs`.
fn load_pct(bits: u64, bitrate: u64, secs: f64) -> f64 {
    if bitrate == 0 || secs <= 0.0 {
        return 0.0;
    }
    (bits as f64 * 1.2 / (bitrate as f64 * secs) * 100.0).min(100.0)
}

#[derive(Debug, Default, Clone)]
struct BusState {
    connected: bool,
    seq: u64,
    write_errors: u64,
    topic: Option<String>,
    error: Option<String>,
}

struct Context<'a> {
    status: &'a str,
    iface: &'a str,
    source: &'a str,
    info: Option<&'a iface::Info>,
    configured_bitrate: u64,
    secs: f64,
    rx_dropped: u64,
    bus: &'a BusState,
    error: Option<&'a str>,
    export_url: &'a Option<String>,
}

fn build_report(w: &Window, c: &Context) -> (Value, [f64; 8]) {
    let ok = c.status != "no_source";
    let rate = if c.secs > 0.0 {
        w.frames as f64 / c.secs
    } else {
        0.0
    };
    let bitrate = c
        .info
        .and_then(|i| i.bitrate)
        .unwrap_or(c.configured_bitrate);
    let load = load_pct(w.bits, bitrate, c.secs);
    let bitrate_ok = c
        .info
        .and_then(|i| i.bitrate)
        .map(|b| b == c.configured_bitrate);
    let vector = if ok {
        [
            round(rate, 2),
            round(load, 2),
            w.error_frames as f64,
            w.bus_off as f64,
            c.info.map_or(0.0, |i| i.rx_errors as f64),
            c.info.map_or(0.0, |i| i.tx_errors as f64),
            c.rx_dropped as f64,
            bitrate_ok.map_or(-1.0, |b| b as u8 as f64),
        ]
    } else {
        [-1.0; 8]
    };
    let mut r = json!({
        "status": c.status,
        "provenance": match c.status { "simulate" => "SYNTHETIC", "replay" => "REPLAY", _ => "MEASURED" },
        "iface": c.iface,
        "stream": format!("{KIND}/{}", c.source),
        "frames": w.frames, "frames_per_s": round(rate, 2),
        "fd_frames": w.fd_frames, "extended_frames": w.ext_frames, "remote_frames_skipped": w.remote,
        "bus_load_pct": round(load, 2),
        "error_frames": w.error_frames, "error_classes": w.errors, "bus_off_events": w.bus_off,
        "read_errors": w.read_errors,
        "kernel_rx_dropped": c.rx_dropped,
        "top_ids": w.top_ids(5),
        "controller": c.info.map(|i| json!({
            "state": i.state, "oper": i.oper, "bitrate": i.bitrate, "data_bitrate": i.data_bitrate,
            "fd": i.fd, "listen_only": i.listen_only, "loopback": i.loopback, "virtual": i.vcan,
            "rx_error_counter": i.rx_errors, "tx_error_counter": i.tx_errors, "restart_ms": i.restart_ms,
        })),
        "configured_bitrate": c.configured_bitrate,
        "bitrate_matches": bitrate_ok,
        "transmit": "never",
        "bus": { "connected": c.bus.connected, "topic": c.bus.topic, "seq": c.bus.seq, "write_errors": c.bus.write_errors, "error": c.bus.error },
        "vector_slots": ["frames_per_s", "bus_load_pct", "error_frames", "bus_off", "rx_error_counter", "tx_error_counter", "kernel_rx_dropped", "bitrate_ok"],
        "vector": vector,
        "export": c.export_url,
        "timestamp": unix_ms() / 1000,
    });
    if let Some(e) = c.error {
        r["error"] = json!(e);
    }
    (r, vector)
}

fn telemetry_items(w: &Window, c: &Context) -> Vec<Value> {
    let i = |n: &str, v: f64, u: &str| json!({"name": n, "value": v, "unit": u});
    let bitrate = c
        .info
        .and_then(|i| i.bitrate)
        .unwrap_or(c.configured_bitrate);
    vec![
        i("frames.rx", w.frames as f64, ""),
        i("frames.errors", w.error_frames as f64, ""),
        i("bus.off", w.bus_off as f64, ""),
        i("bus.load", load_pct(w.bits, bitrate, c.secs), "%"),
        i(
            "rx.err_counter",
            c.info.map_or(0.0, |i| i.rx_errors as f64),
            "",
        ),
        i(
            "tx.err_counter",
            c.info.map_or(0.0, |i| i.tx_errors as f64),
            "",
        ),
        i("rx.dropped", c.rx_dropped as f64, ""),
        i("stream.seq", c.bus.seq as f64, ""),
    ]
}

/// The broker side: connects when it can and writes the frame stream.
struct Link {
    path: Option<String>,
    source: String,
    bus: Option<bus::Bus>,
    writer: Option<bus::RingWriter>,
    state: BusState,
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
            writer: None,
            state: BusState::default(),
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
            self.writer = None;
            self.state.connected = false;
        }
        if self.writer.is_some() || self.last_try.is_some_and(|t| t.elapsed() < self.retry) {
            return;
        }
        self.last_try = Some(Instant::now());
        let token = std::env::var("WEFTOS_COGBUS_TOKEN").unwrap_or_default();
        let r = bus::Bus::connect(&path, &token).and_then(|b| {
            let w = b.stream_writer(KIND, &self.source, SCHEMA, MAX_RECORD, None)?;
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

    /// Publish one data frame; returns the node id for callers that need it.
    fn publish(&mut self, t_ns: u64, f: &Frame) {
        self.connect();
        let (Some(b), Some(w)) = (self.bus.as_ref(), self.writer.as_mut()) else {
            return;
        };
        let body = frame::frame_json(&self.source, &b.node_id, t_ns, w.last_seq() + 1, f);
        let bytes = serde_json::to_vec(&body).unwrap_or_default();
        match w.write(t_ns, &bytes) {
            Ok(s) => self.state.seq = s,
            Err(e) => {
                self.state.write_errors += 1;
                self.state.error = Some(e);
            }
        }
    }

    fn telemetry(&mut self, items: Vec<Value>) {
        self.tseq += 1;
        let Some(b) = self.bus.as_ref().filter(|b| !b.is_closed()) else {
            return;
        };
        let body = json!({"source_id": self.source, "node_id": b.node_id, "t_ns": bus::wall_ns(), "seq": self.tseq, "items": items});
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

/// Where frames come from.
enum Source {
    Live(socket::Can),
    /// Synthetic traffic: three periodic ids and an occasional extended one.
    Sim(u64),
    /// A candump log: (capture ns, frame), in file order.
    Replay(Vec<(u64, Frame)>, usize),
}

/// One synthetic frame per tick (10 ms): id 0x100 every tick, 0x200 every 10th, 0x7e8 (FD) every 50th.
fn sim_frame(n: u64) -> Option<Frame> {
    let mk = |id: u32, ext: bool, fd: bool, len: usize| Frame {
        id,
        ext,
        fd,
        brs: false,
        data: (0..len).map(|i| (n as usize + i) as u8).collect(),
    };
    if n.is_multiple_of(50) {
        Some(mk(0x7e8, false, true, 12))
    } else if n.is_multiple_of(10) {
        Some(mk(0x200, false, false, 4))
    } else if n.is_multiple_of(7) {
        Some(mk(0x18ff_0001, true, false, 8))
    } else {
        Some(mk(0x100, false, false, 8))
    }
}

fn replay_file(text: &str) -> Result<Vec<(u64, Frame)>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| frame::parse_candump_line(l).map_err(|e| format!("line {}: {e}", i + 1)))
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
    let handed = handed_devices();
    let local = o.simulate || o.replay.is_some();
    let iface_name = pick_iface(&o, &handed);
    let name = iface_name.clone().unwrap_or_else(|_| {
        if o.simulate {
            "sim0".into()
        } else {
            "can0".into()
        }
    });
    let source = o.source.clone().unwrap_or_else(|| name.clone());
    eprintln!(
        "{TAG} start (once={}, iface={name}, bitrate={}, source={source}, simulate={}, bus={})",
        o.once,
        o.bitrate,
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
    let mut link = Link::new(o.bus.clone(), &source);
    let mut open_err: Option<String> = None;
    let mut src: Option<Source> = None;
    let mut win = Window::default();
    let mut info: Option<iface::Info> = None;
    let period = Duration::from_secs(if o.once { o.window } else { o.interval });
    let mut t0 = Instant::now();
    let mut dropped_base: Option<u64> = None;
    let mut sim_tick = Instant::now();
    loop {
        if src.is_none() {
            let opened = if o.simulate {
                Ok(Source::Sim(0))
            } else if let Some(f) = &o.replay {
                std::fs::read_to_string(f)
                    .map_err(|e| format!("replay {f}: {e}"))
                    .and_then(|t| replay_file(&t))
                    .map(|v| Source::Replay(v, 0))
            } else {
                iface_name.clone().and_then(|n| {
                    let i = iface::read(&n)?;
                    if let Some(why) = iface::unusable(&i) {
                        info = Some(i);
                        return Err(why);
                    }
                    info = Some(i);
                    socket::Can::open(&n).map(Source::Live)
                })
            };
            match opened {
                Ok(s) => {
                    eprintln!("{TAG} listening");
                    open_err = None;
                    src = Some(s);
                }
                Err(e) => {
                    open_err = Some(e);
                    std::thread::sleep(period.min(Duration::from_secs(1)));
                }
            }
        }
        let mut done = false;
        let mut rx: Option<(u64, Rx)> = None;
        match src.as_mut() {
            Some(Source::Live(can)) => match can.read(Duration::from_millis(100)) {
                Ok(Some(buf)) => match frame::decode(&buf) {
                    Ok(r) => rx = Some((bus::wall_ns(), r)),
                    Err(e) => {
                        win.read_errors += 1;
                        open_err = Some(e);
                    }
                },
                Ok(None) => {}
                Err(e) => {
                    win.read_errors += 1;
                    open_err = Some(e);
                    src = None;
                    std::thread::sleep(Duration::from_secs(1));
                }
            },
            Some(Source::Sim(n)) => {
                if sim_tick.elapsed() >= Duration::from_millis(10) {
                    sim_tick += Duration::from_millis(10);
                    *n += 1;
                    rx = sim_frame(*n).map(|f| (bus::wall_ns(), Rx::Data(f)));
                } else {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            Some(Source::Replay(v, i)) => match v.get(*i) {
                Some((t, f)) => {
                    rx = Some((*t, Rx::Data(f.clone())));
                    *i += 1;
                }
                None => done = true,
            },
            None => {}
        }
        if let Some((t, r)) = rx {
            win.count(&r);
            if let Rx::Data(f) = &r {
                link.publish(t, f);
            }
        }
        if (local && done && o.replay.is_some()) || t0.elapsed() >= period {
            if let (Some(n), false) = (iface_name.as_ref().ok(), local) {
                if let Ok(i) = iface::read(n) {
                    info = Some(i);
                }
            }
            let dropped = iface_name
                .as_ref()
                .ok()
                .and_then(|n| iface::stat(n, "rx_dropped"))
                .unwrap_or(0);
            let base = *dropped_base.get_or_insert(dropped);
            let status = if src.is_none() {
                "no_source"
            } else if o.simulate {
                "simulate"
            } else if o.replay.is_some() {
                "replay"
            } else {
                "ok"
            };
            let err = open_err
                .clone()
                .or_else(|| iface_name.clone().err().filter(|_| src.is_none()));
            // A replayed log covers the time between its first and last frame, not the wall clock.
            let secs = match &src {
                Some(Source::Replay(v, _)) if v.len() > 1 => {
                    ((v[v.len() - 1].0 - v[0].0) as f64 / 1e9).max(0.001)
                }
                _ => t0.elapsed().as_secs_f64(),
            };
            let ctx = Context {
                status,
                iface: &name,
                source: &source,
                info: info.as_ref(),
                configured_bitrate: o.bitrate,
                secs,
                rx_dropped: dropped.saturating_sub(base),
                bus: &link.state,
                error: err.as_deref(),
                export_url: &export_url,
            };
            let (report, vector) = build_report(&win, &ctx);
            println!("{report}");
            if let Ok(mut st) = shared.lock() {
                st.push(report.clone());
            }
            link.telemetry(telemetry_items(&win, &ctx));
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
            "--iface",
            "CAN0",
            "--bitrate",
            "5",
            "--source",
            "Bad Name",
        ]));
        assert!(o.once);
        assert_eq!(
            (o.iface, o.bitrate, o.source),
            (None, 500_000, None),
            "bad values fall back"
        );
        let o = parse_opts(&s(&[
            "--iface",
            "vcan0",
            "--bitrate",
            "250000",
            "--source",
            "bus-a",
        ]));
        assert_eq!(
            (o.iface.as_deref(), o.bitrate, o.source.as_deref()),
            (Some("vcan0"), 250_000, Some("bus-a"))
        );
        assert_eq!(parse_opts(&s(&[])).api_bind, "127.0.0.1:8072");
    }

    #[test]
    fn the_interface_comes_from_the_flag_or_what_the_host_handed() {
        let none = parse_opts(&s(&[]));
        let handed = vec![
            "/dev/video0".to_string(),
            "can1".to_string(),
            "can2".to_string(),
        ];
        assert_eq!(
            pick_iface(&none, &handed).unwrap(),
            "can1",
            "the first interface name, never a path"
        );
        assert_eq!(
            pick_iface(&parse_opts(&s(&["--iface", "can0"])), &handed).unwrap(),
            "can0"
        );
        assert!(pick_iface(&none, &["/dev/video0".to_string()])
            .unwrap_err()
            .contains("no CAN interface"));
    }

    #[test]
    fn the_window_counts_frames_classes_and_bus_off() {
        let mut w = Window::default();
        let d = |id, ext, fd, len| {
            Rx::Data(Frame {
                id,
                ext,
                fd,
                brs: false,
                data: vec![0; len],
            })
        };
        for r in [
            d(0x100, false, false, 8),
            d(0x100, false, false, 8),
            d(0x200, false, true, 12),
            d(0x18ff_0001, true, false, 8),
            Rx::Remote,
        ] {
            w.count(&r);
        }
        w.count(&Rx::Error(frame::ErrorFrame {
            class: frame::ERR_BUSOFF | frame::ERR_CRTL,
            ctrl: 0,
        }));
        w.count(&Rx::Error(frame::ErrorFrame {
            class: frame::ERR_ACK,
            ctrl: 0,
        }));
        assert_eq!(
            (
                w.frames,
                w.fd_frames,
                w.ext_frames,
                w.remote,
                w.error_frames,
                w.bus_off
            ),
            (4, 1, 1, 1, 2, 1)
        );
        assert_eq!(
            (
                w.errors["bus_off"],
                w.errors["controller"],
                w.errors["no_ack"]
            ),
            (1, 1, 1)
        );
        assert_eq!(w.top_ids(1)[0], json!({"id": "0x100", "frames": 2}));
        assert_eq!(w.bits, (47 + 64) * 2 + (47 + 96) + (67 + 64));
    }

    #[test]
    fn load_is_bits_over_capacity_and_capped() {
        assert!(
            (load_pct(250_000, 500_000, 1.0) - 60.0).abs() < 1e-9,
            "50% raw is 60% with stuffing allowance"
        );
        assert_eq!(load_pct(10_000_000, 500_000, 1.0), 100.0);
        assert_eq!(load_pct(1, 0, 1.0), 0.0);
    }

    #[test]
    fn the_report_carries_rates_controller_state_and_the_vector() {
        let mut w = Window::default();
        for _ in 0..100 {
            w.count(&Rx::Data(Frame {
                id: 0x100,
                ext: false,
                fd: false,
                brs: false,
                data: vec![0; 8],
            }));
        }
        w.count(&Rx::Error(frame::ErrorFrame {
            class: frame::ERR_BUSOFF,
            ctrl: 0,
        }));
        let info = iface::Info {
            oper: "UP".into(),
            state: "ERROR-PASSIVE".into(),
            bitrate: Some(500_000),
            mtu: 16,
            rx_errors: 130,
            tx_errors: 2,
            ..Default::default()
        };
        let bus = BusState {
            connected: true,
            seq: 100,
            topic: Some("can.frame/can0".into()),
            ..Default::default()
        };
        let ctx = Context {
            status: "ok",
            iface: "can0",
            source: "can0",
            info: Some(&info),
            configured_bitrate: 500_000,
            secs: 10.0,
            rx_dropped: 3,
            bus: &bus,
            error: None,
            export_url: &None,
        };
        let (r, v) = build_report(&w, &ctx);
        assert_eq!(
            (
                r["frames"].as_u64(),
                r["frames_per_s"].as_f64(),
                r["bus_off_events"].as_u64()
            ),
            (Some(100), Some(10.0), Some(1))
        );
        assert_eq!(
            (
                r["controller"]["state"].as_str(),
                r["bitrate_matches"].as_bool(),
                r["transmit"].as_str()
            ),
            (Some("ERROR-PASSIVE"), Some(true), Some("never"))
        );
        assert_eq!(
            v[0..4],
            [10.0, round(load_pct(w.bits, 500_000, 10.0), 2), 1.0, 1.0]
        );
        assert_eq!((v[4], v[5], v[6], v[7]), (130.0, 2.0, 3.0, 1.0));
        let wrong = iface::Info {
            bitrate: Some(250_000),
            ..info.clone()
        };
        let (r, v) = build_report(
            &w,
            &Context {
                info: Some(&wrong),
                ..ctx
            },
        );
        assert_eq!((r["bitrate_matches"].as_bool(), v[7]), (Some(false), 0.0));
    }

    #[test]
    fn a_virtual_interface_has_no_bitrate_to_compare() {
        let info = iface::Info {
            oper: "UNKNOWN".into(),
            vcan: true,
            mtu: 72,
            ..Default::default()
        };
        let ctx = Context {
            status: "ok",
            iface: "vcan0",
            source: "vcan0",
            info: Some(&info),
            configured_bitrate: 500_000,
            secs: 1.0,
            rx_dropped: 0,
            bus: &BusState::default(),
            error: None,
            export_url: &None,
        };
        let (r, v) = build_report(&Window::default(), &ctx);
        assert!(r["bitrate_matches"].is_null());
        assert_eq!(v[7], -1.0);
    }

    #[test]
    fn no_source_has_an_empty_vector_and_a_reason() {
        let ctx = Context {
            status: "no_source",
            iface: "can0",
            source: "can0",
            info: None,
            configured_bitrate: 500_000,
            secs: 1.0,
            rx_dropped: 0,
            bus: &BusState::default(),
            error: Some("the interface is down; ... operator"),
            export_url: &None,
        };
        let (r, v) = build_report(&Window::default(), &ctx);
        assert_eq!(
            (r["error"].as_str().map(|e| e.contains("operator")), v),
            (Some(true), [-1.0; 8])
        );
    }

    #[test]
    fn synthetic_traffic_has_the_advertised_mix() {
        let mut w = Window::default();
        for n in 1..=100 {
            w.count(&Rx::Data(sim_frame(n).unwrap()));
        }
        assert_eq!(w.fd_frames, 2, "0x7e8 every 50th");
        assert!(w.ext_frames > 5 && w.top[&0x100] > 50);
    }

    #[test]
    fn telemetry_names_are_schema_valid() {
        let ctx = Context {
            status: "ok",
            iface: "c",
            source: "c",
            info: None,
            configured_bitrate: 500_000,
            secs: 1.0,
            rx_dropped: 0,
            bus: &BusState::default(),
            error: None,
            export_url: &None,
        };
        for it in telemetry_items(&Window::default(), &ctx) {
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
    fn replay_files_parse_and_report_the_bad_line() {
        let v = replay_file(
            "(1.000000) can0 123#DEAD\n\n(1.010000) can0 7FF##1000102030405060708090a0b\n",
        )
        .unwrap();
        assert_eq!(v.len(), 2);
        assert!(replay_file("(1.0) can0 123#DEAD\ngarbage")
            .unwrap_err()
            .starts_with("line 2"));
    }
}
