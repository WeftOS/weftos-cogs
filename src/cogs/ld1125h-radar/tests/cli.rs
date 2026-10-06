//! Binary checks that need no radar: help, argument refusal, and the
//! fail-honest line when the device does not exist.

use std::process::Command;
use std::time::{Duration, Instant};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cog-ld1125h-radar"))
}

#[test]
fn help_exits_zero_and_lists_the_simulate_command() {
    let out = bin().arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("cog-ld1125h-radar --once --simulate"),
        "{text}"
    );
}

#[test]
fn unknown_argument_is_refused() {
    let out = bin().args(["--once", "--exec", "sh"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(
        out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn device_outside_dev_is_refused() {
    let out = bin()
        .args(["--once", "--device", "/etc/hosts"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
}

#[test]
fn missing_device_emits_one_null_line_and_exits_zero() {
    let t0 = Instant::now();
    let out = bin()
        .args([
            "--once",
            "--device",
            "/dev/ld1125h-radar-test-does-not-exist",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(t0.elapsed() < Duration::from_secs(15));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.len() < 65536, "{}", stdout.len());
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "{stdout}");
    let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(v["status"], "no_source");
    assert!(v["kind"].is_null());
    assert!(v["distance_m"].is_null());
    assert!(v["source"].is_null());
    for banned in ["heart_rate", "breathing_rate", "track", "person"] {
        assert!(!stdout.contains(banned), "{banned} in {stdout}");
    }
    let ts = v["timestamp"].as_u64().unwrap();
    assert!(ts > 1_000_000_000 && ts < 100_000_000_000, "{ts}");
    let err = v["error"].as_str().unwrap();
    assert!(
        err.contains("/dev/ld1125h-radar-test-does-not-exist"),
        "{err}"
    );
    assert!(err.contains("115200"), "{err}");
}

#[cfg(not(feature = "spatial-evidence"))]
#[test]
fn default_build_refuses_spatial_flags() {
    let out = bin()
        .args([
            "--once",
            "--spatial-out",
            "/tmp/ld1125h-should-not-write.jsonl",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
}

#[cfg(feature = "spatial-evidence")]
#[test]
fn spatial_out_without_a_pose_writes_nothing_and_says_why() {
    let p = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ld1125h-no-pose.jsonl");
    let _ = std::fs::remove_file(&p);
    let out = bin()
        .args([
            "--once",
            "--simulate",
            "--window",
            "1",
            "--spatial-out",
            p.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["spatial"], "no_pose");
    let reasons = v["reasons"].to_string();
    assert!(reasons.contains("no radar pose"), "{reasons}");
    assert!(!p.exists(), "no evidence line without a pose");
}

#[cfg(feature = "spatial-evidence")]
#[test]
fn simulate_with_a_pose_writes_synthetic_radar_range() {
    let p = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ld1125h-range.jsonl");
    let _ = std::fs::remove_file(&p);
    let out = bin()
        .args([
            "--once",
            "--simulate",
            "--window",
            "1",
            "--spatial-out",
            p.to_str().unwrap(),
            "--radar-pose",
            "0.1,1.83,1.0,0",
            "--spatial-region",
            "region/urth/meso/test-room",
            "--spatial-source-id",
            "ld1125h-test",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let report: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(report["simulated"], true);
    assert_eq!(report["spatial"], "emitting");
    assert!(report.get("heart_rate").is_none());
    assert!(report.get("breathing_rate").is_none());
    let text = std::fs::read_to_string(&p).expect("radar_range file");
    let line = text.lines().next().expect("at least one evidence line");
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(v["schema"], "spatial.evidence.v1");
    assert_eq!(v["type"], "radar_range");
    assert_eq!(v["frame"], "room_enu");
    assert_eq!(v["provenance"]["proof"], "SYNTHETIC");
    assert_eq!(v["source_id"], "ld1125h-test");
    let range = v["range_m"].as_f64().unwrap();
    assert!(range == 1.25 || range == 0.75, "{range}");
    assert!(v.get("track").is_none());
    assert!(v.get("heart_rate_bpm").is_none());
    assert!(v.get("breathing_rate_bpm").is_none());
    assert!(
        !stdout.contains("radar_range"),
        "stdout is the report, not the evidence line"
    );
    let _ = std::fs::remove_file(&p);
}

#[cfg(feature = "spatial-evidence")]
#[test]
fn spatial_export_with_once_is_refused() {
    let out = bin()
        .args(["--once", "--spatial-out", "export"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
}
