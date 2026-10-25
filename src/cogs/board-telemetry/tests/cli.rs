use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cog-board-telemetry"))
}

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn run_once(root: &str) -> serde_json::Value {
    let out = bin()
        .args([
            "--once",
            "--window",
            "1",
            "--sample-ms",
            "50",
            "--sysfs-root",
            root,
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "one JSON object per window");
    serde_json::from_str(text.trim()).unwrap()
}

#[test]
fn help_names_the_cog_and_console_commands() {
    let out = bin().arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("cog-board-telemetry"));
    assert!(text.contains("--once --simulate"));
}

#[test]
fn simulate_is_marked_synthetic() {
    let out = bin()
        .args(["--once", "--simulate", "--window", "1", "--sample-ms", "50"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["status"], "simulate");
    assert_eq!(r["provenance"], "SYNTHETIC");
    assert!(r["power_w"].as_f64().unwrap() > 0.0);
}

#[test]
fn ventuno_fixture_reports_every_source() {
    let r = run_once(&fixture("ventuno"));
    assert_eq!(r["status"], "ok");
    assert_eq!(r["source"], format!("sysfs:{}", fixture("ventuno")));
    assert_eq!(r["temp_max_c"], 55.5);
    assert_eq!(
        r["vector"],
        serde_json::json!([12.345, 12.01, 1.028, 55.5, 47.25, 55.5, 3200.0, 0.85])
    );
}

#[test]
fn pi_fixture_reports_cpu_only() {
    let r = run_once(&fixture("pi"));
    assert_eq!(r["status"], "ok");
    assert!(r["power_w"].is_null());
    assert_eq!(r["vector"][3], 48.312);
    assert_eq!(r["vector"][0], -1.0);
}

#[test]
fn empty_fixture_is_no_source_and_not_a_fake_reading() {
    let r = run_once(&fixture("empty"));
    assert_eq!(r["status"], "no_source");
    assert!(r["temp_max_c"].is_null());
    assert!(r["error"].is_string());
}

#[test]
fn a_missing_root_is_no_source_too() {
    let r = run_once(&fixture("does-not-exist"));
    assert_eq!(r["status"], "no_source");
}
