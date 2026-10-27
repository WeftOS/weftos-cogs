//! Binary checks that need no radar: help, argument refusal, the no-device line,
//! and a simulated burst through the real parser.

use std::process::Command;
use std::time::{Duration, Instant};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cog-ld6004-radar"))
}

fn stdout_of(out: &std::process::Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

#[test]
fn help_exits_zero_and_lists_console_commands() {
    let out = bin().arg("--help").output().unwrap();
    assert!(out.status.success(), "{:?}", out.status.code());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("--once --device /dev/ttyUSB0"), "{text}");
    assert!(text.contains("--once --simulate"), "{text}");
}

#[test]
fn unknown_argument_is_refused() {
    let out = bin().args(["--once", "--exec", "sh"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
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
fn simulate_plus_firmware_query_is_refused() {
    let out = bin()
        .args(["--once", "--simulate", "--command", "query-firmware"])
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
            "/dev/ld6004-radar-test-does-not-exist",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(t0.elapsed() < Duration::from_secs(15));
    let stdout = stdout_of(&out);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "{stdout}");
    assert!(lines[0].starts_with('{'));
    let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(v["schema"], "weavelogic.cog-output.v0");
    assert_eq!(v["cog"], "ld6004-radar");
    assert_eq!(v["health"], "no_source");
    assert!(v["quality"].is_null());
    assert_eq!(v["source"]["verified"], false);
    assert_eq!(v["source"]["simulated"], false);
    assert_eq!(v["frame"], "radar_local");
    assert_eq!(v["targets"], serde_json::json!([]));
    assert!(v["zones"].is_null());
    assert_eq!(v["vitals"], "not_defined");
    assert_eq!(v["writes"], "none");
    assert!(v.get("heart_rate_bpm").is_none());
    assert!(v.get("breathing_rate_bpm").is_none());
    assert!(!stdout.contains("bpm"), "{stdout}");
    let ts = v["timestamp"].as_u64().unwrap();
    assert!(ts < 100_000_000_000, "timestamp must be seconds");
    assert_eq!(ts, v["timestamp_ms"].as_u64().unwrap() / 1000);
}

#[test]
fn simulate_labels_the_line_and_does_not_invent_a_rate() {
    let out = bin().args(["--once", "--simulate"]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = stdout_of(&out);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout}");
    let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(v["health"], "ok");
    assert_eq!(v["source"]["verified"], false);
    assert_eq!(v["source"]["simulated"], true);
    assert!(v["reasons"].to_string().contains("simulated"));
    assert_eq!(v["vitals"], "not_defined");
    assert_eq!(v["targets"].as_array().unwrap().len(), 1);
    assert_eq!(v["targets"][0]["cluster_id"], 2);
    assert_eq!(v["targets"][0]["dop_idx"], 7);
    assert!(v["targets"][0].get("velocity_m_s").is_none());
    assert_eq!(v["points"], serde_json::json!([]));
    assert!(v["zones"].is_null());
    assert_eq!(v["low_power_state"]["code"], 1);
    assert_eq!(v["low_power_state"]["name"], "normal");
    assert_eq!(v["trigger_speed"]["code"], 2);
    assert_eq!(v["writes"], "none");
    assert!(v.get("heart_rate_bpm").is_none());
    assert!(v.get("breathing_rate_bpm").is_none());
    let pack: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    let recs = pack.as_array().unwrap();
    assert!(recs
        .iter()
        .all(|r| r["bn"] == "ld6004-radar/" && r["proof_"] == "SYNTHETIC"));
    assert!(recs.iter().any(|r| r["n"] == "low_power_state"));
    let text = stdout.to_lowercase();
    assert!(!text.contains("bpm"), "{stdout}");
    assert!(!text.contains("heart_rate") && !text.contains("breathing"));
}

#[cfg(not(feature = "spatial-evidence"))]
#[test]
fn default_build_has_no_spatial_flags() {
    let out = bin()
        .args(["--once", "--spatial-out", "/tmp/x.jsonl"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
}

#[cfg(feature = "spatial-evidence")]
#[test]
fn spatial_without_pose_writes_nothing() {
    let p = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ld6004-no-pose.jsonl");
    let _ = std::fs::remove_file(&p);
    let out = bin()
        .args(["--once", "--simulate", "--spatial-out", p.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let line = stdout_of(&out).lines().next().unwrap().to_string();
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["spatial"], "no_pose");
    let reasons = v["reasons"].to_string();
    assert!(
        reasons.contains("spatial_evidence_refused: no radar pose"),
        "{reasons}"
    );
    assert!(!p.exists(), "nothing may be written without a pose");
}

#[cfg(feature = "spatial-evidence")]
#[test]
fn spatial_simulate_emits_one_synthetic_track() {
    let p = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ld6004-sim.jsonl");
    let _ = std::fs::remove_file(&p);
    let out = bin()
        .args([
            "--once",
            "--simulate",
            "--spatial-out",
            p.to_str().unwrap(),
            "--radar-pose",
            "0,0,0,90,0,1",
            "--spatial-region",
            "region/urth/meso/test-room",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout_of(&out).lines().next().unwrap()).unwrap();
    assert_eq!(report["spatial"], "emitting");
    assert_eq!(report["source"]["simulated"], true);
    let text = std::fs::read_to_string(&p).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "{text}");
    let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(v["schema"], "spatial.evidence.v1");
    assert_eq!(v["type"], "radar_track_point");
    assert_eq!(v["track"], 2);
    assert_eq!(v["provenance"]["proof"], "SYNTHETIC");
    assert_eq!(v["source_id"], "ld6004-radar");
    assert_eq!(v["uncertainty_m"], 0.3);
    assert!(v.get("velocity").is_none());
    assert!(v.get("dop_idx").is_none());
    // yaw 90, origin, x_sign +1. Local (1.5, -0.5, 2.5): lateral, height 0.5, forward 2.5.
    // At yaw 90, room = [x, z, -y] = [1.5, 2.5, 0.5].
    assert_eq!(v["position"], serde_json::json!([1.5, 2.5, 0.5]));
    std::fs::remove_file(&p).unwrap();
}
