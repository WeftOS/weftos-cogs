use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cog-asr"))
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("asr-cli-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn help_names_the_cog_and_console_commands() {
    let out = bin().arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("cog-asr"));
    assert!(text.contains("--once --simulate"));
}

#[test]
fn simulate_hears_two_synthetic_bursts() {
    let out = bin()
        .args(["--once", "--simulate"])
        .env_remove("WEFTOS_COGBUS")
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    let r: serde_json::Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    assert_eq!(
        (
            r["status"].as_str(),
            r["provenance"].as_str(),
            r["recognizer"].as_str()
        ),
        (Some("simulate"), Some("SYNTHETIC"), Some("synthetic"))
    );
    assert_eq!(
        (r["utterances"].as_u64(), r["transcripts"].as_u64()),
        (Some(2), Some(2))
    );
    assert!(r["last_text"]
        .as_str()
        .unwrap()
        .starts_with("SYNTHETIC utterance of"));
    assert_eq!(text.lines().filter(|l| l.contains("\"heard\"")).count(), 2);
}

#[test]
fn a_missing_worker_is_no_source_with_the_reason() {
    let d = tmp("missing");
    let out = bin()
        .args(["--once", "--window", "2", "--python", "/nonexistent/python"])
        .env_remove("WEFTOS_COGBUS")
        .env("COGNITUM_COG_DATA_DIR", &d)
        .output()
        .unwrap();
    assert!(out.status.success());
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["status"], "no_source");
    assert!(
        r["error"].as_str().unwrap().contains("/nonexistent/python"),
        "{r}"
    );
    assert_eq!(r["vector"], serde_json::json!(vec![-1.0f64; 8]));
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_replayed_file_goes_through_the_segmenter_and_a_stand_in_worker() {
    let d = tmp("replay");
    // Two seconds of room tone then a burst of loud tone, as raw s16le.
    let mut pcm = Vec::new();
    for i in 0..(16_000 * 3) {
        let amp = if (16_000..40_000).contains(&i) {
            8000.0
        } else {
            20.0
        };
        pcm.extend_from_slice(&((((i as f64) * 0.2).sin() * amp) as i16).to_le_bytes());
    }
    let f = d.join("in.raw");
    std::fs::write(&f, &pcm).unwrap();
    // A stand-in for python that speaks the worker protocol.
    let stub = d.join("py.sh");
    std::fs::write(
        &stub,
        "#!/bin/sh\necho '{\"ready\": true}'\nwhile read l; do id=$(echo \"$l\" | sed 's/.*\"id\": *\\([0-9]*\\).*/\\1/'); echo \"{\\\"id\\\": $id, \\\"text\\\": \\\"hello from the stub\\\", \\\"ms\\\": 5}\"; done\n",
    )
    .unwrap();
    Command::new("chmod").arg("+x").arg(&stub).status().unwrap();
    let out = bin()
        .args([
            "--once",
            "--replay",
            f.to_str().unwrap(),
            "--python",
            stub.to_str().unwrap(),
        ])
        .env_remove("WEFTOS_COGBUS")
        .env("COGNITUM_COG_DATA_DIR", &d)
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let r: serde_json::Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    assert_eq!(
        (
            r["status"].as_str(),
            r["transcripts"].as_u64(),
            r["last_text"].as_str()
        ),
        (Some("replay"), Some(1), Some("hello from the stub")),
        "{text}"
    );
    std::fs::remove_dir_all(&d).ok();
}
