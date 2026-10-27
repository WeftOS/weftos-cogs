//! SenML (RFC 8428) object readings. One JSON array per line.
//!
//! These are the state of the radar module, not evidence that a person is
//! present (RuView ADR-384 §2 and §4). Person positions are not put here.
//! Heart rate and breathing rate are not put here either: the LD6004 protocol
//! does not define them, and `bpm` is not a unit this cog emits.
//!
//! `bn` is the installed object's id and always ends with `/`. `proof_` is on
//! every record. `u` is an IANA SenML unit. Milliseconds and dimensionless
//! counters have no registry unit, so those records omit `u`.

use serde_json::{json, Value};

/// Status fields the protocol defines as the module's own settings or state.
/// Absent fields were not in the window.
#[derive(Debug, Default)]
pub struct Status {
    pub work_mode: Option<u8>,
    pub low_power_state: Option<u8>,
    pub sensitivity: Option<u8>,
    pub trigger_speed: Option<u8>,
    pub install: Option<u8>,
    pub gpio_mode: Option<u8>,
    /// Seconds. Protocol §6.11. SenML unit `s`.
    pub hold_delay_s: Option<u32>,
    /// Milliseconds. Protocol §6.17. No SenML unit; `u` is omitted.
    pub sleep_ms: Option<u32>,
    /// Vendor life counter, not a second count. `u` is omitted.
    pub stay_life: Option<u32>,
    /// Frames between reports. Protocol §6.21. `u` is omitted.
    pub output_interval: Option<u32>,
    pub z_min_m: Option<f32>,
    pub z_max_m: Option<f32>,
}

impl Status {
    pub fn is_empty(&self) -> bool {
        self.work_mode.is_none()
            && self.low_power_state.is_none()
            && self.sensitivity.is_none()
            && self.trigger_speed.is_none()
            && self.install.is_none()
            && self.gpio_mode.is_none()
            && self.hold_delay_s.is_none()
            && self.sleep_ms.is_none()
            && self.stay_life.is_none()
            && self.output_interval.is_none()
            && self.z_min_m.is_none()
            && self.z_max_m.is_none()
    }
}

/// `source_id` plus a trailing slash. A slash already present is not doubled.
pub fn base_name(source_id: &str) -> String {
    let id = source_id.trim_end_matches('/');
    format!("{id}/")
}

/// `MEASURED` or `SYNTHETIC`. Anything else is refused by the caller.
pub fn pack(bn: &str, bt_unix_s: u64, proof: &str, status: &Status) -> Option<Vec<Value>> {
    if status.is_empty() {
        return None;
    }
    debug_assert!(bn.ends_with('/'));
    debug_assert!(proof == "MEASURED" || proof == "SYNTHETIC");
    let mut recs = Vec::new();
    let mut push = |n: &str, v: Value, unit: Option<&str>| {
        let mut rec = json!({
            "bn": bn,
            "bt": bt_unix_s,
            "n": n,
            "v": v,
            "proof_": proof,
        });
        if let Some(u) = unit {
            rec.as_object_mut().unwrap().insert("u".into(), json!(u));
        }
        recs.push(rec);
    };
    if let Some(v) = status.work_mode {
        push("work_mode", json!(v), None);
    }
    if let Some(v) = status.low_power_state {
        push("low_power_state", json!(v), None);
    }
    if let Some(v) = status.sensitivity {
        push("sensitivity", json!(v), None);
    }
    if let Some(v) = status.trigger_speed {
        push("trigger_speed", json!(v), None);
    }
    if let Some(v) = status.install {
        push("install", json!(v), None);
    }
    if let Some(v) = status.gpio_mode {
        push("gpio_mode", json!(v), None);
    }
    if let Some(v) = status.hold_delay_s {
        push("hold_delay", json!(v), Some("s"));
    }
    if let Some(v) = status.sleep_ms {
        // Unit is milliseconds, which the SenML registry does not name.
        push("sleep_period", json!(v), None);
    }
    if let Some(v) = status.stay_life {
        push("stay_life", json!(v), None);
    }
    if let Some(v) = status.output_interval {
        push("output_interval", json!(v), None);
    }
    if let Some(v) = status.z_min_m {
        push("z_min", json!(v), Some("m"));
    }
    if let Some(v) = status.z_max_m {
        push("z_max", json!(v), Some("m"));
    }
    Some(recs)
}

pub fn line(recs: &[Value]) -> String {
    serde_json::to_string(recs).unwrap_or_else(|_| "[]".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_name_ends_with_one_slash() {
        assert_eq!(base_name("ld6004-radar"), "ld6004-radar/");
        assert_eq!(base_name("ld6004-radar/"), "ld6004-radar/");
    }

    #[test]
    fn every_record_has_its_own_proof_and_no_invented_unit() {
        let s = Status {
            low_power_state: Some(1),
            hold_delay_s: Some(30),
            sleep_ms: Some(500),
            ..Status::default()
        };
        let recs = pack("ld6004-radar/", 1_700_000_000, "SYNTHETIC", &s).unwrap();
        let text = line(&recs);
        assert!(!text.contains("bpm"), "{text}");
        assert!(!text.contains("breath"), "{text}");
        assert!(!text.contains("heart"), "{text}");
        for rec in &recs {
            assert_eq!(rec["bn"], "ld6004-radar/");
            assert_eq!(rec["proof_"], "SYNTHETIC");
            assert!(rec["bn"].as_str().unwrap().ends_with('/'));
        }
        let sleep = recs.iter().find(|r| r["n"] == "sleep_period").unwrap();
        assert!(
            sleep.get("u").is_none(),
            "milliseconds are not a SenML unit"
        );
        let hold = recs.iter().find(|r| r["n"] == "hold_delay").unwrap();
        assert_eq!(hold["u"], "s");
        assert!(pack("ld6004-radar/", 0, "MEASURED", &Status::default()).is_none());
    }
}
