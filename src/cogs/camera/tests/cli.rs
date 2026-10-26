use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cog-camera"))
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("camera-cli-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn help_names_the_cog_and_console_commands() {
    let out = bin().arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("cog-camera"));
    assert!(text.contains("--once --simulate"));
}

#[test]
fn simulate_announces_synthetic_frames() {
    let d = tmp("sim");
    let out = bin()
        .args(["--once", "--simulate", "--fps", "10"])
        .env("COGNITUM_COG_DATA_DIR", &d)
        .output()
        .unwrap();
    assert!(out.status.success());
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (r["status"].as_str(), r["provenance"].as_str()),
        (Some("simulate"), Some("SYNTHETIC"))
    );
    assert!(r["frames"].as_u64().unwrap() >= 2, "{r}");
    assert_eq!(
        (r["width"].as_u64(), r["height"].as_u64()),
        (Some(1280), Some(720))
    );
    let h = r["last_hash"].as_str().unwrap();
    assert!(
        d.join("frames").join(format!("{h}.jpg")).exists(),
        "the frame is kept under its hash"
    );
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn without_a_device_it_says_no_source() {
    let d = tmp("none");
    let out = bin()
        .args(["--once"])
        .env_remove("WEFTOS_DEVICES")
        .env("COGNITUM_COG_DATA_DIR", &d)
        .output()
        .unwrap();
    assert!(out.status.success());
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["status"], "no_source");
    assert!(
        r["error"].as_str().unwrap().contains("no video node"),
        "{r}"
    );
    assert_eq!(r["vector"], serde_json::json!(vec![-1.0f64; 8]));
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_replayed_jpeg_is_stored_under_its_hash() {
    let d = tmp("replay");
    // A JPEG shape with a start-of-frame for 320x240: the cog reads only the size.
    let jpg: Vec<u8> = [&[
        0xff, 0xd8, 0xff, 0xc0, 0x00, 0x0b, 0x08, 0x00, 0xf0, 0x01, 0x40, 0x01, 0x01, 0x11, 0x00,
        0xff, 0xd9,
    ][..]]
    .concat();
    let f = d.join("in.jpg");
    std::fs::write(&f, &jpg).unwrap();
    let out = bin()
        .args(["--once", "--fps", "10", "--replay", f.to_str().unwrap()])
        .env("COGNITUM_COG_DATA_DIR", &d)
        .output()
        .unwrap();
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (
            r["status"].as_str(),
            r["width"].as_u64(),
            r["height"].as_u64()
        ),
        (Some("replay"), Some(320), Some(240))
    );
    let h = r["last_hash"].as_str().unwrap();
    assert_eq!(
        std::fs::read(d.join("frames").join(format!("{h}.jpg"))).unwrap(),
        jpg
    );
    std::fs::remove_dir_all(&d).ok();
}
