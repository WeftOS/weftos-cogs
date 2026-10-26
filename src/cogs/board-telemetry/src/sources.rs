//! Reads the board's own monitors from a sysfs/procfs tree. Everything is found by name under
//! `<root>/sys/class/hwmon`, `<root>/sys/class/thermal` and `<root>/proc`; no hwmon or zone number
//! is ever assumed, so the same code runs on a Ventuno Q, a Pi and a test fixture.

use std::fs;
use std::path::{Path, PathBuf};

/// Plausible zone range. A reading outside it (a disabled sensor can report -274 or 0x7fffffff)
/// is counted as unreadable instead of being passed on as a temperature.
const TEMP_MIN_C: f64 = -50.0;
const TEMP_MAX_C: f64 = 150.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Cpu,
    Gpu,
    Npu,
    Camera,
    Ddr,
    Other,
}

impl Group {
    pub const ALL: [Group; 6] = [
        Group::Cpu,
        Group::Gpu,
        Group::Npu,
        Group::Camera,
        Group::Ddr,
        Group::Other,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Group::Cpu => "cpu",
            Group::Gpu => "gpu",
            Group::Npu => "npu",
            Group::Camera => "camera",
            Group::Ddr => "ddr",
            Group::Other => "other",
        }
    }

    pub fn index(self) -> usize {
        Group::ALL.iter().position(|g| *g == self).unwrap_or(5)
    }

    /// Group by thermal zone `type`: `cpu*`, `gpu*`, `nsp*` (the Qualcomm NPU), `camss*`, `ddr*`.
    pub fn from_type(kind: &str) -> Group {
        let k = kind.to_ascii_lowercase();
        if k.starts_with("cpu") {
            Group::Cpu
        } else if k.starts_with("gpu") {
            Group::Gpu
        } else if k.starts_with("nsp") {
            Group::Npu
        } else if k.starts_with("camss") {
            Group::Camera
        } else if k.starts_with("ddr") {
            Group::Ddr
        } else {
            Group::Other
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PowerMon {
    pub hwmon: String,
    pub name: String,
    pub power_w: Option<f64>,
    pub voltage_v: Option<f64>,
    pub current_a: Option<f64>,
    /// `power_w` was computed as V x I because the driver exposes no `power1_input`.
    pub power_derived: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fan {
    pub hwmon: String,
    pub name: String,
    pub channel: u32,
    pub rpm: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    pub zone: String,
    pub kind: String,
    pub group: Group,
    pub temp_c: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub power: Vec<PowerMon>,
    pub fans: Vec<Fan>,
    pub zones: Vec<Zone>,
    pub zones_unreadable: usize,
    pub load1: Option<f64>,
    pub mem_available_mb: Option<f64>,
}

impl Snapshot {
    /// The monitor whose numbers feed the headline: the first INA2xx (lowest hwmon number) that
    /// returned anything. Every monitor is still listed in the report.
    pub fn primary_power(&self) -> Option<&PowerMon> {
        self.power.first()
    }

    pub fn fan_max_rpm(&self) -> Option<f64> {
        self.fans.iter().map(|f| f.rpm).reduce(f64::max)
    }

    /// True when any hardware monitor (not just /proc) produced a reading.
    pub fn has_hardware_source(&self) -> bool {
        !self.power.is_empty() || !self.fans.is_empty() || !self.zones.is_empty()
    }
}

fn read_trim(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn read_num(path: &Path) -> Option<f64> {
    read_trim(path)?
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

/// `<dir>/<prefix><N>` entries sorted by N, so hwmon10 follows hwmon9.
fn list_numbered(dir: &Path, prefix: &str) -> Vec<(String, PathBuf)> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(u32, String, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n = name.strip_prefix(prefix)?.parse::<u32>().ok()?;
            Some((n, name, e.path()))
        })
        .collect();
    out.sort_by_key(|(n, _, _)| *n);
    out.into_iter().map(|(_, name, p)| (name, p)).collect()
}

/// INA219 / INA226 / INA230 / INA232 / INA238 and the generic `ina2xx` driver name.
pub fn is_ina2xx(name: &str) -> bool {
    name.to_ascii_lowercase().starts_with("ina2")
}

fn fan_channels(dir: &Path) -> Vec<u32> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut ch: Vec<u32> = rd
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.strip_prefix("fan")?
                .strip_suffix("_input")?
                .parse::<u32>()
                .ok()
        })
        .collect();
    ch.sort_unstable();
    ch
}

fn read_power(dir: &Path, hwmon: &str, name: &str) -> Option<PowerMon> {
    let power_w = read_num(&dir.join("power1_input")).map(|uw| uw / 1_000_000.0);
    let voltage_v = read_num(&dir.join("in1_input")).map(|mv| mv / 1000.0);
    let current_a = read_num(&dir.join("curr1_input")).map(|ma| ma / 1000.0);
    if power_w.is_none() && voltage_v.is_none() && current_a.is_none() {
        return None;
    }
    let derived = match (power_w, voltage_v, current_a) {
        (None, Some(v), Some(a)) => Some(v * a),
        _ => None,
    };
    Some(PowerMon {
        hwmon: hwmon.to_string(),
        name: name.to_string(),
        power_w: power_w.or(derived),
        voltage_v,
        current_a,
        power_derived: derived.is_some(),
    })
}

fn read_zones(root: &Path) -> (Vec<Zone>, usize) {
    let mut zones = Vec::new();
    let mut unreadable = 0;
    for (zone, dir) in list_numbered(&root.join("sys/class/thermal"), "thermal_zone") {
        let kind = read_trim(&dir.join("type")).unwrap_or_default();
        match read_num(&dir.join("temp")).map(|m| m / 1000.0) {
            Some(t) if (TEMP_MIN_C..=TEMP_MAX_C).contains(&t) => zones.push(Zone {
                group: Group::from_type(&kind),
                zone,
                kind,
                temp_c: t,
            }),
            _ => unreadable += 1,
        }
    }
    (zones, unreadable)
}

pub fn parse_loadavg(text: &str) -> Option<f64> {
    text.split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
}

/// `MemAvailable:  5734400 kB` -> megabytes.
pub fn parse_mem_available_mb(text: &str) -> Option<f64> {
    let line = text.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kb = line.split_whitespace().nth(1)?.parse::<f64>().ok()?;
    Some(kb / 1024.0)
}

pub fn read(root: &Path) -> Snapshot {
    let mut snap = Snapshot::default();
    for (hwmon, dir) in list_numbered(&root.join("sys/class/hwmon"), "hwmon") {
        let name = read_trim(&dir.join("name")).unwrap_or_default();
        if is_ina2xx(&name) {
            snap.power.extend(read_power(&dir, &hwmon, &name));
        }
        for channel in fan_channels(&dir) {
            if let Some(rpm) = read_num(&dir.join(format!("fan{channel}_input"))) {
                snap.fans.push(Fan {
                    hwmon: hwmon.clone(),
                    name: name.clone(),
                    channel,
                    rpm,
                });
            }
        }
    }
    (snap.zones, snap.zones_unreadable) = read_zones(root);
    snap.load1 = read_trim(&root.join("proc/loadavg")).and_then(|t| parse_loadavg(&t));
    snap.mem_available_mb =
        read_trim(&root.join("proc/meminfo")).and_then(|t| parse_mem_available_mb(&t));
    snap
}

/// Deterministic Ventuno-shaped values that drift a little with `tick`. Marked SYNTHETIC upstream.
pub fn simulate(tick: u64) -> Snapshot {
    let wobble = ((tick % 8) as f64 - 4.0) / 4.0; // -1.0 ..= 0.75
    let zone = |n: u32, kind: &str, t: f64| Zone {
        zone: format!("thermal_zone{n}"),
        kind: kind.to_string(),
        group: Group::from_type(kind),
        temp_c: t,
    };
    let voltage_v = 12.0;
    let power_w = 11.5 + wobble;
    Snapshot {
        power: vec![PowerMon {
            hwmon: "hwmon3".into(),
            name: "ina232".into(),
            power_w: Some(power_w),
            voltage_v: Some(voltage_v),
            current_a: Some(power_w / voltage_v),
            power_derived: false,
        }],
        fans: vec![Fan {
            hwmon: "hwmon4".into(),
            name: "emc2305".into(),
            channel: 1,
            rpm: 3200.0 + 100.0 * wobble,
        }],
        zones: vec![
            zone(0, "cpu_0_0_0_thermal", 52.0 + 2.0 * wobble),
            zone(1, "nsp_0_0_0_thermal", 47.0 + 3.0 * wobble),
            zone(2, "gpuss_0_thermal", 45.0),
            zone(3, "camss_0_thermal", 41.0),
            zone(4, "ddr_thermal", 44.0),
        ],
        zones_unreadable: 0,
        load1: Some(0.85 + 0.1 * wobble),
        mem_available_mb: Some(5600.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn ina_names_match_the_family_and_nothing_else() {
        for n in ["ina219", "ina226", "ina232", "ina2xx", "INA238"] {
            assert!(is_ina2xx(n), "{n}");
        }
        for n in ["emc2305", "cpu_thermal", "ina3221", "", "nina2"] {
            assert!(!is_ina2xx(n), "{n}");
        }
    }

    #[test]
    fn zone_types_group_by_prefix() {
        let cases = [
            ("cpu_0_0_0_thermal", Group::Cpu),
            ("cpu-thermal", Group::Cpu),
            ("gpuss_0_thermal", Group::Gpu),
            ("nsp_0_0_0_thermal", Group::Npu),
            ("camss_0_thermal", Group::Camera),
            ("ddr_thermal", Group::Ddr),
            ("pm8550_tz", Group::Other),
            ("acpitz", Group::Other),
        ];
        for (kind, want) in cases {
            assert_eq!(Group::from_type(kind), want, "{kind}");
        }
    }

    #[test]
    fn loadavg_and_meminfo_parse() {
        assert_eq!(parse_loadavg("0.85 0.60 0.40 1/512 12345\n"), Some(0.85));
        assert_eq!(parse_loadavg(""), None);
        assert_eq!(parse_loadavg("nan 1 1"), None);
        assert_eq!(parse_loadavg("-1 0 0"), None);
        let mem = "MemTotal: 100 kB\nMemAvailable:    2048 kB\n";
        assert_eq!(parse_mem_available_mb(mem), Some(2.0));
        assert_eq!(parse_mem_available_mb("MemTotal: 100 kB\n"), None);
    }

    #[test]
    fn ventuno_fixture_is_discovered_by_name() {
        let s = read(&fixture("ventuno"));
        // ina232 sits at hwmon3 and the fans at hwmon10; neither number is known to the code.
        assert_eq!(s.power.len(), 1);
        let p = s.primary_power().unwrap();
        assert_eq!((p.hwmon.as_str(), p.name.as_str()), ("hwmon3", "ina232"));
        assert_eq!(p.power_w, Some(12.345));
        assert_eq!(p.voltage_v, Some(12.01));
        assert_eq!(p.current_a, Some(1.028));
        assert!(!p.power_derived);
        assert_eq!(s.fans.len(), 2);
        assert_eq!(s.fan_max_rpm(), Some(3200.0));
        assert_eq!(s.zones.len(), 7);
        assert_eq!(s.zones_unreadable, 1, "zone7 has no temp file");
        assert_eq!(s.load1, Some(0.85));
        assert_eq!(s.mem_available_mb, Some(5600.0));
    }

    #[test]
    fn pi_fixture_has_only_a_cpu_zone() {
        let s = read(&fixture("pi"));
        assert!(s.power.is_empty() && s.fans.is_empty());
        assert_eq!(s.zones.len(), 1);
        assert_eq!(s.zones[0].group, Group::Cpu);
        assert_eq!(s.zones[0].temp_c, 48.312);
        assert!(s.has_hardware_source());
    }

    #[test]
    fn empty_fixture_has_nothing() {
        let s = read(&fixture("empty"));
        assert_eq!(s, Snapshot::default());
        assert!(!s.has_hardware_source());
        // A root that does not exist at all behaves the same.
        assert_eq!(read(&fixture("does-not-exist")), Snapshot::default());
    }

    #[test]
    fn power_is_derived_only_when_the_driver_has_none() {
        let dir = std::env::temp_dir().join(format!("bt-derive-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("in1_input"), "5000\n").unwrap();
        fs::write(dir.join("curr1_input"), "400\n").unwrap();
        let p = read_power(&dir, "hwmon1", "ina219").unwrap();
        assert_eq!(p.power_w, Some(2.0));
        assert!(p.power_derived);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn implausible_zone_temperatures_are_unreadable() {
        let dir = std::env::temp_dir().join(format!("bt-zone-{}", std::process::id()));
        let z = dir.join("sys/class/thermal/thermal_zone0");
        fs::create_dir_all(&z).unwrap();
        fs::write(z.join("type"), "cpu0\n").unwrap();
        fs::write(z.join("temp"), "-274000\n").unwrap();
        let (zones, bad) = read_zones(&dir);
        assert!(zones.is_empty());
        assert_eq!(bad, 1);
        fs::remove_dir_all(&dir).ok();
    }
}
