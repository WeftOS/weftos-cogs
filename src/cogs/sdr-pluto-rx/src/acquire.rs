//! The acquisition loop: the one thread that owns the radio. It runs the scheduler's slots,
//! counts every block against the clock, writes the IQ records, answers commands between blocks
//! and keeps the status document current.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::iq::{self, Header};
use crate::loss::{self, Segment};
use crate::plan::{Band, Gain, Scheduler, DEFAULT_PLAN};
use crate::radio::{all_zero, mono_ns, HwStatus, Radio, Recorder, RefillError, SafeState};
use crate::svc::{self, Cmd};

/// A command and the channel its answer goes back on.
pub type Envelope = (Cmd, Sender<Result<Value, String>>);

/// Where records and readings go.
pub trait Out: Send {
    /// Write one ring record (the `iq.cs16.v1` message for `h` and `samples`, stamped with the
    /// next seq); returns its seq (0 when there is no ring).
    fn record(&mut self, t_ns: u64, h: &Header, samples: &[u8]) -> Result<u64, String>;
    /// Publish the `sdr.status` reading.
    fn status(&mut self, t_ns: u64, doc: &Value);
    /// Connection facts for the status document.
    fn info(&self) -> Value {
        Value::Null
    }
}

/// State the HTTP server, the service and the stdout reporter read.
#[derive(Default)]
pub struct Shared {
    pub status: Mutex<Value>,
    pub raw: Mutex<Vec<u8>>,
    pub healthy: AtomicBool,
    pub stop: AtomicBool,
}

pub struct Settings {
    pub source: String,
    pub backend: String,
    pub status_period: Duration,
    pub hw_period: Duration,
    pub publish_period: Duration,
    pub loss: loss::Params,
    /// Stop after this many data blocks (`--once`); None runs until stopped.
    pub max_blocks: Option<u64>,
    pub raw_samples: usize,
    /// Samples per ring record; an acquisition buffer is cut into records of this size.
    pub record_samples: usize,
}

#[derive(Default, Clone)]
pub struct Totals {
    pub delivered: u64,
    pub lost: u64,
    pub gap_events: u64,
    pub blocks: u64,
    pub zero_blocks: u64,
    pub consecutive_zero: u32,
    pub retunes: u64,
    pub apply_errors: u64,
    pub refill_errors: u64,
    pub overflows: u64,
    pub disconnects: u64,
    pub expected_closed: u64,
    pub pending: u64,
    pub record_errors: u64,
    pub last_seq: u64,
    pub last_block_ns: u64,
    pub power_dbfs: f64,
    pub segments: u32,
}

pub enum Exit {
    Stop,
    /// The radio is gone; reopen it.
    Lost(String),
    Eof,
    /// `max_blocks` reached.
    Done,
}

pub struct Acq {
    pub sched: Scheduler,
    pub totals: Totals,
    pub set: Settings,
    pub out: Box<dyn Out>,
    pub shared: Arc<Shared>,
    pub cmds: Receiver<Envelope>,
    pub started_ns: u64,
    pub last_error: Option<String>,
    pub safe: SafeState,
    pub hw: HwStatus,
    cur: Option<(String, Band, u32)>,
    seg: Option<Segment>,
    last_status: u64,
    last_hw: u64,
    last_pub: u64,
    records: Option<Recorder>,
    state: &'static str,
}

/// libiio errors that mean the device went away (the message ends in "(-errno)").
pub fn is_lost(msg: &str) -> bool {
    let code = msg
        .rsplit_once('(')
        .and_then(|(_, t)| t.strip_suffix(')'))
        .and_then(|t| t.parse::<i32>().ok());
    matches!(
        code,
        Some(-19 | -5 | -110 | -32 | -104 | -107 | -71 | -108 | -9)
    ) || msg.contains("No such device")
        || msg.contains("Broken pipe")
        || msg.contains("Connection reset")
}

impl Acq {
    pub fn new(
        sched: Scheduler,
        set: Settings,
        out: Box<dyn Out>,
        shared: Arc<Shared>,
        cmds: Receiver<Envelope>,
    ) -> Self {
        Self {
            sched,
            totals: Totals::default(),
            set,
            out,
            shared,
            cmds,
            started_ns: mono_ns(),
            last_error: None,
            safe: SafeState::default(),
            hw: HwStatus::default(),
            cur: None,
            seg: None,
            last_status: 0,
            last_hw: 0,
            last_pub: 0,
            records: None,
            state: "starting",
        }
    }

    pub fn record_to(&mut self, r: Recorder) {
        self.records = Some(r);
    }

    pub fn set_state(&mut self, s: &'static str, why: Option<String>) {
        self.state = s;
        if why.is_some() {
            self.last_error = why;
        }
        let now = mono_ns();
        self.publish_status(now, true);
    }

    /// Answer commands that arrived while no radio is running (a missing device still answers
    /// `status`, and refuses what needs a radio only when it tries to run it).
    pub fn service(&mut self) {
        let now = mono_ns();
        self.drain_cmds(now);
        self.publish_status(now, false);
    }

    /// Run commands waiting in the queue; true if one changed what should be tuned.
    fn drain_cmds(&mut self, now: u64) -> bool {
        let mut changed = false;
        while let Ok((cmd, reply)) = self.cmds.try_recv() {
            let doc = self.status_doc(now);
            let r = svc::exec(&mut self.sched, cmd, now, &doc);
            if let Ok((_, c)) = &r {
                changed |= *c;
            }
            let _ = reply.send(r.map(|(v, _)| v));
        }
        changed
    }

    /// Fold the running segment into the totals when it ends.
    fn close_segment(&mut self, now: u64) {
        if let Some(seg) = self.seg.take() {
            self.totals.expected_closed += seg.expected(now);
            self.totals.pending = seg.pending();
        }
    }

    pub fn run(&mut self, radio: &mut dyn Radio) -> Exit {
        let mut buf: Vec<u8> = Vec::new();
        let n = radio.block_samples() as u32;
        let mut band_changed = true;
        self.hw = radio.hw_status();
        self.safe = radio.safe_state();
        'slots: loop {
            if self.shared.stop.load(Ordering::Acquire) {
                return Exit::Stop;
            }
            let now = mono_ns();
            self.drain_cmds(now);
            let Some(slot) = self.sched.next(now) else {
                self.state = "idle";
                self.publish_status(now, false);
                std::thread::sleep(Duration::from_millis(50));
                continue;
            };
            if let Err(e) = radio.apply(&slot.band) {
                self.totals.apply_errors += 1;
                self.last_error = Some(format!("apply {} Hz: {e}", slot.band.centre_hz));
                if is_lost(&e) {
                    return Exit::Lost(e);
                }
                self.publish_status(mono_ns(), false);
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
            self.totals.retunes += 1;
            self.last_error = None;
            self.hw = radio.hw_status();
            self.totals.segments += 1;
            self.state = "streaming";
            self.cur = Some((slot.plan.clone(), slot.band.clone(), self.totals.segments));
            self.seg = Some(Segment::new(slot.band.rate_sps, n, self.set.loss));
            let start = mono_ns();
            let dwell_end =
                (self.sched.multiplexed() || slot.leased).then(|| start + slot.dwell_ns);
            let mut first = true;
            let mut other_errors = 0u32;
            loop {
                if self.shared.stop.load(Ordering::Acquire) {
                    self.close_segment(mono_ns());
                    return Exit::Stop;
                }
                match radio.refill(&mut buf) {
                    Ok(()) => other_errors = 0,
                    Err(RefillError::Eof) => {
                        self.close_segment(mono_ns());
                        return Exit::Eof;
                    }
                    Err(RefillError::Lost(e)) => {
                        self.totals.refill_errors += 1;
                        if e.contains("(-75)") {
                            self.totals.overflows += 1;
                        }
                        self.last_error = Some(format!("refill: {e}"));
                        self.close_segment(mono_ns());
                        return Exit::Lost(e);
                    }
                    Err(RefillError::Other(e)) => {
                        self.totals.refill_errors += 1;
                        self.last_error = Some(format!("refill: {e}"));
                        other_errors += 1;
                        if other_errors >= 50 {
                            self.close_segment(mono_ns());
                            return Exit::Lost(e);
                        }
                        std::thread::sleep(Duration::from_millis(20));
                        continue;
                    }
                }
                let t = mono_ns();
                let seg = self.seg.as_mut().expect("segment open while streaming");
                let arrival = seg.on_block(t, n);
                self.totals.delivered += u64::from(n);
                self.totals.blocks += 1;
                self.totals.last_block_ns = t;
                let zero = all_zero(&buf);
                if zero {
                    self.totals.zero_blocks += 1;
                    self.totals.consecutive_zero += 1;
                } else {
                    self.totals.consecutive_zero = 0;
                    self.totals.power_dbfs = iq::mean_power_dbfs(&buf);
                }
                let (centre, rate, bw) = (slot.band.centre_hz, slot.band.rate_sps, slot.band.bw_hz);
                let seg_id = self.totals.segments;
                let base = Header {
                    kind: iq::KIND_SAMPLES,
                    flags: 0,
                    segment: seg_id,
                    centre_hz: centre,
                    rate_sps: rate,
                    bw_hz: bw,
                    sample_index: arrival.sample_index,
                    lost: 0,
                    n_samples: 0,
                };
                let mut flags = 0u8;
                let mut lost32 = 0u32;
                if arrival.lost_before > 0 {
                    let lost = arrival.lost_before;
                    self.totals.lost += lost;
                    self.totals.gap_events += 1;
                    if self.totals.gap_events <= 20 || self.totals.gap_events.is_multiple_of(100) {
                        eprintln!(
                            "[cog-sdr-pluto-rx] gap #{}: {lost} samples ({:.0} ms at {rate} S/s) declared at +{:.2} s, segment {}",
                            self.totals.gap_events,
                            lost as f64 * 1e3 / f64::from(slot.band.rate_sps),
                            (t - self.started_ns) as f64 / 1e9,
                            self.totals.segments
                        );
                    }
                    lost32 = lost.min(u64::from(u32::MAX)) as u32;
                    let gap = Header {
                        kind: iq::KIND_GAP,
                        flags: iq::F_DISCONT,
                        lost: lost32,
                        sample_index: arrival.sample_index - lost,
                        ..base.clone()
                    };
                    self.write_record(t, &gap, &[]);
                    flags |= iq::F_DISCONT;
                }
                if band_changed || first {
                    flags |= iq::F_BAND_CHANGED;
                    band_changed = false;
                }
                first = false;
                // One acquisition buffer becomes several ring records. They share the buffer's
                // arrival time; each record's t_ns is when its own last sample was captured.
                let rec_bytes = self.set.record_samples.max(1) * 4;
                let chunks = buf.len().div_ceil(rec_bytes) as u64;
                let rec_ns = (self.set.record_samples as f64 * 1e9 / f64::from(rate)) as u64;
                for (k, chunk) in buf.chunks(rec_bytes).enumerate() {
                    let mut h = base.clone();
                    h.sample_index = arrival.sample_index + (k * self.set.record_samples) as u64;
                    h.n_samples = (chunk.len() / 4) as u32;
                    h.flags = if k == 0 { flags } else { 0 };
                    h.lost = if k == 0 { lost32 } else { 0 };
                    if all_zero(chunk) {
                        h.flags |= iq::F_ALL_ZERO;
                    }
                    let tk = t.saturating_sub((chunks - 1 - k as u64) * rec_ns);
                    self.write_record(tk, &h, chunk);
                }
                if let Some(r) = self.records.as_mut() {
                    if let Err(e) = r.push(&buf) {
                        self.last_error = Some(e);
                    }
                }
                self.keep_raw(&buf);
                self.periodic(t, radio);
                if self.drain_cmds(t) {
                    self.close_segment(t);
                    band_changed = true;
                    continue 'slots;
                }
                if self.set.max_blocks.is_some_and(|m| self.totals.blocks >= m) {
                    self.close_segment(t);
                    return Exit::Done;
                }
                if dwell_end.is_some_and(|end| t >= end) {
                    self.close_segment(t);
                    band_changed = true;
                    continue 'slots;
                }
            }
        }
    }

    fn write_record(&mut self, t: u64, h: &Header, samples: &[u8]) {
        match self.out.record(t, h, samples) {
            Ok(seq) => {
                if seq > 0 {
                    self.totals.last_seq = seq;
                }
            }
            Err(e) => {
                self.totals.record_errors += 1;
                self.last_error = Some(format!("ring: {e}"));
            }
        }
    }

    fn keep_raw(&self, buf: &[u8]) {
        let keep = (self.set.raw_samples * 4).min(buf.len());
        if let Ok(mut r) = self.shared.raw.lock() {
            r.clear();
            r.extend_from_slice(&buf[buf.len() - keep..]);
        }
    }

    fn periodic(&mut self, t: u64, radio: &mut dyn Radio) {
        if t - self.last_hw >= self.set.hw_period.as_nanos() as u64 {
            self.last_hw = t;
            self.hw = radio.hw_status();
        }
        self.publish_status(t, false);
    }

    fn publish_status(&mut self, t: u64, force: bool) {
        if !force && t - self.last_status < self.set.status_period.as_nanos() as u64 {
            return;
        }
        self.last_status = t;
        let doc = self.status_doc(t);
        self.shared
            .healthy
            .store(doc["healthy"].as_bool().unwrap_or(false), Ordering::Release);
        if let Ok(mut s) = self.shared.status.lock() {
            *s = doc.clone();
        }
        if force || t - self.last_pub >= self.set.publish_period.as_nanos() as u64 {
            self.last_pub = t;
            self.out.status(t, &doc);
        }
    }

    /// The status document: `GET /status`, the `status` call, the stdout report and the source of
    /// the `sdr.status` reading.
    pub fn status_doc(&mut self, now: u64) -> Value {
        self.sched.credit(now);
        let tt = &self.totals;
        let seg = self.seg.as_ref();
        let expected = tt.expected_closed + seg.map_or(0, |s| s.expected(now));
        let pending = seg.map_or(tt.pending, Segment::pending);
        let elapsed = now.saturating_sub(self.started_ns) as f64 / 1e9;
        let age_ms = if tt.last_block_ns == 0 {
            None
        } else {
            Some(now.saturating_sub(tt.last_block_ns) / 1_000_000)
        };
        let streaming = self.state == "streaming";
        let dead = tt.consecutive_zero >= 4;
        let healthy = streaming && !dead && age_ms.is_some_and(|a| a < 5000);
        let band = self.cur.as_ref().map(|(plan, b, seg)| {
            json!({
                "plan": plan, "segment": seg, "centre_hz": b.centre_hz, "rate_sps": b.rate_sps,
                "bw_hz": b.bw_hz, "gain": match b.gain { Gain::Manual(g) => json!(g), Gain::Auto => json!("auto") },
                "uncalibrated": b.out_of_rated_range(),
            })
        });
        let loss_fraction = if expected > 0 {
            tt.lost as f64 / expected as f64
        } else {
            0.0
        };
        let hw = &self.hw;
        let temp = |t: Option<f64>| t.map_or(-1.0, |v| (v * 10.0).round() / 10.0);
        let vector = [
            self.cur
                .as_ref()
                .map_or(0.0, |(_, b, _)| b.centre_hz as f64 / 1e9),
            self.cur
                .as_ref()
                .map_or(0.0, |(_, b, _)| f64::from(b.rate_sps) / 1e6),
            tt.power_dbfs,
            loss_fraction,
            temp(hw.temp_phy_c),
            temp(hw.temp_zynq_c),
            if tt.blocks > 0 {
                tt.zero_blocks as f64 / tt.blocks as f64
            } else {
                0.0
            },
            tt.gap_events as f64,
        ];
        let duty: Vec<Value> = self
            .sched
            .duty()
            .into_iter()
            .map(|(plan, ns, permille)| json!({"plan": plan, "seconds": ns as f64 / 1e9, "permille": permille}))
            .collect();
        let plans: Vec<Value> = self
            .sched
            .plans()
            .iter()
            .map(|p| json!({"id": p.id, "owner": p.owner, "bands": p.bands.len(), "fallback": p.id == DEFAULT_PLAN}))
            .collect();
        json!({
            "status": self.state,
            "healthy": healthy,
            "source": self.set.source,
            "backend": self.set.backend,
            "error": self.last_error,
            "uptime_s": (elapsed * 10.0).round() / 10.0,
            "band": band,
            "hw": {
                "lo_hz": hw.lo_hz, "rate_sps": hw.rate_sps, "bw_hz": hw.bw_hz, "gain_db": hw.gain_db,
                "gain_mode": hw.gain_mode, "rssi_db": hw.rssi_db,
                "temp_phy_c": hw.temp_phy_c, "temp_zynq_c": hw.temp_zynq_c,
            },
            "samples": {
                "delivered": tt.delivered, "expected": expected, "lost": tt.lost,
                "pending_unconfirmed": pending, "loss_fraction": loss_fraction,
            },
            "counters": {
                "blocks": tt.blocks, "gap_events": tt.gap_events, "overflows": tt.overflows,
                "zero_blocks": tt.zero_blocks, "consecutive_zero": tt.consecutive_zero,
                "dead_receive_path": dead, "retunes": tt.retunes, "apply_errors": tt.apply_errors,
                "refill_errors": tt.refill_errors, "disconnects": tt.disconnects,
                "ring_records": tt.last_seq, "ring_errors": tt.record_errors,
            },
            "last_block_age_ms": age_ms,
            "power_dbfs": tt.power_dbfs,
            "safe_state": {
                "loopback_was": self.safe.loopback_was, "loopback_now": self.safe.loopback_now,
                "tx_atten_was_db": self.safe.tx_atten_was_db, "tx_atten_now_db": self.safe.tx_atten_now_db,
                "dds_tones_off": self.safe.dds_tones_off, "notes": self.safe.notes,
            },
            "bus": self.out.info(),
            "plans": plans,
            "duty": duty,
            "vector": vector,
        })
    }
}

#[cfg(test)]
mod tests;
