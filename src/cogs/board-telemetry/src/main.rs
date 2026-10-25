//! Cognitum Cog: board-telemetry
//!
//! Reads a Linux board's own health monitors from sysfs and reports one window at a time:
//! input power / voltage / current from an INA2xx hwmon, fan RPM, the hottest thermal zone overall
//! and per group (cpu, gpu, npu, camera, ddr, other), the 1-minute load and free memory. Every
//! source is found by name; a source that is absent is reported as absent, and a board with no
//! hardware monitor at all reports `no_source`. Implements the ADR-001 cog-as-plugin contract.
//!
//! Usage:
//!   cog-board-telemetry --once                    # one <window>-second capture, report, exit
//!   cog-board-telemetry --interval 5              # continuous, one report every 5 s
//!   cog-board-telemetry --once --simulate         # synthetic values, nothing posted
//!   cog-board-telemetry --once --sysfs-root DIR   # read DIR/sys and DIR/proc (fixture trees)

mod export;
mod guide;
mod ingest;
mod sources;
mod window;

use export::{round, Shared, State};
use sources::{Group, Snapshot};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use window::{Window, SLOTS};

const TAG: &str = "[cog-board-telemetry]";
/// Per-cog id in the store vector tuple (21=ecg, 22=tof, 23=sound, 24=rd-03e, 25=as201,
/// 26=mentra, 27=ld1040c, 28=board-telemetry).
pub(crate) const STORE_ID: u32 = 28;

struct Opts {
    once: bool,
    interval: u64,
    window: u64,
    sample_ms: u64,
    root: PathBuf,
    api_bind: String,
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
    Opts {
        once: a.iter().any(|x| x == "--once"),
        interval: num(a, "--interval", 5, 1, 300),
        window: num(a, "--window", 2, 1, 30),
        sample_ms: num(a, "--sample-ms", 1000, 50, 10_000),
        root: PathBuf::from(arg(a, "--sysfs-root").unwrap_or("/")),
        api_bind: arg(a, "--api-bind").unwrap_or("127.0.0.1:8062").to_string(),
        simulate: a.iter().any(|x| x == "--simulate"),
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn opt(v: Option<f64>, places: i32) -> serde_json::Value {
    v.map_or(serde_json::Value::Null, |x| {
        serde_json::json!(round(x, places))
    })
}

/// One report for a window. `latest` supplies the per-device detail (the last sample); the
/// headline numbers come from `win`, which covers the whole window.
fn build_report(
    win: &Window,
    latest: &Snapshot,
    source: &str,
    simulated: bool,
    export_url: &Option<String>,
) -> (serde_json::Value, [f64; 8]) {
    let has_power = !latest.power.is_empty();
    let has_fan = !latest.fans.is_empty();
    let has_thermal = !latest.zones.is_empty();
    let has_load = win.load1.is_some();
    let has_mem = win.mem_available_mb.is_some();
    let status = if simulated {
        "simulate"
    } else if win.samples > 0 && latest.has_hardware_source() {
        "ok"
    } else {
        "no_source"
    };
    let missing: Vec<&str> = [
        ("power", has_power),
        ("fan", has_fan),
        ("thermal", has_thermal),
        ("load", has_load),
        ("mem", has_mem),
    ]
    .iter()
    .filter(|(_, present)| !present)
    .map(|(n, _)| *n)
    .collect();

    let by_group: serde_json::Map<String, serde_json::Value> = Group::ALL
        .iter()
        .filter_map(|g| {
            win.group_max(*g)
                .map(|t| (g.name().to_string(), serde_json::json!(round(t, 2))))
        })
        .collect();
    let vector = win.vector();

    let mut report = serde_json::json!({
        "status": status,
        "provenance": if simulated { "SYNTHETIC" } else { "MEASURED" },
        "source": source,
        "sources": {
            "power": has_power, "fan": has_fan, "thermal": has_thermal,
            "load": has_load, "mem": has_mem,
        },
        "missing": missing,
        "power_w": opt(win.power_w, 3),
        "voltage_v": opt(win.voltage_v, 3),
        "current_a": opt(win.current_a, 3),
        "fan_rpm": opt(win.fan_rpm, 0),
        "temp_max_c": opt(win.temp_max_c, 2),
        "temp_by_group_c": by_group,
        "load1": opt(win.load1, 2),
        "mem_available_mb": opt(win.mem_available_mb, 0),
        "power_monitors": latest.power.iter().map(|p| serde_json::json!({
            "hwmon": p.hwmon, "name": p.name,
            "power_w": opt(p.power_w, 3), "voltage_v": opt(p.voltage_v, 3),
            "current_a": opt(p.current_a, 3), "power_derived": p.power_derived,
        })).collect::<Vec<_>>(),
        "fans": latest.fans.iter().map(|f| serde_json::json!({
            "hwmon": f.hwmon, "name": f.name, "channel": f.channel, "rpm": round(f.rpm, 0),
        })).collect::<Vec<_>>(),
        "zones": latest.zones.iter().map(|z| serde_json::json!({
            "zone": z.zone, "type": z.kind, "group": z.group.name(), "temp_c": round(z.temp_c, 2),
        })).collect::<Vec<_>>(),
        "zones_unreadable": latest.zones_unreadable,
        "vector_slots": SLOTS,
        "vector": vector,
        "samples": win.samples,
        "export": export_url,
        "timestamp": unix_ms() / 1000,
    });
    if status == "no_source" {
        report["error"] = serde_json::json!(
            "no INA2xx power monitor, fan controller or thermal zone readable; check that the board \
             loads the ina2xx / emc2305 hwmon drivers and that /sys/class/thermal is mounted \
             (or pass --sysfs-root)"
        );
    }
    (report, vector)
}

/// Takes the samples of one window, `sample_ms` apart (no wait after the last one when the
/// process exits right after reporting).
fn collect(o: &Opts, period_s: u64, tick: &mut u64, last_wait: bool) -> Vec<Snapshot> {
    let n = (period_s * 1000 / o.sample_ms).max(1);
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        out.push(if o.simulate {
            sources::simulate(*tick)
        } else {
            sources::read(&o.root)
        });
        *tick += 1;
        if last_wait || i + 1 < n {
            std::thread::sleep(Duration::from_millis(o.sample_ms));
        }
    }
    out
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
        "{TAG} start (once={}, interval={}s, window={}s, sample={}ms, root={}, simulate={})",
        o.once,
        o.interval,
        o.window,
        o.sample_ms,
        o.root.display(),
        o.simulate
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
    let source = if o.simulate {
        "simulated".to_string()
    } else {
        format!("sysfs:{}", o.root.display())
    };

    let (ingest_target, ingest_warning) =
        ingest::target_from_env(std::env::var(ingest::ENV_URL).ok().as_deref());
    if let Some(w) = ingest_warning {
        eprintln!("{TAG} {w}");
    }
    let token = std::env::var(ingest::ENV_TOKEN).ok();
    let period = if o.once { o.window } else { o.interval };
    let mut tick = 0u64;
    loop {
        let snaps = collect(&o, period, &mut tick, !o.once);
        let win = Window::from_snapshots(&snaps);
        let latest = snaps.last().cloned().unwrap_or_default();
        let (report, vector) = build_report(&win, &latest, &source, o.simulate, &export_url);
        println!("{report}");
        if let Ok(mut st) = shared.lock() {
            st.push(report.clone());
        }
        // Synthetic values never go into the real store; a board with no source has no vector.
        if report["status"] == "ok" {
            if let Err(e) = ingest::store_to_seed(&ingest_target, token.as_deref(), &vector) {
                eprintln!("{TAG} store error: {e}");
            }
        } else if report["status"] == "no_source" {
            eprintln!("{TAG} no source: {}", report["error"]);
        }
        if o.once {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn s(v: &[&str]) -> Vec<String> {
        std::iter::once("cog")
            .chain(v.iter().copied())
            .map(String::from)
            .collect()
    }

    fn fixture_snapshot(name: &str) -> (Window, Snapshot) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        let snap = sources::read(&root);
        (Window::from_snapshots(std::slice::from_ref(&snap)), snap)
    }

    #[test]
    fn options_bounded_and_defaulted() {
        let o = parse_opts(&s(&[
            "--once",
            "--interval",
            "99999",
            "--sysfs-root",
            "/x",
            "--sample-ms",
            "60",
        ]));
        assert!(o.once);
        assert_eq!(o.interval, 5, "out of range falls back to the default");
        assert_eq!(o.sample_ms, 60);
        assert_eq!(o.root, PathBuf::from("/x"));
        let d = parse_opts(&s(&[]));
        assert_eq!(d.api_bind, "127.0.0.1:8062");
        assert_eq!(d.root, PathBuf::from("/"));
        assert_eq!(d.window, 2);
        assert!(!d.simulate);
    }

    #[test]
    fn ventuno_window_assembles_the_full_report_and_vector() {
        let (win, snap) = fixture_snapshot("ventuno");
        let (r, v) = build_report(&win, &snap, "sysfs:test", false, &None);
        assert_eq!(r["status"], "ok");
        assert_eq!(r["provenance"], "MEASURED");
        assert_eq!(r["missing"], serde_json::json!([]));
        assert_eq!(r["temp_max_c"], 55.5);
        assert_eq!(r["temp_by_group_c"]["npu"], 47.25);
        assert_eq!(r["temp_by_group_c"]["cpu"], 55.5);
        assert_eq!(r["temp_by_group_c"]["gpu"], 44.0);
        assert_eq!(r["temp_by_group_c"]["camera"], 41.0);
        assert_eq!(r["temp_by_group_c"]["ddr"], 43.0);
        assert_eq!(r["temp_by_group_c"]["other"], 38.0);
        assert_eq!(r["zones_unreadable"], 1);
        assert_eq!(v, [12.345, 12.01, 1.028, 55.5, 47.25, 55.5, 3200.0, 0.85]);
        assert_eq!(r["vector"].as_array().unwrap().len(), 8);
    }

    #[test]
    fn pi_reports_absent_sources_honestly() {
        let (win, snap) = fixture_snapshot("pi");
        let (r, v) = build_report(&win, &snap, "sysfs:test", false, &None);
        assert_eq!(r["status"], "ok");
        assert_eq!(r["missing"], serde_json::json!(["power", "fan"]));
        assert!(r["power_w"].is_null() && r["fan_rpm"].is_null());
        assert_eq!(r["temp_by_group_c"], serde_json::json!({"cpu": 48.31}));
        assert_eq!(v, [-1.0, -1.0, -1.0, 48.312, -1.0, 48.312, -1.0, 0.12]);
    }

    #[test]
    fn empty_tree_is_no_source_with_an_error() {
        let (win, snap) = fixture_snapshot("empty");
        let (r, v) = build_report(&win, &snap, "sysfs:test", false, &None);
        assert_eq!(r["status"], "no_source");
        assert!(r["error"].as_str().unwrap().contains("--sysfs-root"));
        assert!(r["temp_max_c"].is_null());
        assert_eq!(v, [-1.0; 8]);
    }

    #[test]
    fn proc_alone_does_not_make_a_source() {
        let snap = Snapshot {
            load1: Some(0.3),
            mem_available_mb: Some(100.0),
            ..Snapshot::default()
        };
        let win = Window::from_snapshots(std::slice::from_ref(&snap));
        let (r, _) = build_report(&win, &snap, "sysfs:test", false, &None);
        assert_eq!(r["status"], "no_source");
        assert_eq!(r["load1"], 0.3, "load is still reported");
    }

    #[test]
    fn simulated_report_is_marked_synthetic() {
        let snap = sources::simulate(0);
        let win = Window::from_snapshots(std::slice::from_ref(&snap));
        let (r, v) = build_report(&win, &snap, "simulated", true, &None);
        assert_eq!(r["status"], "simulate");
        assert_eq!(r["provenance"], "SYNTHETIC");
        assert!(v.iter().all(|x| *x >= 0.0));
    }
}
