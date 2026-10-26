use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cog-can-fd"))
}

fn last_json(out: &[u8]) -> serde_json::Value {
    let text = String::from_utf8(out.to_vec()).unwrap();
    serde_json::from_str(text.lines().last().unwrap()).unwrap()
}

#[test]
fn help_names_the_cog_and_console_commands() {
    let out = bin().arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("cog-can-fd"));
    assert!(text.contains("--once --simulate"));
}

#[test]
fn simulate_reports_synthetic_traffic_and_never_transmits() {
    let out = bin()
        .args(["--once", "--simulate", "--window", "1"])
        .env_remove("WEFTOS_COGBUS")
        .output()
        .unwrap();
    assert!(out.status.success());
    let r = last_json(&out.stdout);
    assert_eq!(
        (
            r["status"].as_str(),
            r["provenance"].as_str(),
            r["transmit"].as_str()
        ),
        (Some("simulate"), Some("SYNTHETIC"), Some("never"))
    );
    assert!(r["frames"].as_u64().unwrap() >= 50, "{r}");
    assert!(
        r["fd_frames"].as_u64().unwrap() >= 1 && r["extended_frames"].as_u64().unwrap() >= 1,
        "{r}"
    );
    assert_eq!(r["top_ids"][0]["id"], "0x100");
}

#[test]
fn a_replayed_log_goes_through_the_same_report() {
    let d = std::env::temp_dir().join(format!("canfd-cli-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let f = d.join("t.log");
    let mut log = String::new();
    for i in 0..100 {
        log += &format!(
            "({}.{:06}) can0 123#{:02X}0000\n",
            1_000 + i / 100,
            (i % 100) * 10_000,
            i
        );
    }
    log += "(1001.000000) can0 7FF##1000102030405060708090a0b\n";
    std::fs::write(&f, log).unwrap();
    let out = bin()
        .args(["--once", "--replay", f.to_str().unwrap()])
        .env_remove("WEFTOS_COGBUS")
        .output()
        .unwrap();
    let r = last_json(&out.stdout);
    assert_eq!(
        (
            r["status"].as_str(),
            r["frames"].as_u64(),
            r["fd_frames"].as_u64()
        ),
        (Some("replay"), Some(101), Some(1)),
        "{r}"
    );
    assert_eq!(
        r["top_ids"][0],
        serde_json::json!({"id": "0x123", "frames": 100})
    );
    // 101 frames over the 1.0 s the log covers.
    assert!(
        (r["frames_per_s"].as_f64().unwrap() - 101.0).abs() < 1.0,
        "{r}"
    );
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_bad_replay_line_is_no_source_with_its_line_number() {
    let d = std::env::temp_dir().join(format!("canfd-cli-bad-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let f = d.join("t.log");
    std::fs::write(&f, "(1.0) can0 123#00\nnope\n").unwrap();
    let out = bin()
        .args(["--once", "--replay", f.to_str().unwrap()])
        .env_remove("WEFTOS_COGBUS")
        .output()
        .unwrap();
    let r = last_json(&out.stdout);
    assert_eq!(r["status"], "no_source");
    assert!(r["error"].as_str().unwrap().contains("line 2"), "{r}");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn without_an_interface_it_says_no_source() {
    let out = bin()
        .args(["--once", "--window", "1", "--iface", "nocan9"])
        .env_remove("WEFTOS_COGBUS")
        .output()
        .unwrap();
    assert!(out.status.success());
    let r = last_json(&out.stdout);
    assert_eq!(r["status"], "no_source");
    assert_eq!(r["vector"], serde_json::json!(vec![-1.0f64; 8]));
}
