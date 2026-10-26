//! Cognitum Cog: mcu-bridge
//!
//! Owns the board's arduino-router socket (`/run/arduino-router.sock`) and the WeftOS sketch
//! on the MCU behind it (WeftOS Ventuno cog plan 5.1; brick spike decision: WeftOS owns the
//! MCU). Serves the sketch to other cogs as the cogbus service `svc.mcu/<service>`
//! (`mcu.call.v1`: a method and up to eight scalar parameters, answered with the sketch's
//! return value). Checks on every interval that the sketch answers `weftos_ping` with 42 and
//! reports `mcu: foreign-sketch` if it does not, which means an App Lab app has replaced it.
//!
//! Usage:
//!   cog-mcu-bridge                        # serve svc.mcu/venq-mcu on $WEFTOS_COGBUS
//!   cog-mcu-bridge --once                 # probe the sketch once, report, exit
//!   cog-mcu-bridge --once --simulate      # a built-in fake router, nothing posted
//!   cog-mcu-bridge --once --replay FILE   # answers from JSON lines {"method":..,"result":..}
//!   cog-mcu-bridge --socket PATH          # name the router socket by hand (bench run)

mod bus;
mod export;
mod guide;
mod ingest;
mod msgpack;
mod router;

use export::{round, Shared, State};
use router::{FakeRouter, Router, RpcError, SocketRouter};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TAG: &str = "[cog-mcu-bridge]";
/// Per-cog id in the store vector tuple (31=camera).
pub(crate) const STORE_ID: u32 = 32;
const KIND: &str = "svc.mcu";
const SCHEMA: &str = "mcu.call.v1";
const PING_ANSWER: i64 = 42;
const MAX_METHOD: usize = 64;
const MAX_PARAMS: usize = 8;
const DEFAULT_ALLOW: &str = "weftos_ping,weftos_calls";

#[derive(Debug, Clone, PartialEq)]
struct Opts {
    once: bool,
    interval: u64,
    service: String,
    socket: Option<String>,
    allow: Vec<String>,
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

fn valid_method(m: &str) -> bool {
    let b = m.as_bytes();
    !b.is_empty()
        && b.len() <= MAX_METHOD
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_.".contains(c))
}

fn parse_opts(a: &[String]) -> Opts {
    let service = match arg(a, "--service") {
        Some(s) if valid_source(s) => s.to_string(),
        Some(_) => {
            eprintln!("{TAG} --service must match [a-z0-9][a-z0-9._-]*, using venq-mcu");
            "venq-mcu".into()
        }
        None => "venq-mcu".into(),
    };
    let allow: Vec<String> = arg(a, "--allow")
        .unwrap_or(DEFAULT_ALLOW)
        .split(',')
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .filter(|m| {
            let ok = valid_method(m);
            if !ok {
                eprintln!("{TAG} --allow: {m:?} is not a method name, dropped");
            }
            ok
        })
        .map(String::from)
        .collect();
    Opts {
        once: a.iter().any(|x| x == "--once"),
        interval: num(a, "--interval", 5, 1, 300),
        service,
        socket: arg(a, "--socket")
            .filter(|s| !s.is_empty())
            .map(String::from),
        allow,
        bus: arg(a, "--bus")
            .map(String::from)
            .or_else(|| std::env::var("WEFTOS_COGBUS").ok()),
        api_bind: arg(a, "--api-bind").unwrap_or("127.0.0.1:8066").to_string(),
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

/// The socket the host handed the owner, else the router's default.
fn pick_socket(devices: &[String]) -> String {
    devices
        .iter()
        .find(|d| d.ends_with(".sock") && d.contains("arduino-router"))
        .cloned()
        .unwrap_or_else(|| router::DEFAULT_SOCKET.to_string())
}

fn handed_devices() -> Vec<String> {
    std::env::var("WEFTOS_DEVICES")
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

/// What the last probe found.
#[derive(Debug, Clone, PartialEq)]
struct Mcu {
    /// `ok`, `foreign-sketch`, `no_router` or `no_answer`.
    state: &'static str,
    ping_ms: Option<f64>,
    sketch_calls: Option<u64>,
    error: Option<String>,
}

/// Ask the sketch who it is.
fn probe(r: &mut dyn Router) -> Mcu {
    let t = Instant::now();
    let ping = r.call("weftos_ping", &[], Duration::from_secs(2));
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let mcu = |state, error: Option<String>| Mcu {
        state,
        ping_ms: Some(ms),
        sketch_calls: None,
        error,
    };
    match ping {
        Ok(v) if v.as_i64() == Some(PING_ANSWER) => {
            let calls = r.call("weftos_calls", &[], Duration::from_secs(2)).ok().and_then(|v| v.as_u64());
            Mcu { sketch_calls: calls, ..mcu("ok", None) }
        }
        Ok(v) => mcu("foreign-sketch", Some(format!("weftos_ping returned {v}, not {PING_ANSWER}"))),
        Err(RpcError::Remote(e)) => mcu(
            "foreign-sketch",
            Some(format!("the MCU does not answer weftos_ping ({e}); another sketch is running, flash the WeftOS sketch (guide: sketch)")),
        ),
        Err(RpcError::Timeout) => mcu("no_answer", Some("the router is there but the MCU did not answer weftos_ping in time".into())),
        Err(e) => Mcu { state: "no_router", ping_ms: None, sketch_calls: None, error: Some(e.to_string()) },
    }
}

#[derive(Debug, Default, Clone)]
struct Counters {
    served: u64,
    failed: u64,
    refused: u64,
}

/// Check a call body against `mcu.call.v1` and the allowlist.
fn validate_call(body: &Value, allow: &[String]) -> Result<(String, Vec<Value>), String> {
    let obj = body
        .as_object()
        .ok_or("the request must be a JSON object")?;
    let method = obj
        .get("method")
        .and_then(Value::as_str)
        .ok_or("method is required")?;
    if !valid_method(method) {
        return Err(format!(
            "method must be [a-z][a-z0-9_.]* of at most {MAX_METHOD} chars"
        ));
    }
    let params: Vec<Value> = match obj.get("params") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a.clone(),
        Some(_) => return Err("params must be an array".into()),
    };
    if params.len() > MAX_PARAMS {
        return Err(format!("params holds more than {MAX_PARAMS} entries"));
    }
    if let Some(i) = params
        .iter()
        .position(|p| !(p.is_number() || p.is_boolean() || p.is_string()))
    {
        return Err(format!("params[{i}] must be a number, boolean or string"));
    }
    if !allow.iter().any(|m| m == method) {
        return Err(format!(
            "method {method} is not allowed on this bridge; allowed: {}",
            allow.join(", ")
        ));
    }
    Ok((method.to_string(), params))
}

/// Run one service call against the router.
fn handle_call(
    r: &mut dyn Router,
    allow: &[String],
    body: &Value,
    timeout_ms: u64,
    c: &mut Counters,
) -> Result<Value, String> {
    let (method, params) = match validate_call(body, allow) {
        Ok(x) => x,
        Err(m) => {
            c.refused += 1;
            return Err(m);
        }
    };
    let t = Instant::now();
    match r.call(
        &method,
        &params,
        Duration::from_millis(timeout_ms.clamp(50, 5000)),
    ) {
        Ok(v) => {
            c.served += 1;
            Ok(json!({"result": v, "ms": round(t.elapsed().as_secs_f64() * 1000.0, 2)}))
        }
        Err(e) => {
            c.failed += 1;
            Err(e.to_string())
        }
    }
}

#[derive(Debug, Default, Clone)]
struct BusState {
    connected: bool,
    serving: bool,
    error: Option<String>,
}

/// What a report says besides the counters.
struct Context<'a> {
    status: &'a str,
    router: &'a str,
    service: &'a str,
    mcu: &'a Mcu,
    counters: &'a Counters,
    bus: &'a BusState,
    allow: &'a [String],
    devices: &'a [String],
    export_url: &'a Option<String>,
}

fn build_report(c: &Context) -> (Value, [f64; 8]) {
    let have = c.status != "no_source";
    let vector = if have {
        [
            c.mcu.ping_ms.map_or(-1.0, |m| round(m, 2)),
            c.mcu.sketch_calls.map_or(-1.0, |n| n as f64),
            c.counters.served as f64,
            c.counters.failed as f64,
            c.counters.refused as f64,
            (c.mcu.state == "ok") as u8 as f64,
            c.bus.serving as u8 as f64,
            1.0,
        ]
    } else {
        [-1.0; 8]
    };
    let mut r = json!({
        "status": c.status,
        "provenance": match c.status { "simulate" => "SYNTHETIC", "replay" => "REPLAY", _ => "MEASURED" },
        "router": c.router,
        "mcu": c.mcu.state,
        "ping_ms": c.mcu.ping_ms.map(|m| round(m, 2)),
        "sketch_calls": c.mcu.sketch_calls,
        "service": format!("{KIND}/{}", c.service),
        "allowed_methods": c.allow,
        "calls_served": c.counters.served,
        "calls_failed": c.counters.failed,
        "calls_refused": c.counters.refused,
        "devices": c.devices,
        "bus": { "connected": c.bus.connected, "serving": c.bus.serving, "error": c.bus.error },
        "vector_slots": ["ping_ms", "sketch_calls", "calls_served", "calls_failed", "calls_refused", "mcu_ok", "serving", "router_present"],
        "vector": vector,
        "export": c.export_url,
        "timestamp": unix_ms() / 1000,
    });
    if let Some(e) = &c.mcu.error {
        r["error"] = json!(e);
    }
    (r, vector)
}

fn telemetry_items(mcu: &Mcu, c: &Counters, bus: &BusState) -> Vec<Value> {
    let i = |n: &str, v: f64, u: &str| json!({"name": n, "value": v, "unit": u});
    vec![
        i("mcu.ok", (mcu.state == "ok") as u8 as f64, ""),
        i("ping.latency", mcu.ping_ms.unwrap_or(0.0), "ms"),
        i("sketch.calls", mcu.sketch_calls.unwrap_or(0) as f64, ""),
        i("calls.served", c.served as f64, ""),
        i("calls.failed", c.failed as f64, ""),
        i("calls.refused", c.refused as f64, ""),
        i("bus.serving", bus.serving as u8 as f64, ""),
    ]
}

/// The broker side: connects when it can, serves the service, hands out the calls.
struct Link {
    path: Option<String>,
    service: String,
    bus: Option<bus::Bus>,
    calls: Option<std::sync::mpsc::Receiver<bus::Call>>,
    state: BusState,
    tseq: u64,
    last_try: Option<Instant>,
}

impl Link {
    fn new(path: Option<String>, service: &str) -> Self {
        Self {
            path,
            service: service.into(),
            bus: None,
            calls: None,
            state: BusState::default(),
            tseq: 0,
            last_try: None,
        }
    }

    fn connect(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        if self.bus.as_ref().is_some_and(|b| b.is_closed()) {
            self.bus = None;
            self.calls = None;
            self.state = BusState::default();
        }
        if self.bus.is_some()
            || self
                .last_try
                .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        self.last_try = Some(Instant::now());
        let token = std::env::var("WEFTOS_COGBUS_TOKEN").unwrap_or_default();
        let r = bus::Bus::connect(&path, &token).and_then(|b| {
            b.serve(KIND, &self.service, SCHEMA)?;
            Ok(b)
        });
        match r {
            Ok(b) => {
                eprintln!(
                    "{TAG} serving {KIND}/{} on {path} (node {})",
                    self.service, b.node_id
                );
                self.calls = b.take_calls();
                self.state = BusState {
                    connected: true,
                    serving: true,
                    error: None,
                };
                self.bus = Some(b);
            }
            Err(e) => {
                self.state.error = Some(e.clone());
                eprintln!("{TAG} bus: {e}");
            }
        }
    }

    /// The next pushed call, waiting up to `wait`.
    fn next_call(&mut self, wait: Duration) -> Option<bus::Call> {
        self.connect();
        match self.calls.as_ref() {
            Some(rx) => rx.recv_timeout(wait).ok(),
            None => {
                std::thread::sleep(wait);
                None
            }
        }
    }

    fn answer(&self, call: u64, r: Result<Value, String>) {
        let Some(b) = &self.bus else { return };
        if let Err(e) = b.reply(call, r) {
            eprintln!("{TAG} reply: {e}");
        }
    }

    fn telemetry(&mut self, mcu: &Mcu, c: &Counters) {
        self.tseq += 1;
        let Some(b) = self.bus.as_ref().filter(|b| !b.is_closed()) else {
            return;
        };
        let body = json!({
            "source_id": self.service, "node_id": b.node_id, "t_ns": bus::wall_ns(),
            "seq": self.tseq, "items": telemetry_items(mcu, c, &self.state),
        });
        if let Err(e) = b.publish(
            "telemetry",
            &self.service,
            "telemetry.v1",
            bus::wall_ns(),
            body,
        ) {
            eprintln!("{TAG} telemetry: {e}");
        }
    }
}

fn open_router(o: &Opts, devices: &[String]) -> Result<Box<dyn Router>, String> {
    if o.simulate {
        return Ok(Box::new(FakeRouter::simulated()));
    }
    if let Some(f) = &o.replay {
        let text = std::fs::read_to_string(f).map_err(|e| format!("replay {f}: {e}"))?;
        return FakeRouter::replay(&text).map(|r| Box::new(r) as Box<dyn Router>);
    }
    Ok(Box::new(SocketRouter::new(
        &o.socket.clone().unwrap_or_else(|| pick_socket(devices)),
    )))
}

fn status_of(o: &Opts, mcu: &Mcu) -> &'static str {
    match mcu.state {
        "ok" if o.simulate => "simulate",
        "ok" if o.replay.is_some() => "replay",
        "ok" => "ok",
        "foreign-sketch" => "foreign_sketch",
        _ => "no_source",
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
    let devices = handed_devices();
    eprintln!(
        "{TAG} start (once={}, interval={}s, service={KIND}/{}, simulate={}, bus={})",
        o.once,
        o.interval,
        o.service,
        o.simulate,
        o.bus.as_deref().unwrap_or("none")
    );
    let mut router = match open_router(&o, &devices) {
        Ok(r) => r,
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
    let mut link = Link::new(if o.once { None } else { o.bus.clone() }, &o.service);
    let mut counters = Counters::default();
    let mut next_report = Instant::now();
    loop {
        if Instant::now() >= next_report {
            next_report = Instant::now() + Duration::from_secs(o.interval);
            let mcu = probe(router.as_mut());
            let status = status_of(&o, &mcu);
            let ctx = Context {
                status,
                router: &router.label(),
                service: &o.service,
                mcu: &mcu,
                counters: &counters,
                bus: &link.state,
                allow: &o.allow,
                devices: &devices,
                export_url: &export_url,
            };
            let (report, vector) = build_report(&ctx);
            println!("{report}");
            if let Ok(mut st) = shared.lock() {
                st.raw = Some(
                    json!({"mcu": mcu.state, "ping_ms": mcu.ping_ms, "sketch_calls": mcu.sketch_calls, "t_ms": unix_ms()}),
                );
                st.push(report.clone());
            }
            link.telemetry(&mcu, &counters);
            match status {
                "ok" => {
                    if let Err(e) = ingest::store_to_seed(&target, token.as_deref(), &vector) {
                        eprintln!("{TAG} store error: {e}");
                    }
                }
                "no_source" | "foreign_sketch" => eprintln!("{TAG} {status}: {}", report["error"]),
                _ => {}
            }
            if o.once {
                return;
            }
        }
        if let Some(call) = link.next_call(Duration::from_millis(100)) {
            let r = handle_call(
                router.as_mut(),
                &o.allow,
                &call.body,
                call.timeout_ms,
                &mut counters,
            );
            link.answer(call.call, r);
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

    fn allow() -> Vec<String> {
        vec!["weftos_ping".into(), "weftos_calls".into()]
    }

    #[test]
    fn options_bounded_and_defaulted() {
        let o = parse_opts(&s(&[
            "--once",
            "--interval",
            "0",
            "--service",
            "Bad Name",
            "--allow",
            "a.b, Bad,c_d,,",
        ]));
        assert!(o.once);
        assert_eq!((o.interval, o.service.as_str()), (5, "venq-mcu"));
        assert_eq!(o.allow, ["a.b", "c_d"], "bad method names are dropped");
        let d = parse_opts(&s(&[]));
        assert_eq!((d.api_bind.as_str(), d.allow), ("127.0.0.1:8066", allow()));
    }

    #[test]
    fn the_handed_router_socket_is_used_and_the_default_otherwise() {
        assert_eq!(
            pick_socket(&["/dev/video0".into(), "/run/arduino-router.sock".into()]),
            "/run/arduino-router.sock"
        );
        assert_eq!(pick_socket(&[]), router::DEFAULT_SOCKET);
    }

    #[test]
    fn a_probe_tells_our_sketch_from_a_foreign_one_and_from_no_router() {
        let mut f = FakeRouter::simulated();
        let m = probe(&mut f);
        assert_eq!((m.state, m.sketch_calls, m.error), ("ok", Some(1), None));
        let mut other = FakeRouter::replay("{\"method\":\"blink\",\"result\":0}").unwrap();
        let m = probe(&mut other);
        assert_eq!(m.state, "foreign-sketch");
        assert!(m.error.unwrap().contains("another sketch is running"));
        let mut wrong = FakeRouter::replay("{\"method\":\"weftos_ping\",\"result\":7}").unwrap();
        assert!(probe(&mut wrong).error.unwrap().contains("returned 7"));
        let m = probe(&mut SocketRouter::new("/nonexistent/r.sock"));
        assert_eq!((m.state, m.ping_ms), ("no_router", None));
    }

    #[test]
    fn a_call_is_validated_then_forwarded_and_counted() {
        let (mut r, mut c) = (FakeRouter::simulated(), Counters::default());
        let ok = handle_call(
            &mut r,
            &allow(),
            &json!({"method": "weftos_ping"}),
            1000,
            &mut c,
        )
        .unwrap();
        assert_eq!(ok["result"], 42);
        assert_eq!(
            handle_call(
                &mut r,
                &allow(),
                &json!({"method": "weftos_calls", "params": []}),
                1000,
                &mut c
            )
            .unwrap()["result"],
            1
        );
        for (bad, why) in [
            (
                json!({"method": "gpio.write", "params": [13, true]}),
                "not allowed",
            ),
            (json!({"method": "Bad"}), "method must be"),
            (json!({"params": []}), "method is required"),
            (
                json!({"method": "weftos_ping", "params": [{"a": 1}]}),
                "must be a number",
            ),
            (
                json!({"method": "weftos_ping", "params": [1, 2, 3, 4, 5, 6, 7, 8, 9]}),
                "more than 8",
            ),
            (
                json!({"method": "weftos_ping", "params": 5}),
                "must be an array",
            ),
            (json!("x"), "JSON object"),
        ] {
            let e = handle_call(&mut r, &allow(), &bad, 1000, &mut c).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
        }
        let wide = vec!["nope".to_string()];
        assert!(
            handle_call(&mut r, &wide, &json!({"method": "nope"}), 1000, &mut c).is_err(),
            "the router's error comes back"
        );
        assert_eq!((c.served, c.refused, c.failed), (2, 7, 1));
    }

    #[test]
    fn a_report_carries_the_mcu_state_counters_and_vector() {
        let mcu = Mcu {
            state: "ok",
            ping_ms: Some(2.5),
            sketch_calls: Some(9),
            error: None,
        };
        let c = Counters {
            served: 4,
            failed: 1,
            refused: 2,
        };
        let bus = BusState {
            connected: true,
            serving: true,
            error: None,
        };
        let ctx = Context {
            status: "ok",
            router: "router:/run/arduino-router.sock",
            service: "venq-mcu",
            mcu: &mcu,
            counters: &c,
            bus: &bus,
            allow: &allow(),
            devices: &[],
            export_url: &None,
        };
        let (r, v) = build_report(&ctx);
        assert_eq!(
            (
                r["mcu"].as_str(),
                r["service"].as_str(),
                r["calls_refused"].as_u64()
            ),
            (Some("ok"), Some("svc.mcu/venq-mcu"), Some(2))
        );
        assert_eq!(v, [2.5, 9.0, 4.0, 1.0, 2.0, 1.0, 1.0, 1.0]);
        let bad = Mcu {
            state: "no_router",
            ping_ms: None,
            sketch_calls: None,
            error: Some("router unavailable".into()),
        };
        let ctx = Context {
            status: "no_source",
            mcu: &bad,
            ..ctx
        };
        let (r, v) = build_report(&ctx);
        assert_eq!(
            (r["error"].as_str(), v),
            (Some("router unavailable"), [-1.0; 8])
        );
    }

    #[test]
    fn statuses_follow_the_probe() {
        let o = parse_opts(&s(&[]));
        let m = |state| Mcu {
            state,
            ping_ms: None,
            sketch_calls: None,
            error: None,
        };
        assert_eq!(
            (
                status_of(&o, &m("ok")),
                status_of(&o, &m("foreign-sketch")),
                status_of(&o, &m("no_router")),
                status_of(&o, &m("no_answer"))
            ),
            ("ok", "foreign_sketch", "no_source", "no_source")
        );
        let sim = parse_opts(&s(&["--simulate"]));
        assert_eq!(status_of(&sim, &m("ok")), "simulate");
    }

    #[test]
    fn telemetry_names_are_schema_valid() {
        let m = Mcu {
            state: "ok",
            ping_ms: None,
            sketch_calls: None,
            error: None,
        };
        for it in telemetry_items(&m, &Counters::default(), &BusState::default()) {
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
