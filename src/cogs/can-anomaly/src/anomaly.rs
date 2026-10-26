//! What a CAN bus normally looks like, and what departs from it. No model, no NPU: counts.
//!
//! For the first `learn_ns` the engine only learns, per id (11-bit and 29-bit ids are different
//! ids): how many frames, and which data lengths. Then it watches in windows of `window_ns`:
//!
//! - **`can.new_id`**: an id that was not seen while learning, at its first frame (once per id);
//! - **`can.dlc_change`**: a known id with a data length it never had while learning (once per pair);
//! - **`can.id_missing`** / **`can.id_back`**: a periodic id (baseline 0.5 Hz or more) silent for
//!   `missing_windows` windows or five of its periods, whichever is longer; and its return;
//! - **`can.rate_high`**: an id's window rate above `rate_high_x` times its baseline and above the
//!   baseline plus `rate_high_min_hz`;
//! - **`can.rate_low`**: an id of 2 Hz or more running below `rate_low_frac` of its baseline
//!   while still sending;
//! - **`can.flood`** (alert): the whole bus above three times its baseline total and 50 Hz over.
//!
//! Rate events repeat at most every `cooldown_ns` per id. Time is the frames' own clock
//! (`advance` takes it), so a log replays the same as a live bus.

use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq)]
pub struct Params {
    pub learn_ns: u64,
    pub window_ns: u64,
    pub rate_high_x: f64,
    pub rate_high_min_hz: f64,
    pub rate_low_frac: f64,
    pub missing_windows: u64,
    pub cooldown_ns: u64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            learn_ns: 30_000_000_000,
            window_ns: 5_000_000_000,
            rate_high_x: 3.0,
            rate_high_min_hz: 5.0,
            rate_low_frac: 0.25,
            missing_windows: 2,
            cooldown_ns: 30_000_000_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub t_ns: u64,
    pub kind: &'static str,
    pub severity: &'static str,
    pub subject: String,
    pub detail: String,
    pub value: Option<f64>,
}

/// An id as the bus sees it: width matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub id: u32,
    pub ext: bool,
}

impl Key {
    pub fn label(&self) -> String {
        if self.ext {
            format!("{:#010x}", self.id)
        } else {
            format!("{:#x}", self.id)
        }
    }
}

#[derive(Debug, Default, Clone)]
struct Base {
    count: u64,
    rate_hz: f64,
    dlcs: BTreeSet<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Learning,
    Monitoring,
}

pub struct Engine {
    p: Params,
    start_ns: Option<u64>,
    phase: Phase,
    base: BTreeMap<Key, Base>,
    total_base_hz: f64,
    window_start: u64,
    cur: BTreeMap<Key, u64>,
    last_seen: BTreeMap<Key, u64>,
    announced: BTreeSet<Key>,
    dlc_seen: BTreeSet<(Key, u8)>,
    missing: BTreeSet<Key>,
    cooldown: BTreeMap<(&'static str, Key), u64>,
    pub frames: u64,
    pub counts: BTreeMap<&'static str, u64>,
}

impl Engine {
    pub fn new(p: Params) -> Self {
        Self {
            p,
            start_ns: None,
            phase: Phase::Learning,
            base: BTreeMap::new(),
            total_base_hz: 0.0,
            window_start: 0,
            cur: BTreeMap::new(),
            last_seen: BTreeMap::new(),
            announced: BTreeSet::new(),
            dlc_seen: BTreeSet::new(),
            missing: BTreeSet::new(),
            cooldown: BTreeMap::new(),
            frames: 0,
            counts: BTreeMap::new(),
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn baseline_ids(&self) -> usize {
        self.base.len()
    }

    pub fn learned_hz(&self) -> f64 {
        self.total_base_hz
    }

    fn emit(&mut self, out: &mut Vec<Event>, e: Event) {
        *self.counts.entry(e.kind).or_default() += 1;
        out.push(e);
    }

    fn cool(&mut self, kind: &'static str, k: Key, t: u64) -> bool {
        match self.cooldown.get(&(kind, k)) {
            Some(last) if t < last + self.p.cooldown_ns => false,
            _ => {
                self.cooldown.insert((kind, k), t);
                true
            }
        }
    }

    /// Move the clock to `t_ns`: ends the learning phase and closes the windows that are over.
    pub fn advance(&mut self, t_ns: u64) -> Vec<Event> {
        let mut out = Vec::new();
        let start = *self.start_ns.get_or_insert(t_ns);
        if self.phase == Phase::Learning && t_ns >= start + self.p.learn_ns {
            self.finish_learning(start + self.p.learn_ns, &mut out);
        }
        while self.phase == Phase::Monitoring && t_ns >= self.window_start + self.p.window_ns {
            let end = self.window_start + self.p.window_ns;
            self.close_window(end, &mut out);
            self.window_start = end;
        }
        out
    }

    fn finish_learning(&mut self, at: u64, out: &mut Vec<Event>) {
        let secs = self.p.learn_ns as f64 / 1e9;
        let mut total = 0.0;
        for b in self.base.values_mut() {
            b.rate_hz = b.count as f64 / secs;
            total += b.rate_hz;
        }
        self.total_base_hz = total;
        self.phase = Phase::Monitoring;
        self.window_start = at;
        self.cur.clear();
        let n = self.base.len();
        self.emit(
            out,
            Event {
                t_ns: at,
                kind: "can.baseline",
                severity: "info",
                subject: "bus".into(),
                detail: format!("learned {n} ids, {total:.1} frames/s, in {secs:.0} s"),
                value: Some(n as f64),
            },
        );
    }

    fn close_window(&mut self, end: u64, out: &mut Vec<Event>) {
        let secs = self.p.window_ns as f64 / 1e9;
        let cur = std::mem::take(&mut self.cur);
        let total_now: f64 = cur.values().sum::<u64>() as f64 / secs;
        if total_now > 3.0 * self.total_base_hz && total_now > self.total_base_hz + 50.0 {
            let e = Event {
                t_ns: end,
                kind: "can.flood",
                severity: "alert",
                subject: "bus".into(),
                detail: format!(
                    "{total_now:.0} frames/s against a baseline of {:.0}",
                    self.total_base_hz
                ),
                value: Some(total_now),
            };
            self.emit(out, e);
        }
        let keys: Vec<Key> = self.base.keys().copied().collect();
        for k in keys {
            let base = self.base[&k].clone();
            let n = cur.get(&k).copied().unwrap_or(0);
            let rate = n as f64 / secs;
            if base.rate_hz >= 0.5 {
                let silent_for = end.saturating_sub(
                    self.last_seen
                        .get(&k)
                        .copied()
                        .unwrap_or(self.start_ns.unwrap_or(0)),
                );
                let limit = (self.p.missing_windows * self.p.window_ns)
                    .max((5.0 / base.rate_hz * 1e9) as u64);
                if n == 0 && silent_for >= limit && self.missing.insert(k) {
                    let e = Event {
                        t_ns: end,
                        kind: "can.id_missing",
                        severity: "warn",
                        subject: k.label(),
                        detail: format!(
                            "silent for {:.0} s; it sent {:.1} frames/s while learning",
                            silent_for as f64 / 1e9,
                            base.rate_hz
                        ),
                        value: Some(base.rate_hz),
                    };
                    self.emit(out, e);
                }
            }
            if n > 0
                && rate > self.p.rate_high_x * base.rate_hz
                && rate > base.rate_hz + self.p.rate_high_min_hz
                && self.cool("can.rate_high", k, end)
            {
                let e = Event {
                    t_ns: end,
                    kind: "can.rate_high",
                    severity: "warn",
                    subject: k.label(),
                    detail: format!(
                        "{rate:.1} frames/s against a baseline of {:.1}",
                        base.rate_hz
                    ),
                    value: Some(rate),
                };
                self.emit(out, e);
            }
            if n > 0
                && base.rate_hz >= 2.0
                && rate < self.p.rate_low_frac * base.rate_hz
                && self.cool("can.rate_low", k, end)
            {
                let e = Event {
                    t_ns: end,
                    kind: "can.rate_low",
                    severity: "warn",
                    subject: k.label(),
                    detail: format!(
                        "{rate:.1} frames/s against a baseline of {:.1}",
                        base.rate_hz
                    ),
                    value: Some(rate),
                };
                self.emit(out, e);
            }
        }
    }

    /// One frame at `t_ns`. Returns the events it and the clock moving to `t_ns` caused.
    pub fn feed(&mut self, t_ns: u64, id: u32, ext: bool, dlc: u8) -> Vec<Event> {
        let mut out = self.advance(t_ns);
        self.frames += 1;
        let k = Key { id, ext };
        self.last_seen.insert(k, t_ns);
        match self.phase {
            Phase::Learning => {
                let b = self.base.entry(k).or_default();
                b.count += 1;
                b.dlcs.insert(dlc);
            }
            Phase::Monitoring => {
                *self.cur.entry(k).or_default() += 1;
                match self.base.get(&k) {
                    None => {
                        if self.announced.insert(k) {
                            let e = Event {
                                t_ns,
                                kind: "can.new_id",
                                severity: "warn",
                                subject: k.label(),
                                detail: format!(
                                    "id {} was not on the bus while learning",
                                    k.label()
                                ),
                                value: None,
                            };
                            self.emit(&mut out, e);
                        }
                    }
                    Some(b) => {
                        if !b.dlcs.contains(&dlc) && self.dlc_seen.insert((k, dlc)) {
                            let e = Event {
                                t_ns,
                                kind: "can.dlc_change",
                                severity: "info",
                                subject: k.label(),
                                detail: format!(
                                    "length code {dlc}; while learning it sent {:?}",
                                    b.dlcs
                                ),
                                value: Some(dlc as f64),
                            };
                            self.emit(&mut out, e);
                        }
                        if self.missing.remove(&k) {
                            let e = Event {
                                t_ns,
                                kind: "can.id_back",
                                severity: "info",
                                subject: k.label(),
                                detail: format!("{} is sending again", k.label()),
                                value: None,
                            };
                            self.emit(&mut out, e);
                        }
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = 1_000_000_000;

    fn quick() -> Params {
        Params {
            learn_ns: 10 * S,
            window_ns: 2 * S,
            cooldown_ns: 6 * S,
            ..Params::default()
        }
    }

    /// Drive `secs` of traffic from t0: `ids` is (id, ext, frames per second, dlc).
    fn drive(e: &mut Engine, t0: u64, secs: u64, ids: &[(u32, bool, u64, u8)]) -> Vec<Event> {
        let mut out = Vec::new();
        for ms in (0..secs * 1000).step_by(10) {
            let t = t0 + ms * 1_000_000;
            for &(id, ext, hz, dlc) in ids {
                if ms % (1000 / hz).max(10) == 0 {
                    out.extend(e.feed(t, id, ext, dlc));
                }
            }
            out.extend(e.advance(t));
        }
        out
    }

    fn kinds(v: &[Event]) -> Vec<&'static str> {
        v.iter().map(|e| e.kind).collect()
    }

    const NORMAL: [(u32, bool, u64, u8); 3] = [
        (0x100, false, 50, 8),
        (0x200, false, 10, 4),
        (0x7e8, false, 1, 8),
    ];

    #[test]
    fn learning_ends_with_a_baseline_event_and_nothing_else() {
        let mut e = Engine::new(quick());
        let ev = drive(&mut e, 0, 12, &NORMAL);
        assert_eq!(kinds(&ev), ["can.baseline"], "{ev:?}");
        assert_eq!(e.phase(), Phase::Monitoring);
        assert_eq!(e.baseline_ids(), 3);
        assert!((e.learned_hz() - 61.0).abs() < 8.0, "{}", e.learned_hz());
    }

    #[test]
    fn normal_traffic_after_learning_is_quiet() {
        let mut e = Engine::new(quick());
        drive(&mut e, 0, 12, &NORMAL);
        let ev = drive(&mut e, 12 * S, 30, &NORMAL);
        assert!(ev.is_empty(), "{ev:?}");
    }

    #[test]
    fn a_new_id_is_announced_once() {
        let mut e = Engine::new(quick());
        drive(&mut e, 0, 12, &NORMAL);
        let mut traffic = NORMAL.to_vec();
        traffic.push((0x1a4, false, 5, 8));
        let ev = drive(&mut e, 12 * S, 10, &traffic);
        let new: Vec<&Event> = ev.iter().filter(|x| x.kind == "can.new_id").collect();
        assert_eq!(new.len(), 1, "{ev:?}");
        assert_eq!(
            (new[0].subject.as_str(), new[0].severity),
            ("0x1a4", "warn")
        );
    }

    #[test]
    fn an_extended_id_is_not_the_same_id_as_a_standard_one() {
        let mut e = Engine::new(quick());
        drive(&mut e, 0, 12, &NORMAL);
        let ev = e.feed(13 * S, 0x100, true, 8);
        assert_eq!(kinds(&ev), ["can.new_id"]);
        assert_eq!(ev[0].subject, "0x00000100");
    }

    #[test]
    fn a_changed_data_length_is_reported_once_per_length() {
        let mut e = Engine::new(quick());
        drive(&mut e, 0, 12, &NORMAL);
        assert_eq!(kinds(&e.feed(13 * S, 0x200, false, 8)), ["can.dlc_change"]);
        assert!(e.feed(13 * S + 1, 0x200, false, 8).is_empty());
        assert!(
            e.feed(13 * S + 2, 0x200, false, 4).is_empty(),
            "the learned length is fine"
        );
    }

    #[test]
    fn a_silent_periodic_id_is_missing_and_then_back() {
        let mut e = Engine::new(quick());
        drive(&mut e, 0, 12, &NORMAL);
        let without: Vec<_> = NORMAL.iter().copied().filter(|x| x.0 != 0x200).collect();
        let ev = drive(&mut e, 12 * S, 12, &without);
        let miss: Vec<&Event> = ev.iter().filter(|x| x.kind == "can.id_missing").collect();
        assert_eq!(miss.len(), 1, "once, not every window: {ev:?}");
        assert_eq!(miss[0].subject, "0x200");
        let ev = drive(&mut e, 24 * S, 4, &NORMAL);
        assert!(kinds(&ev).contains(&"can.id_back"), "{ev:?}");
        assert!(!kinds(&ev).contains(&"can.id_missing"));
    }

    #[test]
    fn a_rate_burst_is_reported_with_a_cooldown() {
        let mut e = Engine::new(quick());
        drive(&mut e, 0, 12, &NORMAL);
        let burst = [
            (0x100, false, 50, 8),
            (0x200, false, 100, 4),
            (0x7e8, false, 1, 8),
        ];
        let ev = drive(&mut e, 12 * S, 10, &burst);
        let high: Vec<&Event> = ev.iter().filter(|x| x.kind == "can.rate_high").collect();
        assert!(
            !high.is_empty() && high.len() <= 2,
            "cooldown 6 s over 10 s: {}",
            high.len()
        );
        assert_eq!(high[0].subject, "0x200");
        assert!(high[0].value.unwrap() > 50.0);
    }

    #[test]
    fn a_slowed_id_is_reported_low_but_not_missing() {
        let mut e = Engine::new(quick());
        drive(&mut e, 0, 12, &NORMAL);
        let slow = [
            (0x100, false, 5, 8),
            (0x200, false, 10, 4),
            (0x7e8, false, 1, 8),
        ];
        let ev = drive(&mut e, 12 * S, 8, &slow);
        assert!(
            ev.iter()
                .any(|x| x.kind == "can.rate_low" && x.subject == "0x100"),
            "{ev:?}"
        );
        assert!(!kinds(&ev).contains(&"can.id_missing"));
    }

    #[test]
    fn a_flood_is_an_alert() {
        let mut e = Engine::new(quick());
        drive(&mut e, 0, 12, &NORMAL);
        let flood = [
            (0x000, false, 100, 8),
            (0x001, false, 100, 8),
            (0x002, false, 100, 8),
            (0x100, false, 50, 8),
            (0x200, false, 10, 4),
            (0x7e8, false, 1, 8),
        ];
        let ev = drive(&mut e, 12 * S, 4, &flood);
        assert!(
            ev.iter()
                .any(|x| x.kind == "can.flood" && x.severity == "alert"),
            "{ev:?}"
        );
    }

    #[test]
    fn the_counters_and_labels_are_kept() {
        let mut e = Engine::new(quick());
        drive(&mut e, 0, 12, &NORMAL);
        e.feed(13 * S, 0x555, false, 1);
        assert_eq!((e.counts["can.new_id"], e.counts["can.baseline"]), (1, 1));
        assert_eq!(
            Key {
                id: 0x1a4,
                ext: false
            }
            .label(),
            "0x1a4"
        );
        assert_eq!(
            Key {
                id: 0x18ff0001,
                ext: true
            }
            .label(),
            "0x18ff0001"
        );
        assert!(e.frames > 600);
    }
}
