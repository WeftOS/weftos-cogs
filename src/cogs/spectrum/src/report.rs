//! What the cog says about itself: the readings' sequence numbers, the counters, the status
//! document, the stdout report and the store vector.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;
use std::time::Instant;

use serde_json::{json, Value};

use crate::collect::{Collector, Config, OccMsg, PsdMsg};
use crate::shared::Shared;

/// A monotonic clock in nanoseconds since the process started (plus a second, so zero is never a
/// valid time).
pub struct Mono;

pub static MONO: Mono = Mono;

impl Mono {
    pub fn now(&self) -> u64 {
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1_000_000_000
    }
}

#[derive(Default)]
pub struct Reporter {
    pub records: u64,
    pub decode_errors: u64,
    pub last_record_ns: u64,
    pub ring_changes: u64,
    pub bus_node: Option<String>,
    psd_seq: u64,
    occ_seq: u64,
    pub windows: u64,
    last_psd: Option<PsdMsg>,
    last_occ: BTreeMap<u64, OccMsg>,
    started_ns: u64,
}

impl Reporter {
    pub fn new() -> Self {
        Self {
            started_ns: MONO.now(),
            ..Self::default()
        }
    }

    pub fn psd(&mut self, p: &PsdMsg, source: &str, node: &str) -> Value {
        self.psd_seq += 1;
        self.windows += 1;
        self.last_psd = Some(p.clone());
        p.to_json(source, &node32(node), self.psd_seq)
    }

    pub fn occupancy(&mut self, o: &OccMsg, source: &str, node: &str) -> Value {
        self.occ_seq += 1;
        self.last_occ.insert(o.centre_hz, o.clone());
        o.to_json(source, &node32(node), self.occ_seq)
    }

    /// The 8-float store vector (id 30).
    pub fn vector(&self, col: &Collector) -> [f64; 8] {
        let p = self.last_psd.as_ref();
        let occ = p
            .and_then(|p| self.last_occ.get(&p.centre_hz))
            .map_or(0.0, |o| o.overall);
        [
            p.map_or(0.0, |p| p.centre_hz as f64 / 1e9),
            p.map_or(0.0, |p| f64::from(p.rate_sps) / 1e6),
            occ,
            p.map_or(-300.0, |p| p.floor_db),
            p.map_or(-300.0, |p| p.peak_db),
            col.total.loss_fraction(),
            self.windows as f64,
            p.map_or(0.0, |p| {
                if p.centre_hz < 325_000_000 || p.centre_hz > 3_800_000_000 {
                    1.0
                } else {
                    0.0
                }
            }),
        ]
    }

    pub fn doc(&self, col: &Collector, state: &str, error: Option<&str>, cfg: &Config) -> Value {
        let now = MONO.now();
        let age =
            (self.last_record_ns != 0).then(|| now.saturating_sub(self.last_record_ns) / 1_000_000);
        let c = col.total;
        let occ: Vec<Value> = self
            .last_occ
            .values()
            .map(|o| json!({"centre_hz": o.centre_hz, "rate_sps": o.rate_sps, "overall": o.overall, "coverage": o.coverage, "loss_fraction": o.counts.loss_fraction()}))
            .collect();
        json!({
            "status": state,
            "healthy": state == "streaming" && age.is_some_and(|a| a < 10_000),
            "error": error,
            "uptime_s": now.saturating_sub(self.started_ns) / 1_000_000_000,
            "bus": {"node_id": self.bus_node, "ring_changes": self.ring_changes},
            "stream": {"records": self.records, "decode_errors": self.decode_errors, "last_record_age_ms": age},
            "samples": {"used": c.used, "lost": c.lost, "skipped": c.skipped, "dead": c.dead, "loss_fraction": c.loss_fraction()},
            "config": {"fft": cfg.fft, "bins": cfg.bins_out, "slices": cfg.slices, "threshold_db": cfg.thresh_db, "psd_secs": cfg.psd_ns / 1_000_000_000, "occ_secs": cfg.occ_ns / 1_000_000_000},
            "windows_published": self.windows,
            "last_psd": self.last_psd.as_ref().map(|p| json!({
                "centre_hz": p.centre_hz, "rate_sps": p.rate_sps, "frames": p.frames, "floor_dbfs": p.floor_db,
                "peak_dbfs": p.peak_db, "loss_fraction": p.counts.loss_fraction(), "bins": p.bins_db.len()})),
            "occupancy": occ,
            "vector": self.vector(col),
        })
    }

    pub fn publish_status(
        &self,
        shared: &Shared,
        col: &Collector,
        state: &str,
        error: Option<String>,
        cfg: &Config,
    ) {
        let d = self.doc(col, state, error.as_deref(), cfg);
        shared
            .healthy
            .store(d["healthy"].as_bool().unwrap_or(false), Ordering::Release);
        if let Ok(mut s) = shared.status.lock() {
            *s = d;
        }
    }
}

/// Readings need a 32-hex node id; before the bus names one use zeros.
fn node32(n: &str) -> String {
    if n.len() == 32 && n.bytes().all(|b| b.is_ascii_hexdigit()) {
        n.to_string()
    } else {
        "0".repeat(32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_ids_are_32_hex_or_zeros() {
        assert_eq!(
            node32("0123456789abcdef0123456789abcdef"),
            "0123456789abcdef0123456789abcdef"
        );
        assert_eq!(node32(""), "0".repeat(32));
        assert_eq!(node32("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"), "0".repeat(32));
    }

    #[test]
    fn the_clock_never_reads_zero_and_moves_forward() {
        let a = MONO.now();
        assert!(a >= 1_000_000_000);
        assert!(MONO.now() >= a);
    }

    #[test]
    fn an_idle_cog_reports_waiting_and_unhealthy() {
        let r = Reporter::new();
        let col = Collector::new(Config::default());
        let d = r.doc(&col, "waiting", None, &Config::default());
        assert_eq!(
            (d["status"].as_str(), d["healthy"].as_bool()),
            (Some("waiting"), Some(false))
        );
        assert_eq!(d["vector"].as_array().unwrap().len(), 8);
        assert_eq!(d["samples"]["loss_fraction"], 0.0);
    }
}
