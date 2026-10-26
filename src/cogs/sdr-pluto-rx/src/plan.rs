//! Band plans: who wants the one tuner for how long.
//!
//! The Pluto has a single receive chain, so consumers share it by time. A consumer asks for a
//! plan, a list of bands (centre, rate, bandwidth, gain, dwell). The scheduler runs the bands of
//! all active plans round-robin, one dwell each, and counts the time every plan actually got.
//! A lease is exclusive: one holder keeps one band for N seconds and the round-robin pauses.
//! The scheduler is pure: it is told the time and answers with the band to run next.

use std::collections::BTreeMap;

pub const MIN_LO_HZ: u64 = 70_000_000;
pub const MAX_LO_HZ: u64 = 6_000_000_000;
/// The AD9363 is rated for this range; outside it the part is out of spec.
pub const RATED_LO_HZ: (u64, u64) = (325_000_000, 3_800_000_000);
pub const MIN_RATE: u32 = 520_833;
pub const MAX_RATE: u32 = 61_440_000;
pub const MIN_BW: u32 = 200_000;
pub const MAX_BW: u32 = 56_000_000;
pub const MAX_DWELL_MS: u32 = 600_000;
pub const MAX_BANDS_PER_PLAN: usize = 16;
pub const MAX_PLANS: usize = 16;
pub const MAX_LEASE_SECS: u64 = 3600;
/// Id of the fallback plan from the command line.
pub const DEFAULT_PLAN: &str = "default";

/// Gain: a manual setting in dB, or the chip's slow-attack AGC.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gain {
    Manual(f64),
    Auto,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Band {
    pub centre_hz: u64,
    pub rate_sps: u32,
    pub bw_hz: u32,
    pub gain: Gain,
    pub dwell_ms: u32,
}

impl Band {
    /// A band with the bandwidth defaulted to 80 percent of the rate (the usable part) and the
    /// gain on AGC.
    pub fn new(centre_hz: u64, rate_sps: u32, dwell_ms: u32) -> Band {
        let bw = ((f64::from(rate_sps) * 0.8) as u32).clamp(MIN_BW, MAX_BW);
        Band {
            centre_hz,
            rate_sps,
            bw_hz: bw,
            gain: Gain::Auto,
            dwell_ms,
        }
    }

    /// The reason this band cannot be run, if any. Limits are the measured range, 70 to 6000
    /// MHz; outside the rated 325 to 3800 MHz the readings are uncalibrated.
    pub fn check(&self) -> Result<(), String> {
        if !(MIN_LO_HZ..=MAX_LO_HZ).contains(&self.centre_hz) {
            return Err(format!(
                "centre {} Hz is outside {MIN_LO_HZ}..{MAX_LO_HZ} (the measured tuning range)",
                self.centre_hz
            ));
        }
        if !(MIN_RATE..=MAX_RATE).contains(&self.rate_sps) {
            return Err(format!(
                "rate {} S/s is outside {MIN_RATE}..{MAX_RATE}",
                self.rate_sps
            ));
        }
        if !(MIN_BW..=MAX_BW).contains(&self.bw_hz) {
            return Err(format!(
                "bandwidth {} Hz is outside {MIN_BW}..{MAX_BW}",
                self.bw_hz
            ));
        }
        if self.dwell_ms == 0 || self.dwell_ms > MAX_DWELL_MS {
            return Err(format!(
                "dwell {} ms is outside 1..{MAX_DWELL_MS}",
                self.dwell_ms
            ));
        }
        if let Gain::Manual(g) = self.gain {
            if !(-3.0..=71.0).contains(&g) {
                return Err(format!("gain {g} dB is outside -3..71"));
            }
        }
        Ok(())
    }

    /// True when the centre is outside the range the AD9363 is rated for.
    pub fn out_of_rated_range(&self) -> bool {
        self.centre_hz < RATED_LO_HZ.0 || self.centre_hz > RATED_LO_HZ.1
    }
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub id: String,
    pub owner: String,
    pub bands: Vec<Band>,
}

#[derive(Debug, Clone)]
struct Lease {
    id: u64,
    owner: String,
    band: Band,
    until_ns: u64,
}

/// What to run next.
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub plan: String,
    pub band_index: usize,
    pub band: Band,
    /// Wall time this slot should last, nanoseconds.
    pub dwell_ns: u64,
    pub leased: bool,
}

#[derive(Debug, Default)]
pub struct Scheduler {
    plans: Vec<Plan>,
    /// Index into the flattened band list of the slot that runs next.
    cursor: usize,
    lease: Option<Lease>,
    next_lease: u64,
    duty_ns: BTreeMap<String, u64>,
    running: Option<(String, u64)>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace the plan with this id. A plan of the same id must have the same owner.
    pub fn set_plan(&mut self, p: Plan) -> Result<(), String> {
        if p.bands.is_empty() || p.bands.len() > MAX_BANDS_PER_PLAN {
            return Err(format!("a plan has 1..{MAX_BANDS_PER_PLAN} bands"));
        }
        for (i, b) in p.bands.iter().enumerate() {
            b.check().map_err(|e| format!("band {i}: {e}"))?;
        }
        if let Some(old) = self.plans.iter_mut().find(|x| x.id == p.id) {
            if old.owner != p.owner {
                return Err(format!("plan {} belongs to {}", p.id, old.owner));
            }
            *old = p;
        } else {
            if self.plans.len() >= MAX_PLANS {
                return Err(format!("{MAX_PLANS} plans are active already"));
            }
            self.plans.push(p);
        }
        Ok(())
    }

    pub fn remove_plan(&mut self, id: &str, caller: &str) -> Result<(), String> {
        let i = self
            .plans
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| format!("no plan {id}"))?;
        if self.plans[i].owner != caller {
            return Err(format!("plan {id} belongs to {}", self.plans[i].owner));
        }
        self.plans.remove(i);
        Ok(())
    }

    pub fn plan(&self, id: &str) -> Option<&Plan> {
        self.plans.iter().find(|p| p.id == id)
    }

    pub fn plans(&self) -> &[Plan] {
        &self.plans
    }

    /// Every (plan, band) pair that takes a turn. The plan named `default` (the one the command
    /// line gives) only runs while no consumer has a plan of its own.
    fn flat(&self) -> Vec<(usize, usize)> {
        let consumers = self.plans.iter().any(|p| p.id != DEFAULT_PLAN);
        let mut v = Vec::new();
        for (pi, p) in self.plans.iter().enumerate() {
            if consumers && p.id == DEFAULT_PLAN {
                continue;
            }
            for bi in 0..p.bands.len() {
                v.push((pi, bi));
            }
        }
        v
    }

    /// Take the exclusive lease on one band for `secs`. Refused while another holder has it or
    /// when the band is invalid. The same owner may renew or change its lease.
    pub fn lease(
        &mut self,
        owner: &str,
        band: Band,
        secs: u64,
        now_ns: u64,
    ) -> Result<u64, String> {
        band.check()?;
        if secs == 0 || secs > MAX_LEASE_SECS {
            return Err(format!("a lease lasts 1..{MAX_LEASE_SECS} s"));
        }
        self.expire(now_ns);
        if let Some(l) = &self.lease {
            if l.owner != owner {
                return Err(format!(
                    "leased by {} for another {} s",
                    l.owner,
                    l.until_ns.saturating_sub(now_ns) / 1_000_000_000
                ));
            }
        }
        let id = match &self.lease {
            Some(l) => l.id,
            None => {
                self.next_lease += 1;
                self.next_lease
            }
        };
        self.lease = Some(Lease {
            id,
            owner: owner.to_string(),
            band,
            until_ns: now_ns + secs * 1_000_000_000,
        });
        Ok(id)
    }

    pub fn release(&mut self, owner: &str) -> Result<(), String> {
        match &self.lease {
            Some(l) if l.owner == owner => {
                self.lease = None;
                Ok(())
            }
            Some(l) => Err(format!("leased by {}", l.owner)),
            None => Err("no lease".into()),
        }
    }

    /// The lease holder, if one is active at `now_ns`.
    pub fn lease_holder(&mut self, now_ns: u64) -> Option<String> {
        self.expire(now_ns);
        self.lease.as_ref().map(|l| l.owner.clone())
    }

    fn expire(&mut self, now_ns: u64) {
        if self.lease.as_ref().is_some_and(|l| now_ns >= l.until_ns) {
            self.lease = None;
        }
    }

    /// The slot to run from `now_ns`. Time since the previous call is credited to the plan that
    /// was running. `None` when there is nothing to run (no plan, no lease).
    pub fn next(&mut self, now_ns: u64) -> Option<Slot> {
        self.credit(now_ns);
        self.expire(now_ns);
        if let Some(l) = &self.lease {
            let dwell = l.until_ns - now_ns;
            let key = format!("lease:{}", l.owner);
            self.running = Some((key.clone(), now_ns));
            return Some(Slot {
                plan: key,
                band_index: 0,
                band: l.band.clone(),
                dwell_ns: dwell,
                leased: true,
            });
        }
        let flat = self.flat();
        if flat.is_empty() {
            self.running = None;
            return None;
        }
        let (pi, bi) = flat[self.cursor % flat.len()];
        self.cursor = (self.cursor + 1) % flat.len();
        let p = &self.plans[pi];
        let band = p.bands[bi].clone();
        self.running = Some((p.id.clone(), now_ns));
        Some(Slot {
            plan: p.id.clone(),
            band_index: bi,
            dwell_ns: u64::from(band.dwell_ms) * 1_000_000,
            band,
            leased: false,
        })
    }

    /// Credit the running slot up to `now_ns` (also called when reporting).
    pub fn credit(&mut self, now_ns: u64) {
        if let Some((id, since)) = self.running.take() {
            *self.duty_ns.entry(id.clone()).or_default() += now_ns.saturating_sub(since);
            self.running = Some((id, now_ns));
        }
    }

    /// Share of tuner time per plan since the start, in parts per thousand of the total.
    pub fn duty(&self) -> Vec<(String, u64, u32)> {
        let total: u64 = self.duty_ns.values().sum();
        self.duty_ns
            .iter()
            .map(|(k, v)| {
                let share = (v * 1000).checked_div(total).unwrap_or(0) as u32;
                (k.clone(), *v, share)
            })
            .collect()
    }

    /// True when more than one band is competing, so a dwell ends and the tuner moves on.
    pub fn multiplexed(&self) -> bool {
        self.lease.is_none() && self.flat().len() > 1
    }
}

/// Parse `--band-plan` text: `centre_hz:rate:dwell_ms[:bw[:gain]]` comma separated. `gain` is
/// `auto` or dB.
pub fn parse_bands(spec: &str) -> Result<Vec<Band>, String> {
    let mut out = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let f: Vec<&str> = part.split(':').collect();
        if f.len() < 3 || f.len() > 5 {
            return Err(format!("{part}: want centre:rate:dwell_ms[:bw[:gain]]"));
        }
        let num = |s: &str, what: &str| -> Result<f64, String> {
            s.parse::<f64>()
                .map_err(|_| format!("{part}: bad {what} {s:?}"))
        };
        let mut b = Band::new(
            num(f[0], "centre")? as u64,
            num(f[1], "rate")? as u32,
            num(f[2], "dwell")? as u32,
        );
        if let Some(bw) = f.get(3).filter(|s| !s.is_empty()) {
            b.bw_hz = num(bw, "bandwidth")? as u32;
        }
        if let Some(g) = f.get(4) {
            b.gain = if *g == "auto" {
                Gain::Auto
            } else {
                Gain::Manual(num(g, "gain")?)
            };
        }
        b.check().map_err(|e| format!("{part}: {e}"))?;
        out.push(b);
    }
    if out.is_empty() {
        return Err("no bands".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(id: &str, owner: &str, centres: &[u64]) -> Plan {
        Plan {
            id: id.into(),
            owner: owner.into(),
            bands: centres
                .iter()
                .map(|&c| Band::new(c, 5_000_000, 100))
                .collect(),
        }
    }

    #[test]
    fn bands_are_validated_against_the_measured_range() {
        assert!(Band::new(2_437_000_000, 5_000_000, 100).check().is_ok());
        assert!(Band::new(69_000_000, 5_000_000, 100).check().is_err());
        assert!(Band::new(6_100_000_000, 5_000_000, 100).check().is_err());
        assert!(Band::new(1_090_000_000, 400_000, 100).check().is_err());
        assert!(Band::new(1_090_000_000, 62_000_000, 100).check().is_err());
        assert!(Band::new(1_090_000_000, 5_000_000, 0).check().is_err());
        let mut b = Band::new(1_090_000_000, 5_000_000, 100);
        b.gain = Gain::Manual(80.0);
        assert!(b.check().is_err());
        assert!(Band::new(150_000_000, 5_000_000, 100).out_of_rated_range());
        assert!(!Band::new(433_920_000, 5_000_000, 100).out_of_rated_range());
    }

    #[test]
    fn round_robin_cycles_through_every_band_of_every_plan() {
        let mut s = Scheduler::new();
        s.set_plan(plan("a", "x", &[2_437_000_000, 1_090_000_000]))
            .unwrap();
        s.set_plan(plan("b", "y", &[915_000_000])).unwrap();
        let seq: Vec<(String, u64)> = (0..6)
            .map(|i| {
                let sl = s.next(i * 100_000_000).unwrap();
                (sl.plan, sl.band.centre_hz)
            })
            .collect();
        let want: Vec<(&str, u64)> = vec![
            ("a", 2_437_000_000),
            ("a", 1_090_000_000),
            ("b", 915_000_000),
            ("a", 2_437_000_000),
            ("a", 1_090_000_000),
            ("b", 915_000_000),
        ];
        assert_eq!(
            seq,
            want.iter()
                .map(|(a, b)| (a.to_string(), *b))
                .collect::<Vec<_>>()
        );
        assert!(s.multiplexed());
    }

    #[test]
    fn duty_counts_the_time_each_plan_ran() {
        let mut s = Scheduler::new();
        s.set_plan(plan("a", "x", &[2_437_000_000])).unwrap();
        s.set_plan(plan("b", "y", &[915_000_000])).unwrap();
        let mut t = 0;
        for _ in 0..4 {
            s.next(t).unwrap();
            t += 100_000_000;
            s.next(t).unwrap();
            t += 400_000_000;
        }
        s.credit(t);
        let duty: BTreeMap<String, u32> = s.duty().into_iter().map(|(k, _, p)| (k, p)).collect();
        assert_eq!(duty["a"], 200);
        assert_eq!(duty["b"], 800);
    }

    #[test]
    fn a_plan_belongs_to_its_owner() {
        let mut s = Scheduler::new();
        s.set_plan(plan("a", "x", &[2_437_000_000])).unwrap();
        assert!(s.set_plan(plan("a", "y", &[915_000_000])).is_err());
        assert!(s.remove_plan("a", "y").is_err());
        s.set_plan(plan("a", "x", &[433_920_000])).unwrap();
        assert_eq!(s.plan("a").unwrap().bands[0].centre_hz, 433_920_000);
        s.remove_plan("a", "x").unwrap();
        assert!(s.next(0).is_none());
        for i in 0..MAX_PLANS {
            s.set_plan(plan(&format!("p{i}"), "x", &[433_920_000]))
                .unwrap();
        }
        assert!(s
            .set_plan(plan("one-too-many", "x", &[433_920_000]))
            .is_err());
        assert!(s
            .set_plan(Plan {
                id: "e".into(),
                owner: "x".into(),
                bands: vec![]
            })
            .is_err());
    }

    #[test]
    fn a_lease_pauses_the_round_robin_and_expires() {
        let mut s = Scheduler::new();
        s.set_plan(plan("a", "x", &[2_437_000_000, 1_090_000_000]))
            .unwrap();
        let band = Band::new(868_000_000, 2_000_000, 100);
        let id = s.lease("holder", band.clone(), 10, 1_000_000_000).unwrap();
        assert_eq!(id, 1);
        let sl = s.next(2_000_000_000).unwrap();
        assert!(sl.leased && sl.band.centre_hz == 868_000_000);
        assert_eq!(sl.dwell_ns, 9_000_000_000);
        assert!(!s.multiplexed());
        let err = s
            .lease("other", band.clone(), 5, 3_000_000_000)
            .unwrap_err();
        assert!(err.contains("leased by holder"), "{err}");
        assert!(s.release("other").is_err());
        // the holder may change its band
        assert_eq!(
            s.lease(
                "holder",
                Band::new(433_920_000, 2_000_000, 100),
                10,
                4_000_000_000
            )
            .unwrap(),
            1
        );
        // expiry returns to the plans
        let sl = s.next(20_000_000_000).unwrap();
        assert!(!sl.leased && sl.plan == "a");
        assert!(s.lease("other", band, 5, 20_000_000_000).is_ok());
        assert!(s.release("other").is_ok());
        assert!(s
            .lease("x", Band::new(868_000_000, 2_000_000, 100), 0, 0)
            .is_err());
        assert!(s
            .lease(
                "x",
                Band::new(868_000_000, 2_000_000, 100),
                MAX_LEASE_SECS + 1,
                0
            )
            .is_err());
    }

    #[test]
    fn the_default_plan_yields_to_consumer_plans() {
        let mut s = Scheduler::new();
        s.set_plan(plan(DEFAULT_PLAN, "@cog", &[2_437_000_000, 1_090_000_000]))
            .unwrap();
        assert_eq!(s.next(0).unwrap().plan, DEFAULT_PLAN);
        assert!(s.multiplexed());
        s.set_plan(plan("tune:c1", "c1", &[915_000_000])).unwrap();
        for i in 1..4 {
            assert_eq!(s.next(i).unwrap().plan, "tune:c1");
        }
        assert!(!s.multiplexed());
        s.remove_plan("tune:c1", "c1").unwrap();
        assert_eq!(s.next(9).unwrap().plan, DEFAULT_PLAN);
    }

    #[test]
    fn band_plan_text_parses_and_is_checked() {
        let b = parse_bands("2437e6:5e6:2000, 1090e6:2.4e6:1000:2e6:40, 433.92e6:2e6:500:1e6:auto")
            .unwrap();
        assert_eq!(b.len(), 3);
        assert_eq!(
            (b[0].centre_hz, b[0].rate_sps, b[0].dwell_ms),
            (2_437_000_000, 5_000_000, 2000)
        );
        assert_eq!(b[0].bw_hz, 4_000_000);
        assert_eq!((b[1].bw_hz, b[1].gain), (2_000_000, Gain::Manual(40.0)));
        assert_eq!(b[2].gain, Gain::Auto);
        assert!(parse_bands("2437e6:5e6").is_err());
        assert!(parse_bands("9000e6:5e6:100").is_err());
        assert!(parse_bands("").is_err());
        assert!(parse_bands("x:5e6:100").is_err());
    }
}
