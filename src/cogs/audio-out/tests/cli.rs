use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cog-audio-out"))
}

#[test]
fn help_names_the_cog_and_console_commands() {
    let out = bin().arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("cog-audio-out"));
    assert!(text.contains("--once --simulate"));
}

#[test]
fn simulate_plays_a_tone_into_a_null_sink_at_rate() {
    let out = bin()
        .args(["--once", "--simulate", "--window", "1"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (
            r["status"].as_str(),
            r["provenance"].as_str(),
            r["sink"].as_str()
        ),
        (Some("simulate"), Some("SYNTHETIC"), Some("null"))
    );
    assert!(r["chunks_played"].as_u64().unwrap() >= 40, "{r}");
    assert!(r["rms_dbfs"].as_f64().unwrap() > -20.0);
}

#[test]
fn without_a_device_or_broker_it_says_no_source() {
    let out = bin()
        .args(["--once"])
        .env_remove("WEFTOS_DEVICES")
        .env_remove("WEFTOS_COGBUS")
        .output()
        .unwrap();
    assert!(out.status.success());
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["status"], "no_source");
    assert!(
        r["error"].as_str().unwrap().contains("no playback node"),
        "{r}"
    );
    assert_eq!(r["vector"], serde_json::json!(vec![-1.0f64; 8]));
}

#[test]
fn a_replayed_file_is_played_through_the_same_report() {
    let dir = std::env::temp_dir().join(format!("audio-out-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("t.raw");
    std::fs::write(&f, vec![0u8; 640 * 30]).unwrap();
    // The null sink (--simulate) stands in for the device; --replay supplies the audio.
    let out = bin()
        .args([
            "--once",
            "--simulate",
            "--window",
            "1",
            "--replay",
            f.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["chunks_played"], 30, "{r}");
    assert_eq!(r["rms_dbfs"], -120.0);
    std::fs::remove_dir_all(&dir).ok();
}
