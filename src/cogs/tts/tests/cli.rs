use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cog-tts"))
}

fn last_json(out: &[u8]) -> (serde_json::Value, String) {
    let text = String::from_utf8(out.to_vec()).unwrap();
    (
        serde_json::from_str(text.lines().last().unwrap()).unwrap(),
        text,
    )
}

#[test]
fn help_names_the_cog_and_console_commands() {
    let out = bin().arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("cog-tts"));
    assert!(text.contains("--once --simulate"));
}

#[test]
fn simulate_speaks_a_tone_per_word() {
    let out = bin()
        .args(["--once", "--simulate"])
        .env_remove("WEFTOS_COGBUS")
        .output()
        .unwrap();
    assert!(out.status.success());
    let (r, text) = last_json(&out.stdout);
    assert_eq!(
        (
            r["status"].as_str(),
            r["provenance"].as_str(),
            r["voice"].as_str()
        ),
        (Some("simulate"), Some("SYNTHETIC"), Some("synthetic"))
    );
    // "This is a synthetic voice test." is six words: 480 ms of speech.
    assert_eq!(
        (r["spoken"].as_u64(), r["audio_s"].as_f64()),
        (Some(1), Some(0.48)),
        "{text}"
    );
    assert!(
        text.contains("\"speaking\":\"This is a synthetic voice test.\""),
        "{text}"
    );
}

#[test]
fn a_missing_worker_is_no_source_with_the_reason() {
    let d = std::env::temp_dir().join(format!("tts-cli-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let f = d.join("l.jsonl");
    std::fs::write(&f, "{\"text\":\"hello\"}\n").unwrap();
    let out = bin()
        .args([
            "--once",
            "--replay",
            f.to_str().unwrap(),
            "--python",
            "/nonexistent/python",
        ])
        .env_remove("WEFTOS_COGBUS")
        .env("COGNITUM_COG_DATA_DIR", &d)
        .output()
        .unwrap();
    let (r, _) = last_json(&out.stdout);
    assert_eq!(r["status"], "no_source");
    assert!(
        r["error"].as_str().unwrap().contains("/nonexistent/python"),
        "{r}"
    );
    assert_eq!(r["vector"], serde_json::json!(vec![-1.0f64; 8]));
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_windowed_simulated_run_exits_cleanly() {
    let out = bin()
        .args(["--once", "--window", "1", "--simulate"])
        .env_remove("WEFTOS_COGBUS")
        .output()
        .unwrap();
    assert!(out.status.success());
}
