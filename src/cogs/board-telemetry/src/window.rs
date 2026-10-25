//! Folds the snapshots taken during one report window into the numbers that are reported and
//! the 8-float ingest vector.

use crate::sources::{Group, Snapshot};

/// Slot value for "this source is not present". Every real reading here is zero or more, so the
/// sentinel cannot be mistaken for one. JSON has no NaN, so it is not used on the wire.
pub const MISSING: f64 = -1.0;

/// Ingest vector slots, in order.
pub const SLOTS: [&str; 8] = [
    "power_w",
    "voltage_v",
    "current_a",
    "temp_max_c",
    "temp_npu_c",
    "temp_cpu_c",
    "fan_rpm",
    "load1",
];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Window {
    pub samples: usize,
    pub power_w: Option<f64>,
    pub voltage_v: Option<f64>,
    pub current_a: Option<f64>,
    pub fan_rpm: Option<f64>,
    pub load1: Option<f64>,
    pub mem_available_mb: Option<f64>,
    pub temp_max_c: Option<f64>,
    /// Hottest reading per group over the window, indexed by `Group::index`.
    pub group_max_c: [Option<f64>; 6],
}

fn mean(vals: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    let (sum, n) = vals
        .flatten()
        .fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    (n > 0).then(|| sum / n as f64)
}

fn max_opt(a: Option<f64>, b: f64) -> Option<f64> {
    Some(a.map_or(b, |x| x.max(b)))
}

impl Window {
    pub fn group_max(&self, g: Group) -> Option<f64> {
        self.group_max_c[g.index()]
    }

    pub fn from_snapshots(snaps: &[Snapshot]) -> Window {
        let mut w = Window {
            samples: snaps.len(),
            power_w: mean(
                snaps
                    .iter()
                    .map(|s| s.primary_power().and_then(|p| p.power_w)),
            ),
            voltage_v: mean(
                snaps
                    .iter()
                    .map(|s| s.primary_power().and_then(|p| p.voltage_v)),
            ),
            current_a: mean(
                snaps
                    .iter()
                    .map(|s| s.primary_power().and_then(|p| p.current_a)),
            ),
            fan_rpm: mean(snaps.iter().map(Snapshot::fan_max_rpm)),
            load1: mean(snaps.iter().map(|s| s.load1)),
            mem_available_mb: snaps.iter().rev().find_map(|s| s.mem_available_mb),
            ..Window::default()
        };
        for z in snaps.iter().flat_map(|s| &s.zones) {
            w.temp_max_c = max_opt(w.temp_max_c, z.temp_c);
            let i = z.group.index();
            w.group_max_c[i] = max_opt(w.group_max_c[i], z.temp_c);
        }
        w
    }

    /// The ingest vector. Absent sources are `MISSING`; values are rounded to 3 places.
    pub fn vector(&self) -> [f64; 8] {
        let r = |v: Option<f64>| v.map_or(MISSING, |x| (x * 1000.0).round() / 1000.0);
        [
            r(self.power_w),
            r(self.voltage_v),
            r(self.current_a),
            r(self.temp_max_c),
            r(self.group_max(Group::Npu)),
            r(self.group_max(Group::Cpu)),
            r(self.fan_rpm),
            r(self.load1),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::{self, Fan, PowerMon, Zone};

    fn snap(power: f64, temp: f64, rpm: f64) -> Snapshot {
        Snapshot {
            power: vec![PowerMon {
                hwmon: "hwmon1".into(),
                name: "ina226".into(),
                power_w: Some(power),
                voltage_v: Some(12.0),
                current_a: None,
                power_derived: false,
            }],
            fans: vec![Fan {
                hwmon: "hwmon2".into(),
                name: "emc2305".into(),
                channel: 1,
                rpm,
            }],
            zones: vec![Zone {
                zone: "thermal_zone0".into(),
                kind: "nsp0".into(),
                group: Group::Npu,
                temp_c: temp,
            }],
            zones_unreadable: 0,
            load1: Some(1.0),
            mem_available_mb: Some(100.0),
        }
    }

    #[test]
    fn means_for_rates_and_maxima_for_temperatures() {
        let w = Window::from_snapshots(&[snap(10.0, 40.0, 3000.0), snap(14.0, 55.0, 3200.0)]);
        assert_eq!(w.samples, 2);
        assert_eq!(w.power_w, Some(12.0));
        assert_eq!(w.fan_rpm, Some(3100.0));
        assert_eq!(
            w.temp_max_c,
            Some(55.0),
            "max over the window, not the mean"
        );
        assert_eq!(w.group_max(Group::Npu), Some(55.0));
        assert_eq!(w.group_max(Group::Cpu), None);
        assert_eq!(w.current_a, None, "no current reading in any sample");
    }

    #[test]
    fn vector_layout_and_missing_sentinel() {
        let w = Window::from_snapshots(&[snap(12.5, 47.25, 3200.0)]);
        assert_eq!(
            w.vector(),
            [12.5, 12.0, MISSING, 47.25, 47.25, MISSING, 3200.0, 1.0]
        );
        assert_eq!(SLOTS.len(), w.vector().len());
    }

    #[test]
    fn empty_window_is_all_missing() {
        let w = Window::from_snapshots(&[]);
        assert_eq!(w.samples, 0);
        assert_eq!(w.vector(), [MISSING; 8]);
    }

    #[test]
    fn simulated_snapshot_fills_every_slot() {
        let w = Window::from_snapshots(&[sources::simulate(0)]);
        assert!(w.vector().iter().all(|v| *v >= 0.0));
    }
}
