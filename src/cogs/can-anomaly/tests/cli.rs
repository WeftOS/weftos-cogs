use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cog-can-anomaly"))
}

fn lines(out: &[u8]) -> Vec<serde_json::Value> {
    String::from_utf8(out.to_vec())
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn help_names_the_cog_and_console_commands() {
    let out = bin().arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("cog-can-anomaly"));
    assert!(text.contains("--once --simulate"));
}

#[test]
fn simulate_finds_the_injected_faults() {
    let out = bin()
        .args(["--once", "--simulate"])
        .env_remove("WEFTOS_COGBUS")
        .output()
        .unwrap();
    assert!(out.status.success());
    let l = lines(&out.stdout);
    let r = l.last().unwrap();
    assert_eq!(
        (
            r["status"].as_str(),
            r["provenance"].as_str(),
            r["phase"].as_str()
        ),
        (Some("simulate"), Some("SYNTHETIC"), Some("monitoring"))
    );
    let kinds: Vec<&str> = l.iter().filter_map(|x| x["event"].as_str()).collect();
    for k in [
        "can.baseline",
        "can.new_id",
        "can.id_missing",
        "can.rate_high",
    ] {
        assert!(kinds.contains(&k), "{k} in {kinds:?}");
    }
    assert_eq!(r["baseline_ids"], 3);
}

#[test]
fn a_replayed_log_is_learned_and_judged() {
    let d = std::env::temp_dir().join(format!("canan-cli-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let f = d.join("t.log");
    let mut log = String::new();
    for ms in (0..40_000u32).step_by(20) {
        log += &format!(
            "({}.{:03}000) can0 100#0102030405060708\n",
            100 + ms / 1000,
            ms % 1000
        );
    }
    // An id that was never there, from 30 s on.
    for ms in (30_000..40_000u32).step_by(500) {
        log += &format!("({}.{:03}000) can0 5A0#00\n", 100 + ms / 1000, ms % 1000);
    }
    std::fs::write(&f, log).unwrap();
    let out = bin()
        .args(["--once", "--replay", f.to_str().unwrap(), "--learn-s", "10"])
        .env_remove("WEFTOS_COGBUS")
        .output()
        .unwrap();
    let l = lines(&out.stdout);
    assert!(
        l.iter()
            .any(|x| x["event"] == "can.new_id" && x["subject"] == "0x5a0"),
        "{l:?}"
    );
    let r = l.last().unwrap();
    assert_eq!(
        (r["status"].as_str(), r["baseline_ids"].as_u64()),
        (Some("replay"), Some(1))
    );
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn without_a_broker_it_says_no_source() {
    let out = bin()
        .args(["--once", "--window", "1"])
        .env_remove("WEFTOS_COGBUS")
        .output()
        .unwrap();
    let r = lines(&out.stdout).pop().unwrap();
    assert_eq!(r["status"], "no_source");
    assert!(r["error"].as_str().unwrap().contains("no broker"), "{r}");
    assert_eq!(r["vector"], serde_json::json!(vec![-1.0f64; 8]));
}
