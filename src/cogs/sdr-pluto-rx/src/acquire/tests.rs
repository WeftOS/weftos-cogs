use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::*;
use crate::plan::{Band, Plan, DEFAULT_PLAN};
use crate::radio::Sim;

#[derive(Default)]
struct Captured {
    recs: Vec<(u64, Vec<u8>)>,
    seqs: Vec<u64>,
    statuses: Vec<Value>,
}

struct VecOut(Arc<Mutex<Captured>>);

impl Out for VecOut {
    fn record(&mut self, t_ns: u64, h: &Header, samples: &[u8]) -> Result<u64, String> {
        let mut c = self.0.lock().unwrap();
        let seq = c.recs.len() as u64 + 1;
        let meta = iq::Meta {
            source_id: "test",
            node_id: "0123456789abcdef0123456789abcdef",
        };
        c.recs.push((t_ns, h.encode(samples, &meta, t_ns, seq)));
        c.seqs.push(seq);
        Ok(seq)
    }

    fn status(&mut self, _t: u64, doc: &Value) {
        self.0.lock().unwrap().statuses.push(doc.clone());
    }
}

fn settings(max_blocks: Option<u64>) -> Settings {
    Settings {
        source: "test".into(),
        backend: "test".into(),
        status_period: Duration::from_millis(1),
        hw_period: Duration::from_millis(1),
        publish_period: Duration::from_millis(1),
        loss: loss::Params::default(),
        max_blocks,
        raw_samples: 256,
        record_samples: 1024,
    }
}

fn acq(bands: Vec<Band>, max_blocks: Option<u64>) -> (Acq, Arc<Mutex<Captured>>, Sender<Envelope>) {
    let cap = Arc::new(Mutex::new(Captured::default()));
    let (tx, rx) = channel();
    let mut s = Scheduler::new();
    s.set_plan(Plan {
        id: DEFAULT_PLAN.into(),
        owner: "@cog".into(),
        bands,
    })
    .unwrap();
    let a = Acq::new(
        s,
        settings(max_blocks),
        Box::new(VecOut(cap.clone())),
        Arc::new(Shared::default()),
        rx,
    );
    (a, cap, tx)
}

fn decode(c: &Captured) -> Vec<Header> {
    c.recs
        .iter()
        .map(|(_, r)| Header::decode(r).unwrap().0)
        .collect()
}

#[test]
fn a_single_band_streams_contiguous_records() {
    let (mut a, cap, _tx) = acq(vec![Band::new(2_437_000_000, 5_000_000, 1000)], Some(50));
    let mut sim = Sim::new(1024, 0.0);
    assert!(matches!(a.run(&mut sim), Exit::Done));
    let c = cap.lock().unwrap();
    let hs = decode(&c);
    assert_eq!(hs.len(), 50);
    assert_eq!(hs[0].flags & iq::F_BAND_CHANGED, iq::F_BAND_CHANGED);
    for (i, h) in hs.iter().enumerate() {
        assert_eq!(
            (h.kind, h.segment, h.centre_hz, h.rate_sps),
            (iq::KIND_SAMPLES, 1, 2_437_000_000, 5_000_000)
        );
        assert_eq!(h.sample_index, i as u64 * 1024);
        if i > 0 {
            assert_eq!(h.flags, 0);
        }
    }
    assert_eq!(a.totals.delivered, 50 * 1024);
    assert_eq!(a.totals.last_seq, 50);
    assert_eq!(
        (a.totals.lost, a.totals.gap_events, a.totals.zero_blocks),
        (0, 0, 0)
    );
    assert!(a.shared.raw.lock().unwrap().len() == 256 * 4);
}

#[test]
fn several_bands_take_turns_and_every_switch_is_marked() {
    let bands = vec![
        Band::new(2_437_000_000, 5_000_000, 1),
        Band::new(1_090_000_000, 2_400_000, 1),
    ];
    let (mut a, cap, _tx) = acq(bands, Some(400));
    let mut sim = Sim::new(1024, 1000.0);
    assert!(matches!(a.run(&mut sim), Exit::Done));
    let c = cap.lock().unwrap();
    let hs = decode(&c);
    let centres: std::collections::BTreeSet<u64> = hs.iter().map(|h| h.centre_hz).collect();
    assert_eq!(centres.len(), 2);
    assert!(a.totals.retunes >= 4, "retunes {}", a.totals.retunes);
    // every change of centre or segment is flagged and starts the segment's sample clock at 0
    for w in hs.windows(2) {
        if w[1].segment != w[0].segment {
            assert_eq!(w[1].flags & iq::F_BAND_CHANGED, iq::F_BAND_CHANGED);
            assert_eq!(w[1].sample_index, 0);
        } else {
            assert_eq!(w[1].sample_index, w[0].sample_index + 1024);
        }
    }
    let doc = a.status_doc(mono_ns());
    assert_eq!(doc["duty"].as_array().unwrap().len(), 1);
    assert_eq!(doc["counters"]["retunes"], a.totals.retunes);
}

#[test]
fn a_tune_call_retunes_before_the_next_block() {
    let (mut a, cap, tx) = acq(vec![Band::new(2_437_000_000, 5_000_000, 1000)], Some(20));
    let (rtx, rrx) = channel();
    let cmd = svc::parse(
        "c1",
        &json!({"method": "tune", "centre_hz": 868e6, "rate_sps": 2e6}),
    )
    .unwrap();
    tx.send((cmd, rtx)).unwrap();
    let mut sim = Sim::new(1024, 0.0);
    assert!(matches!(a.run(&mut sim), Exit::Done));
    assert_eq!(rrx.recv().unwrap().unwrap()["ok"], true);
    let c = cap.lock().unwrap();
    let hs = decode(&c);
    // the consumer's plan replaces the fallback plan at once
    assert!(
        hs.iter().all(|h| h.centre_hz == 868_000_000),
        "{:?}",
        hs.iter().map(|h| h.centre_hz).collect::<Vec<_>>()
    );
}

struct Scripted {
    block: usize,
    left: u32,
    zero: bool,
    lose_after: Option<u32>,
    band: Option<Band>,
}

impl Radio for Scripted {
    fn describe(&self) -> String {
        "scripted".into()
    }
    fn block_samples(&self) -> usize {
        self.block
    }
    fn apply(&mut self, b: &Band) -> Result<(), String> {
        self.band = Some(b.clone());
        Ok(())
    }
    fn refill(&mut self, out: &mut Vec<u8>) -> Result<(), RefillError> {
        if self.lose_after == Some(0) {
            return Err(RefillError::Lost(
                "buffer refill: No such device (-19)".into(),
            ));
        }
        if let Some(n) = self.lose_after.as_mut() {
            *n -= 1;
        }
        self.left = self.left.saturating_sub(1);
        out.clear();
        out.resize(self.block * 4, if self.zero { 0 } else { 7 });
        Ok(())
    }
    fn hw_status(&mut self) -> HwStatus {
        HwStatus::default()
    }
}

#[test]
fn a_dead_receive_path_is_flagged_and_unhealthy() {
    let (mut a, cap, _tx) = acq(vec![Band::new(2_437_000_000, 5_000_000, 1000)], Some(10));
    let mut r = Scripted {
        block: 256,
        left: 100,
        zero: true,
        lose_after: None,
        band: None,
    };
    assert!(matches!(a.run(&mut r), Exit::Done));
    let c = cap.lock().unwrap();
    assert!(decode(&c).iter().all(|h| h.flags & iq::F_ALL_ZERO != 0));
    let doc = a.status_doc(mono_ns());
    assert_eq!(doc["counters"]["dead_receive_path"], true);
    assert_eq!(doc["healthy"], false);
    assert_eq!(doc["counters"]["zero_blocks"], 10);
}

#[test]
fn a_vanished_radio_ends_the_run_with_the_reason() {
    let (mut a, _cap, _tx) = acq(vec![Band::new(2_437_000_000, 5_000_000, 1000)], None);
    let mut r = Scripted {
        block: 256,
        left: 100,
        zero: false,
        lose_after: Some(3),
        band: None,
    };
    match a.run(&mut r) {
        Exit::Lost(e) => assert!(is_lost(&e), "{e}"),
        _ => panic!("expected the radio to be lost"),
    }
    assert_eq!(a.totals.blocks, 3);
    assert!(a.last_error.as_deref().unwrap().contains("No such device"));
}

#[test]
fn a_stalled_stretch_that_never_arrives_is_counted_as_lost() {
    // 1 MS/s in 1024-sample blocks paced in real time; after 150 blocks the "radio" loses
    // 600 ms of samples: the clock moves on, the data does not.
    struct Gappy {
        n: u32,
        next: Option<std::time::Instant>,
        band: Option<Band>,
    }
    impl Radio for Gappy {
        fn describe(&self) -> String {
            "gappy".into()
        }
        fn block_samples(&self) -> usize {
            1024
        }
        fn apply(&mut self, b: &Band) -> Result<(), String> {
            self.band = Some(b.clone());
            self.next = None;
            Ok(())
        }
        fn refill(&mut self, out: &mut Vec<u8>) -> Result<(), RefillError> {
            let step = Duration::from_micros(1024);
            let mut due = self.next.unwrap_or_else(std::time::Instant::now) + step;
            self.n += 1;
            if self.n == 150 {
                due += Duration::from_millis(600);
            }
            let now = std::time::Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            }
            self.next = Some(due);
            out.clear();
            out.resize(1024 * 4, 5);
            Ok(())
        }
        fn hw_status(&mut self) -> HwStatus {
            HwStatus::default()
        }
    }
    let (mut a, cap, _tx) = acq(vec![Band::new(2_437_000_000, 1_000_000, 1000)], Some(1100));
    a.set.loss.slack_ns = 20_000_000;
    let mut r = Gappy {
        n: 0,
        next: None,
        band: None,
    };
    assert!(matches!(a.run(&mut r), Exit::Done));
    let c = cap.lock().unwrap();
    let hs = decode(&c);
    let gaps: Vec<&Header> = hs.iter().filter(|h| h.kind == iq::KIND_GAP).collect();
    assert_eq!(gaps.len(), 1, "gap events: {}", gaps.len());
    let lost = u64::from(gaps[0].lost);
    assert!((500_000..=620_000).contains(&lost), "lost {lost}");
    assert_eq!(a.totals.lost, lost);
    // the next samples carry the discontinuity flag and a jump on the sample clock
    let i = hs.iter().position(|h| h.kind == iq::KIND_GAP).unwrap();
    assert_eq!(hs[i + 1].flags & iq::F_DISCONT, iq::F_DISCONT);
    assert_eq!(hs[i + 1].sample_index - hs[i - 1].sample_index, 1024 + lost);
    let doc = a.status_doc(mono_ns());
    assert_eq!(doc["counters"]["gap_events"], 1);
    assert_eq!(doc["samples"]["lost"], lost);
}

#[test]
fn lost_device_errors_are_recognised() {
    assert!(is_lost("buffer refill: No such device (-19)"));
    assert!(is_lost("read x: Input/output error (-5)"));
    assert!(is_lost("write y: Connection timed out (-110)"));
    assert!(!is_lost("write y: Invalid argument (-22)"));
    assert!(!is_lost("rate 5 is outside"));
}
