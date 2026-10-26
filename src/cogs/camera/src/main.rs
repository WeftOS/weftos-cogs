//! Cognitum Cog: camera
//!
//! Owns a V4L2 camera (WeftOS Ventuno cog plan 5.2; the owner's USB camera first) and
//! announces its frames on the node's cogbus as `video.frame-ref.v1` readings on
//! `video.frame-ref/<source>`: a BLAKE3 hash, the size, `mjpeg`, and how long the bytes are
//! kept. The JPEG itself stays in this cog's data directory (and on `GET /frame.jpg` on
//! loopback); raw frames do not cross the bus or the mesh. The device is the one the host
//! hands the owner in `WEFTOS_DEVICES`. H.264 on request is not done yet.
//!
//! Usage:
//!   cog-camera                          # capture continuously, announce on $WEFTOS_COGBUS
//!   cog-camera --once                   # capture a few frames, report the last, exit
//!   cog-camera --once --simulate        # structurally valid synthetic frames, nothing posted
//!   cog-camera --once --replay FILE     # announce a JPEG file as if the camera had sent it
//!   cog-camera --device /dev/video0     # name the node by hand (bench run)

mod bus;
mod camera;
mod export;
mod guide;
mod ingest;

use camera::{Capture, FrameStore};
use export::{round, Shared, State};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TAG: &str = "[cog-camera]";
/// Per-cog id in the store vector tuple (30=audio-out).
pub(crate) const STORE_ID: u32 = 31;
const KIND: &str = "video.frame-ref";
const SCHEMA: &str = "video.frame-ref.v1";

#[derive(Debug, Clone, PartialEq)]
struct Opts {
    once: bool,
    interval: u64,
    fps: u32,
    width: u32,
    height: u32,
    ttl_ms: u64,
    no_blob: bool,
    source: String,
    device: Option<String>,
    camera_name: Option<String>,
    frames_dir: Option<PathBuf>,
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

fn parse_opts(a: &[String]) -> Opts {
    let source = match arg(a, "--source") {
        Some(s) if valid_source(s) => s.to_string(),
        Some(_) => {
            eprintln!("{TAG} --source must match [a-z0-9][a-z0-9._-]*, using cam0");
            "cam0".into()
        }
        None => "cam0".into(),
    };
    Opts {
        once: a.iter().any(|x| x == "--once"),
        interval: num(a, "--interval", 5, 1, 300),
        fps: num(a, "--fps", 2, 1, 30),
        width: num(a, "--width", 1280, 64, 7680),
        height: num(a, "--height", 720, 64, 4320),
        ttl_ms: num(a, "--ttl-ms", 10_000, 1000, 3_600_000),
        source,
        device: arg(a, "--device")
            .filter(|s| !s.is_empty())
            .map(String::from),
        camera_name: arg(a, "--camera-name")
            .filter(|s| !s.is_empty())
            .map(String::from),
        frames_dir: arg(a, "--frames-dir").map(PathBuf::from),
        bus: arg(a, "--bus")
            .map(String::from)
            .or_else(|| std::env::var("WEFTOS_COGBUS").ok()),
        api_bind: arg(a, "--api-bind").unwrap_or("127.0.0.1:8065").to_string(),
        no_blob: a.iter().any(|x| x == "--no-blob"),
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

fn handed_devices() -> Vec<String> {
    std::env::var("WEFTOS_DEVICES")
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

fn sysfs_video(node: &str, file: &str) -> Option<String> {
    std::fs::read_to_string(format!("/sys/class/video4linux/{node}/{file}")).ok()
}

/// Counters over one report window.
#[derive(Debug, Default, Clone)]
struct Window {
    frames: u64,
    bytes: u64,
    refs: u64,
    blobs: u64,
    errors: u64,
    width: u32,
    height: u32,
    last_hash: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct BusState {
    connected: bool,
    seq: u64,
    error: Option<String>,
}

/// What a report says besides the window's counters.
struct Context<'a> {
    status: &'a str,
    source: &'a str,
    device: &'a str,
    secs: f64,
    bus: &'a BusState,
    devices: &'a [String],
    error: Option<&'a str>,
    export_url: &'a Option<String>,
}

fn build_report(win: &Window, c: &Context) -> (Value, [f64; 8]) {
    let ok = matches!(c.status, "ok" | "simulate" | "replay");
    let fps = if c.secs > 0.0 {
        win.frames as f64 / c.secs
    } else {
        0.0
    };
    let mean_kb = if win.frames > 0 {
        win.bytes as f64 / win.frames as f64 / 1024.0
    } else {
        0.0
    };
    let vector = if ok {
        [
            round(fps, 2),
            win.width as f64,
            win.height as f64,
            round(mean_kb, 1),
            win.frames as f64,
            c.bus.connected as u8 as f64,
            win.errors as f64,
            win.refs as f64,
        ]
    } else {
        [-1.0; 8]
    };
    let mut r = json!({
        "status": c.status,
        "provenance": match c.status { "simulate" => "SYNTHETIC", "replay" => "REPLAY", _ => "MEASURED" },
        "source": c.source,
        "device": c.device,
        "pixel_format": "mjpeg",
        "frames": win.frames,
        "fps": round(fps, 2),
        "width": if win.frames > 0 { json!(win.width) } else { Value::Null },
        "height": if win.frames > 0 { json!(win.height) } else { Value::Null },
        "mean_frame_kb": round(mean_kb, 1),
        "frame_refs_published": win.refs,
        "blobs_put": win.blobs,
        "capture_errors": win.errors,
        "last_hash": win.last_hash,
        "devices": c.devices,
        "bus": { "connected": c.bus.connected, "seq": c.bus.seq, "error": c.bus.error },
        "vector_slots": ["fps", "width", "height", "mean_frame_kb", "frames", "bus_connected", "capture_errors", "refs_published"],
        "vector": vector,
        "export": c.export_url,
        "timestamp": unix_ms() / 1000,
    });
    if let Some(e) = c.error {
        r["error"] = json!(e);
    }
    (r, vector)
}

fn telemetry_items(win: &Window, bus: &BusState, secs: f64) -> Vec<Value> {
    let i = |n: &str, v: f64, u: &str| json!({"name": n, "value": v, "unit": u});
    vec![
        i("frames.total", win.frames as f64, ""),
        i(
            "frames.rate",
            if secs > 0.0 {
                win.frames as f64 / secs
            } else {
                0.0
            },
            "fps",
        ),
        i("frames.refs", win.refs as f64, ""),
        i("capture.errors", win.errors as f64, ""),
        i("frame.width", win.width as f64, "px"),
        i("frame.height", win.height as f64, "px"),
        i("bus.connected", bus.connected as u8 as f64, ""),
    ]
}

/// A stored frame as the reading announces it.
struct Stored<'a> {
    hash: &'a str,
    width: u32,
    height: u32,
    ttl_ms: u64,
    /// The frame's `blob.v1` handle, when it was put in the node's blob store.
    blob: Option<Value>,
}

/// The `video.frame-ref.v1` body for a stored frame.
fn frame_ref(source: &str, node_id: &str, t_ns: u64, seq: u64, f: &Stored) -> Value {
    let mut v = json!({
        "source_id": source, "node_id": node_id, "t_ns": t_ns, "seq": seq,
        "hash": f.hash, "width": f.width, "height": f.height, "pixel_format": "mjpeg",
        "ttl_ms": f.ttl_ms.min(3_600_000),
    });
    // Carried in the reading, the handle gives its readers access to the bytes; they can pass
    // it on in a service call.
    if let Some(b) = &f.blob {
        v["blob"] = b.clone();
    }
    v
}

/// The broker side: connects when it can and publishes the readings.
struct Link {
    path: Option<String>,
    bus: Option<bus::Bus>,
    state: BusState,
    tseq: u64,
    last_try: Option<Instant>,
    /// Wait before the next connect attempt (longer after `busy`).
    retry: Duration,
}

impl Link {
    fn new(path: Option<String>) -> Self {
        Self {
            path,
            bus: None,
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
            self.state.connected = false;
        }
        if self.bus.is_some() || self.last_try.is_some_and(|t| t.elapsed() < self.retry) {
            return;
        }
        self.last_try = Some(Instant::now());
        let token = std::env::var("WEFTOS_COGBUS_TOKEN").unwrap_or_default();
        match bus::Bus::connect(&path, &token) {
            Ok(b) => {
                eprintln!("{TAG} announcing {KIND} on {path} (node {})", b.node_id);
                self.state = BusState {
                    connected: true,
                    ..BusState::default()
                };
                self.bus = Some(b);
            }
            Err(e) => {
                self.retry = bus::retry_after(&e);
                self.state.error = Some(e.clone());
                eprintln!("{TAG} bus: {e}");
            }
        }
    }

    /// Announce a stored frame. `Ok(false)` when there is no broker to tell.
    fn announce(&mut self, source: &str, f: &Stored) -> Result<bool, String> {
        self.connect();
        let Some(b) = self.bus.as_ref().filter(|b| !b.is_closed()) else {
            return Ok(false);
        };
        self.state.seq += 1;
        let t = bus::wall_ns();
        let body = frame_ref(source, &b.node_id, t, self.state.seq, f);
        b.publish(KIND, source, SCHEMA, t, body)
            .map(|_| true)
            .inspect_err(|e| self.state.error = Some(e.clone()))
    }

    /// Put a frame in the blob store; `None` when there is no broker or it refused (the
    /// reason is kept in the bus state, and the reading goes out without a handle).
    fn put_blob(&mut self, bytes: &[u8], ttl_ms: u64) -> Option<Value> {
        self.connect();
        let b = self.bus.as_ref().filter(|b| !b.is_closed())?;
        match b.put_blob(
            "image/jpeg",
            bytes,
            Duration::from_millis(ttl_ms.clamp(1000, 3_600_000)),
        ) {
            Ok(h) => Some(h),
            Err(e) => {
                self.state.error = Some(format!("blob: {e}"));
                None
            }
        }
    }

    fn telemetry(&mut self, source: &str, win: &Window, secs: f64) {
        self.tseq += 1;
        let Some(b) = self.bus.as_ref().filter(|b| !b.is_closed()) else {
            return;
        };
        let body = json!({
            "source_id": source, "node_id": b.node_id, "t_ns": bus::wall_ns(),
            "seq": self.tseq, "items": telemetry_items(win, &self.state, secs),
        });
        if let Err(e) = b.publish("telemetry", source, "telemetry.v1", bus::wall_ns(), body) {
            eprintln!("{TAG} telemetry: {e}");
        }
    }
}

/// Where frames come from.
enum Source {
    Camera(Capture, String),
    Synthetic(u64),
    Replay(Vec<u8>),
}

impl Source {
    fn label(&self) -> String {
        match self {
            Source::Camera(_, d) => format!("v4l2:{d}"),
            Source::Synthetic(_) => "simulated".into(),
            Source::Replay(_) => "replay".into(),
        }
    }
}

fn open_source(o: &Opts, data_dir: &std::path::Path) -> Result<Source, String> {
    if o.simulate {
        return Ok(Source::Synthetic(0));
    }
    if let Some(f) = &o.replay {
        let b = std::fs::read(f).map_err(|e| format!("replay {f}: {e}"))?;
        camera::jpeg_dims(&b)
            .ok_or_else(|| format!("replay {f}: not a JPEG with a start-of-frame"))?;
        return Ok(Source::Replay(b));
    }
    let dev = match &o.device {
        Some(d) => d.clone(),
        None => camera::pick_device(&handed_devices(), &sysfs_video, o.camera_name.as_deref())?,
    };
    // Source buffers at the camera's own rate (30 fps): about 1.5 s, then videorate has kept a few.
    let buffers = o.once.then_some(45);
    Capture::start(
        &dev,
        o.width,
        o.height,
        o.fps,
        data_dir.join("raw"),
        buffers,
    )
    .map(|c| Source::Camera(c, dev))
}

/// One frame arrived: keep it, announce it, count it.
fn take_frame(
    bytes: Vec<u8>,
    o: &Opts,
    store: &FrameStore,
    link: &mut Link,
    win: &mut Window,
    shared: &Shared,
) {
    let Some((w, h)) = camera::jpeg_dims(&bytes) else {
        win.errors += 1;
        return;
    };
    match store.put(&bytes) {
        Ok(hash) => {
            win.frames += 1;
            win.bytes += bytes.len() as u64;
            (win.width, win.height) = (w, h);
            let blob = if o.no_blob {
                None
            } else {
                link.put_blob(&bytes, o.ttl_ms)
            };
            win.blobs += blob.is_some() as u64;
            match link.announce(
                &o.source,
                &Stored {
                    hash: &hash,
                    width: w,
                    height: h,
                    ttl_ms: o.ttl_ms,
                    blob,
                },
            ) {
                Ok(true) => win.refs += 1,
                Ok(false) => {}
                Err(e) => eprintln!("{TAG} announce: {e}"),
            }
            if let Ok(mut st) = shared.lock() {
                st.raw = Some(json!({
                    "hash": hash, "width": w, "height": h, "pixel_format": "mjpeg", "bytes": bytes.len(),
                    "t_ms": unix_ms(), "ttl_ms": o.ttl_ms, "jpeg": "/frame.jpg",
                }));
                st.frame = Some(bytes);
            }
            win.last_hash = Some(hash);
        }
        Err(e) => {
            win.errors += 1;
            eprintln!("{TAG} {e}");
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
    eprintln!(
        "{TAG} start (once={}, fps={}, {}x{}, source={}, simulate={}, bus={})",
        o.once,
        o.fps,
        o.width,
        o.height,
        o.source,
        o.simulate,
        o.bus.as_deref().unwrap_or("none")
    );
    let data_dir = std::env::var("COGNITUM_COG_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::temp_dir().join(format!("cog-camera-{}", std::process::id()))
        });
    let store_dir = o
        .frames_dir
        .clone()
        .unwrap_or_else(|| data_dir.join("frames"));
    let mut store = match FrameStore::new(store_dir, Duration::from_millis(o.ttl_ms)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{TAG} {e}");
            std::process::exit(1);
        }
    };
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
    let period = Duration::from_secs(if o.once { 2 } else { o.interval });
    let mut link = Link::new(if o.simulate && o.once {
        None
    } else {
        o.bus.clone()
    });

    let mut source: Option<Source> = None;
    let mut open_err: Option<String> = None;
    let mut win = Window::default();
    let mut t0 = Instant::now();
    let mut next_synth = Instant::now();
    let frame_every = Duration::from_millis(1000 / o.fps as u64);
    let mut once_done = false;
    loop {
        if source.is_none() {
            match open_source(&o, &data_dir) {
                Ok(s) => {
                    eprintln!("{TAG} capturing from {}", s.label());
                    source = Some(s);
                }
                Err(e) => {
                    open_err = Some(e);
                    std::thread::sleep(period.min(Duration::from_secs(1)));
                }
            }
        }
        match source.as_mut() {
            Some(Source::Camera(c, _)) => {
                if o.once {
                    match c.finish(Duration::from_secs(10)) {
                        Ok(files) => {
                            if let Some(p) = files.last() {
                                match std::fs::read(p) {
                                    Ok(b) => {
                                        take_frame(b, &o, &store, &mut link, &mut win, &shared)
                                    }
                                    Err(e) => open_err = Some(format!("read frame: {e}")),
                                }
                            }
                            if win.frames == 0 {
                                open_err.get_or_insert_with(|| "the camera wrote no frame".into());
                            }
                        }
                        Err(e) => open_err = Some(e),
                    }
                    once_done = true;
                } else {
                    for p in c.poll() {
                        match std::fs::read(&p) {
                            Ok(b) => take_frame(b, &o, &store, &mut link, &mut win, &shared),
                            Err(_) => win.errors += 1,
                        }
                        let _ = std::fs::remove_file(&p);
                        if win.frames > 0 {
                            open_err = None;
                        }
                    }
                    if let Some(st) = c.exited() {
                        open_err = Some(format!("gst-launch-1.0 {st}"));
                        source = None;
                        // Do not respawn a failing pipeline in a tight loop.
                        std::thread::sleep(Duration::from_secs(1));
                    } else {
                        std::thread::sleep(Duration::from_millis(30));
                    }
                }
            }
            Some(Source::Synthetic(n)) => {
                if Instant::now() >= next_synth {
                    next_synth += frame_every;
                    *n += 1;
                    let b = camera::synthetic_jpeg(
                        *n,
                        o.width.min(65535) as u16,
                        o.height.min(65535) as u16,
                    );
                    take_frame(b, &o, &store, &mut link, &mut win, &shared);
                } else {
                    std::thread::sleep(Duration::from_millis(5));
                }
                once_done = o.once && win.frames >= 2;
            }
            Some(Source::Replay(b)) => {
                if Instant::now() >= next_synth {
                    next_synth += frame_every;
                    let b = b.clone();
                    take_frame(b, &o, &store, &mut link, &mut win, &shared);
                } else {
                    std::thread::sleep(Duration::from_millis(5));
                }
                once_done = o.once && win.frames >= 2;
            }
            None => {}
        }
        store.gc();
        if once_done || t0.elapsed() >= period {
            if win.frames == 0
                && !o.once
                && matches!(source, Some(Source::Camera(..)))
                && open_err.is_none()
            {
                open_err = Some(format!(
                    "no frame arrived in the last {} s",
                    period.as_secs()
                ));
            }
            let status = match (&source, open_err.is_some()) {
                (_, true) if win.frames == 0 => "no_source",
                _ if o.simulate => "simulate",
                _ if o.replay.is_some() => "replay",
                _ => "ok",
            };
            let label = source
                .as_ref()
                .map_or_else(|| "none".to_string(), Source::label);
            let secs = t0.elapsed().as_secs_f64();
            let ctx = Context {
                status,
                source: &o.source,
                device: &label,
                secs,
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
            link.telemetry(&o.source, &win, secs);
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
            "--fps",
            "500",
            "--source",
            "Bad Name",
            "--device",
            "/dev/video3",
        ]));
        assert!(o.once);
        assert_eq!(o.fps, 2, "out of range falls back to the default");
        assert_eq!(o.source, "cam0");
        assert_eq!(o.device.as_deref(), Some("/dev/video3"));
        let d = parse_opts(&s(&[]));
        assert_eq!(
            (d.api_bind.as_str(), d.width, d.height, d.ttl_ms),
            ("127.0.0.1:8065", 1280, 720, 10_000)
        );
        assert!(!d.simulate && d.replay.is_none() && d.device.is_none());
    }

    #[test]
    fn the_frame_ref_passes_the_registered_schema_shape() {
        let h = blake3::hash(b"x").to_hex().to_string();
        let f = Stored {
            hash: &h,
            width: 1280,
            height: 720,
            ttl_ms: 10_000,
            blob: None,
        };
        let v = frame_ref("cam0", &"a".repeat(32), 5, 1, &f);
        assert!(
            v.get("blob").is_none(),
            "no handle unless the frame was put in the store"
        );
        let handle = json!({"hash": h, "size": 51200, "media_type": "image/jpeg", "expires_ms": 9});
        let with = Stored {
            blob: Some(handle.clone()),
            ..f
        };
        assert_eq!(
            frame_ref("cam0", &"a".repeat(32), 5, 1, &with)["blob"],
            handle,
            "the handle rides in the reading"
        );
        let f = with;
        assert_eq!(v["hash"].as_str().unwrap().len(), 64);
        assert_eq!(
            (
                v["pixel_format"].as_str(),
                v["width"].as_u64(),
                v["ttl_ms"].as_u64()
            ),
            (Some("mjpeg"), Some(1280), Some(10_000))
        );
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        for k in [
            "source_id",
            "node_id",
            "t_ns",
            "seq",
            "hash",
            "width",
            "height",
            "pixel_format",
            "ttl_ms",
        ] {
            assert!(keys.contains(&k), "{k}");
        }
        let long = Stored {
            ttl_ms: 99_999_999,
            ..f
        };
        assert_eq!(
            frame_ref("c", "n", 0, 1, &long)["ttl_ms"],
            3_600_000,
            "ttl is capped at the schema's limit"
        );
    }

    #[test]
    fn a_window_reports_rate_size_and_the_vector() {
        let win = Window {
            frames: 10,
            bytes: 10 * 51_200,
            refs: 10,
            blobs: 10,
            errors: 1,
            width: 1280,
            height: 720,
            last_hash: Some("ab".repeat(32)),
        };
        let bus = BusState {
            connected: true,
            seq: 10,
            error: None,
        };
        let ctx = Context {
            status: "ok",
            source: "cam0",
            device: "v4l2:/dev/video0",
            secs: 5.0,
            bus: &bus,
            devices: &["/dev/video0".into()],
            error: None,
            export_url: &None,
        };
        let (r, v) = build_report(&win, &ctx);
        assert_eq!(
            (
                r["status"].as_str(),
                r["provenance"].as_str(),
                r["fps"].as_f64()
            ),
            (Some("ok"), Some("MEASURED"), Some(2.0))
        );
        assert_eq!(
            (r["width"].as_u64(), r["mean_frame_kb"].as_f64()),
            (Some(1280), Some(50.0))
        );
        assert_eq!(v, [2.0, 1280.0, 720.0, 50.0, 10.0, 1.0, 1.0, 10.0]);
    }

    #[test]
    fn no_source_reports_the_reason_and_an_empty_vector() {
        let ctx = Context {
            status: "no_source",
            source: "cam0",
            device: "none",
            secs: 2.0,
            bus: &BusState::default(),
            devices: &[],
            error: Some("no video node"),
            export_url: &None,
        };
        let (r, v) = build_report(&Window::default(), &ctx);
        assert_eq!(
            (r["error"].as_str(), r["width"].is_null()),
            (Some("no video node"), true)
        );
        assert_eq!(v, [-1.0; 8]);
    }

    #[test]
    fn telemetry_names_are_schema_valid() {
        for it in telemetry_items(&Window::default(), &BusState::default(), 1.0) {
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
    fn a_taken_frame_is_stored_counted_and_served() {
        let dir = std::env::temp_dir().join(format!("camera-take-{}", std::process::id()));
        let store = FrameStore::new(dir.clone(), Duration::from_secs(60)).unwrap();
        let shared: Shared = Arc::new(Mutex::new(State::default()));
        let (mut link, mut win) = (Link::new(None), Window::default());
        let o = parse_opts(&s(&[]));
        take_frame(
            camera::synthetic_jpeg(1, 640, 480),
            &o,
            &store,
            &mut link,
            &mut win,
            &shared,
        );
        take_frame(vec![1, 2, 3], &o, &store, &mut link, &mut win, &shared);
        assert_eq!(
            (win.frames, win.errors, win.refs, win.width, win.height),
            (1, 1, 0, 640, 480),
            "a non-JPEG is an error, no broker means no ref"
        );
        let h = win.last_hash.clone().unwrap();
        assert!(store.path(&h).exists());
        let st = shared.lock().unwrap();
        assert_eq!(st.raw.as_ref().unwrap()["hash"], h.as_str());
        assert!(st.frame.is_some());
        std::fs::remove_dir_all(&dir).ok();
    }
}
