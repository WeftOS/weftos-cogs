//! Cognitum Cog: npu-consumer
//!
//! A test consumer for the NPU inference service (`npu-infer`, WeftOS ADR-122
//! section 12). It calls `svc.infer/<service>` on the node's cogbus with a
//! cycle of models, one call at a time, and logs every call (model, priority,
//! total/queue/load/run milliseconds, whether it had to load, errors) as JSON
//! lines. With `--watch` it also logs the service's telemetry (queue depth,
//! resident models, evictions, restarts) every few seconds. Two instances with
//! different `--models` make the two-consumer soak. It is a tool for
//! acceptance runs, not something a node needs to run.
//!
//! Usage:
//!   cog-npu-consumer --models gemma3-1b,qwen3-4b --log /data/npu-soak/a.jsonl --watch
//!   cog-npu-consumer --once --simulate     # one synthetic line, exit
//!   cog-npu-consumer --help

mod bus;

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use bus::{Answer, Bus};

const TAG: &str = "[cog-npu-consumer]";

const HELP: &str = "cog-npu-consumer: calls the node's NPU inference service and logs every call

  --service NAME       service name, calls svc.infer/NAME (venq-npu)
  --bus PATH           broker socket (default $WEFTOS_COGBUS); token from $WEFTOS_COGBUS_TOKEN
  --models A,B,...     aliases to cycle through, one per call (gemma3-1b)
  --priority N         0..=9 (4)
  --max-tokens N       tokens per call (24)
  --text TEXT          the prompt
  --pause-ms N         wait between calls (0)
  --timeout-secs N     per-call deadline (180)
  --calls N            stop after N calls (0: no limit)
  --duration-secs N    stop after N seconds (0: no limit)
  --role NAME          label in the log (consumer)
  --log FILE           append JSON lines here as well as stdout
  --watch              also log the service's telemetry
  --watch-every N      seconds between telemetry lines (10)
  --summary-secs N     seconds between summary lines (60)
  --once --simulate    print one synthetic line, exit
";

struct Opts {
    service: String,
    bus: Option<String>,
    models: Vec<String>,
    priority: u8,
    max_tokens: u32,
    text: String,
    pause: Duration,
    timeout: Duration,
    calls: u64,
    duration: Duration,
    role: String,
    log: Option<PathBuf>,
    watch: bool,
    watch_every: Duration,
    summary_every: Duration,
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
    let models: Vec<String> = arg(a, "--models")
        .unwrap_or("gemma3-1b")
        .split(',')
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect();
    Opts {
        service: arg(a, "--service").unwrap_or("venq-npu").to_string(),
        bus: arg(a, "--bus")
            .map(String::from)
            .or_else(|| std::env::var("WEFTOS_COGBUS").ok()),
        models: if models.is_empty() {
            vec!["gemma3-1b".into()]
        } else {
            models
        },
        priority: num(a, "--priority", 4, 0, 9),
        max_tokens: num(a, "--max-tokens", 24, 1, 4096),
        text: arg(a, "--text")
            .unwrap_or("In five words, say something about the number {n}.")
            .to_string(),
        pause: Duration::from_millis(num(a, "--pause-ms", 0, 0, 3_600_000)),
        timeout: Duration::from_secs(num(a, "--timeout-secs", 180, 1, 600)),
        calls: num(a, "--calls", 0, 0, u64::MAX),
        duration: Duration::from_secs(num(a, "--duration-secs", 0, 0, 10_000_000)),
        role: arg(a, "--role").unwrap_or("consumer").to_string(),
        log: arg(a, "--log").map(PathBuf::from),
        watch: a.iter().any(|x| x == "--watch"),
        watch_every: Duration::from_secs(num(a, "--watch-every", 10, 1, 3600)),
        summary_every: Duration::from_secs(num(a, "--summary-secs", 60, 1, 86_400)),
        once: a.iter().any(|x| x == "--once"),
        simulate: a.iter().any(|x| x == "--simulate"),
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

struct Log {
    file: Option<File>,
}

impl Log {
    fn open(p: &Option<PathBuf>) -> Result<Self, String> {
        let file = match p {
            Some(p) => Some(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(p)
                    .map_err(|e| format!("{}: {e}", p.display()))?,
            ),
            None => None,
        };
        Ok(Self { file })
    }

    fn line(&mut self, v: Value) {
        let s = v.to_string();
        println!("{s}");
        if let Some(f) = &mut self.file {
            let _ = writeln!(f, "{s}");
        }
    }
}

#[derive(Default)]
struct Stats {
    ok: u64,
    failed: u64,
    loaded: u64,
    totals_ms: Vec<u64>,
    errors: std::collections::BTreeMap<String, u64>,
}

impl Stats {
    fn pct(&self, p: f64) -> u64 {
        if self.totals_ms.is_empty() {
            return 0;
        }
        let mut v = self.totals_ms.clone();
        v.sort_unstable();
        v[(((v.len() - 1) as f64) * p).round() as usize]
    }

    fn summary(&self, role: &str, elapsed: Duration) -> Value {
        json!({"kind": "summary", "t_ms": unix_ms(), "role": role, "elapsed_s": elapsed.as_secs(),
            "ok": self.ok, "failed": self.failed, "loads": self.loaded,
            "total_ms_p50": self.pct(0.5), "total_ms_p95": self.pct(0.95), "total_ms_max": self.pct(1.0),
            "errors": self.errors})
    }
}

fn items_of(f: &Value) -> Value {
    let get = |n: &str| {
        f["body"]["items"]
            .as_array()
            .and_then(|a| a.iter().find(|i| i["name"] == n))
            .and_then(|i| i["value"].as_f64())
    };
    let loaded: Vec<String> = f["body"]["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| {
            i["name"]
                .as_str()?
                .strip_prefix("model.loaded.")
                .map(|n| format!("{n}={}", i["value"]))
        })
        .collect();
    json!({"kind": "telemetry", "t_ms": unix_ms(), "seq": f["body"]["seq"],
        "queue_depth": get("queue.depth"), "queue_oldest_wait_ms": get("queue.oldest_wait"), "npu_busy": get("npu.busy"),
        "models_resident": get("models.resident"), "mem_resident_mib": get("mem.resident"), "mem_budget_mib": get("mem.budget"),
        "mem_available_mib": get("mem.available"), "sd_busy": get("sd.busy"), "loaded": loaded,
        "served": get("calls.served"), "failed": get("calls.failed"), "rejected": get("calls.rejected"), "expired": get("calls.expired"),
        "loads": get("models.loads"), "evictions": get("models.evictions"), "restarts": get("geniex.restarts")})
}

/// Calls until the stop condition or the connection ends. `true`: finished.
fn session(
    o: &Opts,
    bus: &Bus,
    msgs: &Receiver<Value>,
    log: &mut Log,
    stats: &mut Stats,
    n: &mut u64,
    t0: Instant,
) -> bool {
    let topic = format!("svc.infer/{}", o.service);
    if o.watch {
        if let Answer::Refused(c, m) = bus.subscribe("telemetry/*", "telemetry.v1") {
            log.line(json!({"kind": "warn", "t_ms": unix_ms(), "msg": format!("telemetry subscribe refused: {c}: {m}")}));
        }
    }
    let (mut last_watch, mut last_summary) = (Instant::now() - o.watch_every, Instant::now());
    loop {
        if (o.calls > 0 && *n >= o.calls) || (!o.duration.is_zero() && t0.elapsed() >= o.duration) {
            return true;
        }
        while let Ok(f) = msgs.try_recv() {
            if last_watch.elapsed() >= o.watch_every {
                log.line(items_of(&f));
                last_watch = Instant::now();
            }
        }
        if last_summary.elapsed() >= o.summary_every {
            log.line(stats.summary(&o.role, t0.elapsed()));
            last_summary = Instant::now();
        }
        let model = &o.models[(*n as usize) % o.models.len()];
        let body = json!({"model": model, "priority": o.priority, "max_tokens": o.max_tokens,
            "text": o.text.replace("{n}", &n.to_string())});
        let started = Instant::now();
        let answer = bus.call(&topic, body, o.timeout);
        let total = started.elapsed().as_millis() as u64;
        *n += 1;
        match answer {
            Answer::Ok(v) => {
                stats.ok += 1;
                stats.totals_ms.push(total);
                if v["loaded"] == true {
                    stats.loaded += 1;
                }
                log.line(json!({"kind": "call", "t_ms": unix_ms(), "role": o.role, "n": *n, "model": model, "priority": o.priority,
                    "ok": true, "total_ms": total, "queue_ms": v["queue_ms"], "load_ms": v["load_ms"], "run_ms": v["run_ms"],
                    "loaded": v["loaded"], "tokens": v["usage"]["completion_tokens"]}));
            }
            Answer::Refused(code, msg) => {
                stats.failed += 1;
                *stats.errors.entry(code.clone()).or_default() += 1;
                log.line(json!({"kind": "call", "t_ms": unix_ms(), "role": o.role, "n": *n, "model": model, "priority": o.priority,
                    "ok": false, "total_ms": total, "code": code, "error": msg}));
                if code == "closed" || code == "io" {
                    return false;
                }
                // Nothing serves it yet, or it is busy: do not spin.
                if code == "unavailable" || code == "busy" {
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
        }
        if !o.pause.is_zero() {
            std::thread::sleep(o.pause);
        }
    }
}

fn run(o: &Opts) -> i32 {
    let (Some(sock), Ok(token)) = (o.bus.clone(), std::env::var("WEFTOS_COGBUS_TOKEN")) else {
        eprintln!("{TAG} no cogbus: set WEFTOS_COGBUS and WEFTOS_COGBUS_TOKEN (or --bus)");
        return 2;
    };
    let mut log = match Log::open(&o.log) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{TAG} {e}");
            return 2;
        }
    };
    let (mut stats, mut n, t0) = (Stats::default(), 0u64, Instant::now());
    log.line(json!({"kind": "start", "t_ms": unix_ms(), "role": o.role, "service": o.service, "models": o.models, "priority": o.priority}));
    let mut backoff = Duration::from_secs(1);
    loop {
        match Bus::connect(&sock, &token) {
            Ok((bus, msgs)) => {
                backoff = Duration::from_secs(1);
                if session(o, &bus, &msgs, &mut log, &mut stats, &mut n, t0) {
                    break;
                }
                log.line(json!({"kind": "warn", "t_ms": unix_ms(), "msg": "connection lost; reconnecting"}));
            }
            Err(e) => log.line(json!({"kind": "warn", "t_ms": unix_ms(), "msg": e})),
        }
        if !o.duration.is_zero() && t0.elapsed() >= o.duration {
            break;
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_secs(15));
    }
    log.line(stats.summary(&o.role, t0.elapsed()));
    if stats.ok == 0 && stats.failed > 0 {
        1
    } else {
        0
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
        println!(
            "{}",
            json!({"status": "simulate", "provenance": "SYNTHETIC", "kind": "call", "role": o.role,
            "model": o.models[0], "ok": true, "total_ms": 0, "note": "no call was made"})
        );
        return;
    }
    if o.once || o.simulate {
        eprintln!("{TAG} --once and --simulate go together; see --help");
        std::process::exit(2);
    }
    std::process::exit(run(&o));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_and_the_summary() {
        let mut s = Stats::default();
        for t in [10, 20, 30, 40, 100] {
            s.totals_ms.push(t);
            s.ok += 1;
        }
        assert_eq!((s.pct(0.5), s.pct(0.95), s.pct(1.0)), (30, 100, 100));
        assert_eq!(Stats::default().pct(0.5), 0);
        let v = s.summary("a", Duration::from_secs(3));
        assert_eq!(
            (v["ok"].as_u64(), v["total_ms_p50"].as_u64()),
            (Some(5), Some(30))
        );
    }

    #[test]
    fn options_have_limits_and_a_model_cycle() {
        let a: Vec<String> = ["--models", "a, b,,c", "--priority", "12", "--calls", "5"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let o = parse_opts(&a);
        assert_eq!(o.models, ["a", "b", "c"]);
        assert_eq!(
            (o.priority, o.calls),
            (4, 5),
            "an out-of-range priority falls back"
        );
    }

    #[test]
    fn telemetry_items_are_picked_out() {
        let f = json!({"type": "msg", "body": {"seq": 7, "items": [
            {"name": "queue.depth", "value": 2.0, "unit": ""}, {"name": "model.loaded.qwen3_4b", "value": 4500.0, "unit": "MiB"}]}});
        let v = items_of(&f);
        assert_eq!(
            (v["queue_depth"].as_f64(), v["seq"].as_u64()),
            (Some(2.0), Some(7))
        );
        assert_eq!(v["loaded"][0], "qwen3_4b=4500.0");
        assert!(v["evictions"].is_null());
    }
}
