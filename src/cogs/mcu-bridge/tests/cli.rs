use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cog-mcu-bridge"))
}

#[test]
fn help_names_the_cog_and_console_commands() {
    let out = bin().arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("cog-mcu-bridge"));
    assert!(text.contains("--once --simulate"));
}

#[test]
fn simulate_probes_a_fake_router() {
    let out = bin().args(["--once", "--simulate"]).output().unwrap();
    assert!(out.status.success());
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (
            r["status"].as_str(),
            r["provenance"].as_str(),
            r["mcu"].as_str()
        ),
        (Some("simulate"), Some("SYNTHETIC"), Some("ok"))
    );
    assert_eq!(r["sketch_calls"], 1);
}

#[test]
fn without_a_router_it_says_no_source() {
    let out = bin()
        .args(["--once", "--socket", "/nonexistent/arduino-router.sock"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (r["status"].as_str(), r["mcu"].as_str()),
        (Some("no_source"), Some("no_router"))
    );
    assert!(
        r["error"].as_str().unwrap().contains("router unavailable"),
        "{r}"
    );
    assert_eq!(r["vector"], serde_json::json!(vec![-1.0f64; 8]));
}

#[test]
fn a_replayed_foreign_sketch_is_reported_as_such() {
    let dir = std::env::temp_dir().join(format!("mcu-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("r.jsonl");
    std::fs::write(&f, "{\"method\":\"blink\",\"result\":0}\n").unwrap();
    let out = bin()
        .args(["--once", "--replay", f.to_str().unwrap()])
        .output()
        .unwrap();
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (r["status"].as_str(), r["mcu"].as_str()),
        (Some("foreign_sketch"), Some("foreign-sketch"))
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A stand-in router on a real Unix socket: the binary talks msgpack to it over the wire.
#[test]
fn the_binary_probes_a_router_socket() {
    let dir = std::env::temp_dir().join(format!("mcu-cli-sock-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("arduino-router.sock");
    let l = UnixListener::bind(&path).unwrap();
    let h = std::thread::spawn(move || {
        for answer in [0x2au8, 0x07] {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = [0u8; 128];
            let n = s.read(&mut buf).unwrap();
            // [0, msgid, "weftos_...", []]: fixarray(4), fixint 0, msgid, fixstr...
            assert_eq!((buf[0], buf[1]), (0x94, 0x00));
            let id = buf[2];
            assert!(n > 4);
            // [1, id, nil, answer]; 0x2a is a fixint 42, and 7 is the counter.
            s.write_all(&[0x94, 0x01, id, 0xc0, answer]).unwrap();
        }
    });
    let out = bin()
        .args(["--once", "--socket", path.to_str().unwrap()])
        .output()
        .unwrap();
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (
            r["status"].as_str(),
            r["mcu"].as_str(),
            r["sketch_calls"].as_u64()
        ),
        (Some("ok"), Some("ok"), Some(7)),
        "{r}"
    );
    h.join().unwrap();
    std::fs::remove_dir_all(&dir).ok();
}
