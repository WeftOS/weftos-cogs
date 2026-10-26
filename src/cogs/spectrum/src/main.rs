//! Cognitum Cog: spectrum
//!
//! A consumer of the PlutoSDR owner's IQ stream (WeftOS Ventuno Q cog plan 4.3, ADR-122). It reads
//! `iq.cs16/<source>` from the node's cogbus, averages Welch power spectra and publishes them as
//! `spectrum.psd.v1` readings, and every minute publishes a band occupancy reading per band the
//! radio visited, with the samples lost on the way counted rather than hidden. Implements the
//! ADR-001 cog-as-plugin contract.
//!
//! Usage:
//!   cog-spectrum                       # read the owner's stream on the node's cogbus
//!   cog-spectrum --once --simulate     # a synthetic band through the whole pipeline, then exit
//!   cog-spectrum --once --replay FILE  # analyse a capture made with sdr-pluto-rx --record
//!   cog-spectrum --help

mod bus;
mod collect;
mod feed;
mod fft;
mod guide;
mod httpd;
mod ingest;
mod iq;
mod live;
mod psd;
mod report;
mod ring;
mod shared;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use collect::{Collector, Config};
use feed::{Band, Replay, Source, Synthetic, RECORD};
use report::{Reporter, MONO};
use shared::Shared;

const TAG: &str = "[cog-spectrum]";
/// Per-cog id used in the store vector tuple.
pub const STORE_ID: u32 = 30;

const HELP: &str = "cog-spectrum: Welch spectrum and band occupancy from the PlutoSDR stream

  --once               analyse --simulate or --replay input, print the report, exit
  --simulate           a synthetic band (SYNTHETIC); --window seconds of it (3)
  --replay FILE        a capture made with sdr-pluto-rx --record
  --source NAME        source id of the owner's iq.cs16/NAME (venq-pluto)
  --service NAME       source id of this cog's readings (default: the source)
  --bus PATH           broker socket (default $WEFTOS_COGBUS); token from $WEFTOS_COGBUS_TOKEN
  --ask-plan SPEC      ask the owner to visit these bands: centre_hz:rate:dwell_ms[:bw[:gain|auto]],...
  --fft N              FFT size, a power of two (1024)
  --bins N             bins in the published spectrum (256)
  --slices N           occupancy slices across the usable band (8)
  --thresh-db X        a bin is busy this far above the frame's median (10)
  --psd-secs N         seconds per spectrum window (2)
  --occ-secs N         seconds per occupancy reading (60)
  --interval N         seconds between stdout reports (60)
  --api-bind ADDR      /status /raw /guide /healthz (127.0.0.1:8064)
  --no-store           do not post the store vector
";

struct Opts {
    once: bool,
    simulate: bool,
    replay: Option<String>,
    window: f64,
    source: String,
    service: String,
    bus: Option<String>,
    ask_plan: Option<String>,
    cfg: Config,
    interval: u64,
    api_bind: String,
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

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c))
}

fn parse_opts(a: &[String]) -> Result<Opts, String> {
    let source = arg(a, "--source").unwrap_or("venq-pluto").to_string();
    let service = arg(a, "--service").map_or_else(|| source.clone(), String::from);
    for (n, v) in [("--source", &source), ("--service", &service)] {
        if !valid_id(v) {
            return Err(format!(
                "{n} {v:?} must be [a-z0-9][a-z0-9._-]*, at most 64 characters"
            ));
        }
    }
    let fft = num(a, "--fft", 1024usize, 64, 16_384);
    if !fft.is_power_of_two() {
        return Err(format!("--fft {fft} is not a power of two"));
    }
    let cfg = Config {
        fft,
        bins_out: num(a, "--bins", 256usize, 16, 512),
        slices: num(a, "--slices", 8usize, 1, 32),
        thresh_db: num(a, "--thresh-db", 10.0f64, 3.0, 40.0),
        psd_ns: num(a, "--psd-secs", 2u64, 1, 3600) * 1_000_000_000,
        occ_ns: num(a, "--occ-secs", 60u64, 1, 86_400) * 1_000_000_000,
        stride: 4,
    };
    Ok(Opts {
        once: a.iter().any(|x| x == "--once"),
        simulate: a.iter().any(|x| x == "--simulate"),
        replay: arg(a, "--replay").map(String::from),
        window: num(a, "--window", 3.0f64, 0.1, 600.0),
        source,
        service,
        bus: arg(a, "--bus")
            .map(String::from)
            .or_else(|| std::env::var("WEFTOS_COGBUS").ok()),
        ask_plan: arg(a, "--ask-plan").map(String::from),
        cfg,
        interval: num(a, "--interval", 60u64, 1, 3600),
        api_bind: arg(a, "--api-bind").unwrap_or("127.0.0.1:8064").to_string(),
        store: !a.iter().any(|x| x == "--no-store"),
    })
}

/// `centre:rate:dwell_ms[:bw[:gain]]` list as the body of a `plan` call; the owner validates it.
fn plan_body(spec: &str) -> Result<Value, String> {
    let mut bands = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let f: Vec<&str> = part.split(':').collect();
        if f.len() < 3 || f.len() > 5 {
            return Err(format!("{part}: want centre:rate:dwell_ms[:bw[:gain]]"));
        }
        let n = |s: &str| {
            s.parse::<f64>()
                .map_err(|_| format!("{part}: bad number {s:?}"))
        };
        let mut b = json!({"centre_hz": n(f[0])?, "rate_sps": n(f[1])?, "dwell_ms": n(f[2])?});
        if let Some(bw) = f.get(3).filter(|s| !s.is_empty()) {
            b["bw_hz"] = json!(n(bw)?);
        }
        if let Some(g) = f.get(4) {
            b["gain_db"] = if *g == "auto" {
                json!("auto")
            } else {
                json!(n(g)?)
            };
        }
        bands.push(b);
    }
    if bands.is_empty() {
        return Err("no bands".into());
    }
    Ok(json!({"method": "plan", "id": "spectrum", "bands": bands}))
}

fn unix_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn no_source(err: &str) -> i32 {
    println!(
        "{}",
        json!({"status": "no_source", "error": err, "timestamp": unix_s()})
    );
    eprintln!("{TAG} no source: {err}");
    0
}

/// Run an offline source through the collector in the stream's own time and report.
fn offline(
    mut src: Box<dyn Source>,
    cfg: &Config,
    provenance: Option<&str>,
    max_secs: f64,
) -> Value {
    let band = Band {
        centre_hz: src.band().centre_hz,
        rate_sps: src.band().rate_sps,
        bw_hz: src.band().bw_hz,
    };
    let mut col = Collector::new(cfg.clone());
    let mut rep = Reporter::new();
    let sink = Shared::default();
    struct Keep(Vec<(String, Value)>);
    impl live::Publish for Keep {
        fn publish(&mut self, kind: &str, _s: &str, _t: u64, body: Value) {
            self.0.push((kind.to_string(), body));
        }
    }
    let mut out = Keep(Vec::new());
    let node = "0".repeat(32);
    let (mut idx, mut records) = (0u64, 0u64);
    let limit = (f64::from(band.rate_sps) * max_secs) as u64;
    while idx < limit {
        let Some(samples) = src.next() else { break };
        let n = samples.len() / 4;
        let h = feed::header(&band, idx, n);
        let t_ns = 1_000_000_000 + idx * 1_000_000_000 / u64::from(band.rate_sps);
        let now = t_ns + n as u64 * 1_000_000_000 / u64::from(band.rate_sps);
        let ev = col.on_record(&h, &samples, t_ns, now);
        live::emit(ev, &node, "offline", &mut rep, &mut out, &sink);
        idx += n as u64;
        records += 1;
        rep.records += 1;
    }
    let end = 1_000_000_000 + idx * 1_000_000_000 / u64::from(band.rate_sps);
    live::emit(col.flush(end), &node, "offline", &mut rep, &mut out, &sink);
    let mut doc = rep.doc(&col, "done", None, cfg);
    doc["stream"]["records"] = json!(records);
    doc["input_samples"] = json!(idx);
    doc["band"] =
        json!({"centre_hz": band.centre_hz, "rate_sps": band.rate_sps, "bw_hz": band.bw_hz});
    doc["psd"] = out
        .0
        .iter()
        .rev()
        .find(|(k, _)| k == live::PSD_KIND)
        .map_or(Value::Null, |(_, b)| b.clone());
    doc["occupancy_reading"] = out
        .0
        .iter()
        .rev()
        .find(|(k, _)| k == live::OCC_KIND)
        .map_or(Value::Null, |(_, b)| b.clone());
    doc["readings"] = json!({"spectrum.psd": out.0.iter().filter(|(k, _)| k == live::PSD_KIND).count(),
                              "band.occupancy": out.0.iter().filter(|(k, _)| k == live::OCC_KIND).count()});
    if let Some(p) = provenance {
        doc["provenance"] = json!(p);
    }
    doc["timestamp"] = json!(unix_s());
    doc
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
    std::process::exit(run(o));
}

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Release);
}

fn run(o: Opts) -> i32 {
    eprintln!(
        "{TAG} start (once={}, simulate={}, source={})",
        o.once, o.simulate, o.source
    );
    if o.once || o.simulate || o.replay.is_some() {
        if !o.once {
            eprintln!("{TAG} --simulate and --replay need --once");
            return 2;
        }
        let (src, prov): (Box<dyn Source>, Option<&str>) = if let Some(f) = &o.replay {
            match Replay::open(f) {
                Ok(r) => (Box::new(r), None),
                Err(e) => return no_source(&e),
            }
        } else if o.simulate {
            (
                Box::new(Synthetic::new(
                    Band {
                        centre_hz: 2_437_000_000,
                        rate_sps: 5_000_000,
                        bw_hz: 4_000_000,
                    },
                    o.window,
                )),
                Some("SYNTHETIC"),
            )
        } else {
            return no_source("--once needs --simulate or --replay");
        };
        let max = if o.simulate { o.window } else { 3600.0 };
        println!("{}", offline(src, &o.cfg, prov, max));
        return 0;
    }
    let (Some(path), Ok(token)) = (o.bus.clone(), std::env::var("WEFTOS_COGBUS_TOKEN")) else {
        return no_source("WEFTOS_COGBUS and WEFTOS_COGBUS_TOKEN are not set");
    };
    let ask_plan = match o.ask_plan.as_deref().map(plan_body).transpose() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{TAG} --ask-plan: {e}");
            return 2;
        }
    };
    let shared = Arc::new(Shared::default());
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
    let sh = shared.clone();
    std::thread::spawn(move || loop {
        if STOP.load(Ordering::Acquire) {
            sh.stop.store(true, Ordering::Release);
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    });
    match httpd::serve(&o.api_bind, shared.clone(), guide::bundle_json) {
        Ok(a) => eprintln!("{TAG} http on {a}"),
        Err(e) => eprintln!("{TAG} http disabled: {e}"),
    }
    spawn_reporter(shared.clone(), o.interval, o.store);
    let live_opts = live::LiveOpts {
        path,
        token,
        source: o.source.clone(),
        service: o.service.clone(),
        ask_plan,
    };
    live::run(
        live_opts,
        o.cfg.clone(),
        shared,
        Reporter::new(),
        Collector::new(o.cfg.clone()),
    );
    let _ = (MONO.now(), RECORD);
    0
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
            if store && doc["last_psd"].is_object() {
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

#[cfg(test)]
mod tests;
