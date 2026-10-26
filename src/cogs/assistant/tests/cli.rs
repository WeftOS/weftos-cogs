use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cog-assistant"))
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
    assert!(text.contains("cog-assistant"));
    assert!(text.contains("--once --simulate"));
}

#[test]
fn simulate_answers_two_canned_commands() {
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
            r["brain"].as_str()
        ),
        (Some("simulate"), Some("SYNTHETIC"), Some("synthetic"))
    );
    assert_eq!(
        (
            r["commands"].as_u64(),
            r["replies"].as_u64(),
            r["failures"].as_u64()
        ),
        (Some(2), Some(2), Some(0))
    );
    assert!(
        text.contains("SYNTHETIC answer to: what time is it."),
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
fn a_replayed_command_with_no_service_gets_the_apology() {
    let d = std::env::temp_dir().join(format!("assistant-cli-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let f = d.join("c.jsonl");
    std::fs::write(&f, "{\"text\":\"what time is it\"}\n").unwrap();
    let out = bin()
        .args(["--once", "--replay", f.to_str().unwrap()])
        .env_remove("WEFTOS_COGBUS")
        .output()
        .unwrap();
    let (r, text) = last_json(&out.stdout);
    assert_eq!(
        (
            r["status"].as_str(),
            r["commands"].as_u64(),
            r["failures"].as_u64()
        ),
        (Some("replay"), Some(1), Some(1)),
        "{text}"
    );
    assert!(
        text.contains("Sorry, I could not reach my language model."),
        "{text}"
    );
    std::fs::remove_dir_all(&d).ok();
}
