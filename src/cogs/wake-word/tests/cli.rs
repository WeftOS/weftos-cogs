use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cog-wake-word"))
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
    assert!(text.contains("cog-wake-word"));
    assert!(text.contains("--once --simulate"));
}

#[test]
fn simulate_passes_two_commands_of_five() {
    let out = bin()
        .args(["--once", "--simulate"])
        .env_remove("WEFTOS_COGBUS")
        .output()
        .unwrap();
    assert!(out.status.success());
    let (r, text) = last_json(&out.stdout);
    assert_eq!(
        (r["status"].as_str(), r["provenance"].as_str()),
        (Some("simulate"), Some("SYNTHETIC"))
    );
    assert_eq!(
        (
            r["transcripts_seen"].as_u64(),
            r["commands"].as_u64(),
            r["bare_wakes"].as_u64(),
            r["ignored"].as_u64()
        ),
        (Some(5), Some(2), Some(1), Some(2))
    );
    assert!(text.contains("\"command\":\"what time is it\""), "{text}");
    assert!(
        text.contains("\"command\":\"turn on the kitchen light\""),
        "{text}"
    );
}

#[test]
fn without_a_broker_it_says_no_source() {
    let out = bin()
        .args(["--once", "--window", "1"])
        .env_remove("WEFTOS_COGBUS")
        .output()
        .unwrap();
    let (r, _) = last_json(&out.stdout);
    assert_eq!(r["status"], "no_source");
    assert!(r["error"].as_str().unwrap().contains("no broker"), "{r}");
    assert_eq!(r["vector"], serde_json::json!(vec![-1.0f64; 8]));
}

#[test]
fn a_replayed_file_uses_another_phrase() {
    let d = std::env::temp_dir().join(format!("wake-cli-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let f = d.join("t.jsonl");
    std::fs::write(
        &f,
        "{\"text\":\"Computer, lights off\"}\n{\"text\":\"hey arduino lights on\"}\n",
    )
    .unwrap();
    let out = bin()
        .args([
            "--once",
            "--replay",
            f.to_str().unwrap(),
            "--phrase",
            "computer",
        ])
        .env_remove("WEFTOS_COGBUS")
        .output()
        .unwrap();
    let (r, text) = last_json(&out.stdout);
    assert_eq!(
        (
            r["status"].as_str(),
            r["commands"].as_u64(),
            r["last_command"].as_str()
        ),
        (Some("replay"), Some(1), Some("lights off")),
        "{text}"
    );
    std::fs::remove_dir_all(&d).ok();
}
