//! `spatial.evidence.v1` for HLK-LD6004 target positions (RuView ADR-384).
//!
//! Built only with cargo feature `spatial-evidence`, and silent unless
//! `--spatial-out` is set. The only accepted type is `radar_track_point`, and
//! only from protocol `0x0A04` records the caller has already decoded. Point
//! clouds, zones, and module status are not evidence. Heart rate and breathing
//! rate are not defined by this manual and are not placed on a track.
//!
//! The wire shape is local serde, with no weftos or ruview crate dependency.
//! One cluster id is a radar track. It is not a person. No pose means the
//! caller writes no line.

use serde::Serialize;

use crate::decode::Position;

pub const SCHEMA_V1: &str = "spatial.evidence.v1";
pub const PRODUCER: &str = concat!("ld6004-radar@", env!("CARGO_PKG_VERSION"));
const MAX_COORD_M: f64 = 1_000.0;
const MAX_LINE: usize = 16 * 1024;
const MAX_ID_LEN: usize = 128;
/// 1-sigma position uncertainty. An assumption, not the vendor's accuracy figure.
pub const DEFAULT_UNCERTAINTY_M: f64 = 0.3;
/// Vendor datasheet V1.1 §4.3 beam half-angles, degrees. Not a measurement.
pub const ELEV_HALF_DEG: f64 = 60.0;
pub const AZ_HALF_DEG: f64 = 60.0;

/// Where the radar is in the room and which way it faces.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// Boresight bearing, degrees counter-clockwise from room +x (east).
    pub yaw_deg: f64,
    /// Boresight pitch, degrees above horizontal. The 5-number pose form sets 0,
    /// which means the transform treats the boresight as horizontal.
    pub pitch_deg: f64,
    /// +1 when the radar's +x is to the right looking along +Z (forward), -1 when it is to the left.
    /// The bench reading did not measure which physical edge that is beyond the silkscreen direction.
    pub x_sign: f64,
}

impl Pose {
    pub fn parse(s: &str) -> Result<Pose, String> {
        let v: Vec<f64> = s
            .split(',')
            .map(|p| p.trim().parse::<f64>())
            .collect::<Result<_, _>>()
            .map_err(|_| {
                format!("--radar-pose must be x,y,z,yaw_deg,pitch_deg,x_sign, got {s:?}")
            })?;
        let (x, y, z, yaw_deg, pitch_deg, x_sign) = match v[..] {
            [x, y, z, yaw, sign] => (x, y, z, yaw, 0.0, sign),
            [x, y, z, yaw, pitch, sign] => (x, y, z, yaw, pitch, sign),
            _ => {
                return Err(format!(
                    "--radar-pose needs 5 or 6 numbers, got {}",
                    v.len()
                ))
            }
        };
        if !pitch_deg.is_finite() || !(-90.0..=90.0).contains(&pitch_deg) {
            return Err("--radar-pose: pitch_deg must be within -90..=90".into());
        }
        if ![x, y, z, yaw_deg]
            .iter()
            .all(|c| c.is_finite() && c.abs() <= MAX_COORD_M)
        {
            return Err("--radar-pose: x, y, z within +-1000 m, yaw_deg finite".into());
        }
        if !(-360.0..=360.0).contains(&yaw_deg) {
            return Err("--radar-pose: yaw_deg must be within -360..=360".into());
        }
        if x_sign != 1.0 && x_sign != -1.0 {
            return Err("--radar-pose: x_sign must be 1 or -1".into());
        }
        Ok(Pose {
            x,
            y,
            z,
            yaw_deg,
            pitch_deg,
            x_sign,
        })
    }

    /// Radar-local metres to room ENU.
    /// `x_m` is lateral. `z_m` is forward, out of the antenna. `y_m` points toward
    /// the bottom edge of the board, so height above the radar is `-y_m`.
    /// Pitch rotates the forward/up plane. At pitch 0 the boresight is horizontal:
    /// east/north come from yaw applied to Z, and room up is `-y`.
    /// `right = x_sign * x_m`. A sideways return is not removed.
    pub fn radar_to_room(&self, x_m: f64, y_m: f64, z_m: f64) -> [f64; 3] {
        let (s, c) = self.yaw_deg.to_radians().sin_cos();
        let (sp, cp) = self.pitch_deg.to_radians().sin_cos();
        let right = self.x_sign * x_m;
        let up = -y_m;
        let horiz = z_m * cp - up * sp;
        let vert = z_m * sp + up * cp;
        [
            self.x + horiz * c + right * s,
            self.y + horiz * s - right * c,
            self.z + vert,
        ]
    }

    pub fn sensor(&self) -> Sensor {
        Sensor {
            position: [mm(self.x), mm(self.y), mm(self.z)],
            yaw_deg: self.yaw_deg,
            pitch_deg: self.pitch_deg,
            elev_half_deg: ELEV_HALF_DEG,
            az_half_deg: AZ_HALF_DEG,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Sensor {
    pub position: [f64; 3],
    pub yaw_deg: f64,
    pub pitch_deg: f64,
    pub elev_half_deg: f64,
    pub az_half_deg: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Sink {
    File(String),
    Export,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub sink: Sink,
    pub pose: Option<Pose>,
    pub region: Option<String>,
    pub source_id: String,
    pub uncertainty_m: f64,
}

impl Settings {
    pub fn refusal(&self) -> Option<String> {
        if self.pose.is_none() {
            return Some(
                "no radar pose: set --radar-pose x,y,z,yaw_deg,pitch_deg,x_sign (room_enu metres, \
                 yaw counter-clockwise from +x (east), pitch up-positive, x_sign +1 when the \
                 radar's +x is to its right looking along +z)"
                    .into(),
            );
        }
        if self.region.is_none() {
            return Some("no region: set --spatial-region region/urth/meso/<room>".into());
        }
        None
    }

    pub fn refusal_status(&self) -> Option<&'static str> {
        if self.pose.is_none() {
            Some("no_pose")
        } else if self.region.is_none() {
            Some("no_region")
        } else {
            None
        }
    }
}

#[derive(Debug, Default)]
pub struct Args {
    sink: Option<Sink>,
    pose: Option<Pose>,
    region: Option<String>,
    uncertainty_m: Option<f64>,
}

impl Args {
    pub const FLAGS: [&'static str; 4] = [
        "--spatial-out",
        "--radar-pose",
        "--spatial-region",
        "--spatial-uncertainty-m",
    ];

    pub fn set(&mut self, flag: &str, v: &str) -> Result<(), String> {
        match flag {
            "--spatial-out" => {
                if v == "export" {
                    self.sink = Some(Sink::Export);
                } else if valid_path(v) {
                    self.sink = Some(Sink::File(v.to_string()));
                } else {
                    return Err(format!(
                        "--spatial-out must be `export` or a file path, got {v:?}"
                    ));
                }
            }
            "--radar-pose" => self.pose = Some(Pose::parse(v)?),
            "--spatial-region" => {
                if !valid_id(v) {
                    return Err(format!(
                        "{flag} must match [A-Za-z0-9._:/-@]{{1,128}}, got {v:?}"
                    ));
                }
                self.region = Some(v.to_string());
            }
            "--spatial-uncertainty-m" => {
                self.uncertainty_m = Some(
                    v.parse::<f64>()
                        .ok()
                        .filter(|u| *u > 0.0 && *u <= 100.0)
                        .ok_or(format!(
                            "--spatial-uncertainty-m must be in (0, 100], got {v:?}"
                        ))?,
                );
            }
            _ => return Err(format!("unknown spatial flag {flag:?}")),
        }
        Ok(())
    }

    /// `source_id` is the installed object id from `--source-id`, with no trailing slash.
    pub fn finish(self, source_id: String) -> Result<Option<Settings>, String> {
        let Some(sink) = self.sink else {
            let other =
                self.pose.is_some() || self.region.is_some() || self.uncertainty_m.is_some();
            return if other {
                Err("--radar-pose and --spatial-* need --spatial-out export|FILE".into())
            } else {
                Ok(None)
            };
        };
        if !valid_id(&source_id) {
            return Err(format!("source id is not a spatial id: {source_id:?}"));
        }
        Ok(Some(Settings {
            sink,
            pose: self.pose,
            region: self.region,
            source_id,
            uncertainty_m: self.uncertainty_m.unwrap_or(DEFAULT_UNCERTAINTY_M),
        }))
    }
}

pub fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_ID_LEN
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/-@".contains(&b))
}

pub fn valid_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 4096
        && !["/dev/", "/proc/", "/sys/"]
            .iter()
            .any(|d| p.starts_with(d))
        && !p.split('/').any(|c| c == "..")
        && !p.contains('\0')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Proof {
    Measured,
    Synthetic,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Provenance {
    pub receipt: String,
    pub producer: &'static str,
    pub proof: Proof,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RadarTrackPoint {
    pub schema: &'static str,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub t_ns: u64,
    pub frame: &'static str,
    pub region: String,
    pub source_id: String,
    pub uncertainty_m: f64,
    pub provenance: Provenance,
    /// `cluster_id` from the `0x0A04` record when it is >= 0. Ephemeral. Not a person.
    pub track: u32,
    pub position: [f64; 3],
    pub sensor: Sensor,
}

fn mm(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

#[derive(Debug)]
pub struct Emitter {
    settings: Settings,
    pose: Pose,
    region: String,
    proof: Proof,
    seq: u64,
}

impl Emitter {
    pub fn new(settings: Settings, simulated: bool) -> Result<Emitter, String> {
        if let Some(why) = settings.refusal() {
            return Err(why);
        }
        let (Some(pose), Some(region)) = (settings.pose, settings.region.clone()) else {
            return Err("spatial settings missing pose or region".into());
        };
        Ok(Emitter {
            settings,
            pose,
            region,
            proof: if simulated {
                Proof::Synthetic
            } else {
                Proof::Measured
            },
            seq: 0,
        })
    }

    pub fn file_path(&self) -> Option<&str> {
        match &self.settings.sink {
            Sink::File(p) => Some(p),
            Sink::Export => None,
        }
    }

    /// One line per `0x0A04` record whose `cluster_id` is >= 0 and whose room
    /// position is finite and inside +-1000 m. `dop_idx` is not a velocity.
    pub fn tracks(&mut self, t_ms: u64, pts: &[Position]) -> Vec<String> {
        let mut out = Vec::new();
        for p in pts {
            let Some(track) = u32::try_from(p.cluster_id).ok() else {
                continue;
            };
            if let Some(line) = self.line(t_ms, track, p.x_m, p.y_m, p.z_m) {
                out.push(line);
            }
        }
        out
    }

    fn line(&mut self, t_ms: u64, track: u32, x_m: f32, y_m: f32, z_m: f32) -> Option<String> {
        let [e, n, u] = self
            .pose
            .radar_to_room(f64::from(x_m), f64::from(y_m), f64::from(z_m));
        let position = [mm(e), mm(n), mm(u)];
        if !position
            .iter()
            .all(|c| c.is_finite() && c.abs() <= MAX_COORD_M)
        {
            return None;
        }
        self.seq += 1;
        let point = RadarTrackPoint {
            schema: SCHEMA_V1,
            kind: "radar_track_point",
            t_ns: t_ms.saturating_mul(1_000_000),
            frame: "room_enu",
            region: self.region.clone(),
            source_id: self.settings.source_id.clone(),
            uncertainty_m: self.settings.uncertainty_m,
            provenance: Provenance {
                receipt: format!("{}:{:06}", self.settings.source_id, self.seq),
                producer: PRODUCER,
                proof: self.proof,
            },
            track,
            position,
            sensor: self.pose.sensor(),
        };
        let text = serde_json::to_string(&point).ok()?;
        if text.len() > MAX_LINE || !valid_id(&point.provenance.receipt) {
            return None;
        }
        Some(text)
    }
}

pub fn append(path: &str, lines: &[String]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let mut buf = String::new();
    for l in lines {
        buf.push_str(l);
        buf.push('\n');
    }
    f.write_all(buf.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(pose: Option<Pose>) -> Settings {
        Settings {
            sink: Sink::File("/tmp/unused.jsonl".into()),
            pose,
            region: Some("region/urth/meso/test-room".into()),
            source_id: "ld6004-1".into(),
            uncertainty_m: 0.15,
        }
    }

    fn pose(x: f64, y: f64, yaw: f64, sign: f64) -> Pose {
        Pose {
            x,
            y,
            z: 0.0,
            yaw_deg: yaw,
            pitch_deg: 0.0,
            x_sign: sign,
        }
    }

    fn pos(x: f32, y: f32, z: f32, dop: i32, id: i32) -> Position {
        Position {
            x_m: x,
            y_m: y,
            z_m: z,
            dop_idx: dop,
            cluster_id: id,
        }
    }

    #[test]
    fn golden_track_matches_adr_107_shared_keys() {
        // Origin, yaw 90, x_sign +1. Local (x 2.10, y 0, z 1.40) maps to room (2.10, 1.40, 0):
        // forward is Z, and yaw 90 turns that forward into north. Track is the cluster id.
        let mut e = Emitter::new(settings(Some(pose(0.0, 0.0, 90.0, 1.0))), false).unwrap();
        let lines = e.tracks(1_759_500_001_250, &[pos(2.10, 0.0, 1.40, 7, 2)]);
        let ours: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert!(ours.get("velocity").is_none());
        assert!(ours.get("dop_idx").is_none());
        assert!(!lines[0].contains("bpm"));
        assert_eq!(ours["type"], "radar_track_point");
        assert_eq!(ours["track"], 2);
        assert_eq!(ours["position"], serde_json::json!([2.1, 1.4, 0.0]));
        assert_eq!(ours["provenance"]["proof"], "MEASURED");
        assert_eq!(ours["sensor"]["elev_half_deg"], 60.0);
        assert!(lines[0].len() < MAX_LINE);
        let adr = "{\"schema\":\"spatial.evidence.v1\",\"type\":\"radar_track_point\",\"t_ns\":1759500001250000000,\"frame\":\"room_enu\",\"region\":\"region/urth/meso/test-room\",\"source_id\":\"ld6004-1\",\"uncertainty_m\":0.15,\"provenance\":{\"receipt\":\"ld6004-1:000001\",\"producer\":\"ruview-adapter@0.1\",\"proof\":\"MEASURED\"},\"track\":2,\"position\":[2.10,1.40,0.0],\"velocity\":[0.4,0.1,0.0]}";
        let mut want: serde_json::Value = serde_json::from_str(adr).unwrap();
        want.as_object_mut().unwrap().remove("velocity");
        want.as_object_mut()
            .unwrap()
            .insert("sensor".into(), ours["sensor"].clone());
        let keys =
            |v: &serde_json::Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
        assert_eq!(keys(&ours), keys(&want));
        for k in [
            "schema",
            "type",
            "t_ns",
            "frame",
            "region",
            "source_id",
            "uncertainty_m",
            "track",
            "position",
        ] {
            assert_eq!(ours[k], want[k], "{k}");
        }
    }

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9 && (a[2] - b[2]).abs() < 1e-9
    }

    #[test]
    fn yaw_0_90_225_and_x_sign() {
        assert!(close(
            pose(0.0, 0.0, 0.0, 1.0).radar_to_room(0.0, -0.5, 2.0),
            [2.0, 0.0, 0.5]
        ));
        assert!(close(
            pose(0.0, 0.0, 90.0, 1.0).radar_to_room(0.0, 0.0, 2.0),
            [0.0, 2.0, 0.0]
        ));
        assert!(close(
            pose(1.0, 2.0, 0.0, 1.0).radar_to_room(0.5, 0.0, 2.0),
            [3.0, 1.5, 0.0]
        ));
        assert!(close(
            pose(1.0, 2.0, 0.0, -1.0).radar_to_room(0.5, 0.0, 2.0),
            [3.0, 2.5, 0.0]
        ));
        let h = std::f64::consts::FRAC_1_SQRT_2;
        assert!(close(
            pose(6.2, 3.5, 225.0, 1.0).radar_to_room(0.0, 0.0, 1.0),
            [6.2 - h, 3.5 - h, 0.0]
        ));
        let [ex, ey, _] = pose(0.0, 0.0, 30.0, 1.0).radar_to_room(1.0, 0.0, 2.0);
        let (fx, fy) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
        assert!(
            fx * ey - fy * ex < 0.0,
            "a point to the right is clockwise of the boresight"
        );
        let [e, n, _] = pose(2.0, 1.0, 37.0, -1.0).radar_to_room(-1.2, 0.0, 3.4);
        assert!(((e - 2.0).hypot(n - 1.0) - 1.2f64.hypot(3.4)).abs() < 1e-9);
        // Bench sample at yaw 0. Forward (Z) is east. +X is right, which is −north.
        // Y ≈ −0.32 is lens height. The 0.44 m lateral return is kept.
        let [e, n, u] = pose(0.0, 0.0, 0.0, 1.0).radar_to_room(0.44, -0.32, 0.92);
        assert!((e - 0.92).abs() < 1e-9);
        assert!((n - (-0.44)).abs() < 1e-9);
        assert!((u - 0.32).abs() < 1e-9);
        assert!((u - f64::from(crate::decode::CAMERA_ABOVE_RADAR_M)).abs() < 0.05);
    }

    #[test]
    fn negative_cluster_and_out_of_range_are_dropped() {
        let mut e = Emitter::new(settings(Some(pose(0.0, 0.0, 0.0, 1.0))), false).unwrap();
        let lines = e.tracks(
            1,
            &[
                pos(0.0, 0.0, 1.0, 0, -1),
                pos(0.0, 0.0, 2000.0, 0, 3),
                pos(f32::NAN, 1.0, 0.0, 0, 4),
                pos(0.0, -0.5, 2.0, 9, 5),
            ],
        );
        assert_eq!(lines.len(), 1);
        let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(v["track"], 5);
        assert_eq!(v["position"][2], 0.5);
        assert!(v.get("velocity").is_none());
    }

    #[test]
    fn refuses_without_a_pose() {
        let why = Emitter::new(settings(None), false).unwrap_err();
        assert!(
            why.contains("no radar pose") && why.contains("--radar-pose"),
            "{why}"
        );
        let mut s = settings(Some(pose(0.0, 0.0, 0.0, 1.0)));
        s.region = None;
        assert_eq!(s.refusal_status(), Some("no_region"));
    }

    #[test]
    fn simulated_is_synthetic() {
        let mut sim = Emitter::new(settings(Some(pose(0.0, 0.0, 0.0, 1.0))), true).unwrap();
        let lines = sim.tracks(1, &[pos(0.0, 2.0, 0.0, 0, 1)]);
        let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(v["provenance"]["proof"], "SYNTHETIC");
    }

    #[test]
    fn args_accept_export_and_default_uncertainty() {
        let mut a = Args::default();
        a.set("--spatial-out", "export").unwrap();
        let s = a.finish("ld6004-radar".into()).unwrap().unwrap();
        assert_eq!(s.sink, Sink::Export);
        assert_eq!(s.uncertainty_m, 0.3);
        assert_eq!(s.refusal_status(), Some("no_pose"));
        assert_eq!(s.source_id, "ld6004-radar");
        let mut a = Args::default();
        a.set("--radar-pose", "0,0,0,90,1").unwrap();
        assert!(a.finish("ld6004-radar".into()).is_err());
        let mut a = Args::default();
        assert!(a.set("--spatial-out", "/dev/null").is_err());
        assert!(a.set("--spatial-out", "../x.jsonl").is_err());
    }

    #[test]
    fn pose_parse_bounds() {
        assert_eq!(Pose::parse("0,0,1.5,90,-1").unwrap().pitch_deg, 0.0);
        assert!(Pose::parse("0,0,0,90,91,1").is_err());
        assert!(Pose::parse("0,0,0,90,0,2").is_err());
    }
}
