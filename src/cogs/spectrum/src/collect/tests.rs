use super::*;
use crate::iq::{F_DISCONT, KIND_SAMPLES};

const N: u32 = 8192;

fn cfg() -> Config {
    Config {
        psd_ns: 1_000_000_000,
        occ_ns: 4_000_000_000,
        stride: 1,
        ..Config::default()
    }
}

fn head(segment: u32, centre: u64, idx: u64) -> Header {
    Header {
        kind: KIND_SAMPLES,
        flags: 0,
        segment,
        centre_hz: centre,
        rate_sps: 5_000_000,
        bw_hz: 4_000_000,
        sample_index: idx,
        lost: 0,
        n_samples: N,
    }
}

/// Noise, plus a tone `offset` bins from the centre when `tone` is set.
fn samples(seed: u64, tone: Option<i32>) -> Vec<u8> {
    let mut r = seed | 1;
    let mut out = Vec::new();
    for t in 0..N as usize {
        let mut nz = || {
            r ^= r << 13;
            r ^= r >> 7;
            r ^= r << 17;
            ((r >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 200.0
        };
        let (ni, nq) = (nz(), nz());
        let (c, s) = match tone {
            Some(o) => {
                let ph = 2.0 * std::f64::consts::PI * f64::from(o) * t as f64 / 1024.0;
                (1500.0 * ph.cos(), 1500.0 * ph.sin())
            }
            None => (0.0, 0.0),
        };
        out.extend_from_slice(&((c + ni) as i16).to_le_bytes());
        out.extend_from_slice(&((s + nq) as i16).to_le_bytes());
    }
    out
}

/// Feed `n` records at 5 MS/s of stream time.
fn feed(
    c: &mut Collector,
    segment: u32,
    centre: u64,
    from: u64,
    n: u64,
    tone: Option<i32>,
    t0: u64,
) -> Vec<Event> {
    let mut ev = Vec::new();
    let step = u64::from(N) * 1_000_000_000 / 5_000_000;
    for k in 0..n {
        let h = head(segment, centre, from + k * u64::from(N));
        ev.extend(c.on_record(
            &h,
            &samples(k + 1, tone),
            t0 + k * step,
            t0 + (k + 1) * step,
        ));
    }
    ev
}

fn psds(ev: &[Event]) -> Vec<&PsdMsg> {
    ev.iter()
        .filter_map(|e| if let Event::Psd(p) = e { Some(p) } else { None })
        .collect()
}

fn occs(ev: &[Event]) -> Vec<&OccMsg> {
    ev.iter()
        .filter_map(|e| {
            if let Event::Occupancy(o) = e {
                Some(o)
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn a_window_ends_by_time_and_carries_the_spectrum_and_the_counts() {
    let mut c = Collector::new(cfg());
    // 1 s of stream time is 610 records
    let ev = feed(&mut c, 1, 2_437_000_000, 0, 700, Some(-150), 1_000_000_000);
    let p = psds(&ev);
    assert_eq!(p.len(), 1);
    let m = p[0];
    assert_eq!(
        (m.centre_hz, m.rate_sps, m.fft),
        (2_437_000_000, 5_000_000, 1024)
    );
    assert_eq!(m.bins_db.len(), 256);
    assert!(m.counts.used >= 600 * u64::from(N) && m.counts.loss_fraction() == 0.0);
    // the tone sits 150 bins below the centre of 1024: output bin (512 - 150) / 4
    let peak = m
        .bins_db
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap()
        .0;
    assert!(
        (peak as i32 - (512 - 150) / 4).abs() <= 1,
        "peak bin {peak}"
    );
    assert!(m.peak_db > m.floor_db + 30.0);
}

#[test]
fn the_psd_reading_is_a_registered_spectrum_message() {
    let mut c = Collector::new(cfg());
    let ev = feed(&mut c, 1, 2_437_000_000, 0, 700, None, 1_000_000_000);
    let v = psds(&ev)[0].to_json("venq-pluto", &"0".repeat(32), 3);
    // the registered fields and rules (contracts/cogs spectrum.psd.v1)
    assert_eq!(
        (v["center_hz"].as_u64(), v["span_hz"].as_u64()),
        (Some(2_437_000_000), Some(5_000_000))
    );
    let bins = v["bins"].as_array().unwrap();
    assert!((1..=4096).contains(&bins.len()));
    assert!(bins
        .iter()
        .all(|b| (-300.0..=100.0).contains(&b.as_f64().unwrap())));
    assert_eq!(
        (v["seq"].as_u64(), v["source_id"].as_str()),
        (Some(3), Some("venq-pluto"))
    );
    assert!(v["t_ns"].is_u64() && v["node_id"].as_str().unwrap().len() == 32);
    assert_eq!(v["uncalibrated"], false);
    assert!(
        serde_json::to_vec(&v).unwrap().len() < 6000,
        "a reading stays small"
    );
    assert_eq!(v["samples_lost"], 0);
}

#[test]
fn occupancy_is_reported_per_band_with_its_coverage() {
    let mut c = Collector::new(cfg());
    // 3 s of quiet at 2.437 GHz, then 1 s with a strong tone, then flush the interval
    let mut ev = feed(&mut c, 1, 2_437_000_000, 0, 1830, None, 1_000_000_000);
    ev.extend(feed(
        &mut c,
        1,
        2_437_000_000,
        1830 * u64::from(N),
        610,
        Some(-100),
        4_000_000_000,
    ));
    ev.extend(c.flush(5_100_000_000));
    let o = occs(&ev);
    assert_eq!(o.len(), 1, "one band");
    let m = o[0];
    assert_eq!((m.centre_hz, m.slices.len()), (2_437_000_000, 8));
    assert!(m.overall > 0.0 && m.overall < 0.2, "overall {}", m.overall);
    // the tone sits at bin 412 of 1024; the usable 80 percent starts at bin 102: slice 3 of 8
    let best = m
        .slices
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap();
    assert_eq!(best.0, 3, "{:?}", m.slices);
    assert!(*best.1 > 0.003, "{:?}", m.slices);
    assert!(m.coverage > 0.9, "coverage {}", m.coverage);
    let v = m.to_json("venq-pluto", &"0".repeat(32), 1);
    let items = v["items"].as_array().unwrap();
    assert!(items.len() <= 64);
    for i in items {
        let name = i["name"].as_str().unwrap();
        assert!(
            name.starts_with(|c: char| c.is_ascii_lowercase()) && name.len() <= 64,
            "{name}"
        );
        assert!(
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_'),
            "{name}"
        );
        assert!(i["value"].as_f64().unwrap().is_finite());
    }
}

#[test]
fn bands_taking_turns_get_one_occupancy_reading_each() {
    let mut c = Collector::new(cfg());
    let mut ev = Vec::new();
    for (idx, (seg, centre)) in [
        (1, 2_437_000_000u64),
        (2, 1_090_000_000),
        (3, 433_920_000),
        (4, 2_437_000_000),
    ]
    .into_iter()
    .enumerate()
    {
        let idx = idx as u64;
        ev.extend(feed(
            &mut c,
            seg,
            centre,
            0,
            200,
            None,
            1_000_000_000 + idx * 200_000_000,
        ));
    }
    ev.extend(c.flush(2_000_000_000));
    let o = occs(&ev);
    let centres: Vec<u64> = o.iter().map(|m| m.centre_hz).collect();
    assert_eq!(centres.len(), 3, "{centres:?}");
    // a segment change ends the window: four windows of PSD
    assert_eq!(psds(&ev).len(), 4);
    let low = o.iter().find(|m| m.centre_hz == 433_920_000).unwrap();
    assert!(low.to_json("s", &"0".repeat(32), 1)["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["name"] == "uncalibrated" && i["value"] == 0.0));
    let below_rated = Collector::new(cfg());
    let _ = below_rated;
    let m150 = OccMsg {
        centre_hz: 150_000_000,
        ..(*low).clone()
    };
    assert!(m150.to_json("s", &"0".repeat(32), 1)["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["name"] == "uncalibrated" && i["value"] == 1.0));
}

#[test]
fn a_declared_gap_is_counted_once_and_breaks_the_frame_overlap() {
    let mut c = Collector::new(cfg());
    let step = u64::from(N) * 200;
    let mut ev = feed(&mut c, 1, 2_437_000_000, 0, 100, None, 1_000_000_000);
    // the owner writes a gap record and then the next chunk with the same number in `lost`
    let gap = Header {
        kind: KIND_GAP,
        flags: F_DISCONT,
        n_samples: 0,
        lost: 400_000,
        sample_index: 100 * u64::from(N),
        ..head(1, 2_437_000_000, 0)
    };
    ev.extend(c.on_record(&gap, &[], 0, 1_164_000_000));
    let after = Header {
        flags: F_DISCONT,
        lost: 400_000,
        ..head(1, 2_437_000_000, 100 * u64::from(N) + 400_000)
    };
    ev.extend(c.on_record(&after, &samples(9, None), 0, 1_165_000_000));
    ev.extend(feed(
        &mut c,
        1,
        2_437_000_000,
        101 * u64::from(N) + 400_000,
        50,
        None,
        1_166_000_000,
    ));
    ev.extend(c.flush(1_300_000_000));
    let _ = step;
    let total = c.total;
    assert_eq!(total.lost, 400_000, "counted once, not twice");
    assert_eq!(total.skipped, 0);
    assert_eq!(total.used, 151 * u64::from(N));
    let p = psds(&ev);
    let last = p.last().unwrap();
    assert!(last.counts.lost > 0 || p.iter().any(|m| m.counts.lost == 400_000));
    let frac: f64 = total.loss_fraction();
    assert!((frac - 400_000.0 / (151.0 * f64::from(N) + 400_000.0)).abs() < 1e-9);
}

#[test]
fn a_jump_the_owner_did_not_declare_is_this_readers_miss() {
    let mut c = Collector::new(cfg());
    feed(&mut c, 1, 2_437_000_000, 0, 10, None, 1_000_000_000);
    // 5 records never read (the ring overwrote them), nothing declared
    let h = head(1, 2_437_000_000, 15 * u64::from(N));
    c.on_record(&h, &samples(3, None), 0, 1_100_000_000);
    assert_eq!((c.total.skipped, c.total.lost), (5 * u64::from(N), 0));
    // a declared loss that explains the whole jump is not a miss
    let mut d = Collector::new(cfg());
    feed(&mut d, 1, 2_437_000_000, 0, 10, None, 1_000_000_000);
    let h = Header {
        lost: 7000,
        flags: F_DISCONT,
        ..head(1, 2_437_000_000, 10 * u64::from(N) + 7000)
    };
    d.on_record(&h, &samples(3, None), 0, 1_100_000_000);
    assert_eq!((d.total.skipped, d.total.lost), (0, 7000));
}

#[test]
fn a_dead_receive_path_is_kept_out_of_the_spectrum_and_counted() {
    let mut c = Collector::new(cfg());
    let step = u64::from(N) * 1_000_000_000 / 5_000_000;
    let mut ev = Vec::new();
    for k in 0..20u64 {
        let h = Header {
            flags: crate::iq::F_ALL_ZERO,
            ..head(1, 2_437_000_000, k * u64::from(N))
        };
        ev.extend(c.on_record(&h, &vec![0u8; N as usize * 4], 0, 1_000_000_000 + k * step));
    }
    ev.extend(c.flush(1_100_000_000));
    assert_eq!((c.total.used, c.total.dead), (0, 20 * u64::from(N)));
    assert!(psds(&ev).is_empty(), "no spectrum of zeros");
    let o = occs(&ev);
    assert_eq!(o.len(), 1);
    assert_eq!(o[0].counts.loss_fraction(), 1.0);
}

#[test]
fn a_quiet_stream_still_flushes_on_a_tick() {
    let mut c = Collector::new(cfg());
    feed(&mut c, 1, 2_437_000_000, 0, 100, None, 1_000_000_000);
    assert!(c.tick(1_100_000_000).is_empty());
    let ev = c.tick(2_300_000_000);
    assert_eq!(psds(&ev).len(), 1);
}
