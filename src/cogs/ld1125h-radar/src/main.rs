//! Cognitum cog: ld1125h-radar
//!
//! Hi-Link HLK-LD1125H-24G, a 24 GHz FMCW human-presence radar. Application
//! manual V1.2 (2021-9-16): factory UART is 115200 8N1, ASCII. A larger motion
//! is `mov, dis=<metres>`; a still body or a small motion is `occ, dis=<metres>`.
//! When the module detects nothing it stops sending. Breathing micro-motion is
//! how it detects a still person. It does not report a heart rate or a breathing
//! rate, and this cog does not send configuration.
//!
//!   cog-ld1125h-radar --once
//!   cog-ld1125h-radar --once --simulate
//!   cog-ld1125h-radar --device /dev/ttyUSB0
//!
//! A build with `--features spatial-evidence` also accepts `--spatial-out`,
//! which stays off unless that flag is set. See src/spatial.rs.

mod export;
mod guide;
mod lines;
mod serial;
#[cfg(feature = "spatial-evidence")]
mod spatial;

use export::{Shared, State};
use lines::{Kind, Parser, Reading, Stats, MAX_RANGE_M};
use serial::Serial;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TAG: &str = "[cog-ld1125h-radar]";
/// Per-cog id in the local store tuple. 21..=27 are the other sensor cogs in
/// this tree; 28 is the next free id. Ingest is skipped when there is no line.
const STORE_ID: u32 = 28;
const BAUD: u32 = 115_200;
const BIND: &str = "127.0.0.1:8054";
const DEFAULT_DEVICE: &str = "/dev/ttyUSB0";

#[derive(Debug)]
struct Config {
    once: bool,
    interval: u64,
    window: u64,
    device: String,
    baud: u32,
    simulate: bool,
    #[cfg(feature = "spatial-evidence")]
    spatial: Option<spatial::Settings>,
}

fn parse_args(args: &[String]) -> Result<Config, String> {
    let mut cfg = Config {
        once: false,
        interval: 1,
        window: 2,
        device: DEFAULT_DEVICE.into(),
        baud: BAUD,
        simulate: false,
        #[cfg(feature = "spatial-evidence")]
        spatial: None,
    };
    #[cfg(feature = "spatial-evidence")]
    let mut sp = spatial::Args::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--once" => cfg.once = true,
            "--simulate" => cfg.simulate = true,
            "--interval" => {
                let v = value()?;
                cfg.interval = v
                    .parse()
                    .ok()
                    .filter(|n| (1..=60).contains(n))
                    .ok_or(format!("--interval must be an integer 1..=60, got {v:?}"))?;
            }
            "--window" => {
                let v = value()?;
                cfg.window = v
                    .parse()
                    .ok()
                    .filter(|n| (1..=30).contains(n))
                    .ok_or(format!("--window must be an integer 1..=30, got {v:?}"))?;
            }
            "--baud" => {
                let v = value()?;
                cfg.baud = v
                    .parse()
                    .ok()
                    .filter(|n| (9600..=921_600).contains(n))
                    .ok_or(format!(
                        "--baud must be an integer 9600..=921600, got {v:?}"
                    ))?;
            }
            "--device" => {
                let v = value()?;
                validate_device(&v)?;
                cfg.device = v;
            }
            #[cfg(feature = "spatial-evidence")]
            f if spatial::Args::FLAGS.contains(&f) => sp.set(f, &value()?)?,
            other => return Err(format!("unknown argument {other:?} (see --help)")),
        }
    }
    #[cfg(feature = "spatial-evidence")]
    {
        cfg.spatial = sp.finish()?;
        if cfg.once {
            if let Some(spatial::Settings {
                sink: spatial::Sink::Export,
                ..
            }) = &cfg.spatial
            {
                return Err(
                    "--spatial-out export needs continuous mode (no --once); use a file path"
                        .into(),
                );
            }
        }
    }
    Ok(cfg)
}

/// Only a path under `/dev/`, bounded, no traversal.
fn validate_device(path: &str) -> Result<(), String> {
    let ok = path.starts_with("/dev/")
        && path.len() <= 128
        && !path.split('/').any(|c| c == "..")
        && path.bytes().all(|b| b.is_ascii_graphic());
    if ok {
        Ok(())
    } else {
        Err(format!("--device must be a path under /dev/, got {path:?}"))
    }
}

fn unix_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

trait ByteSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
    fn describe(&self) -> String;
}

struct SerialSource {
    dev: Serial,
    desc: String,
}

impl ByteSource for SerialSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.dev.read(buf)
    }
    fn describe(&self) -> String {
        self.desc.clone()
    }
}

/// Vendor-format bytes, not pre-decoded values. The numbers are synthetic.
fn sim_line(i: u64) -> &'static [u8] {
    if i.is_multiple_of(2) {
        b"mov, dis=1.25\n"
    } else {
        b"occ, dis=0.75\n"
    }
}

struct RadarSim {
    next: Instant,
    i: u64,
}

impl RadarSim {
    fn new() -> Self {
        Self {
            next: Instant::now(),
            i: 0,
        }
    }
}

impl ByteSource for RadarSim {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let now = Instant::now();
        if now < self.next {
            std::thread::sleep(self.next - now);
        }
        self.next = Instant::now() + Duration::from_millis(50);
        let line = sim_line(self.i);
        self.i += 1;
        let n = line.len().min(buf.len());
        buf[..n].copy_from_slice(&line[..n]);
        Ok(n)
    }
    fn describe(&self) -> String {
        "simulated HLK-LD1125H-24G (ASCII mov/occ lines)".into()
    }
}

fn open_source(cfg: &Config) -> Result<Box<dyn ByteSource + Send>, String> {
    if cfg.simulate {
        return Ok(Box::new(RadarSim::new()));
    }
    let dev = Serial::open(&cfg.device, cfg.baud)?;
    let desc = format!("HLK-LD1125H-24G on {} @ {} 8N1", cfg.device, cfg.baud);
    Ok(Box::new(SerialSource { dev, desc }))
}

fn no_source_error(device: &str, baud: u32, open_err: &str) -> String {
    format!(
        "{open_err}. Nothing read at {baud} 8N1 on {device}. Next check: module pin 4 UTX \
         (silk UT) to Pi pin 10 RXD, common ground on pin 2 (silk G) to Pi pin 6, VCC pin 1 \
         (silk 5V) from Pi pin 2 (5 V). Leave URX unconnected while the serial console owns \
         the UART. /dev/serial0 stays the console until the owner frees it. A USB-serial \
         adapter on /dev/ttyUSB0 needs no boot change."
    )
}

fn no_frames_error(device: &str, baud: u32, undecoded: u64) -> String {
    let extra = if undecoded > 0 {
        format!(" {undecoded} non-report line(s) arrived and were not decoded.")
    } else {
        String::new()
    };
    format!(
        "no frames on {device} at {baud} 8N1.{extra} The module stops sending when it detects \
         nothing (application manual V1.2), so silence is not a measured empty room. Check \
         UTX to Pi RXD (pin 10), common ground, 5 V on VCC, baud 115200, and that another \
         program is not holding the tty."
    )
}

/// Fields shared by a `no_source` line and a window report. Counters are this
/// cog's observations. A distance the module did not send stays null.
struct ReportIn<'a> {
    status: &'a str,
    source: Option<&'a str>,
    simulated: bool,
    kind: Option<&'a str>,
    distance_m: Option<f64>,
    frames: u64,
    mov_lines: u64,
    occ_lines: u64,
    delta: Stats,
    window_s: u64,
    export_url: &'a Option<String>,
    error: Option<String>,
}

fn report_json(r: ReportIn<'_>) -> serde_json::Value {
    serde_json::json!({
        "status": r.status,
        "source": r.source,
        "simulated": r.simulated,
        "kind": r.kind,
        "distance_m": r.distance_m,
        "frames": r.frames,
        "mov_lines": r.mov_lines,
        "occ_lines": r.occ_lines,
        "undecoded_lines": r.delta.undecoded_lines,
        "rejected_lines": r.delta.rejected_lines,
        "resync_bytes": r.delta.resync_bytes,
        "checksum_errors": r.delta.checksum_errors,
        "window_s": r.window_s,
        "export": r.export_url,
        "error": r.error,
        "timestamp": unix_s(),
    })
}

struct WindowIn<'a> {
    readings: &'a [Reading],
    delta: Stats,
    source: &'a str,
    simulated: bool,
    window_s: u64,
    export_url: &'a Option<String>,
    device: &'a str,
    baud: u32,
}

fn report_from(w: WindowIn<'_>) -> (serde_json::Value, Option<Vec<f64>>) {
    let mov_lines = w.readings.iter().filter(|r| r.kind == Kind::Mov).count() as u64;
    let occ_lines = w.readings.iter().filter(|r| r.kind == Kind::Occ).count() as u64;
    let (status, kind, distance, err, vector) = match w.readings.last() {
        None => (
            "no_frames",
            None,
            None,
            Some(no_frames_error(w.device, w.baud, w.delta.undecoded_lines)),
            None,
        ),
        Some(last) => (
            last.kind.as_str(),
            Some(last.kind.as_str()),
            Some(last.distance_m),
            None,
            Some(store_vector(last.kind, last.distance_m)),
        ),
    };
    let report = report_json(ReportIn {
        status,
        source: Some(w.source),
        simulated: w.simulated,
        kind,
        distance_m: distance,
        frames: w.readings.len() as u64,
        mov_lines,
        occ_lines,
        delta: w.delta,
        window_s: w.window_s,
        export_url: w.export_url,
        error: err,
    });
    (report, vector)
}

/// Kind code and normalised distance, each in [0, 1]. Not a person score.
fn store_vector(kind: Kind, distance_m: f64) -> Vec<f64> {
    let code = match kind {
        Kind::Mov => 1.0,
        Kind::Occ => 0.5,
    };
    let d = (distance_m / MAX_RANGE_M).clamp(0.0, 1.0);
    vec![code, d, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
}

fn emit_line(v: &serde_json::Value) {
    let line = match serde_json::to_string(v) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{TAG} serialize: {e}");
            return;
        }
    };
    debug_assert!(line.len() < 65536);
    let mut out = std::io::stdout().lock();
    if writeln!(out, "{line}").and_then(|_| out.flush()).is_err() {
        std::process::exit(0);
    }
}

#[cfg(feature = "spatial-evidence")]
fn attach_spatial(report: &mut serde_json::Value, status: Option<&str>, reasons: &[String]) {
    if let Some(s) = status {
        report["spatial"] = serde_json::json!(s);
    }
    if !reasons.is_empty() {
        report["reasons"] = serde_json::json!(reasons);
    }
}

#[cfg(feature = "spatial-evidence")]
fn emit_spatial(e: &mut spatial::Emitter, t_ms: u64, reading: Reading, st: &mut State) {
    let Some(line) = e
        .record(t_ms, reading.distance_m)
        .and_then(|r| serde_json::to_string(&r).ok())
    else {
        return;
    };
    if line.len() > 16 * 1024 {
        st.spatial_error = Some("spatial line longer than 16 KiB".into());
        return;
    }
    match e.sink() {
        spatial::Sink::File(path) => {
            if let Err(err) = spatial::append(path, &[line]) {
                st.spatial_error = Some(format!("{path}: {err}"));
            }
        }
        spatial::Sink::Export => st.push_evidence(t_ms, line),
    }
}

fn read_loop(
    mut src: Box<dyn ByteSource + Send>,
    shared: Shared,
    #[cfg(feature = "spatial-evidence")] mut spatial_out: Option<spatial::Emitter>,
) {
    let mut parser = Parser::new();
    let mut buf = [0u8; 256];
    let mut errors = 0u64;
    loop {
        let n = match src.read(&mut buf) {
            Ok(0) => continue,
            Ok(n) => n,
            Err(e) => {
                errors += 1;
                if errors.is_multiple_of(200) {
                    eprintln!("{TAG} read error: {e}");
                }
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
        };
        let readings = parser.feed(&buf[..n]);
        let Ok(mut st) = shared.lock() else {
            continue;
        };
        st.stats = parser.stats;
        let t_ms = unix_ms();
        for reading in readings {
            #[cfg(feature = "spatial-evidence")]
            if let Some(e) = spatial_out.as_mut() {
                emit_spatial(e, t_ms, reading, &mut st);
            }
            st.push_reading(t_ms, reading);
        }
    }
}

fn store_to_seed(vector: &[f64]) -> Result<(), String> {
    let payload = serde_json::json!({ "vectors": [[STORE_ID, vector]], "dedup": true });
    let body = serde_json::to_vec(&payload).map_err(|e| format!("json: {e}"))?;
    let mut conn =
        std::net::TcpStream::connect("127.0.0.1:80").map_err(|e| format!("connect: {e}"))?;
    conn.set_read_timeout(Some(Duration::from_secs(5))).ok();
    conn.set_write_timeout(Some(Duration::from_secs(5))).ok();
    write!(
        conn,
        "POST /api/v1/store/ingest HTTP/1.0\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .map_err(|e| format!("write: {e}"))?;
    conn.write_all(&body).map_err(|e| format!("body: {e}"))?;
    let mut resp = Vec::new();
    let mut buf = [0u8; 1024];
    while !response_complete(&resp) {
        match conn.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => resp.extend_from_slice(&buf[..n]),
        }
    }
    let status = String::from_utf8_lossy(&resp)
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_string();
    if status.starts_with('2') {
        Ok(())
    } else {
        Err(format!("ingest replied {status:?}"))
    }
}

fn response_complete(resp: &[u8]) -> bool {
    let Some(end) = resp.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let head = String::from_utf8_lossy(&resp[..end]).to_ascii_lowercase();
    let len = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    resp.len() >= end + 4 + len
}

fn publish(
    shared: &Shared,
    window_s: u64,
    export_url: &Option<String>,
    device: &str,
    baud: u32,
    #[cfg(feature = "spatial-evidence")] spatial_status: Option<&str>,
    #[cfg(feature = "spatial-evidence")] spatial_refusal: &Option<String>,
) -> serde_json::Value {
    let Ok(mut st) = shared.lock() else {
        eprintln!("{TAG} state poisoned");
        std::process::exit(1);
    };
    let now = unix_ms();
    let readings = st.in_window(now, window_s);
    let delta = st.stats.since(st.reported);
    st.reported = st.stats;
    let (report, vector) = report_from(WindowIn {
        readings: &readings,
        delta,
        source: &st.source,
        simulated: st.simulated,
        window_s,
        export_url,
        device,
        baud,
    });
    #[cfg(feature = "spatial-evidence")]
    let report = {
        let mut report = report;
        let mut reasons = Vec::new();
        let mut status = spatial_status;
        if let Some(why) = spatial_refusal {
            reasons.push(format!("spatial_evidence_refused: {why}"));
        }
        if let Some(e) = st.spatial_error.take() {
            reasons.push(format!("spatial_evidence_error: {e}"));
            status = Some("write_error");
        }
        attach_spatial(&mut report, status, &reasons);
        report
    };
    st.latest_report = Some(report.clone());
    drop(st);
    emit_line(&report);
    if let Some(vector) = vector {
        if let Err(e) = store_to_seed(&vector) {
            eprintln!("{TAG} store error: {e}");
        }
    }
    report
}

fn main() {
    let all: Vec<String> = std::env::args().collect();
    cog_sensor_sources::handle_help(
        &all,
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        include_str!("../cog.toml"),
    );
    let args: Vec<String> = all.into_iter().skip(1).collect();
    let cfg = match parse_args(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{TAG} {e}");
            std::process::exit(2);
        }
    };

    #[cfg(feature = "spatial-evidence")]
    let (spatial_emitter, spatial_refusal, spatial_status): (
        Option<spatial::Emitter>,
        Option<String>,
        Option<&'static str>,
    ) = match cfg.spatial.clone() {
        None => (None, None, None),
        Some(s) => {
            let status = s.refusal_status();
            match spatial::Emitter::new(s, cfg.simulate) {
                Ok(e) => (Some(e), None, Some("emitting")),
                Err(why) => {
                    eprintln!("{TAG} not emitting spatial.evidence.v1: {why}");
                    (None, Some(why), status)
                }
            }
        }
    };

    eprintln!(
        "{TAG} start (once={}, interval={}s, window={}s, device={}, baud={}, simulate={})",
        cfg.once, cfg.interval, cfg.window, cfg.device, cfg.baud, cfg.simulate
    );

    let shared: Shared = Arc::new(Mutex::new(State::new()));
    let export_url = match export::serve(BIND, shared.clone()) {
        Ok(addr) => Some(format!("http://{addr}")),
        Err(e) => {
            eprintln!("{TAG} export disabled: {e}");
            None
        }
    };

    let src = loop {
        match open_source(&cfg) {
            Ok(s) => break s,
            Err(e) => {
                let error = no_source_error(&cfg.device, cfg.baud, &e);
                let report = report_json(ReportIn {
                    status: "no_source",
                    source: None,
                    simulated: false,
                    kind: None,
                    distance_m: None,
                    frames: 0,
                    mov_lines: 0,
                    occ_lines: 0,
                    delta: Stats::default(),
                    window_s: if cfg.once { cfg.window } else { cfg.interval },
                    export_url: &export_url,
                    error: Some(error.clone()),
                });
                #[cfg(feature = "spatial-evidence")]
                let report = {
                    let mut report = report;
                    let mut reasons = Vec::new();
                    if let Some(why) = &spatial_refusal {
                        reasons.push(format!("spatial_evidence_refused: {why}"));
                    }
                    attach_spatial(&mut report, spatial_status, &reasons);
                    report
                };
                if let Ok(mut st) = shared.lock() {
                    st.latest_report = Some(report.clone());
                }
                emit_line(&report);
                eprintln!("{TAG} no source: {error}");
                if cfg.once {
                    return;
                }
                std::thread::sleep(Duration::from_secs(cfg.interval.clamp(1, 5)));
            }
        }
    };

    if let Ok(mut st) = shared.lock() {
        st.source = src.describe();
        st.simulated = cfg.simulate;
    }

    std::thread::spawn({
        let shared = shared.clone();
        move || {
            read_loop(
                src,
                shared,
                #[cfg(feature = "spatial-evidence")]
                spatial_emitter,
            )
        }
    });

    let period = if cfg.once { cfg.window } else { cfg.interval };
    loop {
        std::thread::sleep(Duration::from_secs(period));
        publish(
            &shared,
            period,
            &export_url,
            &cfg.device,
            cfg.baud,
            #[cfg(feature = "spatial-evidence")]
            spatial_status,
            #[cfg(feature = "spatial-evidence")]
            &spatial_refusal,
        );
        if cfg.once {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().copied().map(String::from).collect()
    }

    #[test]
    fn export_binds_loopback_on_the_assigned_port() {
        assert_eq!(BIND, "127.0.0.1:8054");
    }

    #[test]
    fn unknown_and_unbounded_arguments_are_refused() {
        assert!(parse_args(&s(&["--exec", "sh"])).is_err());
        assert!(parse_args(&s(&["--device", "/etc/hosts"])).is_err());
        assert!(parse_args(&s(&["--device", "/dev/../etc/passwd"])).is_err());
        assert!(parse_args(&s(&["--interval", "0"])).is_err());
        assert!(parse_args(&s(&["--interval", "99"])).is_err());
        assert!(parse_args(&s(&["--window"])).is_err());
        assert!(parse_args(&s(&["--baud", "1"])).is_err());
        let c = parse_args(&s(&["--once", "--simulate"])).unwrap();
        assert!(c.once && c.simulate);
        assert_eq!(c.device, DEFAULT_DEVICE);
        assert_eq!(c.baud, 115_200);
    }

    #[test]
    fn store_vector_stays_in_unit_interval() {
        for (k, d) in [(Kind::Mov, 1.25), (Kind::Occ, 0.75), (Kind::Mov, 100.0)] {
            let v = store_vector(k, d);
            assert!(v.iter().all(|x| (0.0..=1.0).contains(x)), "{v:?}");
            assert_eq!(v.len(), 8);
        }
    }

    #[test]
    fn simulator_bytes_go_through_the_real_parser() {
        let mut p = Parser::new();
        let mut got = Vec::new();
        for i in 0..4 {
            got.extend(p.feed(sim_line(i)));
        }
        assert_eq!(got.len(), 4);
        assert_eq!(got[0].kind, Kind::Mov);
        assert_eq!(got[0].distance_m, 1.25);
        assert_eq!(got[1].kind, Kind::Occ);
        assert_eq!(got[1].distance_m, 0.75);
        let v = store_vector(got[0].kind, got[0].distance_m);
        assert!(v.iter().all(|x| (0.0..=1.0).contains(x)));
    }

    #[test]
    fn no_source_report_has_null_sensor_fields_and_no_vitals() {
        let report = report_json(ReportIn {
            status: "no_source",
            source: None,
            simulated: false,
            kind: None,
            distance_m: None,
            frames: 0,
            mov_lines: 0,
            occ_lines: 0,
            delta: Stats::default(),
            window_s: 2,
            export_url: &None,
            error: Some("open failed".into()),
        });
        assert_eq!(report["status"], "no_source");
        assert!(report["kind"].is_null());
        assert!(report["distance_m"].is_null());
        assert!(report["source"].is_null());
        assert_eq!(report["frames"], 0);
        assert_eq!(report["checksum_errors"], 0);
        let text = report.to_string();
        assert!(text.len() < 65536);
        for banned in ["heart_rate", "breathing_rate", "track", "person"] {
            assert!(!text.contains(banned), "{banned} in {text}");
        }
    }

    #[test]
    fn a_quiet_window_does_not_carry_a_distance() {
        let (report, vector) = report_from(WindowIn {
            readings: &[],
            delta: Stats {
                undecoded_lines: 2,
                ..Stats::default()
            },
            source: "HLK-LD1125H-24G on /dev/ttyUSB0 @ 115200 8N1",
            simulated: false,
            window_s: 2,
            export_url: &None,
            device: "/dev/ttyUSB0",
            baud: 115_200,
        });
        assert_eq!(report["status"], "no_frames");
        assert!(report["distance_m"].is_null());
        assert!(report["kind"].is_null());
        assert!(vector.is_none());
        assert_eq!(report["undecoded_lines"], 2);
        let err = report["error"].as_str().unwrap();
        assert!(
            err.contains("/dev/ttyUSB0") && err.contains("115200"),
            "{err}"
        );
    }

    #[test]
    fn a_window_reports_the_last_line_only() {
        let lines = [
            Reading {
                kind: Kind::Mov,
                distance_m: 1.25,
            },
            Reading {
                kind: Kind::Occ,
                distance_m: 0.75,
            },
        ];
        let (report, vector) = report_from(WindowIn {
            readings: &lines,
            delta: Stats::default(),
            source: "sim",
            simulated: true,
            window_s: 2,
            export_url: &None,
            device: "sim",
            baud: 115_200,
        });
        assert_eq!(report["status"], "occ");
        assert_eq!(report["kind"], "occ");
        assert_eq!(report["distance_m"], 0.75);
        assert_eq!(report["mov_lines"], 1);
        assert_eq!(report["occ_lines"], 1);
        assert!(report["error"].is_null());
        assert_eq!(report["simulated"], true);
        assert!(vector.unwrap().iter().all(|x| (0.0..=1.0).contains(x)));
    }

    #[cfg(not(feature = "spatial-evidence"))]
    #[test]
    fn default_build_refuses_spatial_flags() {
        let err = parse_args(&s(&["--spatial-out", "/tmp/x.jsonl"])).unwrap_err();
        assert!(err.contains("unknown argument"), "{err}");
    }
}
