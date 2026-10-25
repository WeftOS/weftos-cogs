//! The service's state as a `telemetry.v1` reading on `telemetry/<service>`:
//! queue depth, resident models and what they cost, load, and the counters.
//! The node reads this to put queue depth and `model.loaded.*` in its facts.

use serde_json::{json, Value};

use crate::scheduler::Snapshot;

fn item(name: &str, value: f64, unit: &str) -> Value {
    json!({"name": name, "value": value, "unit": unit})
}

/// `[a-z0-9._-]` alias to the `[a-z0-9_.]` an item name allows.
fn name_part(alias: &str) -> String {
    alias
        .chars()
        .map(|c| if c == '-' || c == '.' { '_' } else { c })
        .collect()
}

/// At most 64 items (the schema's limit).
pub fn items(s: &Snapshot, sd_busy: Option<bool>, mem_available_mib: Option<u64>) -> Vec<Value> {
    let c = &s.counters;
    let mut v = vec![
        item("queue.depth", s.depth as f64, ""),
        item("queue.oldest_wait", s.oldest_wait_ms as f64, "ms"),
        item("npu.busy", if s.running.is_some() { 1.0 } else { 0.0 }, ""),
        item("models.resident", s.resident.len() as f64, ""),
        item("mem.resident", s.used_mib as f64, "MiB"),
        item("mem.budget", s.budget_mib as f64, "MiB"),
        item("calls.served", c.served as f64, ""),
        item("calls.failed", c.failed as f64, ""),
        item("calls.rejected", c.rejected as f64, ""),
        item("calls.expired", c.expired as f64, ""),
        item("models.loads", c.loads as f64, ""),
        item("models.evictions", c.evictions as f64, ""),
        item("geniex.restarts", c.restarts as f64, ""),
    ];
    if let Some(b) = sd_busy {
        v.push(item("sd.busy", if b { 1.0 } else { 0.0 }, ""));
    }
    if let Some(m) = mem_available_mib {
        v.push(item("mem.available", m as f64, "MiB"));
    }
    for r in &s.resident {
        if v.len() >= 64 {
            break;
        }
        v.push(item(
            &format!("model.loaded.{}", name_part(&r.alias)),
            r.cost_mib as f64,
            "MiB",
        ));
    }
    v
}

/// The full `telemetry.v1` body.
pub fn body(source_id: &str, node_id: &str, t_ns: u64, seq: u64, items: Vec<Value>) -> Value {
    json!({"source_id": source_id, "node_id": node_id, "t_ns": t_ns, "seq": seq, "items": items})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::{Counters, ResidentInfo};

    fn snap() -> Snapshot {
        Snapshot {
            depth: 3,
            running: Some("qwen3-4b".into()),
            oldest_wait_ms: 1200,
            resident: vec![
                ResidentInfo {
                    alias: "qwen3.5-0.8b".into(),
                    cost_mib: 1400,
                },
                ResidentInfo {
                    alias: "qwen3-4b".into(),
                    cost_mib: 4500,
                },
            ],
            used_mib: 5900,
            budget_mib: 8000,
            measured: vec![],
            counters: Counters {
                served: 9,
                restarts: 1,
                ..Default::default()
            },
        }
    }

    fn val(items: &[Value], name: &str) -> Option<f64> {
        items
            .iter()
            .find(|i| i["name"] == name)
            .and_then(|i| i["value"].as_f64())
    }

    #[test]
    fn queue_depth_and_resident_models_are_reported() {
        let it = items(&snap(), Some(false), Some(4000));
        assert_eq!(val(&it, "queue.depth"), Some(3.0));
        assert_eq!(val(&it, "npu.busy"), Some(1.0));
        assert_eq!(val(&it, "models.resident"), Some(2.0));
        assert_eq!(val(&it, "model.loaded.qwen3_5_0_8b"), Some(1400.0));
        assert_eq!(val(&it, "model.loaded.qwen3_4b"), Some(4500.0));
        assert_eq!(val(&it, "mem.resident"), Some(5900.0));
        assert_eq!(val(&it, "geniex.restarts"), Some(1.0));
        assert_eq!(val(&it, "sd.busy"), Some(0.0));
        assert_eq!(val(&it, "mem.available"), Some(4000.0));
        assert_eq!(val(&items(&snap(), None, None), "sd.busy"), None);
    }

    #[test]
    fn every_name_and_unit_fits_telemetry_v1() {
        for i in items(&snap(), Some(true), Some(1)) {
            let n = i["name"].as_str().unwrap();
            let b = n.as_bytes();
            assert!(b.len() <= 64 && b[0].is_ascii_lowercase(), "{n}");
            assert!(
                b.iter()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_.".contains(c)),
                "{n}"
            );
            assert!(i["unit"].as_str().unwrap().chars().count() <= 16);
        }
        let b = body(
            "venq-npu",
            &"0".repeat(32),
            1,
            2,
            items(&snap(), None, None),
        );
        assert_eq!(b["seq"], 2);
    }
}
