use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cog-vision-detect"))
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
    assert!(text.contains("cog-vision-detect"));
    assert!(text.contains("--once --simulate"));
}

#[test]
fn simulate_looks_at_one_canned_frame() {
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
            r["frames_seen"].as_u64(),
            r["boxes"].as_u64(),
            r["labels_only_boxes"].as_u64()
        ),
        (Some(1), Some(2), Some(1))
    );
    assert!(
        text.contains("SYNTHETIC person") && text.contains("SYNTHETIC chair"),
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
fn a_replayed_frame_without_a_handle_is_counted_unusable() {
    let d = std::env::temp_dir().join(format!("vision-cli-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let f = d.join("f.jsonl");
    std::fs::write(&f, format!("{{\"hash\":\"{}\"}}\n", "ab".repeat(32))).unwrap();
    let out = bin()
        .args(["--once", "--replay", f.to_str().unwrap()])
        .env_remove("WEFTOS_COGBUS")
        .output()
        .unwrap();
    let (r, text) = last_json(&out.stdout);
    assert_eq!(
        (
            r["status"].as_str(),
            r["frames_seen"].as_u64(),
            r["unusable_frames"].as_u64(),
            r["frames_asked"].as_u64()
        ),
        (Some("replay"), Some(1), Some(1), Some(0)),
        "{text}"
    );
    assert!(
        r["last_error"].as_str().unwrap().contains("no blob handle"),
        "{r}"
    );
    std::fs::remove_dir_all(&d).ok();
}
