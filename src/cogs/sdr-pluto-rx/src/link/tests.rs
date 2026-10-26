use std::os::unix::net::UnixListener;

use super::*;
use crate::bus::{read_frame, write_frame};

fn broker(name: &str) -> (String, UnixListener) {
    let dir = std::env::temp_dir().join(format!("sdr-link-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("b.sock");
    let l = UnixListener::bind(&path).unwrap();
    (path.to_string_lossy().into_owned(), l)
}

#[test]
fn calls_reach_the_tuner_and_the_answers_go_back() {
    let (path, l) = broker("calls");
    let h = std::thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        let hello = read_frame(&mut s).unwrap().unwrap();
        assert_eq!(hello["token"], "tok");
        write_frame(
            &mut s,
            &json!({"type": "ok", "id": 1, "result": {"node_id": "venq"}}),
        )
        .unwrap();
        let serve = read_frame(&mut s).unwrap().unwrap();
        assert_eq!(
            (
                serve["topic_kind"].as_str(),
                serve["source_id"].as_str(),
                serve["schema"].as_str()
            ),
            (Some("svc.sdr"), Some("venq-pluto"), Some("sdr.v1"))
        );
        write_frame(
            &mut s,
            &json!({"type": "ok", "id": serve["id"], "result": {}}),
        )
        .unwrap();
        let mut answers = Vec::new();
        for (call, body) in [
            (1, json!({"method": "status"})),
            (2, json!({"method": "tx_config", "attenuation_db": -20})),
            (3, json!({"method": "tune", "centre_hz": 5e6})),
        ] {
            write_frame(&mut s, &json!({"type": "call", "call": call, "from": "c1", "timeout_ms": 3000, "body": body})).unwrap();
            // the connection thread also asks for the IQ ring: refuse it, as a broker without the
            // stream class would
            loop {
                let f = read_frame(&mut s).unwrap().unwrap();
                if f["op"] == "stream_open" {
                    write_frame(&mut s, &json!({"type": "error", "id": f["id"], "code": "unsupported", "message": "no rings here"})).unwrap();
                    continue;
                }
                answers.push(f);
                break;
            }
        }
        answers
    });
    let link = Arc::new(Link::default());
    let (tx, rx) = channel::<Envelope>();
    let shared = Arc::new(Shared::default());
    spawn(
        LinkOpts {
            path,
            token: "tok".into(),
            service: "venq-pluto".into(),
            max_record: 4096,
            ring_bytes: 1 << 20,
        },
        link.clone(),
        tx,
        shared.clone(),
    );
    // the tuner: answer status the way the acquisition thread would
    let tuner = std::thread::spawn(move || {
        let (cmd, reply) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(cmd, svc::Cmd::Status);
        reply.send(Ok(json!({"status": "streaming"}))).unwrap();
        rx
    });
    let answers = h.join().unwrap();
    assert_eq!(answers[0]["op"], "reply");
    assert_eq!(answers[0]["body"]["status"], "streaming");
    assert!(answers[1]["error"].as_str().unwrap().contains("refused"));
    assert!(
        answers[2]["error"].as_str().unwrap().contains("outside"),
        "{}",
        answers[2]
    );
    let _rx = tuner.join().unwrap();
    shared.stop.store(true, Ordering::Release);
}

#[test]
fn the_status_reading_carries_the_loss_numbers() {
    let doc = json!({
        "healthy": true,
        "hw": {"lo_hz": 2437000000u64, "rate_sps": 5000000, "bw_hz": 4000000, "gain_db": 40.0, "rssi_db": null, "temp_phy_c": 41.5, "temp_zynq_c": 61.0},
        "power_dbfs": -24.0,
        "samples": {"delivered": 100, "expected": 120, "lost": 15, "pending_unconfirmed": 5, "loss_fraction": 0.125},
        "counters": {"gap_events": 2, "overflows": 0, "zero_blocks": 0, "dead_receive_path": false, "retunes": 3},
    });
    let b = status_body("venq-pluto", "venq", 9, 4, &doc);
    assert_eq!(
        (b["source_id"].as_str(), b["seq"].as_u64()),
        (Some("venq-pluto"), Some(4))
    );
    let items = b["items"].as_array().unwrap();
    let get = |n: &str| {
        items
            .iter()
            .find(|i| i["name"] == n)
            .map(|i| i["value"].as_f64().unwrap())
    };
    assert_eq!(get("samples.lost"), Some(15.0));
    assert_eq!(get("samples.expected"), Some(120.0));
    assert_eq!(get("healthy"), Some(1.0));
    assert_eq!(get("dead.receive.path"), Some(0.0));
    assert_eq!(
        get("rssi"),
        None,
        "a null reading is left out, not reported as zero"
    );
    assert!(items.len() <= 64);
}

#[test]
fn without_a_connection_records_are_dropped_quietly() {
    let mut o = BusOut::new(Arc::new(Link::default()), "venq-pluto");
    let h = iq::Header {
        kind: iq::KIND_SAMPLES,
        flags: 0,
        segment: 1,
        centre_hz: 2_437_000_000,
        rate_sps: 5_000_000,
        bw_hz: 4_000_000,
        sample_index: 0,
        lost: 0,
        n_samples: 4,
    };
    assert_eq!(o.record(1, &h, &[0u8; 16]).unwrap(), 0);
    o.status(1, &json!({}));
    assert_eq!(o.info()["connected"], false);
}

#[test]
fn the_ring_task_writes_registered_messages_in_order() {
    use crate::ring::{test_ring, Geometry};
    let link = Arc::new(Link::default());
    let g = Geometry {
        slot_count: 8,
        slot_size: (max_record_for(256) as usize).div_ceil(64) as u64 * 64 + 64,
        max_record: max_record_for(256),
    };
    let (w, r) = test_ring(g);
    *link.ring.lock().unwrap() = Some((0, w));
    let stats = Arc::new(RingStats::default());
    let mut t = RingTask {
        link: link.clone(),
        source: "venq-pluto".into(),
        ring: None,
        stats: stats.clone(),
    };
    for i in 0..3u64 {
        let head = iq::Header {
            kind: iq::KIND_SAMPLES,
            flags: if i == 0 { iq::F_BAND_CHANGED } else { 0 },
            segment: 1,
            centre_hz: 2_437_000_000,
            rate_sps: 5_000_000,
            bw_hz: 4_000_000,
            sample_index: i * 256,
            lost: 0,
            n_samples: 256,
        };
        t.write(&Job {
            t_ns: 100 + i,
            head,
            samples: vec![i as u8; 1024],
        });
    }
    assert_eq!(stats.seq.load(Ordering::Relaxed), 3);
    assert_eq!(link.last_seq.load(Ordering::Relaxed), 3);
    let mut next = 1;
    for i in 0..3u64 {
        let rec = r.read(&mut next).unwrap();
        assert_eq!((rec.seq, rec.t_ns), (i + 1, 100 + i));
        let v: Value = serde_json::from_slice(&rec.data).unwrap();
        assert_eq!(v["seq"], i + 1, "the message seq is the ring seq");
        assert_eq!(v["node_id"].as_str().unwrap().len(), 32);
        let (h, body) = iq::Header::decode(&rec.data).unwrap();
        assert_eq!(
            (h.sample_index, body.len(), body[0]),
            (i * 256, 1024, i as u8)
        );
    }
}
