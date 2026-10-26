//! `svc.sdr/<node>`: the calls consumers make on the tuner (weftos ADR-122 section 12).
//!
//! Body: `{"method": "...", ...fields}`; the reply body is `{"ok": true, ...}` or the call fails
//! with a message. Methods:
//!
//! - `tune {centre_hz, rate_sps?, bw_hz?, gain_db? (number | "auto"), dwell_ms?}`: the caller's
//!   own single-band plan; replaces its previous `tune`.
//! - `set_rate {rate_sps}`, `set_bw {bw_hz}`, `set_gain {gain_db}`: change that field of the
//!   caller's `tune` band.
//! - `plan {id, bands: [{centre_hz, rate_sps, dwell_ms, bw_hz?, gain_db?}, ...]}` and
//!   `plan_remove {id}`: a band plan that takes turns with the others.
//! - `lease {centre_hz, rate_sps?, bw_hz?, gain_db?, secs}` and `release`: exclusive use of the
//!   tuner for up to an hour; the round-robin pauses and other callers' tuning is refused.
//! - `status`: the same JSON as `GET /status`.
//! - `tx_config`: always refused; there is no transmit cog yet.
//!
//! The caller is the `from` of the pushed call (the broker's name for the instance).

use serde_json::{json, Value};

use crate::plan::{Band, Gain, Plan, Scheduler, DEFAULT_PLAN, MAX_LEASE_SECS};

#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Tune {
        from: String,
        band: Band,
    },
    SetRate {
        from: String,
        rate_sps: u32,
    },
    SetBw {
        from: String,
        bw_hz: u32,
    },
    SetGain {
        from: String,
        gain: Gain,
    },
    SetPlan {
        from: String,
        id: String,
        bands: Vec<Band>,
    },
    RemovePlan {
        from: String,
        id: String,
    },
    Lease {
        from: String,
        band: Band,
        secs: u64,
    },
    Release {
        from: String,
    },
    Status,
}

fn f64_of(v: &Value, key: &str) -> Result<Option<f64>, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(x) => x
            .as_f64()
            .filter(|f| f.is_finite() && *f >= 0.0)
            .map(Some)
            .ok_or_else(|| format!("{key} must be a non-negative number")),
    }
}

fn gain_of(v: &Value) -> Result<Option<Gain>, String> {
    match v.get("gain_db") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s == "auto" => Ok(Some(Gain::Auto)),
        Some(x) => x
            .as_f64()
            .filter(|g| g.is_finite())
            .map(|g| Some(Gain::Manual(g)))
            .ok_or_else(|| "gain_db must be a number or \"auto\"".to_string()),
    }
}

/// A band from call fields; `base` supplies what the call leaves out.
fn band_of(v: &Value, base: Option<&Band>) -> Result<Band, String> {
    let centre = f64_of(v, "centre_hz")?
        .map(|f| f as u64)
        .or_else(|| base.map(|b| b.centre_hz))
        .ok_or("centre_hz is required")?;
    let rate = f64_of(v, "rate_sps")?
        .map(|f| f as u32)
        .or_else(|| base.map(|b| b.rate_sps))
        .unwrap_or(5_000_000);
    let dwell = f64_of(v, "dwell_ms")?
        .map(|f| f as u32)
        .or_else(|| base.map(|b| b.dwell_ms))
        .unwrap_or(1000);
    let mut b = Band::new(centre, rate, dwell);
    if let Some(bw) = f64_of(v, "bw_hz")? {
        b.bw_hz = bw as u32;
    } else if let Some(base) = base.filter(|x| x.rate_sps == rate) {
        b.bw_hz = base.bw_hz;
    }
    if let Some(g) = gain_of(v)? {
        b.gain = g;
    } else if let Some(base) = base {
        b.gain = base.gain;
    }
    b.check()?;
    Ok(b)
}

pub fn parse(from: &str, body: &Value) -> Result<Cmd, String> {
    let from = from.to_string();
    let method = body["method"].as_str().ok_or("method is required")?;
    Ok(match method {
        "tune" => Cmd::Tune { from, band: band_of(body, None)? },
        "set_rate" => Cmd::SetRate {
            from,
            rate_sps: f64_of(body, "rate_sps")?.ok_or("rate_sps is required")? as u32,
        },
        "set_bw" => Cmd::SetBw {
            from,
            bw_hz: f64_of(body, "bw_hz")?.ok_or("bw_hz is required")? as u32,
        },
        "set_gain" => Cmd::SetGain {
            from,
            gain: gain_of(body)?.ok_or("gain_db is required")?,
        },
        "plan" => {
            let id = body["id"].as_str().filter(|s| !s.is_empty() && s.len() <= 48).ok_or("id is required (1..48 characters)")?;
            if id == DEFAULT_PLAN || id.starts_with("tune:") || id.starts_with("lease:") {
                return Err(format!("plan id {id} is reserved"));
            }
            let bands = body["bands"]
                .as_array()
                .ok_or("bands is required")?
                .iter()
                .map(|b| band_of(b, None))
                .collect::<Result<Vec<_>, _>>()?;
            Cmd::SetPlan { from, id: id.to_string(), bands }
        }
        "plan_remove" => Cmd::RemovePlan {
            from,
            id: body["id"].as_str().ok_or("id is required")?.to_string(),
        },
        "lease" => {
            let secs = f64_of(body, "secs")?.ok_or("secs is required")? as u64;
            if secs == 0 || secs > MAX_LEASE_SECS {
                return Err(format!("secs must be 1..{MAX_LEASE_SECS}"));
            }
            Cmd::Lease { from, band: band_of(body, None)?, secs }
        }
        "release" => Cmd::Release { from },
        "status" => Cmd::Status,
        "tx_config" => {
            return Err("tx_config refused: transmit is not available. There is no transmit cog yet (plan phase R5), and this cog never writes the TX path".into())
        }
        other => return Err(format!("unknown method {other:?}")),
    })
}

fn tune_id(from: &str) -> String {
    format!("tune:{from}")
}

/// Apply a command to the scheduler. The bool is true when the running slot must be
/// re-selected now. `status` is the current status document, for `Cmd::Status`.
pub fn exec(
    sched: &mut Scheduler,
    cmd: Cmd,
    now_ns: u64,
    status: &Value,
) -> Result<(Value, bool), String> {
    let refuse_if_leased = |s: &mut Scheduler, who: &str| -> Result<(), String> {
        match s.lease_holder(now_ns) {
            Some(h) if h != who => Err(format!("the tuner is leased by {h}")),
            Some(_) => Err("you hold the lease: call lease again to change its band".into()),
            None => Ok(()),
        }
    };
    match cmd {
        Cmd::Status => Ok((status.clone(), false)),
        Cmd::Tune { from, band } => {
            refuse_if_leased(sched, &from)?;
            sched.set_plan(Plan {
                id: tune_id(&from),
                owner: from.clone(),
                bands: vec![band.clone()],
            })?;
            Ok((
                json!({"ok": true, "plan": tune_id(&from), "uncalibrated": band.out_of_rated_range()}),
                true,
            ))
        }
        Cmd::SetRate { from, rate_sps } => modify(sched, &from, now_ns, |b| b.rate_sps = rate_sps),
        Cmd::SetBw { from, bw_hz } => modify(sched, &from, now_ns, |b| b.bw_hz = bw_hz),
        Cmd::SetGain { from, gain } => modify(sched, &from, now_ns, |b| b.gain = gain),
        Cmd::SetPlan { from, id, bands } => {
            let n = bands.len();
            sched.set_plan(Plan {
                id: id.clone(),
                owner: from,
                bands,
            })?;
            Ok((json!({"ok": true, "plan": id, "bands": n}), true))
        }
        Cmd::RemovePlan { from, id } => {
            sched.remove_plan(&id, &from)?;
            Ok((json!({"ok": true}), true))
        }
        Cmd::Lease { from, band, secs } => {
            let id = sched.lease(&from, band, secs, now_ns)?;
            Ok((json!({"ok": true, "lease": id, "secs": secs}), true))
        }
        Cmd::Release { from } => {
            sched.release(&from)?;
            Ok((json!({"ok": true}), true))
        }
    }
}

fn modify(
    sched: &mut Scheduler,
    from: &str,
    now_ns: u64,
    f: impl FnOnce(&mut Band),
) -> Result<(Value, bool), String> {
    match sched.lease_holder(now_ns) {
        Some(h) if h != from => return Err(format!("the tuner is leased by {h}")),
        Some(_) => return Err("you hold the lease: call lease again to change its band".into()),
        None => {}
    }
    let id = tune_id(from);
    let mut band = sched
        .plan(&id)
        .and_then(|p| p.bands.first().cloned())
        .ok_or("no tuning of yours to change: call tune first")?;
    f(&mut band);
    sched.set_plan(Plan {
        id: id.clone(),
        owner: from.to_string(),
        bands: vec![band],
    })?;
    Ok((json!({"ok": true, "plan": id}), true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(from: &str, v: Value) -> Result<Cmd, String> {
        parse(from, &v)
    }

    #[test]
    fn calls_are_parsed_and_checked() {
        let c = p(
            "a",
            json!({"method": "tune", "centre_hz": 2.437e9, "rate_sps": 5e6, "gain_db": 40}),
        )
        .unwrap();
        match c {
            Cmd::Tune { band, .. } => {
                assert_eq!(
                    (band.centre_hz, band.rate_sps, band.bw_hz),
                    (2_437_000_000, 5_000_000, 4_000_000)
                );
                assert_eq!(band.gain, Gain::Manual(40.0));
            }
            _ => panic!(),
        }
        assert!(p("a", json!({"method": "tune"})).is_err());
        assert!(p("a", json!({"method": "tune", "centre_hz": 50e6}))
            .unwrap_err()
            .contains("outside"));
        assert!(p("a", json!({"method": "tune", "centre_hz": -1})).is_err());
        assert!(p(
            "a",
            json!({"method": "tune", "centre_hz": 2.4e9, "gain_db": "loud"})
        )
        .is_err());
        assert!(p("a", json!({"method": "nope"}))
            .unwrap_err()
            .contains("unknown"));
        assert!(p("a", json!({"x": 1})).is_err());
        assert_eq!(p("a", json!({"method": "status"})).unwrap(), Cmd::Status);
        assert!(p(
            "a",
            json!({"method": "lease", "centre_hz": 2.4e9, "secs": 7200})
        )
        .is_err());
        assert!(
            p("a", json!({"method": "plan", "id": "default", "bands": []}))
                .unwrap_err()
                .contains("reserved")
        );
        assert!(p("a", json!({"method": "plan", "id": "x", "bands": [{"centre_hz": 1e9, "rate_sps": 2e6, "dwell_ms": 200}]})).is_ok());
    }

    #[test]
    fn tx_config_is_always_refused() {
        let e = p(
            "tx-cog",
            json!({"method": "tx_config", "attenuation_db": -10}),
        )
        .unwrap_err();
        assert!(e.contains("refused") && e.contains("R5"), "{e}");
    }

    #[test]
    fn tune_then_modify_fields_of_ones_own_band() {
        let mut s = Scheduler::new();
        let st = json!({});
        let tune = p(
            "a",
            json!({"method": "tune", "centre_hz": 2.437e9, "rate_sps": 5e6}),
        )
        .unwrap();
        assert!(exec(&mut s, tune, 0, &st).unwrap().1);
        let r = exec(
            &mut s,
            p("a", json!({"method": "set_rate", "rate_sps": 2e6})).unwrap(),
            0,
            &st,
        )
        .unwrap();
        assert!(r.1);
        exec(
            &mut s,
            p("a", json!({"method": "set_gain", "gain_db": 30})).unwrap(),
            0,
            &st,
        )
        .unwrap();
        exec(
            &mut s,
            p("a", json!({"method": "set_bw", "bw_hz": 1e6})).unwrap(),
            0,
            &st,
        )
        .unwrap();
        let b = &s.plan("tune:a").unwrap().bands[0];
        assert_eq!(
            (b.rate_sps, b.bw_hz, b.gain),
            (2_000_000, 1_000_000, Gain::Manual(30.0))
        );
        // someone with no tuning cannot modify
        let e = exec(
            &mut s,
            p("b", json!({"method": "set_rate", "rate_sps": 2e6})).unwrap(),
            0,
            &st,
        )
        .unwrap_err();
        assert!(e.contains("tune first"), "{e}");
        // an invalid change leaves the band as it was
        assert!(exec(
            &mut s,
            p("a", json!({"method": "set_rate", "rate_sps": 100})).unwrap(),
            0,
            &st
        )
        .is_err());
        assert_eq!(s.plan("tune:a").unwrap().bands[0].rate_sps, 2_000_000);
    }

    #[test]
    fn a_lease_blocks_other_callers_tuning() {
        let mut s = Scheduler::new();
        let st = json!({});
        let lease = p(
            "holder",
            json!({"method": "lease", "centre_hz": 1.09e9, "rate_sps": 2.4e6, "secs": 30}),
        )
        .unwrap();
        exec(&mut s, lease, 0, &st).unwrap();
        let e = exec(
            &mut s,
            p("b", json!({"method": "tune", "centre_hz": 2.4e9})).unwrap(),
            1,
            &st,
        )
        .unwrap_err();
        assert!(e.contains("leased by holder"), "{e}");
        assert!(exec(
            &mut s,
            p("holder", json!({"method": "tune", "centre_hz": 2.4e9})).unwrap(),
            1,
            &st
        )
        .is_err());
        exec(
            &mut s,
            Cmd::Release {
                from: "holder".into(),
            },
            2,
            &st,
        )
        .unwrap();
        assert!(exec(
            &mut s,
            p("b", json!({"method": "tune", "centre_hz": 2.4e9})).unwrap(),
            3,
            &st
        )
        .is_ok());
    }

    #[test]
    fn status_returns_the_document() {
        let mut s = Scheduler::new();
        let (v, changed) = exec(&mut s, Cmd::Status, 0, &json!({"state": "streaming"})).unwrap();
        assert_eq!(v["state"], "streaming");
        assert!(!changed);
    }
}
