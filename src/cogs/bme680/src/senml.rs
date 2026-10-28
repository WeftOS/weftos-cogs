//! SenML packs (RFC 8428) for one BME680 sample.
//!
//! One JSON array per line. `bn` is the installed object's id and ends with `/`.
//! It is a base name, so it is set on the first record only. `proof_` is not a
//! base attribute and is repeated on every record (RuView ADR-384 §4). `t` is
//! absolute Unix seconds (no `bt`). Units are the IANA SenML registry names:
//! `Cel`, `%RH`, `Pa`, `Ohm`.
//!
//! A missing quantity is omitted. It is never written as zero. A pack is not
//! emitted when every quantity is missing.

use crate::compensate::Reading;

pub const PROOF_MEASURED: &str = "MEASURED";
pub const PROOF_SYNTHETIC: &str = "SYNTHETIC";

pub fn pack(source_id: &str, t: u64, proof: &str, reading: &Reading) -> Option<serde_json::Value> {
    let mut recs = Vec::new();
    push(&mut recs, "temperature", "Cel", reading.temperature_c);
    push(&mut recs, "humidity", "%RH", reading.humidity_rh);
    push(&mut recs, "pressure", "Pa", reading.pressure_pa);
    push(&mut recs, "gas_resistance", "Ohm", reading.gas_ohm);
    if recs.is_empty() {
        return None;
    }
    if let serde_json::Value::Object(map) = &mut recs[0] {
        map.insert(
            "bn".to_string(),
            serde_json::Value::String(source_id.to_string()),
        );
    }
    for rec in &mut recs {
        if let serde_json::Value::Object(map) = rec {
            map.insert("t".to_string(), serde_json::json!(t));
            map.insert(
                "proof_".to_string(),
                serde_json::Value::String(proof.to_string()),
            );
        }
    }
    Some(serde_json::Value::Array(recs))
}

fn push(recs: &mut Vec<serde_json::Value>, name: &str, unit: &str, value: Option<f64>) {
    let Some(v) = value else {
        return;
    };
    if !v.is_finite() {
        return;
    }
    recs.push(serde_json::json!({ "n": name, "u": unit, "v": v }));
}

pub struct StatusIn<'a> {
    pub health: &'a str,
    pub t: u64,
    pub error: Option<String>,
    pub kind: &'a str,
    pub bus: &'a str,
    pub addr: u8,
    pub source_id: &'a str,
    pub simulated: bool,
    pub reading: Option<&'a Reading>,
}

pub fn status(input: &StatusIn<'_>) -> serde_json::Value {
    let health = input.health;
    let t = input.t;
    let error = input.error.clone();
    let kind = input.kind;
    let bus = input.bus;
    let addr = input.addr;
    let source_id = input.source_id;
    let simulated = input.simulated;
    let reading = input.reading;
    let num = |f: Option<f64>| -> serde_json::Value {
        match f {
            Some(v) if v.is_finite() => serde_json::json!(v),
            _ => serde_json::Value::Null,
        }
    };
    let (temp, pres, hum, gas) = match reading {
        Some(r) => (
            num(r.temperature_c),
            num(r.pressure_pa),
            num(r.humidity_rh),
            num(r.gas_ohm),
        ),
        None => (
            serde_json::Value::Null,
            serde_json::Value::Null,
            serde_json::Value::Null,
            serde_json::Value::Null,
        ),
    };
    serde_json::json!({
        "schema": "weavelogic.cog-output.v0",
        "cog": "bme680",
        "health": health,
        "timestamp": t,
        "error": error,
        "source": {
            "kind": kind,
            "bus": bus,
            "address": format!("0x{addr:02x}"),
            "source_id": source_id,
            "simulated": simulated,
            "verified": false
        },
        "temperature_c": temp,
        "humidity_rh": hum,
        "pressure_pa": pres,
        "gas_ohm": gas
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compensate::{reading_from_image, synthetic_image};

    #[test]
    fn pack_repeats_proof_and_does_not_invent_a_base_unit() {
        let reading = reading_from_image(&synthetic_image()).unwrap();
        let pack = pack("bme680/", 1_700_000_000, PROOF_SYNTHETIC, &reading).unwrap();
        let arr = pack.as_array().unwrap();
        assert_eq!(arr.len(), 4);
        assert_eq!(arr[0]["bn"], "bme680/");
        assert!(
            arr[1].get("bn").is_none(),
            "bn is a base name, not repeated"
        );
        assert!(arr[0].get("bu").is_none());
        for rec in arr {
            assert_eq!(rec["proof_"], "SYNTHETIC");
            assert_eq!(rec["t"], 1_700_000_000);
        }
        assert_eq!(arr[0]["n"], "temperature");
        assert_eq!(arr[0]["u"], "Cel");
        assert_eq!(arr[1]["u"], "%RH");
        assert_eq!(arr[2]["u"], "Pa");
        assert_eq!(arr[3]["n"], "gas_resistance");
        assert_eq!(arr[3]["u"], "Ohm");
        let text = pack.to_string();
        assert!(!text.contains("spatial.evidence"));
        assert!(!text.contains("aabb"));
    }

    #[test]
    fn a_missing_gas_reading_is_omitted_not_zero() {
        let mut reading = reading_from_image(&synthetic_image()).unwrap();
        reading.gas_ohm = None;
        let pack = pack("board-1/", 10, PROOF_MEASURED, &reading).unwrap();
        let arr = pack.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert!(arr.iter().all(|r| r["n"] != "gas_resistance"));
        assert!(arr.iter().all(|r| r["v"] != 0));
        assert_eq!(arr[0]["bn"], "board-1/");
        assert!(arr.iter().all(|r| r["proof_"] == "MEASURED"));
    }

    #[test]
    fn no_quantities_is_no_pack() {
        let reading = Reading {
            temperature_c: None,
            pressure_pa: None,
            humidity_rh: None,
            gas_ohm: None,
            gaps: vec!["empty".into()],
        };
        assert!(pack("bme680/", 1, PROOF_MEASURED, &reading).is_none());
    }
}
