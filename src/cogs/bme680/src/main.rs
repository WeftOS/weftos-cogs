//! Cognitum cog: Bosch BME680 on I2C.
//!
//! One forced-mode sample of temperature, humidity, pressure and gas resistance.
//! The status line is one JSON object. A SenML pack (RFC 8428) is a second line,
//! or the file given to `--readings-out`. There is no `spatial.evidence.v1`
//! output: ADR-107 and RuView ADR-384 have no type for these quantities.
//! ADR-174.
//!
//!   cog-bme680 --once
//!   cog-bme680 --once --simulate
//!   cog-bme680 --interval 10

mod compensate;
mod export;
mod guide;
mod senml;
mod sensor;

use std::io::Write;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use compensate::Reading;
use export::Shared;

const TAG: &str = "[cog-bme680]";
const DEFAULT_DEVICE: &str = "/dev/i2c-1";
const DEFAULT_ADDR: u8 = 0x76;
const DEFAULT_SOURCE: &str = "bme680/";
const DEFAULT_BIND: &str = "127.0.0.1:8057";

struct Opts {
    once: bool,
    simulate: bool,
    interval: u64,
    device: String,
    addr: u8,
    source_id: String,
    api_bind: String,
    heater_c: u16,
    heat_ms: u16,
    readings_out: Option<String>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    cog_sensor_sources::handle_help(
        &std::env::args().collect::<Vec<_>>(),
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        include_str!("../cog.toml"),
    );
    let opts = match parse_args(&args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{TAG} {e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(&opts) {
        eprintln!("{TAG} {e}");
        std::process::exit(1);
    }
}

fn run(opts: &Opts) -> Result<(), String> {
    let shared: Shared = std::sync::Arc::new(std::sync::Mutex::new(export::State::default()));
    match export::serve(&opts.api_bind, shared.clone()) {
        Ok(addr) => eprintln!("{TAG} export http://{addr}/"),
        Err(e) => eprintln!("{TAG} export disabled: {e}"),
    }

    let mut dev: Option<sensor::Bme680> = None;

    loop {
        let result = if opts.simulate {
            sensor::simulate()
        } else {
            if dev.is_none() {
                match sensor::Bme680::open(&opts.device, opts.addr) {
                    Ok(d) => dev = Some(d),
                    Err(e) => {
                        emit(
                            opts,
                            &shared,
                            "no_source",
                            Some(e),
                            None,
                            senml::PROOF_MEASURED,
                        );
                        if opts.once {
                            return Ok(());
                        }
                        std::thread::sleep(Duration::from_secs(opts.interval));
                        continue;
                    }
                }
            }
            match dev.as_mut() {
                Some(d) => d.sample(opts.heater_c, opts.heat_ms),
                None => Err("internal: I2C device was not open".to_string()),
            }
        };
        match result {
            Ok(reading) => {
                let health = if reading.all_present() {
                    "ok"
                } else {
                    "degraded"
                };
                let err = if reading.gaps.is_empty() {
                    None
                } else {
                    Some(reading.gaps.join("; "))
                };
                let proof = if opts.simulate {
                    senml::PROOF_SYNTHETIC
                } else {
                    senml::PROOF_MEASURED
                };
                emit(opts, &shared, health, err, Some(&reading), proof);
            }
            Err(e) => {
                // A failed sample must not keep a stale device state that hides
                // the next error. Re-open next time if the bus went away.
                if !opts.simulate {
                    dev = None;
                }
                emit(
                    opts,
                    &shared,
                    "no_source",
                    Some(e),
                    None,
                    senml::PROOF_MEASURED,
                );
            }
        }
        if opts.once {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(opts.interval));
    }
}

fn emit(
    opts: &Opts,
    shared: &Shared,
    health: &str,
    error: Option<String>,
    reading: Option<&Reading>,
    proof: &str,
) {
    let t = unix_s();
    let kind = if opts.simulate {
        "bme680-simulator"
    } else {
        "bme680"
    };
    let mut status = senml::status(&senml::StatusIn {
        health,
        t,
        error,
        kind,
        bus: &opts.device,
        addr: opts.addr,
        source_id: &opts.source_id,
        simulated: opts.simulate,
        reading,
    });
    let pack = reading.and_then(|r| senml::pack(&opts.source_id, t, proof, r));
    let pack_text = pack.as_ref().map(ToString::to_string);
    let mut also_stdout = opts.readings_out.is_none();
    if let (Some(path), Some(text)) = (&opts.readings_out, &pack_text) {
        if let Err(e) = std::fs::write(path, format!("{text}\n")) {
            status["health"] = serde_json::json!("degraded");
            status["error"] = serde_json::json!(format!("readings-out {path}: {e}"));
            also_stdout = true;
        }
    }
    if let Ok(mut state) = shared.lock() {
        state.status = Some(status.clone());
        state.pack = pack_text.clone();
    }
    println!("{status}");
    if also_stdout {
        if let Some(text) = pack_text {
            println!("{text}");
        }
    }
    let _ = std::io::stdout().flush();
}

fn unix_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn parse_args(args: &[String]) -> Result<Opts, String> {
    let mut once = false;
    let mut simulate = false;
    let mut interval: Option<u64> = None;
    let mut bus: Option<u8> = None;
    let mut device: Option<String> = None;
    let mut addr: Option<u8> = None;
    let mut source_id: Option<String> = None;
    let mut api_bind: Option<String> = None;
    let mut heater_c: Option<u16> = None;
    let mut heat_ms: Option<u16> = None;
    let mut readings_out: Option<String> = None;

    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{a} needs a value"))
        };
        match a.as_str() {
            "--once" => once = true,
            "--simulate" => simulate = true,
            "--interval" => {
                let v = value()?;
                interval = Some(parse_range("--interval", &v, 1, 3600)?);
            }
            "--i2c-bus" => {
                let v = value()?;
                bus = Some(parse_range("--i2c-bus", &v, 0, 9)?);
            }
            "--i2c-addr" => {
                let v = value()?;
                addr = Some(parse_addr(&v)?);
            }
            "--device" => {
                let v = value()?;
                validate_device(&v)?;
                device = Some(v);
            }
            "--source-id" => {
                let v = value()?;
                validate_source_id(&v)?;
                source_id = Some(v);
            }
            "--api-bind" => {
                let v = value()?;
                validate_bind(&v)?;
                api_bind = Some(v);
            }
            "--heater-c" => {
                let v = value()?;
                heater_c = Some(parse_range("--heater-c", &v, 200, 400)?);
            }
            "--heat-ms" => {
                let v = value()?;
                heat_ms = Some(parse_range("--heat-ms", &v, 1, 4032)?);
            }
            "--readings-out" => {
                let v = value()?;
                if v.is_empty() {
                    readings_out = None;
                } else {
                    validate_readings_out(&v)?;
                    readings_out = Some(v);
                }
            }
            other => return Err(format!("unknown argument {other:?} (see --help)")),
        }
    }

    let device = match (device, bus) {
        (Some(path), Some(bus)) => {
            let expect = format!("/dev/i2c-{bus}");
            if path != expect {
                return Err(format!(
                    "--device {path} does not match --i2c-bus {bus} ({expect})"
                ));
            }
            path
        }
        (Some(path), None) => path,
        (None, Some(bus)) => format!("/dev/i2c-{bus}"),
        (None, None) => DEFAULT_DEVICE.to_string(),
    };

    Ok(Opts {
        once,
        simulate,
        interval: interval.unwrap_or(1),
        device,
        addr: addr.unwrap_or(DEFAULT_ADDR),
        source_id: source_id.unwrap_or_else(|| DEFAULT_SOURCE.to_string()),
        api_bind: api_bind.unwrap_or_else(|| DEFAULT_BIND.to_string()),
        heater_c: heater_c.unwrap_or(300),
        heat_ms: heat_ms.unwrap_or(100),
        readings_out,
    })
}

fn parse_range<T>(flag: &str, raw: &str, lo: T, hi: T) -> Result<T, String>
where
    T: std::str::FromStr + PartialOrd + Copy + std::fmt::Display,
{
    let v = raw
        .parse::<T>()
        .map_err(|_| format!("{flag} must be an integer {lo}..={hi}, got {raw:?}"))?;
    if v < lo || v > hi {
        return Err(format!(
            "{flag} must be an integer {lo}..={hi}, got {raw:?}"
        ));
    }
    Ok(v)
}

fn parse_addr(raw: &str) -> Result<u8, String> {
    let v = if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        u8::from_str_radix(hex, 16).ok()
    } else {
        raw.parse::<u8>().ok()
    };
    match v {
        Some(a) if a == 0x76 || a == 0x77 => Ok(a),
        _ => Err(format!(
            "--i2c-addr must be 0x76 or 0x77 (118 or 119), got {raw:?}"
        )),
    }
}

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

fn validate_source_id(id: &str) -> Result<(), String> {
    let ok = (2..=128).contains(&id.len())
        && id.ends_with('/')
        && !id.contains("..")
        && id.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'-' | b'@')
        });
    if ok {
        Ok(())
    } else {
        Err(format!(
            "--source-id must be 2..=128 bytes, end with '/', and contain only [A-Za-z0-9._:/-@], got {id:?}"
        ))
    }
}

fn validate_bind(bind: &str) -> Result<(), String> {
    let Some((host, port)) = bind.rsplit_once(':') else {
        return Err(format!("--api-bind must be 127.0.0.1:PORT, got {bind:?}"));
    };
    if host != "127.0.0.1" {
        return Err(format!(
            "--api-bind must use 127.0.0.1 (this export is loopback only), got {bind:?}"
        ));
    }
    let port: u16 = port
        .parse()
        .map_err(|_| format!("--api-bind port is not an integer, got {bind:?}"))?;
    let _ = port;
    Ok(())
}

fn validate_readings_out(path: &str) -> Result<(), String> {
    let banned =
        path.starts_with("/dev/") || path.starts_with("/proc/") || path.starts_with("/sys/");
    let ok = path.len() <= 256
        && !banned
        && !path.split('/').any(|c| c == "..")
        && path.bytes().all(|b| b.is_ascii_graphic());
    if ok {
        Ok(())
    } else {
        Err(format!(
            "--readings-out must be a plain path, not under /dev, /proc or /sys, got {path:?}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn defaults_are_the_header_bus_and_loopback_export() {
        let o = parse_args(&s(&[])).unwrap();
        assert_eq!(o.device, "/dev/i2c-1");
        assert_eq!(o.addr, 0x76);
        assert_eq!(o.source_id, "bme680/");
        assert_eq!(o.api_bind, "127.0.0.1:8057");
        assert_eq!(o.heater_c, 300);
        assert_eq!(o.heat_ms, 100);
        assert!(!o.simulate);
    }

    #[test]
    fn address_77_and_bus_are_accepted() {
        let o = parse_args(&s(&["--i2c-addr", "0x77", "--i2c-bus", "1"])).unwrap();
        assert_eq!(o.addr, 0x77);
        assert_eq!(o.device, "/dev/i2c-1");
        let o = parse_args(&s(&["--i2c-addr", "119"])).unwrap();
        assert_eq!(o.addr, 0x77);
    }

    #[test]
    fn malformed_arguments_are_refused() {
        assert!(parse_args(&s(&["--exec", "sh"])).is_err());
        assert!(parse_args(&s(&["--i2c-addr", "0x48"])).is_err());
        assert!(parse_args(&s(&["--device", "/etc/hosts"])).is_err());
        assert!(parse_args(&s(&["--api-bind", "0.0.0.0:8057"])).is_err());
        assert!(parse_args(&s(&["--source-id", "bme680"])).is_err());
        assert!(parse_args(&s(&["--heater-c", "10"])).is_err());
        assert!(parse_args(&s(&["--spatial-out", "x"])).is_err());
        assert!(parse_args(&s(&["--i2c-bus", "1", "--device", "/dev/i2c-2"])).is_err());
    }
}
