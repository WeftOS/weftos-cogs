use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cog-audio-in"))
}

#[test]
fn help_names_the_cog_and_console_commands() {
    let out = bin().arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("cog-audio-in"));
    assert!(text.contains("--once --simulate"));
}

#[test]
fn simulate_is_marked_synthetic_and_runs_at_rate() {
    let out = bin()
        .args(["--once", "--simulate", "--window", "1"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["status"], "simulate");
    assert_eq!(r["provenance"], "SYNTHETIC");
    assert_eq!(r["chunk_bytes"], 640);
    assert!(r["chunks"].as_u64().unwrap() >= 40, "{r}");
    assert!(r["rms_dbfs"].as_f64().unwrap() > -30.0);
}

#[test]
fn without_a_device_it_says_no_source() {
    let out = bin()
        .args(["--once"])
        .env_remove("WEFTOS_DEVICES")
        .output()
        .unwrap();
    assert!(out.status.success());
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["status"], "no_source");
    assert!(
        r["error"].as_str().unwrap().contains("no capture node"),
        "{r}"
    );
    assert_eq!(r["vector"], serde_json::json!(vec![-1.0f64; 8]));
}

#[test]
fn a_replayed_file_goes_through_the_same_report() {
    let dir = std::env::temp_dir().join(format!("audio-in-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("t.raw");
    std::fs::write(&f, vec![0u8; 640 * 100]).unwrap();
    let out = bin()
        .args(["--once", "--window", "1", "--replay", f.to_str().unwrap()])
        .output()
        .unwrap();
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (r["status"].as_str(), r["provenance"].as_str()),
        (Some("replay"), Some("REPLAY"))
    );
    assert_eq!(r["rms_dbfs"], -120.0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_report_shows_the_gain_and_the_clip_counters() {
    let out = bin()
        .args(["--once", "--simulate", "--gain-db", "30"])
        .output()
        .unwrap();
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["gain_db"], 30.0);
    assert!(
        r["mixer_gain_pct"].as_u64().is_some(),
        "the default capture level is reported: {r}"
    );
    assert!(r["mixer"].is_null(), "a simulated source touches no mixer");
    assert_eq!(r["clipping"], true, "+30 dB on the tone saturates it: {r}");
    assert!(
        r["clipped_chunks"].as_u64().unwrap() > 0 && r["clipped_samples"].as_u64().unwrap() > 0
    );
    let quiet = bin().args(["--once", "--simulate"]).output().unwrap();
    let q: serde_json::Value = serde_json::from_slice(&quiet.stdout).unwrap();
    assert_eq!(
        (q["clipping"].clone(), q["clipped_chunks"].clone()),
        (serde_json::json!(false), serde_json::json!(0))
    );
}
