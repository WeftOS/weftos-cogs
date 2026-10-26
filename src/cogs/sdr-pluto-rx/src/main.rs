//! Cognitum Cog: sdr-pluto-rx
//!
//! The owner of the PlutoSDR (Zynq-7010 + AD9363A, run as an AD9364) on a Linux or macOS host
//! (WeftOS Ventuno Q cog plan 4.1, ADR-122). It is the only process that opens the radio: it puts
//! the chip in a known receive-only state, streams `iq.cs16.v1` to the node's cogbus with the
//! samples it did not get counted, publishes `sdr.status`, and serves `svc.sdr` so consumers share
//! the one tuner by band plan. Implements the ADR-001 cog-as-plugin contract.
//!
//! Usage:
//!   cog-sdr-pluto-rx                       # own the radio, serve the node's cogbus
//!   cog-sdr-pluto-rx --once                # one capture window, print the report, exit
//!   cog-sdr-pluto-rx --once --simulate     # synthetic signal, no radio
//!   cog-sdr-pluto-rx --once --replay FILE  # play back a capture made with --record FILE
//!   cog-sdr-pluto-rx --help

mod acquire;
mod bus;
mod guide;
mod httpd;
mod iio;
mod ingest;
mod iq;
mod link;
mod loss;
mod plan;
mod pluto;
mod radio;
mod ring;
mod svc;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use acquire::{Acq, Exit, Out, Settings, Shared};
use plan::{Band, Gain, Plan, Scheduler, DEFAULT_PLAN};
use radio::{
    mono_ns, CaptureHeader, Radio, Recorder, Replay, Sim, DEFAULT_BLOCK, DEFAULT_IIO_BUFFER,
};

const TAG: &str = "[cog-sdr-pluto-rx]";
/// Per-cog id used in the store vector tuple.
pub const STORE_ID: u32 = 29;
const PLUTO_IP: &str = "ip:192.168.2.1";

const HELP: &str =
    "cog-sdr-pluto-rx: owner of the PlutoSDR; IQ stream, status and the tuner service

  --once               capture --window seconds of the first band, print the report, exit
  --window N           seconds for --once (3)
  --interval N         seconds between reports in continuous mode (10)
  --band-plan SPEC     bands that take turns: centre_hz:rate:dwell_ms[:bw[:gain|auto]],...
                       (default 2437e6:5e6:1000)
  --centre-hz N --rate N --bw N --gain DB|auto   a single band instead
  --backend auto|ip|usb   how to reach the radio; auto measures both at start (auto)
  --uri URI            a libiio URI, overriding --backend
  --service NAME       source id of iq.cs16/NAME, sdr.status/NAME, svc.sdr/NAME (venq-pluto)
  --bus PATH           broker socket (default $WEFTOS_COGBUS); token from $WEFTOS_COGBUS_TOKEN
  --block N            samples per ring record (8192)
  --iio-buffer N       samples per libiio read, cut into records (65536)
  --ring-mib N         IQ ring size (16)
  --api-bind ADDR      /status /raw /guide /healthz (127.0.0.1:8063)
  --publish-secs N     sdr.status period (2)
  --simulate           a synthetic signal instead of the radio (SYNTHETIC)
  --sim-speed X        simulator pace, 1 is real time, 0 as fast as read (1)
  --replay FILE        play back a capture instead of the radio
  --record FILE        with --once: keep the window as a capture file
  --loop               replay again from the start at the end of the file
  --no-store           do not post the store vector
";

struct Opts {
    once: bool,
    window: u64,
    interval: u64,
    plan: Vec<Band>,
    backend: String,
    uri: Option<String>,
    service: String,
    bus: Option<String>,
    block: usize,
    iio_buffer: usize,
    ring_mib: u64,
    api_bind: String,
    publish_secs: u64,
    simulate: bool,
    sim_speed: f64,
    replay: Option<String>,
    record: Option<String>,
    loop_replay: bool,
    store: bool,
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

fn parse_opts(a: &[String]) -> Result<Opts, String> {
    let plan = if let Some(spec) = arg(a, "--band-plan") {
        plan::parse_bands(spec)?
    } else {
        let centre = num(a, "--centre-hz", 2_437_000_000.0f64, 70e6, 6e9) as u64;
        let rate = num(a, "--rate", 5_000_000.0f64, 520_833.0, 61.44e6) as u32;
        let mut b = Band::new(centre, rate, 1000);
        if let Some(bw) = arg(a, "--bw").and_then(|v| v.parse::<f64>().ok()) {
            b.bw_hz = bw as u32;
        }
        match arg(a, "--gain") {
            Some("auto") | None => {}
            Some(g) => b.gain = Gain::Manual(g.parse().map_err(|_| format!("bad --gain {g:?}"))?),
        }
        b.check()?;
        vec![b]
    };
    let service = arg(a, "--service").unwrap_or("venq-pluto").to_string();
    let ok = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c);
    if service.is_empty()
        || service.len() > 64
        || !service.chars().all(ok)
        || !service.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return Err(format!(
            "--service {service:?} must be [a-z0-9][a-z0-9._-]*, at most 64 characters"
        ));
    }
    Ok(Opts {
        once: a.iter().any(|x| x == "--once"),
        window: num(a, "--window", 3, 1, 3600),
        interval: num(a, "--interval", 10, 1, 3600),
        plan,
        backend: arg(a, "--backend").unwrap_or("auto").to_string(),
        uri: arg(a, "--uri").map(String::from),
        service,
        bus: arg(a, "--bus")
            .map(String::from)
            .or_else(|| std::env::var("WEFTOS_COGBUS").ok()),
        block: num(a, "--block", DEFAULT_BLOCK, 256, 65_536),
        iio_buffer: num(a, "--iio-buffer", DEFAULT_IIO_BUFFER, 4096, 4_194_304),
        ring_mib: num(a, "--ring-mib", 16, 1, 256),
        api_bind: arg(a, "--api-bind").unwrap_or("127.0.0.1:8063").to_string(),
        publish_secs: num(a, "--publish-secs", 2, 1, 3600),
        simulate: a.iter().any(|x| x == "--simulate"),
        sim_speed: num(a, "--sim-speed", 1.0, 0.0, 10_000.0),
        replay: arg(a, "--replay").map(String::from),
        record: arg(a, "--record").map(String::from),
        loop_replay: a.iter().any(|x| x == "--loop"),
        store: !a.iter().any(|x| x == "--no-store"),
    })
}

/// Output when there is no broker: records and readings go nowhere.
struct NullOut;

impl Out for NullOut {
    fn record(&mut self, _t: u64, _h: &iq::Header, _s: &[u8]) -> Result<u64, String> {
        Ok(0)
    }
    fn status(&mut self, _t: u64, _doc: &Value) {}
}

/// Opens the radio the options name, remembering which backend won the measurement.
struct Opener {
    chosen: Option<(String, String)>,
}

impl Opener {
    fn backend(&mut self, o: &Opts) -> Result<(String, String), String> {
        if let Some(c) = &self.chosen {
            return Ok(c.clone());
        }
        let c = if let Some(u) = &o.uri {
            (u.clone(), format!("{u} (given)"))
        } else {
            match o.backend.as_str() {
                "ip" => (PLUTO_IP.to_string(), format!("{PLUTO_IP} (chosen)")),
                "usb" => ("usb:".to_string(), "usb: (chosen)".to_string()),
                "auto" => {
                    // Linux reaches the network gadget; macOS does not drive it.
                    let order = if cfg!(target_os = "linux") {
                        [PLUTO_IP, "usb:"]
                    } else {
                        ["usb:", PLUTO_IP]
                    };
                    let mut best: Option<(String, f64)> = None;
                    let mut notes = Vec::new();
                    for uri in order {
                        match pluto::measure(uri) {
                            Ok(ms) => {
                                notes.push(format!("{uri} {ms:.1} MS/s"));
                                if best.as_ref().is_none_or(|(_, b)| ms > b * 1.05) {
                                    best = Some((uri.to_string(), ms));
                                }
                            }
                            Err(e) => notes.push(format!("{uri} unavailable ({e})")),
                        }
                    }
                    let (uri, _) = best.ok_or_else(|| format!("no radio: {}", notes.join("; ")))?;
                    (
                        uri.clone(),
                        format!("{uri} (measured: {})", notes.join(", ")),
                    )
                }
                other => return Err(format!("unknown --backend {other:?}")),
            }
        };
        eprintln!("{TAG} backend {}", c.1);
        self.chosen = Some(c.clone());
        Ok(c)
    }

    fn open(&mut self, o: &Opts) -> Result<(Box<dyn Radio>, String, String), String> {
        if o.simulate {
            return Ok((
                Box::new(Sim::new(o.iio_buffer, o.sim_speed)),
                "simulated".into(),
                "simulated receiver".into(),
            ));
        }
        if let Some(f) = &o.replay {
            let r = Replay::open(f, o.sim_speed)?;
            return Ok((Box::new(r), "replay".into(), format!("replay of {f}")));
        }
        let (uri, desc) = self.backend(o)?;
        let p = pluto::Pluto::open(&uri, o.iio_buffer)?;
        let src = p.describe();
        Ok((Box::new(p), desc, src))
    }
}

fn unix_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Release);
}

fn install_signals(shared: Arc<Shared>) {
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        libc::signal(
            libc::SIGINT,
            on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGTERM,
            on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t,
        );
    }
    std::thread::spawn(move || loop {
        if STOP.load(Ordering::Acquire) {
            shared.stop.store(true, Ordering::Release);
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    });
}

/// Prints the status every interval and posts the store vector.
fn spawn_reporter(shared: Arc<Shared>, interval: u64, store: bool) {
    let (target, warn) = ingest::target_from_env(std::env::var(ingest::ENV_URL).ok().as_deref());
    if let Some(w) = warn {
        eprintln!("{TAG} {w}");
    }
    let token = std::env::var(ingest::ENV_TOKEN).ok();
    std::thread::spawn(move || {
        let mut last_err = std::time::Instant::now() - Duration::from_secs(120);
        loop {
            for _ in 0..interval * 10 {
                if shared.stop.load(Ordering::Acquire) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let doc = shared
                .status
                .lock()
                .map(|s| s.clone())
                .unwrap_or(Value::Null);
            if doc.is_null() {
                continue;
            }
            println!("{doc}");
            if store && doc["status"] == "streaming" {
                let v: Vec<f64> = doc["vector"]
                    .as_array()
                    .map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect())
                    .unwrap_or_default();
                if let Err(e) = ingest::store_to_seed(&target, token.as_deref(), &v) {
                    if last_err.elapsed() > Duration::from_secs(60) {
                        eprintln!("{TAG} store error: {e}");
                        last_err = std::time::Instant::now();
                    }
                }
            }
        }
    });
}

fn no_source(err: &str) -> Value {
    let r = json!({"status": "no_source", "error": err, "timestamp": unix_s()});
    println!("{r}");
    eprintln!("{TAG} no source: {err}");
    r
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return;
    }
    let o = match parse_opts(&args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{TAG} {e}");
            std::process::exit(2);
        }
    };
    eprintln!(
        "{TAG} start (once={}, bands={}, simulate={}, service={})",
        o.once,
        o.plan.len(),
        o.simulate,
        o.service
    );
    std::process::exit(run(o));
}

fn run(mut o: Opts) -> i32 {
    let shared = Arc::new(Shared::default());
    install_signals(shared.clone());
    let (cmd_tx, cmd_rx) = channel();
    let mut opener = Opener { chosen: None };

    // A replay has one tuning, the capture's.
    if let Some(f) = &o.replay {
        match Replay::open(f, 0.0) {
            Ok(r) => {
                let mut b = Band::new(r.head.centre_hz, r.head.rate_sps, 1000);
                b.bw_hz = r.head.bw_hz;
                o.plan = vec![b];
                o.iio_buffer = r.head.block as usize;
                o.block = o.block.min(o.iio_buffer);
            }
            Err(e) => {
                no_source(&e);
                return 0;
            }
        }
    }
    let mut sched = Scheduler::new();
    let bands = if o.once {
        vec![o.plan[0].clone()]
    } else {
        o.plan.clone()
    };
    if let Err(e) = sched.set_plan(Plan {
        id: DEFAULT_PLAN.into(),
        owner: "@cog".into(),
        bands,
    }) {
        eprintln!("{TAG} {e}");
        return 2;
    }

    let link = Arc::new(link::Link::default());
    let out: Box<dyn Out> = match (&o.bus, std::env::var("WEFTOS_COGBUS_TOKEN")) {
        (Some(path), Ok(token)) if !o.once => {
            link::spawn(
                link::LinkOpts {
                    path: path.clone(),
                    token,
                    service: o.service.clone(),
                    max_record: link::max_record_for(o.block),
                    ring_bytes: o.ring_mib << 20,
                },
                link.clone(),
                cmd_tx.clone(),
                shared.clone(),
            );
            Box::new(link::BusOut::new(link.clone(), &o.service))
        }
        (Some(_), Err(_)) if !o.once => {
            eprintln!("{TAG} WEFTOS_COGBUS is set but WEFTOS_COGBUS_TOKEN is not; running without the bus");
            Box::new(NullOut)
        }
        _ => Box::new(NullOut),
    };
    let max_blocks = o
        .once
        .then(|| (u64::from(o.plan[0].rate_sps) * o.window).div_ceil(o.iio_buffer as u64));
    let set = Settings {
        source: String::new(),
        backend: String::new(),
        status_period: Duration::from_secs(1),
        hw_period: Duration::from_secs(2),
        publish_period: Duration::from_secs(o.publish_secs),
        loss: loss::Params::default(),
        max_blocks,
        raw_samples: 65_536,
        record_samples: o.block,
    };
    let mut acq = Acq::new(sched, set, out, shared.clone(), cmd_rx);
    if !o.once {
        match httpd::serve(&o.api_bind, shared.clone(), guide::bundle_json) {
            Ok(a) => eprintln!("{TAG} http on {a}"),
            Err(e) => eprintln!("{TAG} http disabled: {e}"),
        }
        spawn_reporter(
            shared.clone(),
            o.interval,
            o.store && !o.simulate && o.replay.is_none(),
        );
    }

    let mut backoff = 1u64;
    loop {
        if shared.stop.load(Ordering::Acquire) {
            return 0;
        }
        let (mut radio, backend, source) = match opener.open(&o) {
            Ok(r) => r,
            Err(e) => {
                if o.once {
                    no_source(&e);
                    return 0;
                }
                acq.set_state("no_source", Some(e.clone()));
                eprintln!("{TAG} no source: {e}; retrying in {backoff} s");
                for _ in 0..backoff * 10 {
                    acq.service();
                    if shared.stop.load(Ordering::Acquire) {
                        return 0;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                backoff = (backoff * 2).min(15);
                continue;
            }
        };
        backoff = 1;
        acq.set.source = source;
        acq.set.backend = backend;
        if let Some(path) = o.record.as_deref().filter(|_| o.once) {
            let b = &o.plan[0];
            let head = CaptureHeader {
                centre_hz: b.centre_hz,
                rate_sps: b.rate_sps,
                bw_hz: b.bw_hz,
                block: o.iio_buffer as u32,
            };
            match Recorder::create(path, &head) {
                Ok(r) => acq.record_to(r),
                Err(e) => eprintln!("{TAG} {e}"),
            }
        }
        match acq.run(radio.as_mut()) {
            Exit::Stop => return 0,
            Exit::Done | Exit::Eof if o.once || o.replay.is_some() && !o.loop_replay => {
                let mut doc = acq.status_doc(mono_ns());
                doc["window_s"] = json!(o.window);
                if o.simulate {
                    doc["provenance"] = json!("SYNTHETIC");
                }
                doc["timestamp"] = json!(unix_s());
                println!("{doc}");
                if o.record.is_some() && o.once {
                    eprintln!(
                        "{TAG} capture written to {}",
                        o.record.as_deref().unwrap_or("")
                    );
                }
                return 0;
            }
            Exit::Done | Exit::Eof => {}
            Exit::Lost(e) => {
                acq.totals.disconnects += 1;
                eprintln!("{TAG} the radio went away: {e}; waiting");
                acq.set_state("no_source", Some(format!("radio lost: {e}")));
                if o.once {
                    no_source(&e);
                    return 0;
                }
            }
        }
        drop(radio);
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn options_are_bounded_and_defaulted() {
        let o = parse_opts(&s(&[])).unwrap();
        assert_eq!(
            (o.plan.len(), o.plan[0].centre_hz, o.plan[0].rate_sps),
            (1, 2_437_000_000, 5_000_000)
        );
        assert_eq!(
            (o.block, o.ring_mib, o.api_bind.as_str(), o.service.as_str()),
            (8192, 16, "127.0.0.1:8063", "venq-pluto")
        );
        assert!(o.store && !o.once && o.backend == "auto");
        let o = parse_opts(&s(&["--block", "1", "--ring-mib", "9999", "--window", "0"])).unwrap();
        assert_eq!((o.block, o.ring_mib, o.window), (8192, 16, 3));
    }

    #[test]
    fn a_band_plan_or_a_single_band_can_be_given() {
        let o = parse_opts(&s(&["--band-plan", "2437e6:5e6:2000,1090e6:2.4e6:1000"])).unwrap();
        assert_eq!(o.plan.len(), 2);
        let o = parse_opts(&s(&[
            "--centre-hz",
            "433.92e6",
            "--rate",
            "2e6",
            "--gain",
            "40",
        ]))
        .unwrap();
        assert_eq!(
            (o.plan[0].centre_hz, o.plan[0].rate_sps, o.plan[0].gain),
            (433_920_000, 2_000_000, Gain::Manual(40.0))
        );
        assert!(parse_opts(&s(&["--band-plan", "9000e6:5e6:100"])).is_err());
        assert!(parse_opts(&s(&["--gain", "loud"])).is_err());
    }
}
