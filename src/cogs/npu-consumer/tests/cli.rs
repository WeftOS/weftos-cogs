//! The real binary against a stand-in broker: it cycles the models, logs every
//! call with the service's timings, and reports a refusal without stopping.

use std::os::unix::net::UnixListener;
use std::process::Command;

use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_cog-npu-consumer");

fn read_frame(s: &mut impl std::io::Read) -> Value {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).unwrap();
    let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
    s.read_exact(&mut buf).unwrap();
    serde_json::from_slice(&buf).unwrap()
}

fn write_frame(s: &mut impl std::io::Write, v: &Value) {
    let b = serde_json::to_vec(v).unwrap();
    s.write_all(&(b.len() as u32).to_be_bytes()).unwrap();
    s.write_all(&b).unwrap();
}

#[test]
fn help_and_simulate_exit_zero() {
    assert!(Command::new(BIN)
        .arg("--help")
        .output()
        .unwrap()
        .status
        .success());
    let o = Command::new(BIN)
        .args(["--once", "--simulate", "--models", "qwen3-4b"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(
        (v["provenance"].as_str(), v["model"].as_str()),
        (Some("SYNTHETIC"), Some("qwen3-4b"))
    );
    assert_eq!(
        Command::new(BIN)
            .env_remove("WEFTOS_COGBUS")
            .output()
            .unwrap()
            .status
            .code(),
        Some(2)
    );
    // --once alone is the host's run mode, not an error: it needs a broker like any run.
    let once = Command::new(BIN)
        .arg("--once")
        .env_remove("WEFTOS_COGBUS")
        .output()
        .unwrap();
    assert_eq!(once.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&once.stderr).contains("no cogbus"));
    // --simulate without --once is refused
    assert_eq!(
        Command::new(BIN)
            .arg("--simulate")
            .output()
            .unwrap()
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn it_cycles_models_and_logs_each_call() {
    let dir = std::env::temp_dir().join(format!("npu-consumer-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (sock, logf) = (dir.join("b.sock"), dir.join("log.jsonl"));
    let l = UnixListener::bind(&sock).unwrap();
    let child = Command::new(BIN)
        .args([
            "--models",
            "gemma3-1b,qwen3-4b",
            "--calls",
            "3",
            "--role",
            "a",
            "--summary-secs",
            "3600",
            "--log",
        ])
        .arg(&logf)
        .env("WEFTOS_COGBUS", &sock)
        .env("WEFTOS_COGBUS_TOKEN", "tok")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let (mut s, _) = l.accept().unwrap();
    let hello = read_frame(&mut s);
    write_frame(
        &mut s,
        &json!({"type": "ok", "id": hello["id"], "result": {}}),
    );
    let mut models = Vec::new();
    for i in 0..3 {
        let c = read_frame(&mut s);
        assert_eq!(
            (c["op"].as_str(), c["topic"].as_str(), c["schema"].as_str()),
            (Some("call"), Some("svc.infer/venq-npu"), Some("infer.v1"))
        );
        models.push(c["body"]["model"].as_str().unwrap().to_string());
        if i == 2 {
            write_frame(
                &mut s,
                &json!({"type": "error", "id": c["id"], "code": "service_error", "message": "unknown model"}),
            );
        } else {
            write_frame(
                &mut s,
                &json!({"type": "ok", "id": c["id"], "result": {"body": {
                "text": "hi", "queue_ms": 5, "load_ms": 100, "run_ms": 20, "loaded": i == 0, "usage": {"completion_tokens": 3}}}}),
            );
        }
    }
    assert_eq!(models, ["gemma3-1b", "qwen3-4b", "gemma3-1b"]);
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "some calls succeeded");
    let lines: Vec<Value> = std::fs::read_to_string(&logf)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let calls: Vec<&Value> = lines.iter().filter(|l| l["kind"] == "call").collect();
    assert_eq!(calls.len(), 3);
    assert_eq!(
        (
            calls[0]["ok"].as_bool(),
            calls[0]["loaded"].as_bool(),
            calls[0]["load_ms"].as_u64()
        ),
        (Some(true), Some(true), Some(100))
    );
    assert_eq!(
        (calls[2]["ok"].as_bool(), calls[2]["code"].as_str()),
        (Some(false), Some("service_error"))
    );
    let summary = lines.last().unwrap();
    assert_eq!(
        (
            summary["kind"].as_str(),
            summary["ok"].as_u64(),
            summary["failed"].as_u64()
        ),
        (Some("summary"), Some(2), Some(1))
    );
    std::fs::remove_dir_all(&dir).ok();
}
