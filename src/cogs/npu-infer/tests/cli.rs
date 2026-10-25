//! The real binary: its flags, and its bus loop against a stand-in broker
//! (the weftos tests cover the real broker; this covers the wire contract
//! from the cog's side) with the synthetic backend, so no GenieX is needed.

use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_cog-npu-infer");

fn read_frame(s: &mut impl Read) -> Value {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).unwrap();
    let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
    s.read_exact(&mut buf).unwrap();
    serde_json::from_slice(&buf).unwrap()
}

fn write_frame(s: &mut impl Write, v: &Value) {
    let b = serde_json::to_vec(v).unwrap();
    s.write_all(&(b.len() as u32).to_be_bytes()).unwrap();
    s.write_all(&b).unwrap();
}

#[test]
fn help_and_the_simulated_once_run_exit_zero() {
    let h = Command::new(BIN).arg("--help").output().unwrap();
    assert!(h.status.success() && String::from_utf8_lossy(&h.stdout).contains("--budget-mib"));
    let o = Command::new(BIN)
        .args(["--once", "--simulate"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["status"], "simulate");
    assert_eq!(v["provenance"], "SYNTHETIC");
    assert!(v["answer"]["text"].as_str().unwrap().contains("ping"));
}

#[test]
fn without_a_broker_or_with_a_bare_once_it_refuses_to_start() {
    let no_bus = Command::new(BIN)
        .env_remove("WEFTOS_COGBUS")
        .env_remove("WEFTOS_COGBUS_TOKEN")
        .output()
        .unwrap();
    assert_eq!(no_bus.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&no_bus.stderr).contains("WEFTOS_COGBUS"));
    let once = Command::new(BIN).arg("--once").output().unwrap();
    assert_eq!(once.status.code(), Some(2));
    let nowrap = Command::new(BIN)
        .args([
            "--bus",
            "/nonexistent.sock",
            "--geniex-bin",
            "/nonexistent/geniex",
        ])
        .env("WEFTOS_COGBUS_TOKEN", "t")
        .output()
        .unwrap();
    assert_eq!(nowrap.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&nowrap.stderr).contains("wrapper not found"));
}

#[test]
fn it_serves_the_topic_answers_calls_refuses_unknown_models_and_publishes_telemetry() {
    let dir = std::env::temp_dir().join(format!("npu-infer-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cogbus.sock");
    let l = UnixListener::bind(&path).unwrap();
    let mut child = Command::new(BIN)
        .args([
            "--simulate",
            "--publish-secs",
            "1",
            "--sd-url",
            "none",
            "--service",
            "venq-npu",
        ])
        .env("WEFTOS_COGBUS", &path)
        .env("WEFTOS_COGBUS_TOKEN", "tok")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let (mut s, _) = l.accept().unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let hello = read_frame(&mut s);
    assert_eq!(
        (hello["op"].as_str(), hello["token"].as_str()),
        (Some("hello"), Some("tok"))
    );
    let node = "0123456789abcdef0123456789abcdef";
    write_frame(
        &mut s,
        &json!({"type": "ok", "id": hello["id"], "result": {"instance_id": "i", "node_id": node}}),
    );
    let mut telemetry = None;
    let serve = loop {
        let f = read_frame(&mut s);
        match f["op"].as_str() {
            Some("serve") => break f,
            Some("publish") => telemetry = Some(f),
            other => panic!("unexpected {other:?}"),
        }
    };
    assert_eq!(
        (
            serve["topic_kind"].as_str(),
            serve["source_id"].as_str(),
            serve["schema"].as_str()
        ),
        (Some("svc.infer"), Some("venq-npu"), Some("infer.v1"))
    );
    write_frame(
        &mut s,
        &json!({"type": "ok", "id": serve["id"], "result": {}}),
    );
    // a good call, a high-priority one behind it, and one for a model that is not served
    write_frame(
        &mut s,
        &json!({"type": "call", "call": 5, "timeout_ms": 5000, "from": "a", "topic": "svc.infer/venq-npu",
        "body": {"model": "qwen3-4b", "text": "hello npu", "priority": 7}}),
    );
    write_frame(
        &mut s,
        &json!({"type": "call", "call": 6, "timeout_ms": 5000, "from": "b", "topic": "svc.infer/venq-npu",
        "body": {"model": "gemma4-e4b", "text": "x"}}),
    );
    let (mut ok, mut refused) = (None, None);
    while ok.is_none() || refused.is_none() || telemetry.is_none() {
        let f = read_frame(&mut s);
        match (f["op"].as_str(), f["call"].as_u64()) {
            (Some("reply"), Some(5)) => ok = Some(f),
            (Some("reply"), Some(6)) => refused = Some(f),
            (Some("publish"), _) => telemetry = Some(f),
            other => panic!("unexpected {other:?}: {f}"),
        }
    }
    let ok = ok.unwrap();
    assert!(
        ok["body"]["text"].as_str().unwrap().contains("hello npu"),
        "{ok}"
    );
    assert_eq!(ok["body"]["model"], "qwen3-4b");
    let refused = refused.unwrap();
    assert!(
        refused["error"].as_str().unwrap().contains("unknown model"),
        "{refused}"
    );
    let t = telemetry.unwrap();
    assert_eq!(
        (t["topic_kind"].as_str(), t["schema"].as_str()),
        (Some("telemetry"), Some("telemetry.v1"))
    );
    assert_eq!(t["body"]["node_id"], node);
    assert!(t["body"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["name"] == "queue.depth"));
    child.kill().unwrap();
    child.wait().unwrap();
    std::fs::remove_dir_all(&dir).ok();
}
