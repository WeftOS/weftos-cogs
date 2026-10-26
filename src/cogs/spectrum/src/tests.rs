use super::*;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn options_are_bounded_and_defaulted() {
    let o = parse_opts(&s(&[])).unwrap();
    assert_eq!(
        (
            o.source.as_str(),
            o.service.as_str(),
            o.cfg.fft,
            o.cfg.bins_out
        ),
        ("venq-pluto", "venq-pluto", 1024, 256)
    );
    assert_eq!(
        (o.cfg.psd_ns, o.cfg.occ_ns, o.api_bind.as_str()),
        (2_000_000_000, 60_000_000_000, "127.0.0.1:8064")
    );
    assert!(parse_opts(&s(&["--fft", "1000"])).is_err());
    assert!(parse_opts(&s(&["--source", "Bad Name"])).is_err());
    let o = parse_opts(&s(&["--bins", "9999", "--psd-secs", "0"])).unwrap();
    assert_eq!((o.cfg.bins_out, o.cfg.psd_ns), (256, 2_000_000_000));
}

#[test]
fn a_plan_spec_becomes_a_plan_call() {
    let b = plan_body("2437e6:5e6:5000, 1090e6:2.4e6:2000:2e6:40").unwrap();
    assert_eq!(b["method"], "plan");
    assert_eq!(b["bands"][0]["centre_hz"], 2.437e9);
    assert_eq!(b["bands"][1]["gain_db"], 40.0);
    assert!(plan_body("2437e6").is_err());
    assert!(plan_body("x:1:2").is_err());
    assert!(plan_body("").is_err());
}

#[test]
fn the_synthetic_band_goes_through_the_whole_pipeline() {
    let cfg = Config {
        psd_ns: 1_000_000_000,
        occ_ns: 60_000_000_000,
        ..Config::default()
    };
    let src = Synthetic::new(
        Band {
            centre_hz: 2_437_000_000,
            rate_sps: 5_000_000,
            bw_hz: 4_000_000,
        },
        2.5,
    );
    let d = offline(Box::new(src), &cfg, Some("SYNTHETIC"), 2.5);
    assert_eq!(d["provenance"], "SYNTHETIC");
    assert_eq!(d["samples"]["loss_fraction"], 0.0);
    assert!(d["readings"]["spectrum.psd"].as_u64().unwrap() >= 2);
    assert_eq!(d["readings"]["band.occupancy"], 1);
    let bins = d["psd"]["bins"].as_array().unwrap();
    assert_eq!(bins.len(), 256);
    // the tone is a quarter of the way up the band: bin 128 + 64
    let peak = bins
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.as_f64().unwrap().total_cmp(&b.1.as_f64().unwrap()))
        .unwrap()
        .0;
    assert!((peak as i32 - 192).abs() <= 1, "peak {peak}");
    let occ = d["occupancy_reading"]["items"].as_array().unwrap();
    let get = |n: &str| {
        occ.iter().find(|i| i["name"] == n).unwrap()["value"]
            .as_f64()
            .unwrap()
    };
    assert!(
        get("occ.overall") > 0.0 && get("occ.overall") < 0.5,
        "occupancy {}",
        get("occ.overall")
    );
    assert!(get("peak") > get("floor") + 30.0);
}

#[test]
fn a_missing_source_is_reported_not_invented() {
    assert_eq!(no_source("nothing here"), 0);
}
