//! The real binary: its flags, the replay of recorded fixtures, and its bus loop against a
//! stand-in broker (the weftos tests cover the real broker; this covers the wire contract from
//! the cog's side): the stand-in hands it a ring, writes the owner's `iq.cs16.v1` messages into
//! it, and checks what the cog publishes.

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_cog-spectrum");

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

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

fn b64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        s.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    s
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("spec-cli-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run_once(args: &[&str]) -> Value {
    let o = Command::new(BIN).args(args).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_slice(&o.stdout).unwrap()
}

fn items(v: &Value) -> std::collections::BTreeMap<String, f64> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["name"].as_str().unwrap().to_string(),
                i["value"].as_f64().unwrap(),
            )
        })
        .collect()
}

#[test]
fn help_and_the_simulated_once_run_exit_zero() {
    let h = Command::new(BIN).arg("--help").output().unwrap();
    assert!(h.status.success());
    let t = String::from_utf8_lossy(&h.stdout);
    assert!(t.contains("--ask-plan") && t.contains("--replay") && t.contains("--thresh-db"));
    let v = run_once(&["--once", "--simulate"]);
    assert_eq!(v["provenance"], "SYNTHETIC");
    assert_eq!(v["samples"]["loss_fraction"], 0.0);
    assert!(v["readings"]["spectrum.psd"].as_u64().unwrap() >= 1);
    assert_eq!(v["vector"].as_array().unwrap().len(), 8);
    let bad = Command::new(BIN).args(["--fft", "1000"]).output().unwrap();
    assert_eq!(bad.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("power of two"));
}

#[test]
fn without_a_bus_or_an_input_the_cog_reports_no_source() {
    let o = Command::new(BIN)
        .env_remove("WEFTOS_COGBUS")
        .env_remove("WEFTOS_COGBUS_TOKEN")
        .output()
        .unwrap();
    assert!(o.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&o.stdout).unwrap()["status"],
        "no_source"
    );
    let o = Command::new(BIN).arg("--once").output().unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&o.stdout).unwrap()["status"],
        "no_source"
    );
    let d = scratch("junk");
    std::fs::write(
        d.join("junk.iq"),
        b"not a capture, but longer than a header would be....",
    )
    .unwrap();
    let o = Command::new(BIN)
        .args(["--once", "--replay"])
        .arg(d.join("junk.iq"))
        .output()
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&o.stdout).unwrap()["status"],
        "no_source"
    );
    std::fs::remove_dir_all(&d).ok();
}

/// A capture of the 2.4 GHz band from the Pluto on the Ventuno Q: Wi-Fi traffic in the room.
#[test]
fn a_recorded_wifi_capture_has_a_floor_and_a_peak_and_loses_nothing() {
    let f = fixture("wifi-2437-5M.iq");
    let v = run_once(&["--once", "--replay", f.to_str().unwrap()]);
    assert_eq!(v["band"]["centre_hz"], 2_437_000_000u64);
    assert_eq!(v["input_samples"], 65_536);
    assert_eq!(v["samples"]["used"], 65_536);
    assert_eq!(v["samples"]["loss_fraction"], 0.0);
    let psd = &v["psd"];
    assert_eq!(psd["bins"].as_array().unwrap().len(), 256);
    assert_eq!(psd["uncalibrated"], false);
    let (floor, peak) = (
        psd["floor_dbfs"].as_f64().unwrap(),
        psd["peak_dbfs"].as_f64().unwrap(),
    );
    // the gain was automatic; the floor of a 12-bit converter's bin is far below full scale, and
    // something in the room is well above it
    assert!((-80.0..-30.0).contains(&floor), "floor {floor}");
    assert!(peak > floor + 10.0 && peak < 0.0, "peak {peak}");
    let occ = items(&v["occupancy_reading"]);
    assert!(occ["occ.overall"] >= 0.0 && occ["occ.overall"] < 0.5);
    assert_eq!(occ["samples.used"], 65_536.0);
    assert_eq!(occ["uncalibrated"], 0.0);
}

/// Captures from the same board: the 1090 MHz ADS-B band, a 433.92 MHz ISM channel and the FM
/// broadcast band at 98 MHz, which is outside the chip's rated range.
#[test]
fn recorded_captures_of_other_bands_are_read_with_the_right_labels() {
    for (name, centre, rate, bw, uncal) in [
        (
            "adsb-1090-2M4.iq",
            1_090_000_000u64,
            2_400_000u64,
            1_920_000u64,
            false,
        ),
        ("ism-433-2M.iq", 433_920_000, 2_000_000, 1_600_000, false),
        ("fm-98-5M.iq", 98_000_000, 5_000_000, 4_000_000, true),
    ] {
        let f = fixture(name);
        let v = run_once(&["--once", "--replay", f.to_str().unwrap()]);
        let p = &v["psd"];
        assert_eq!(
            (
                p["center_hz"].as_u64(),
                p["sample_rate_hz"].as_u64(),
                p["bandwidth_hz"].as_u64()
            ),
            (Some(centre), Some(rate), Some(bw)),
            "{name}"
        );
        assert_eq!(p["uncalibrated"], uncal, "{name}");
        assert_eq!(v["samples"]["used"], 65_536, "{name}");
        assert_eq!(v["samples"]["loss_fraction"], 0.0, "{name}");
        let (floor, peak) = (
            p["floor_dbfs"].as_f64().unwrap(),
            p["peak_dbfs"].as_f64().unwrap(),
        );
        assert!((-110.0..-40.0).contains(&floor), "{name} floor {floor}");
        assert!(
            peak > floor + 10.0 && peak < 0.0,
            "{name} peak {peak} floor {floor}"
        );
        assert_eq!(p["bins"].as_array().unwrap().len(), 256);
        let occ = items(&v["occupancy_reading"]);
        assert_eq!(occ["uncalibrated"], if uncal { 1.0 } else { 0.0 }, "{name}");
        assert!(
            (0.0..0.5).contains(&occ["occ.overall"]),
            "{name} occupancy {}",
            occ["occ.overall"]
        );
        assert_eq!(occ["band.centre"], centre as f64, "{name}");
    }
}

struct Killer(Child);

impl Drop for Killer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

const HEADER: u64 = 128;

/// The owner's message for record `k`: a tone `offset` bins from the centre, plus noise.
fn message(k: u64, idx: u64, offset: i32, lost: u64) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8192 * 4);
    let mut r = (k + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    for t in 0..8192u64 {
        let mut nz = || {
            r ^= r << 13;
            r ^= r >> 7;
            r ^= r << 17;
            ((r >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 100.0
        };
        let ph = 2.0 * std::f64::consts::PI * f64::from(offset) * (idx + t) as f64 / 1024.0;
        bytes.extend_from_slice(&((1200.0 * ph.cos() + nz()) as i16).to_le_bytes());
        bytes.extend_from_slice(&((1200.0 * ph.sin() + nz()) as i16).to_le_bytes());
    }
    let mut m = json!({
        "source_id": "venq-pluto", "node_id": "0123456789abcdef0123456789abcdef", "t_ns": 1_000_000 + k, "seq": k + 1,
        "center_hz": 2_437_000_000u64, "sample_rate_hz": 5_000_000, "samples": 8192, "data_b64": b64(&bytes),
        "bandwidth_hz": 4_000_000, "segment": 1, "sample_index": idx, "lost": lost,
    });
    if lost > 0 {
        m["discontinuity"] = json!(true);
    }
    serde_json::to_vec(&m).unwrap()
}

#[test]
fn the_bus_loop_reads_a_ring_and_publishes_spectrum_and_occupancy_with_loss_counted() {
    let d = scratch("bus");
    let sock = d.join("b.sock");
    let l = UnixListener::bind(&sock).unwrap();
    let child = Command::new(BIN)
        .args([
            "--psd-secs",
            "1",
            "--occ-secs",
            "2",
            "--no-store",
            "--api-bind",
            "127.0.0.1:0",
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
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let hello = read_frame(&mut s).unwrap();
    assert_eq!(hello["token"], "tok");
    s.write_all(&frame_bytes(&json!({"type": "ok", "id": 1, "result": {"instance_id": "i1", "node_id": "0123456789abcdef0123456789abcdef"}}))).unwrap();
    let sub = read_frame(&mut s).unwrap();
    assert_eq!(
        (
            sub["op"].as_str(),
            sub["topic"].as_str(),
            sub["schema"].as_str()
        ),
        (
            Some("stream_subscribe"),
            Some("iq.cs16/venq-pluto"),
            Some("iq.cs16.v1")
        )
    );

    // the ring, empty: the cog reads from the head on
    let (count, size) = (48u64, 64 * ((32 + 45_000u64).div_ceil(64)));
    let ring = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(d.join("ring"))
        .unwrap();
    ring.set_len(HEADER + count * size).unwrap();
    for (off, v) in [
        (0u64, u64::from_le_bytes(*b"COGRING1")),
        (8, 1),
        (16, count),
        (24, size),
        (32, 45_000),
        (40, 1),
    ] {
        ring.write_all_at(&v.to_le_bytes(), off).unwrap();
    }
    let body = frame_bytes(&json!({"type": "ok", "id": sub["id"], "result": {
        "sub": 1, "policy": "drop-oldest", "cursor": false, "fds": 1,
        "ring": {"ring_id": 1, "head": 0, "geometry": {"slot_count": count, "slot_size": size, "max_record": 45_000}}}}));
    send_with_fd(&s, &body, ring.as_raw_fd());

    // the owner writes about 3.2 s of 5 MS/s: a tone 100 bins above the centre, and a declared
    // loss of 200000 samples after record 600 that the next chunk carries as `lost`
    let writer = {
        let ring = ring.try_clone().unwrap();
        std::thread::spawn(move || {
            let mut idx = 0u64;
            for k in 0..1950u64 {
                let lost = if k == 600 { 200_000 } else { 0 };
                idx += lost;
                let m = message(k, idx, 100, lost);
                let seq = k + 1;
                let base = HEADER + ((seq - 1) % count) * size;
                ring.write_all_at(&u64::MAX.to_le_bytes(), base).unwrap();
                ring.write_all_at(&(m.len() as u64).to_le_bytes(), base + 8)
                    .unwrap();
                ring.write_all_at(&(1_000_000 + k).to_le_bytes(), base + 16)
                    .unwrap();
                ring.write_all_at(&0u64.to_le_bytes(), base + 24).unwrap();
                ring.write_all_at(&m, base + 32).unwrap();
                ring.write_all_at(&seq.to_le_bytes(), base).unwrap();
                ring.write_all_at(&seq.to_le_bytes(), 64).unwrap();
                idx += 8192;
                std::thread::sleep(Duration::from_micros(1600));
            }
        })
    };

    let (mut psd, mut occ): (Vec<Value>, Vec<Value>) = (Vec::new(), Vec::new());
    let deadline = Instant::now() + Duration::from_secs(20);
    s.set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    while Instant::now() < deadline && (psd.len() < 2 || occ.is_empty()) {
        let Some(f) = read_frame(&mut s) else {
            continue;
        };
        if f["op"] != "publish" {
            continue;
        }
        match f["topic_kind"].as_str() {
            Some("spectrum.psd") => {
                assert_eq!(f["schema"], "spectrum.psd.v1");
                psd.push(f["body"].clone());
            }
            Some("band.occupancy") => {
                assert_eq!(f["schema"], "telemetry.v1");
                occ.push(f["body"].clone());
            }
            other => panic!("unexpected topic {other:?}"),
        }
    }
    writer.join().unwrap();
    assert!(psd.len() >= 2, "{} spectrum readings", psd.len());
    assert!(!occ.is_empty(), "an occupancy reading");
    for p in &psd {
        assert_eq!(
            (p["center_hz"].as_u64(), p["span_hz"].as_u64()),
            (Some(2_437_000_000), Some(5_000_000))
        );
        assert_eq!(p["source_id"], "venq-pluto");
        let bins = p["bins"].as_array().unwrap();
        assert_eq!(bins.len(), 256);
        // the tone is 100 bins above the centre of 1024: output bin (512 + 100) / 4
        let peak = bins
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.as_f64().unwrap().total_cmp(&b.1.as_f64().unwrap()))
            .unwrap()
            .0;
        assert!((peak as i32 - 153).abs() <= 1, "peak bin {peak}");
    }
    // the declared loss is in exactly one window, and counted once
    let lost: u64 = psd
        .iter()
        .map(|p| p["samples_lost"].as_u64().unwrap())
        .sum::<u64>()
        + occ.iter().map(|_| 0).sum::<u64>();
    assert_eq!(lost, 200_000, "lost samples across the spectrum readings");
    let o = items(&occ[0]);
    assert_eq!(o["band.centre"], 2_437_000_000.0);
    assert!(o["occ.overall"] >= 0.0 && o["samples.used"] > 1_000_000.0);
    assert!(o["samples.lost"] <= 200_000.0);
    std::fs::remove_dir_all(&d).ok();
}
