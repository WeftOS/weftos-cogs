//! Turns the owner's IQ records into spectrum and occupancy readings, counting what it did not
//! see.
//!
//! Windows are cut by time (`psd_ns`) and at every retune (a new segment). Each window produces a
//! `spectrum.psd.v1` reading; the windows of one band (centre and rate) add up into an occupancy
//! reading every `occ_ns`. Loss is never hidden: samples the owner declared lost, samples this
//! reader missed (a jump in the sample clock beyond what the owner declared) and samples of a dead
//! (all-zero) receive path are counted apart, are kept out of the spectrum, and are reported with
//! the fraction of the stream they make up. A gap also breaks the overlap between FFT frames, so
//! no frame straddles missing samples.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::iq::{Header, F_ALL_ZERO, KIND_GAP};
use crate::psd::{decimate_db, Occupancy, Params, Welch};

/// The AD9363's rated range; levels outside it are not calibrated.
pub const RATED_HZ: (u64, u64) = (325_000_000, 3_800_000_000);

#[derive(Debug, Clone)]
pub struct Config {
    pub fft: usize,
    pub bins_out: usize,
    pub slices: usize,
    pub thresh_db: f64,
    pub psd_ns: u64,
    pub occ_ns: u64,
    pub stride: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            fft: 1024,
            bins_out: 256,
            slices: 8,
            thresh_db: 10.0,
            psd_ns: 2_000_000_000,
            occ_ns: 60_000_000_000,
            stride: 4,
        }
    }
}

/// Sample counts of a window or an interval.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub used: u64,
    /// Declared lost by the owner.
    pub lost: u64,
    /// Missed by this reader: the sample clock jumped by more than the owner declared.
    pub skipped: u64,
    /// Delivered as zeros by a dead receive path, kept out of the spectrum.
    pub dead: u64,
}

impl Counts {
    fn add(&mut self, o: &Counts) {
        self.used += o.used;
        self.lost += o.lost;
        self.skipped += o.skipped;
        self.dead += o.dead;
    }

    /// Share of the stream that did not make it into the spectrum.
    pub fn loss_fraction(&self) -> f64 {
        let all = self.used + self.lost + self.skipped + self.dead;
        if all == 0 {
            0.0
        } else {
            (self.lost + self.skipped + self.dead) as f64 / all as f64
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Band {
    centre_hz: u64,
    rate_sps: u32,
}

#[derive(Debug, Clone, PartialEq)]
struct Key {
    segment: u32,
    band: Band,
    bw_hz: u32,
}

#[derive(Debug, Clone)]
pub struct PsdMsg {
    pub centre_hz: u64,
    pub rate_sps: u32,
    pub bw_hz: u32,
    pub segment: u32,
    pub bins_db: Vec<f32>,
    pub fft: usize,
    pub frames: u64,
    pub integration_ms: u64,
    pub counts: Counts,
    pub floor_db: f64,
    pub peak_db: f64,
    pub t_ns: u64,
}

#[derive(Debug, Clone)]
pub struct OccMsg {
    pub centre_hz: u64,
    pub rate_sps: u32,
    pub bw_hz: u32,
    pub slices: Vec<f64>,
    pub overall: f64,
    pub floor_db: f64,
    pub peak_db: f64,
    pub thresh_db: f64,
    pub coverage: f64,
    pub frames: u64,
    pub counts: Counts,
    pub t_ns: u64,
}

#[derive(Debug, Clone)]
pub enum Event {
    Psd(PsdMsg),
    Occupancy(OccMsg),
}

#[derive(Default)]
struct BandAcc {
    occ: Option<Occupancy>,
    counts: Counts,
    bw_hz: u32,
    duration_ns: u64,
    frames: u64,
}

pub struct Collector {
    cfg: Config,
    welch: Option<Welch>,
    params: Option<Params>,
    key: Option<Key>,
    win: Counts,
    win_start: u64,
    win_end: u64,
    occ_start: u64,
    next_idx: Option<u64>,
    gap_counted: u64,
    bands: BTreeMap<Band, BandAcc>,
    last_t_ns: u64,
    pub total: Counts,
}

fn uncalibrated(centre_hz: u64) -> bool {
    centre_hz < RATED_HZ.0 || centre_hz > RATED_HZ.1
}

impl Collector {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            welch: None,
            params: None,
            key: None,
            win: Counts::default(),
            win_start: 0,
            win_end: 0,
            occ_start: 0,
            next_idx: None,
            gap_counted: 0,
            bands: BTreeMap::new(),
            last_t_ns: 0,
            total: Counts::default(),
        }
    }

    /// One record. `now_ns` is this reader's clock (or the stream's own time when replaying) and
    /// decides when windows end; `t_ns` is the record's capture clock, copied into the readings.
    pub fn on_record(&mut self, h: &Header, samples: &[u8], t_ns: u64, now_ns: u64) -> Vec<Event> {
        let mut ev = Vec::new();
        self.last_t_ns = t_ns;
        let key = Key {
            segment: h.segment,
            band: Band {
                centre_hz: h.centre_hz,
                rate_sps: h.rate_sps,
            },
            bw_hz: h.bw_hz,
        };
        if self.key.as_ref() != Some(&key) {
            self.close_window(now_ns, &mut ev);
            self.open_window(key, now_ns);
        }
        let mut c = Counts::default();
        if h.kind == KIND_GAP {
            c.lost = u64::from(h.lost);
            self.gap_counted = u64::from(h.lost);
            self.break_run();
        } else {
            if h.lost > 0 && u64::from(h.lost) != self.gap_counted {
                c.lost = u64::from(h.lost);
                self.break_run();
            }
            self.gap_counted = 0;
            if let Some(exp) = self.next_idx {
                if h.sample_index > exp + u64::from(h.lost) {
                    c.skipped = h.sample_index - exp - u64::from(h.lost);
                    self.break_run();
                } else if h.sample_index < exp {
                    self.break_run();
                }
            }
            self.next_idx = Some(h.sample_index + u64::from(h.n_samples));
            if h.flags & F_ALL_ZERO != 0 {
                c.dead = u64::from(h.n_samples);
                self.break_run();
            } else if let (Some(w), Some(p)) = (self.welch.as_mut(), self.params.as_ref()) {
                w.push(samples, p);
                c.used = u64::from(h.n_samples);
            }
        }
        self.win.add(&c);
        self.total.add(&c);
        self.win_end = now_ns;
        if now_ns.saturating_sub(self.win_start) >= self.cfg.psd_ns {
            self.close_window(now_ns, &mut ev);
            if let Some(k) = self.key.clone() {
                self.open_window(k, now_ns);
            }
        }
        if now_ns.saturating_sub(self.occ_start) >= self.cfg.occ_ns {
            self.flush_occupancy(now_ns, &mut ev);
        }
        ev
    }

    fn break_run(&mut self) {
        if let Some(w) = self.welch.as_mut() {
            w.break_continuity();
        }
    }

    fn open_window(&mut self, key: Key, now_ns: u64) {
        let usable =
            (f64::from(key.bw_hz) / f64::from(key.band.rate_sps).max(1.0)).clamp(0.1, 0.95);
        let p = Params {
            n: self.cfg.fft,
            slices: self.cfg.slices,
            thresh_db: self.cfg.thresh_db,
            usable,
            stride: self.cfg.stride,
        };
        // A bad FFT size was refused at start; a failure here would leave the window empty.
        self.welch = Welch::new(&p).ok();
        self.params = Some(p);
        self.key = Some(key);
        self.win = Counts::default();
        self.win_start = now_ns;
        self.win_end = now_ns;
        self.next_idx = None;
        self.gap_counted = 0;
        if self.occ_start == 0 {
            self.occ_start = now_ns;
        }
    }

    fn close_window(&mut self, now_ns: u64, ev: &mut Vec<Event>) {
        let (Some(key), Some(w)) = (self.key.clone(), self.welch.as_mut()) else {
            return;
        };
        let dur = now_ns.saturating_sub(self.win_start);
        let spec = w.take();
        let acc = self.bands.entry(key.band.clone()).or_default();
        acc.bw_hz = key.bw_hz;
        acc.duration_ns += dur;
        acc.counts.add(&self.win);
        if let Some(s) = spec {
            acc.frames += s.frames;
            match acc.occ.as_mut() {
                Some(o) => o.merge(&s.occ),
                None => acc.occ = Some(s.occ.clone()),
            }
            ev.push(Event::Psd(PsdMsg {
                centre_hz: key.band.centre_hz,
                rate_sps: key.band.rate_sps,
                bw_hz: key.bw_hz,
                segment: key.segment,
                bins_db: decimate_db(&s.bins_db, self.cfg.bins_out),
                fft: self.cfg.fft,
                frames: s.frames,
                integration_ms: dur / 1_000_000,
                counts: self.win,
                floor_db: s.occ.floor_db(),
                peak_db: s.occ.peak_db,
                t_ns: self.last_t_ns,
            }));
        }
        self.win = Counts::default();
        self.win_start = now_ns;
    }

    /// End the running window and the interval now (end of input, or a quiet stream).
    pub fn flush(&mut self, now_ns: u64) -> Vec<Event> {
        let mut ev = Vec::new();
        self.close_window(now_ns, &mut ev);
        self.flush_occupancy(now_ns, &mut ev);
        ev
    }

    /// Flush windows that have outlived their time although no record arrived.
    pub fn tick(&mut self, now_ns: u64) -> Vec<Event> {
        let mut ev = Vec::new();
        if self.key.is_some() && now_ns.saturating_sub(self.win_start) >= self.cfg.psd_ns {
            self.close_window(now_ns, &mut ev);
        }
        if self.occ_start != 0 && now_ns.saturating_sub(self.occ_start) >= self.cfg.occ_ns {
            self.flush_occupancy(now_ns, &mut ev);
        }
        ev
    }

    fn flush_occupancy(&mut self, now_ns: u64, ev: &mut Vec<Event>) {
        let span = now_ns.saturating_sub(self.occ_start).max(1);
        for (band, acc) in std::mem::take(&mut self.bands) {
            let Some(o) = acc.occ else {
                if acc.counts.lost + acc.counts.skipped + acc.counts.dead > 0 {
                    ev.push(Event::Occupancy(OccMsg {
                        centre_hz: band.centre_hz,
                        rate_sps: band.rate_sps,
                        bw_hz: acc.bw_hz,
                        slices: vec![0.0; self.cfg.slices],
                        overall: 0.0,
                        floor_db: -300.0,
                        peak_db: -300.0,
                        thresh_db: self.cfg.thresh_db,
                        coverage: acc.duration_ns as f64 / span as f64,
                        frames: 0,
                        counts: acc.counts,
                        t_ns: self.last_t_ns,
                    }));
                }
                continue;
            };
            ev.push(Event::Occupancy(OccMsg {
                centre_hz: band.centre_hz,
                rate_sps: band.rate_sps,
                bw_hz: acc.bw_hz,
                slices: (0..o.busy.len()).map(|i| o.slice(i)).collect(),
                overall: o.overall(),
                floor_db: o.floor_db(),
                peak_db: o.peak_db,
                thresh_db: self.cfg.thresh_db,
                coverage: (acc.duration_ns as f64 / span as f64).min(1.0),
                frames: acc.frames,
                counts: acc.counts,
                t_ns: self.last_t_ns,
            }));
        }
        self.occ_start = now_ns;
    }
}

fn round1(v: f32) -> f64 {
    (f64::from(v.clamp(-300.0, 100.0)) * 10.0).round() / 10.0
}

fn counts_json(c: &Counts, v: &mut Value) {
    v["samples_used"] = json!(c.used);
    v["samples_lost"] = json!(c.lost);
    v["samples_skipped"] = json!(c.skipped);
    v["samples_dead"] = json!(c.dead);
    v["loss_fraction"] = json!((c.loss_fraction() * 1e6).round() / 1e6);
}

impl PsdMsg {
    /// The registered `spectrum.psd.v1` reading with this cog's optional fields.
    pub fn to_json(&self, source: &str, node: &str, seq: u64) -> Value {
        let mut v = json!({
            "source_id": source, "node_id": node, "t_ns": self.t_ns, "seq": seq,
            "center_hz": self.centre_hz,
            "span_hz": self.rate_sps,
            "bins": self.bins_db.iter().map(|&b| round1(b)).collect::<Vec<_>>(),
            "sample_rate_hz": self.rate_sps,
            "bandwidth_hz": self.bw_hz,
            "bin_hz": f64::from(self.rate_sps) / self.bins_db.len().max(1) as f64,
            "fft_size": self.fft,
            "frames": self.frames,
            "integration_ms": self.integration_ms,
            "segment": self.segment,
            "floor_dbfs": (self.floor_db * 10.0).round() / 10.0,
            "peak_dbfs": (self.peak_db * 10.0).round() / 10.0,
            "uncalibrated": uncalibrated(self.centre_hz),
        });
        counts_json(&self.counts, &mut v);
        v
    }
}

impl OccMsg {
    /// A `telemetry.v1` reading: one item per number, named `occ.*`.
    pub fn to_json(&self, source: &str, node: &str, seq: u64) -> Value {
        let mut items = Vec::new();
        let mut add = |name: String, value: f64, unit: &str| {
            items.push(json!({"name": name, "value": (value * 1e6).round() / 1e6, "unit": unit}));
        };
        add("band.centre".into(), self.centre_hz as f64, "Hz");
        add("band.rate".into(), f64::from(self.rate_sps), "S/s");
        add("band.bandwidth".into(), f64::from(self.bw_hz), "Hz");
        add("occ.overall".into(), self.overall, "");
        for (i, s) in self.slices.iter().enumerate() {
            add(format!("occ.slice{i}"), *s, "");
        }
        add("occ.threshold".into(), self.thresh_db, "dB");
        add("floor".into(), self.floor_db, "dBFS");
        add("peak".into(), self.peak_db, "dBFS");
        add("coverage".into(), self.coverage, "");
        add("frames".into(), self.frames as f64, "");
        add("samples.used".into(), self.counts.used as f64, "");
        add("samples.lost".into(), self.counts.lost as f64, "");
        add("samples.skipped".into(), self.counts.skipped as f64, "");
        add("samples.dead".into(), self.counts.dead as f64, "");
        add("loss.fraction".into(), self.counts.loss_fraction(), "");
        add(
            "uncalibrated".into(),
            if uncalibrated(self.centre_hz) {
                1.0
            } else {
                0.0
            },
            "",
        );
        json!({"source_id": source, "node_id": node, "t_ns": self.t_ns, "seq": seq, "items": items})
    }
}

#[cfg(test)]
mod tests;
