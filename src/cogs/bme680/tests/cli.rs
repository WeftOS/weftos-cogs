//! Binary checks that need no sensor: help, refused arguments, no_source, simulate.

use std::process::Command;
use std::time::{Duration, Instant};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cog-bme680"))
}

fn once(args: &[&str]) -> std::process::Output {
    bin().args(args).output().unwrap()
}

#[test]
fn help_exits_zero_and_lists_console_commands() {
    let out = bin().arg("--help").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("--once --simulate"), "{text}");
    assert!(text.contains("--help"), "{text}");
}

#[test]
fn unknown_argument_is_refused() {
    let out = once(&["--once", "--exec", "sh"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn spatial_flag_is_refused_because_there_is_no_adapter() {
    let out = once(&["--once", "--spatial-out", "export"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
}

#[test]
fn device_outside_dev_is_refused() {
    let out = once(&["--once", "--device", "/etc/hosts"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
}

#[test]
fn bad_address_is_refused() {
    let out = once(&["--once", "--i2c-addr", "0x48"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.is_empty());
}

#[test]
fn source_id_without_a_trailing_slash_is_refused() {
    let out = once(&["--once", "--simulate", "--source-id", "bme680"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
}

#[test]
fn missing_device_emits_one_null_line_and_exits_zero() {
    let t0 = Instant::now();
    let out = once(&[
        "--once",
        "--device",
        "/dev/bme680-test-does-not-exist",
        "--api-bind",
        "127.0.0.1:0",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(t0.elapsed() < Duration::from_secs(15));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.len() < 65536,
        "status line exceeds output_limit_bytes"
    );
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "{stdout}");
    let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(v["health"], "no_source");
    assert_eq!(v["source"]["verified"], false);
    assert_eq!(v["source"]["simulated"], false);
    assert!(v["error"].as_str().unwrap().contains("0x76"));
    assert!(v["error"]
        .as_str()
        .unwrap()
        .contains("/dev/bme680-test-does-not-exist"));
    for k in ["temperature_c", "humidity_rh", "pressure_pa", "gas_ohm"] {
        assert!(v[k].is_null(), "{k} must be null, got {}", v[k]);
    }
    let ts = v["timestamp"].as_u64().unwrap();
    assert!(
        ts > 1_000_000_000 && ts < 100_000_000_000,
        "timestamp must be unix seconds"
    );
    assert!(!stdout.contains("spatial.evidence"));
}

#[test]
fn simulate_prints_a_synthetic_senml_pack() {
    let out = once(&["--once", "--simulate", "--api-bind", "127.0.0.1:0"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.len() < 65536);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout}");
    let status: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(status["health"], "ok");
    assert_eq!(status["source"]["simulated"], true);
    assert_eq!(status["source"]["kind"], "bme680-simulator");
    assert_eq!(status["source"]["source_id"], "bme680/");
    assert_eq!(status["source"]["verified"], false);
    assert!((status["temperature_c"].as_f64().unwrap() - 31.589_253_301_313_146).abs() < 1e-6);
    let pack: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    let arr = pack.as_array().unwrap();
    assert_eq!(arr.len(), 4);
    assert_eq!(arr[0]["bn"], "bme680/");
    assert!(arr[0]["bn"].as_str().unwrap().ends_with('/'));
    for rec in arr {
        assert_eq!(rec["proof_"], "SYNTHETIC");
        assert!(rec.get("bt").is_none());
    }
    assert_eq!(arr[0]["u"], "Cel");
    assert_eq!(arr[1]["u"], "%RH");
    assert_eq!(arr[2]["u"], "Pa");
    assert_eq!(arr[3]["n"], "gas_resistance");
    assert_eq!(arr[3]["u"], "Ohm");
    assert!(!lines[1].contains("spatial.evidence"));
}

#[test]
fn readings_out_keeps_the_pack_off_stdout() {
    let path = std::env::temp_dir().join(format!(
        "bme680-senml-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&path);
    let out = once(&[
        "--once",
        "--simulate",
        "--readings-out",
        path.to_str().unwrap(),
        "--api-bind",
        "127.0.0.1:0",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    let body = std::fs::read_to_string(&path).unwrap();
    let pack: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
    assert_eq!(pack.as_array().unwrap().len(), 4);
    assert!(pack.to_string().contains("SYNTHETIC"));
    let _ = std::fs::remove_file(&path);
}
