//! The real binary: its flags, the record and replay of a capture, and its bus loop against a
//! stand-in broker (the weftos tests cover the real broker; this covers the wire contract from
//! the cog's side) with the simulated radio, so no Pluto is needed.

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_cog-sdr-pluto-rx");

fn read_frame(s: &mut impl Read) -> Option<Value> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).ok()?;
    let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
    s.read_exact(&mut buf).ok()?;
    serde_json::from_slice(&buf).ok()
}

fn frame_bytes(v: &Value) -> Vec<u8> {
    let b = serde_json::to_vec(v).unwrap();
    let mut out = (b.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(&b);
    out
}

fn write_frame(s: &mut impl Write, v: &Value) {
    s.write_all(&frame_bytes(v)).unwrap();
}

fn send_with_fd(s: &UnixStream, bytes: &[u8], fd: i32) {
    // SAFETY: a single-descriptor SCM_RIGHTS message built in a local buffer.
    unsafe {
        let cap = libc::CMSG_SPACE(std::mem::size_of::<i32>() as u32) as usize;
        let mut ctl = vec![0u8; cap];
        let mut iov = libc::iovec {
            iov_base: bytes.as_ptr() as *mut _,
            iov_len: bytes.len(),
        };
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = ctl.as_mut_ptr().cast();
        msg.msg_controllen = cap as _;
        let c = libc::CMSG_FIRSTHDR(&msg);
        (*c).cmsg_level = libc::SOL_SOCKET;
        (*c).cmsg_type = libc::SCM_RIGHTS;
        (*c).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<i32>() as u32) as _;
        std::ptr::write_unaligned(libc::CMSG_DATA(c).cast::<i32>(), fd);
        assert!(libc::sendmsg(s.as_raw_fd(), &msg, 0) >= 0);
    }
}

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("sdr-rx-cli-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn help_and_the_simulated_once_run_exit_zero() {
    let h = Command::new(BIN).arg("--help").output().unwrap();
    assert!(h.status.success());
    let t = String::from_utf8_lossy(&h.stdout);
    assert!(t.contains("--band-plan") && t.contains("--replay") && t.contains("--iio-buffer"));
    let o = Command::new(BIN)
        .args(["--once", "--simulate", "--sim-speed", "0", "--window", "1"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["status"], "streaming");
    assert_eq!(v["provenance"], "SYNTHETIC");
    assert!(v["samples"]["delivered"].as_u64().unwrap() >= 5_000_000);
    assert_eq!(v["samples"]["lost"], 0);
    assert_eq!(v["counters"]["gap_events"], 0);
    assert_eq!(v["vector"].as_array().unwrap().len(), 8);
    // a bad option is refused with a message, not a panic
    let bad = Command::new(BIN)
        .args(["--once", "--band-plan", "9000e6:5e6:100"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("outside"));
}

#[test]
fn without_a_radio_the_cog_reports_no_source() {
    // no libiio, or no radio on the backend: either way the report says so and the run is clean
    let o = Command::new(BIN)
        .args(["--once", "--uri", "ip:127.0.0.1:1"])
        .env("LIBIIO_PATH", "/nonexistent/libiio.so")
        .output()
        .unwrap();
    assert!(o.status.success());
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["status"], "no_source");
    assert!(v["error"].as_str().unwrap().len() > 3);
}

#[test]
fn a_recorded_window_replays_to_the_same_samples() {
    let d = scratch("rec");
    let f = d.join("cap.iq");
    let rec = Command::new(BIN)
        .args([
            "--once",
            "--simulate",
            "--sim-speed",
            "0",
            "--window",
            "1",
            "--rate",
            "2e6",
            "--iio-buffer",
            "16384",
            "--record",
        ])
        .arg(&f)
        .output()
        .unwrap();
    assert!(
        rec.status.success(),
        "{}",
        String::from_utf8_lossy(&rec.stderr)
    );
    let a: Value = serde_json::from_slice(&rec.stdout).unwrap();
    let rep = Command::new(BIN)
        .args(["--once", "--sim-speed", "0", "--replay"])
        .arg(&f)
        .output()
        .unwrap();
    assert!(
        rep.status.success(),
        "{}",
        String::from_utf8_lossy(&rep.stderr)
    );
    let b: Value = serde_json::from_slice(&rep.stdout).unwrap();
    assert_eq!(b["band"]["centre_hz"], a["band"]["centre_hz"]);
    assert_eq!(b["band"]["rate_sps"], 2_000_000);
    // the capture holds whole reads: the replay delivers what was recorded
    assert_eq!(b["samples"]["delivered"], a["samples"]["delivered"]);
    assert!((b["power_dbfs"].as_f64().unwrap() - a["power_dbfs"].as_f64().unwrap()).abs() < 1e-9);
    // a file that is not a capture is a no_source, not a crash
    std::fs::write(
        d.join("junk.iq"),
        b"not a capture, but longer than a header would be....",
    )
    .unwrap();
    let junk = Command::new(BIN)
        .args(["--once", "--replay"])
        .arg(d.join("junk.iq"))
        .output()
        .unwrap();
    assert!(junk.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&junk.stdout).unwrap()["status"],
        "no_source"
    );
    std::fs::remove_dir_all(&d).ok();
}

/// A capture of the 2.4 GHz band taken from the Pluto on the Ventuno Q.
#[test]
fn a_recorded_capture_from_the_board_replays_as_it_was_taken() {
    let f =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wifi-2437-5M.iq");
    let o = Command::new(BIN)
        .args(["--once", "--sim-speed", "0", "--replay"])
        .arg(&f)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["band"]["centre_hz"], 2_437_000_000u64);
    assert_eq!(v["band"]["rate_sps"], 5_000_000);
    assert_eq!(v["samples"]["delivered"], 65_536);
    assert_eq!(v["counters"]["zero_blocks"], 0);
    assert_eq!(v["counters"]["dead_receive_path"], false);
    let p = v["power_dbfs"].as_f64().unwrap();
    assert!((-25.0..-10.0).contains(&p), "power {p} dBFS");
}

struct Killer(Child);

impl Drop for Killer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Ring layout facts the stand-in needs (weftos-cogbus `stream/ring.rs`).
const HEADER: u64 = 128;

fn geometry(max_record: u64, ring_bytes: u64) -> (u64, u64) {
    let slot_size = (32 + max_record.div_ceil(8) * 8).div_ceil(64) * 64;
    ((ring_bytes - HEADER) / slot_size, slot_size)
}

#[test]
fn the_bus_loop_streams_a_ring_serves_calls_and_publishes_status() {
    let d = scratch("bus");
    let sock = d.join("b.sock");
    let l = UnixListener::bind(&sock).unwrap();
    let child = Command::new(BIN)
        .args([
            "--simulate",
            "--sim-speed",
            "20",
            "--api-bind",
            "127.0.0.1:0",
            "--no-store",
            "--publish-secs",
            "1",
            "--interval",
            "3600",
        ])
        .env("WEFTOS_COGBUS", &sock)
        .env("WEFTOS_COGBUS_TOKEN", "tok")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _kill = Killer(child);
    let (mut s, _) = l.accept().unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let hello = read_frame(&mut s).unwrap();
    assert_eq!(
        (hello["op"].as_str(), hello["token"].as_str()),
        (Some("hello"), Some("tok"))
    );
    write_frame(
        &mut s,
        &json!({"type": "ok", "id": 1, "result": {"instance_id": "i1", "node_id": "0123456789abcdef0123456789abcdef"}}),
    );

    let ring_path = d.join("ring");
    let mut ring: Option<(std::fs::File, u64, u64)> = None;
    let mut status_reading: Option<Value> = None;
    let mut served = false;
    let deadline = Instant::now() + Duration::from_secs(30);
    // read the cog's requests until the ring has records and a status reading has come in
    while Instant::now() < deadline {
        if let Some((f, count, size)) = &ring {
            let mut head = [0u8; 8];
            f.read_exact_at(&mut head, 64).unwrap();
            if u64::from_le_bytes(head) >= 12 && status_reading.is_some() && served {
                let _ = (count, size);
                break;
            }
        }
        s.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let Some(f) = read_frame(&mut s) else {
            continue;
        };
        match f["op"].as_str() {
            Some("serve") => {
                assert_eq!(
                    (f["topic_kind"].as_str(), f["schema"].as_str()),
                    (Some("svc.sdr"), Some("sdr.v1"))
                );
                write_frame(&mut s, &json!({"type": "ok", "id": f["id"], "result": {}}));
                served = true;
            }
            Some("stream_open") => {
                assert_eq!(
                    (f["topic_kind"].as_str(), f["schema"].as_str()),
                    (Some("iq.cs16"), Some("iq.cs16.v1"))
                );
                let max_record = f["max_record"].as_u64().unwrap();
                let (count, size) = geometry(max_record, f["ring_bytes"].as_u64().unwrap());
                let file = std::fs::File::options()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&ring_path)
                    .unwrap();
                file.set_len(HEADER + count * size).unwrap();
                for (off, v) in [
                    (0u64, u64::from_le_bytes(*b"COGRING1")),
                    (8, 1),
                    (16, count),
                    (24, size),
                    (32, max_record),
                    (40, 1),
                ] {
                    file.write_all_at(&v.to_le_bytes(), off).unwrap();
                }
                let body = frame_bytes(&json!({"type": "ok", "id": f["id"], "result": {
                    "fds": 1, "ring_id": 1, "block_deadline_ms": 50,
                    "geometry": {"slot_count": count, "slot_size": size, "max_record": max_record}}}));
                send_with_fd(&s, &body, file.as_raw_fd());
                ring = Some((file, count, size));
            }
            Some("publish") => {
                assert_eq!(
                    (f["topic_kind"].as_str(), f["schema"].as_str()),
                    (Some("sdr.status"), Some("telemetry.v1"))
                );
                status_reading = Some(f["body"].clone());
            }
            _ => {}
        }
    }
    let (file, count, size) = ring.expect("the cog opened its ring");
    assert!(served, "the cog served svc.sdr");

    // the records in the ring are registered iq.cs16.v1 messages with consecutive seq
    let mut head = [0u8; 8];
    file.read_exact_at(&mut head, 64).unwrap();
    let head = u64::from_le_bytes(head);
    assert!(head >= 12, "head {head}");
    let first = head.saturating_sub(100.min(count - 20)).max(1);
    let mut last_idx: Option<u64> = None;
    for seq in first..first + 8 {
        let base = HEADER + ((seq - 1) % count) * size;
        let mut slot = vec![0u8; size as usize];
        file.read_exact_at(&mut slot, base).unwrap();
        assert_eq!(
            u64::from_le_bytes(slot[0..8].try_into().unwrap()),
            seq,
            "slot seq"
        );
        let len = u64::from_le_bytes(slot[8..16].try_into().unwrap()) as usize;
        let msg: Value = serde_json::from_slice(&slot[32..32 + len]).unwrap();
        assert_eq!(
            msg["seq"].as_u64(),
            Some(seq),
            "the message seq is the ring seq"
        );
        assert_eq!(msg["source_id"], "venq-pluto");
        assert_eq!(msg["node_id"], "0123456789abcdef0123456789abcdef");
        assert_eq!(
            (
                msg["center_hz"].as_u64(),
                msg["sample_rate_hz"].as_u64(),
                msg["samples"].as_u64()
            ),
            (Some(2_437_000_000), Some(5_000_000), Some(8192))
        );
        let b64 = msg["data_b64"].as_str().unwrap();
        assert_eq!(b64.len(), (8192usize * 4).div_ceil(3) * 4);
        if let Some(prev) = last_idx {
            assert_eq!(
                msg["sample_index"].as_u64(),
                Some(prev + 8192),
                "the sample clock advances by one record"
            );
        }
        last_idx = msg["sample_index"].as_u64();
    }

    // the status reading is telemetry.v1 with the loss numbers
    let r = status_reading.unwrap();
    let names: Vec<&str> = r["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    for want in [
        "samples.delivered",
        "samples.expected",
        "samples.lost",
        "gap.events",
        "temp.phy",
        "lo",
    ] {
        assert!(names.contains(&want), "{want} in {names:?}");
    }

    // calls: status works, tx_config is refused, a bad tune is refused
    let mut answers = Vec::new();
    for (call, body) in [
        (11, json!({"method": "status"})),
        (12, json!({"method": "tx_config", "attenuation_db": -10})),
        (13, json!({"method": "tune", "centre_hz": 1e6})),
        (
            14,
            json!({"method": "tune", "centre_hz": 915e6, "rate_sps": 2e6}),
        ),
    ] {
        write_frame(
            &mut s,
            &json!({"type": "call", "call": call, "from": "consumer-a", "timeout_ms": 5000, "body": body}),
        );
        loop {
            let f = read_frame(&mut s).expect("an answer to the call");
            if f["op"] == "reply" && f["call"] == call {
                answers.push(f);
                break;
            }
        }
    }
    assert_eq!(answers[0]["body"]["band"]["centre_hz"], 2_437_000_000u64);
    assert!(answers[1]["error"].as_str().unwrap().contains("refused"));
    assert!(answers[2]["error"].as_str().unwrap().contains("outside"));
    assert_eq!(answers[3]["body"]["ok"], true);
    std::fs::remove_dir_all(&d).ok();
}
