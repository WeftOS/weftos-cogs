//! Cognitum Cog: npu-infer
//!
//! The node's NPU inference service (WeftOS ADR-122 section 12, Ventuno Q cog
//! plan 4.5 and 5.3). It owns GenieX: starts it on loopback, one server per
//! resident model, restarts it through its wrapper when a model fails to
//! load, and is the only client. Other cogs call `svc.infer/<service>` on the
//! node's cogbus with a model id, a prompt (and, for a vision model, one small
//! inline image) and a priority. Calls queue by priority and run one at a
//! time; models stay resident within a memory budget and the least recently
//! used one is evicted to make room. The image service on the same NPU is
//! waited for, never raced. Queue depth, resident models and the counters are
//! published as `telemetry/<service>`.
//!
//! Usage:
//!   cog-npu-infer                         # serve on the broker named by $WEFTOS_COGBUS
//!   cog-npu-infer --once --simulate       # one synthetic call through the scheduler, exit
//!   cog-npu-infer --help

mod b64;
mod backend;
mod bus;
mod catalog;
mod geniex;
mod http;
mod launcher;
mod mem;
mod queue;
mod residency;
mod scheduler;
mod sd;
mod service;
mod sim;
mod telemetry;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use backend::{Backend, BlobRef, BlobSource, Gate};
use catalog::Catalog;
use residency::Residency;
use scheduler::Scheduler;

const TAG: &str = "[cog-npu-infer]";
const KIND: &str = "svc.infer";
const SCHEMA: &str = "infer.v1";

const HELP: &str = "cog-npu-infer: NPU inference service on the node's cogbus

  --service NAME      service name, the source id of svc.infer/NAME (venq-npu)
  --bus PATH          broker socket (default $WEFTOS_COGBUS); token from $WEFTOS_COGBUS_TOKEN
  --geniex-bin PATH   GenieX wrapper (default $HOME/.local/bin/geniex)
  --geniex-home DIR   $HOME for GenieX (default: the wrapper's <home>/.local/bin parent)
  --base-port N       first loopback port for GenieX servers (18181)
  --slots N           most models resident at once, also the port range (3)
  --budget-mib N      memory the resident models may use together (8000)
  --reserve-mib N     MemAvailable that must remain after a load (2048)
  --queue N           calls that may wait (64)
  --catalog FILE      JSON model catalog instead of the built-in Ventuno one
  --state-dir DIR     measured model costs and GenieX logs
  --sd-url HOST:PORT  image service to wait for (127.0.0.1:18700), `none` to skip
  --publish-secs N    telemetry period (5)
  --simulate          serve with a synthetic backend instead of GenieX (SYNTHETIC answers)
  --once --simulate   one synthetic call through the scheduler, print it, exit
";

struct Opts {
    service: String,
    bus: Option<String>,
    geniex_bin: PathBuf,
    geniex_home: Option<PathBuf>,
    base_port: u16,
    slots: u16,
    budget_mib: u64,
    reserve_mib: u64,
    queue: usize,
    catalog: Option<PathBuf>,
    state_dir: Option<PathBuf>,
    sd_url: Option<String>,
    publish_secs: u64,
    once: bool,
    simulate: bool,
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

fn parse_opts(a: &[String]) -> Opts {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    Opts {
        service: arg(a, "--service").unwrap_or("venq-npu").to_string(),
        bus: arg(a, "--bus")
            .map(String::from)
            .or_else(|| std::env::var("WEFTOS_COGBUS").ok()),
        geniex_home: arg(a, "--geniex-home")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from),
        geniex_bin: arg(a, "--geniex-bin")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(home).join(".local/bin/geniex")),
        base_port: num(a, "--base-port", 18181, 1024, 65000),
        slots: num(a, "--slots", 3, 1, 8),
        budget_mib: num(a, "--budget-mib", 8000, 256, 1_000_000),
        reserve_mib: num(a, "--reserve-mib", 2048, 0, 1_000_000),
        queue: num(a, "--queue", 64, 1, 1024),
        catalog: arg(a, "--catalog").map(PathBuf::from),
        state_dir: arg(a, "--state-dir")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from),
        sd_url: match arg(a, "--sd-url") {
            Some("none") => None,
            Some(u) => Some(u.to_string()),
            None => Some("127.0.0.1:18700".into()),
        },
        publish_secs: num(a, "--publish-secs", 5, 1, 3600),
        once: a.iter().any(|x| x == "--once"),
        simulate: a.iter().any(|x| x == "--simulate"),
    }
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

fn load_costs(dir: &Path) -> Vec<(String, u64)> {
    std::fs::read_to_string(dir.join("costs.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .map(|m| {
            m.into_iter()
                .filter_map(|(k, v)| v.as_u64().map(|n| (k, n)))
                .collect()
        })
        .unwrap_or_default()
}

fn save_costs(dir: &Path, costs: &[(String, u64)]) {
    let m: serde_json::Map<String, Value> =
        costs.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    let tmp = dir.join("costs.json.tmp");
    if std::fs::create_dir_all(dir).is_ok()
        && std::fs::write(&tmp, Value::Object(m).to_string()).is_ok()
        && std::fs::rename(&tmp, dir.join("costs.json")).is_err()
    {
        eprintln!("{TAG} could not save costs.json");
    }
}

fn gate(o: &Opts) -> Box<dyn Gate> {
    match &o.sd_url {
        Some(u) => Box::new(sd::SdGate::new(u)),
        None => Box::new(sd::NoGate),
    }
}

/// The broker connection the worker reads blobs through; empty between connections.
struct LiveBus(Arc<Mutex<Option<Arc<bus::Bus>>>>);

impl BlobSource for LiveBus {
    fn fetch(&self, b: &BlobRef) -> Result<Vec<u8>, String> {
        let bus = self
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or("no broker connection")?;
        bus.blob_get(&b.hash, b.size)
    }
}

/// `--once --simulate` has no broker.
struct NoBlobs;

impl BlobSource for NoBlobs {
    fn fetch(&self, _: &BlobRef) -> Result<Vec<u8>, String> {
        Err("no broker in this run".into())
    }
}

fn run_once_simulated(o: &Opts) -> i32 {
    let catalog = Catalog::ventuno();
    let sched = Scheduler::new(o.queue, o.budget_mib);
    let h = sched.run_worker(
        Box::new(sim::Sim),
        Box::new(sd::NoGate),
        Box::new(NoBlobs),
        Residency::new(o.budget_mib, 0, o.slots as usize),
    );
    let (tx, rx) = std::sync::mpsc::channel();
    let body = json!({"model": "qwen3-4b", "text": "ping", "priority": 4});
    service::submit(
        &catalog,
        &sched,
        &body,
        Duration::from_secs(5),
        Box::new(move |r| {
            let _ = tx.send(r);
        }),
    );
    let answer = rx.recv_timeout(Duration::from_secs(5));
    let snap = sched.snapshot();
    sched.shutdown();
    let _ = h.join();
    let it = telemetry::items(&snap, None, None);
    match answer {
        Ok(Ok(a)) => {
            println!(
                "{}",
                json!({"status": "simulate", "provenance": "SYNTHETIC", "answer": a, "telemetry_items": it.len(), "resident": snap.resident.len()})
            );
            0
        }
        other => {
            eprintln!("{TAG} simulated call failed: {other:?}");
            1
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return;
    }
    let o = parse_opts(&args);
    if o.once && o.simulate {
        std::process::exit(run_once_simulated(&o));
    }
    if o.once {
        eprintln!("{TAG} --once runs only with --simulate; see --help");
        std::process::exit(2);
    }
    std::process::exit(serve(&o));
}

fn serve(o: &Opts) -> i32 {
    let Some(sock) = o.bus.clone() else {
        eprintln!("{TAG} no cogbus: set WEFTOS_COGBUS (or --bus); this cog serves calls and has no other job");
        return 2;
    };
    let Ok(token) = std::env::var("WEFTOS_COGBUS_TOKEN") else {
        eprintln!("{TAG} WEFTOS_COGBUS_TOKEN is not set");
        return 2;
    };
    let catalog = match &o.catalog {
        Some(p) => match std::fs::read_to_string(p)
            .map_err(|e| e.to_string())
            .and_then(|t| Catalog::from_json(&t))
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{TAG} {}: {e}", p.display());
                return 2;
            }
        },
        None => Catalog::ventuno(),
    };
    let backend: Box<dyn Backend> = if o.simulate {
        Box::new(sim::Sim)
    } else {
        if !o.geniex_bin.is_file() {
            eprintln!(
                "{TAG} GenieX wrapper not found at {} (--geniex-bin)",
                o.geniex_bin.display()
            );
            return 2;
        }
        Box::new(geniex::GeniexBackend::new(
            Box::new(launcher::WrapperLauncher {
                bin: o.geniex_bin.clone(),
                log_dir: o.state_dir.clone(),
                home: o
                    .geniex_home
                    .clone()
                    .or_else(|| launcher::home_of(&o.geniex_bin))
                    .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
                    .unwrap_or_else(|| PathBuf::from("/")),
            }),
            o.base_port,
            o.slots,
        ))
    };
    let mut res = Residency::new(o.budget_mib, o.reserve_mib, o.slots as usize);
    if let Some(d) = &o.state_dir {
        res.seed_costs(load_costs(d));
    }
    let sched = Scheduler::new(o.queue, o.budget_mib);
    let live: Arc<Mutex<Option<Arc<bus::Bus>>>> = Arc::default();
    let _worker = sched.run_worker(backend, gate(o), Box::new(LiveBus(live.clone())), res);
    if let Some(d) = o.state_dir.clone() {
        let s = sched.clone();
        std::thread::spawn(move || {
            let mut saved = Vec::new();
            loop {
                std::thread::sleep(Duration::from_secs(10));
                let m = s.snapshot().measured;
                if m != saved {
                    save_costs(&d, &m);
                    saved = m;
                }
            }
        });
    }
    eprintln!(
        "{TAG} serving {KIND}/{} on {sock}; GenieX on 127.0.0.1:{}..{}",
        o.service,
        o.base_port,
        o.base_port + o.slots - 1
    );
    let mut backoff = Duration::from_secs(1);
    loop {
        match bus::Bus::connect(&sock, &token) {
            Ok(c) => {
                backoff = Duration::from_secs(1);
                *live.lock().unwrap_or_else(|p| p.into_inner()) = Some(c.bus.clone());
                one_connection(o, &catalog, &sched, c);
                *live.lock().unwrap_or_else(|p| p.into_inner()) = None;
                eprintln!("{TAG} cogbus connection ended; reconnecting");
            }
            Err(e) => eprintln!("{TAG} {e}; retrying in {backoff:?}"),
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_secs(15));
    }
}

/// Serve calls on one connection until it ends.
fn one_connection(o: &Opts, catalog: &Catalog, sched: &Scheduler, c: bus::Connected) {
    let bus::Connected {
        bus,
        calls,
        node_id,
    } = c;
    if let Err(e) = bus.serve(KIND, &o.service, SCHEMA) {
        return eprintln!("{TAG} the broker refused serve: {e}");
    }
    let stop = Arc::new(AtomicBool::new(false));
    {
        let (bus, sched, stop) = (bus.clone(), sched.clone(), stop.clone());
        let (service, period, sd) = (
            o.service.clone(),
            Duration::from_secs(o.publish_secs),
            gate(o),
        );
        std::thread::spawn(move || {
            let mut seq = 0u64;
            while !stop.load(Ordering::Relaxed) {
                seq += 1;
                let items = telemetry::items(
                    &sched.snapshot(),
                    sd.busy(),
                    mem::mem_available_mib(Path::new("/")),
                );
                let body = telemetry::body(&service, &node_id, now_ns(), seq, items);
                if let Err(e) = bus.publish("telemetry", &service, "telemetry.v1", now_ns(), body) {
                    return eprintln!("{TAG} telemetry: {e}");
                }
                let until = std::time::Instant::now() + period;
                while std::time::Instant::now() < until && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        });
    }
    // Ends when the reader thread does: the connection is gone.
    for frame in calls.iter() {
        let Some(call) = frame["call"].as_u64() else {
            continue;
        };
        let timeout = Duration::from_millis(frame["timeout_ms"].as_u64().unwrap_or(30_000));
        let b = bus.clone();
        service::submit(
            catalog,
            sched,
            &frame["body"],
            timeout,
            Box::new(move |r| {
                if let Err(e) = b.reply(call, r) {
                    eprintln!("{TAG} reply to call {call}: {e}");
                }
            }),
        );
    }
    stop.store(true, Ordering::Relaxed);
}
