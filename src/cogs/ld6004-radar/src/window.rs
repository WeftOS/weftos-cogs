//! One output window of HLK-LD6004 frames.
//!
//! Meanings come from decode.rs (the LD6004 presence protocol). A checksum-valid
//! frame whose type this module does not know is counted and left undecoded.
//! A zero-length payload of a known type is an ack: it is counted and does not
//! clear the last value. Heart rate and breathing rate are not fields.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::decode::{self, Firmware, Position, Region};
use crate::frame::{Frame, FramerStats};
use crate::senml::{self, Status};

pub const SCHEMA: &str = "weavelogic.cog-output.v0";
pub const SOURCE_KIND: &str = "ld6004-uart";
/// Metres in the radar's own axes. Room placement is the spatial adapter's job.
pub const TARGET_FRAME: &str = "radar_local";
/// Cap per list. The vendor's "up to 3 people" is a spec, not a frame limit.
pub const MAX_TARGETS: usize = 64;

/// Firmware identity seen so far, and whether section 6 may be applied.
#[derive(Debug, Clone, Default)]
pub struct LinkState {
    /// Set when a `0xFFFF` status names a project other than 0 or 6.
    /// Cleared only by a later status that [`Firmware::is_presence`] accepts.
    pub foreign: bool,
    pub firmware: Option<Firmware>,
}

#[derive(Debug, Default)]
pub struct Window {
    targets: Vec<Position>,
    points: Vec<Position>,
    targets_dropped: u64,
    points_dropped: u64,
    target_parse_errors: u64,
    point_parse_errors: u64,
    other_parse_errors: u64,
    zones: Option<[Option<bool>; 4]>,
    noise: Option<Vec<Region>>,
    detect: Option<Vec<Region>>,
    dwell: Option<Vec<Region>>,
    status: Status,
    type_counts: BTreeMap<u16, u64>,
    acks: u64,
    presence_refused: u64,
}

/// Inputs to a report that are not themselves decoded frames.
pub struct WindowContext {
    pub start_ms: u64,
    pub end_ms: u64,
    pub device: String,
    pub stats: FramerStats,
    pub bytes_read: u64,
    pub device_error: Option<String>,
    pub simulated: bool,
    pub link: LinkState,
    pub writes: &'static str,
    pub source_id: String,
    pub command_error: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct NamedCode {
    code: u8,
    name: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub(crate) struct PointOut {
    x_m: f64,
    y_m: f64,
    z_m: f64,
    /// `-y_m`. +Y points toward the bottom edge of the board.
    above_radar_m: f64,
    dop_idx: i32,
    cluster_id: i32,
}

#[derive(Debug, Serialize)]
pub struct Axes {
    pub x: &'static str,
    pub y: &'static str,
    pub z: &'static str,
    pub above_radar: &'static str,
    /// Bench pairing: the camera lens was about a foot above the module.
    /// Not a lateral offset. Do not subtract a sideways return from X.
    pub camera_above_radar_m: f64,
}

#[derive(Debug, Serialize)]
pub(crate) struct RegionOut {
    x_min_m: f64,
    x_max_m: f64,
    y_min_m: f64,
    y_max_m: f64,
    z_min_m: f64,
    z_max_m: f64,
}

#[derive(Debug, Serialize)]
pub(crate) struct FirmwareOut {
    project_code: u8,
    project_name: Option<&'static str>,
    major: u8,
    sub: u8,
    modified: u8,
    presence_project: bool,
}

#[derive(Debug, Serialize)]
pub struct Source {
    pub kind: &'static str,
    pub device: String,
    /// True only when checksum-valid frames arrived and they were not simulated.
    pub verified: bool,
    pub simulated: bool,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub schema: &'static str,
    pub cog: &'static str,
    pub version: &'static str,
    pub timestamp: u64,
    pub timestamp_ms: u64,
    pub window_start_ms: u64,
    pub sensor_class: &'static str,
    pub source: Source,
    pub health: &'static str,
    pub quality: Option<f64>,
    pub reasons: Vec<String>,
    pub frame: &'static str,
    pub axes: Axes,
    pub targets: Vec<PointOut>,
    pub targets_dropped: u64,
    pub target_parse_errors: u64,
    pub points: Vec<PointOut>,
    pub points_dropped: u64,
    pub point_parse_errors: u64,
    pub zones: Option<[Option<bool>; 4]>,
    pub noise_areas: Option<Vec<RegionOut>>,
    pub detect_areas: Option<Vec<RegionOut>>,
    pub dwell_areas: Option<Vec<RegionOut>>,
    pub work_mode: Option<NamedCode>,
    pub low_power_state: Option<NamedCode>,
    pub sensitivity: Option<NamedCode>,
    pub trigger_speed: Option<NamedCode>,
    pub install: Option<NamedCode>,
    pub gpio_mode: Option<NamedCode>,
    pub hold_delay_s: Option<u32>,
    pub sleep_ms: Option<u32>,
    pub stay_life: Option<u32>,
    pub output_interval: Option<u32>,
    pub z_min_m: Option<f64>,
    pub z_max_m: Option<f64>,
    pub firmware: Option<FirmwareOut>,
    pub foreign_firmware: bool,
    pub writes: &'static str,
    /// This protocol does not define heart rate or breathing rate.
    pub vitals: &'static str,
    pub acks: u64,
    pub presence_refused: u64,
    pub frames: u64,
    pub checksum_errors: u64,
    pub oversize_frames: u64,
    pub resync_bytes: u64,
    pub bytes_read: u64,
    pub frame_counts: BTreeMap<String, u64>,
    pub undecoded_frame_counts: BTreeMap<String, u64>,
    /// `null` (off), `emitting`, `no_pose`, `no_region`, or `write_error`.
    #[cfg(feature = "spatial-evidence")]
    pub spatial: Option<&'static str>,
}

fn r3(v: f32) -> f64 {
    (f64::from(v) * 1000.0).round() / 1000.0
}

fn point_out(p: &Position) -> PointOut {
    PointOut {
        x_m: r3(p.x_m),
        y_m: r3(p.y_m),
        z_m: r3(p.z_m),
        above_radar_m: r3(decode::above_radar_m(p.y_m)),
        dop_idx: p.dop_idx,
        cluster_id: p.cluster_id,
    }
}

fn region_out(r: &Region) -> RegionOut {
    RegionOut {
        x_min_m: r3(r.x_min_m),
        x_max_m: r3(r.x_max_m),
        y_min_m: r3(r.y_min_m),
        y_max_m: r3(r.y_max_m),
        z_min_m: r3(r.z_min_m),
        z_max_m: r3(r.z_max_m),
    }
}

fn named(code: u8, name: Option<&'static str>) -> NamedCode {
    NamedCode { code, name }
}

fn push_capped(dst: &mut Vec<Position>, src: Vec<Position>, dropped: &mut u64) -> Vec<Position> {
    let room = MAX_TARGETS.saturating_sub(dst.len());
    *dropped += src.len().saturating_sub(room) as u64;
    let kept: Vec<Position> = src.into_iter().take(room).collect();
    dst.extend(kept.iter().cloned());
    kept
}

impl Window {
    fn clear_presence(&mut self) {
        self.targets.clear();
        self.points.clear();
        self.zones = None;
        self.noise = None;
        self.detect = None;
        self.dwell = None;
        self.status = Status::default();
    }

    /// Decode one frame. The returned positions are the `0x0A04` records kept
    /// in this window, and only when section 6 still applies. Callers use that
    /// list for `radar_track_point` and for nothing else.
    pub fn ingest(&mut self, f: &Frame, link: &mut LinkState) -> Vec<Position> {
        *self.type_counts.entry(f.ty).or_default() += 1;
        if f.payload.is_empty() && decode::DECODED.contains(&f.ty) {
            self.acks += 1;
            return Vec::new();
        }
        if link.foreign && f.ty != decode::T_FIRMWARE {
            self.presence_refused += 1;
            return Vec::new();
        }
        match f.ty {
            decode::T_FIRMWARE => match decode::firmware(&f.payload) {
                Some(fw) => {
                    let presence = fw.is_presence();
                    link.firmware = Some(fw);
                    link.foreign = !presence;
                    if !presence {
                        self.clear_presence();
                    }
                }
                None => self.other_parse_errors += 1,
            },
            decode::T_TARGET => match decode::positions(&f.payload) {
                Some(pts) => {
                    return push_capped(&mut self.targets, pts, &mut self.targets_dropped);
                }
                None => self.target_parse_errors += 1,
            },
            decode::T_CLOUD => match decode::positions(&f.payload) {
                Some(pts) => {
                    push_capped(&mut self.points, pts, &mut self.points_dropped);
                }
                None => self.point_parse_errors += 1,
            },
            decode::T_ZONES => match decode::zones(&f.payload) {
                Some(z) => self.zones = Some(z),
                None => self.other_parse_errors += 1,
            },
            decode::T_NOISE_AREA => self.take_regions(&f.payload, Area::Noise),
            decode::T_DETECT_AREA => self.take_regions(&f.payload, Area::Detect),
            decode::T_DWELL_AREA => self.take_regions(&f.payload, Area::Dwell),
            decode::T_HOLD => match decode::u32_le(&f.payload) {
                Some(v) => self.status.hold_delay_s = Some(v),
                None => self.other_parse_errors += 1,
            },
            decode::T_SENS => self.take_u8(&f.payload, |s, c| s.sensitivity = Some(c)),
            decode::T_TRIGGER => self.take_u8(&f.payload, |s, c| s.trigger_speed = Some(c)),
            decode::T_Z_RANGE => match decode::z_range(&f.payload) {
                Some((lo, hi)) => {
                    self.status.z_min_m = Some(lo);
                    self.status.z_max_m = Some(hi);
                }
                None => self.other_parse_errors += 1,
            },
            decode::T_INSTALL => self.take_u8(&f.payload, |s, c| s.install = Some(c)),
            decode::T_WORK => self.take_u8(&f.payload, |s, c| s.work_mode = Some(c)),
            decode::T_SLEEP => match decode::u32_le(&f.payload) {
                Some(v) => self.status.sleep_ms = Some(v),
                None => self.other_parse_errors += 1,
            },
            decode::T_LOW_POWER => self.take_u8(&f.payload, |s, c| s.low_power_state = Some(c)),
            decode::T_GPIO => self.take_u8(&f.payload, |s, c| s.gpio_mode = Some(c)),
            decode::T_STAY => match decode::u32_le(&f.payload) {
                Some(v) => self.status.stay_life = Some(v),
                None => self.other_parse_errors += 1,
            },
            decode::T_INTERVAL => match decode::u32_le(&f.payload) {
                Some(v) => self.status.output_interval = Some(v),
                None => self.other_parse_errors += 1,
            },
            _ => {}
        }
        Vec::new()
    }

    fn take_u8(&mut self, payload: &[u8], set: fn(&mut Status, u8)) {
        match decode::u8_code(payload) {
            Some(c) => set(&mut self.status, c),
            None => self.other_parse_errors += 1,
        }
    }

    fn take_regions(&mut self, payload: &[u8], which: Area) {
        match decode::regions(payload) {
            Some(regs) => match which {
                Area::Noise => self.noise = Some(regs),
                Area::Detect => self.detect = Some(regs),
                Area::Dwell => self.dwell = Some(regs),
            },
            None => self.other_parse_errors += 1,
        }
    }

    pub fn finish(mut self, ctx: WindowContext) -> (Report, Option<String>) {
        let s = ctx.stats;
        let got_frames = s.frames > 0;
        let verified = got_frames && !ctx.simulated;
        let (health, quality, mut reasons) = match (&ctx.device_error, got_frames) {
            (Some(e), _) => ("no_source", None, vec![format!("device_error: {e}")]),
            (None, false) if ctx.bytes_read == 0 => ("no_source", None, vec!["no_bytes".into()]),
            (None, false) => ("no_source", None, vec!["no_valid_frames".into()]),
            (None, true) => {
                let q = s.frame_bytes as f64 / (s.frame_bytes + s.resync_bytes) as f64;
                let q = (q * 1000.0).round() / 1000.0;
                let health = if s.checksum_errors() > 0 {
                    "degraded"
                } else {
                    "ok"
                };
                (health, Some(q), Vec::new())
            }
        };
        if ctx.simulated {
            reasons.insert(0, "simulated".into());
        }
        if got_frames && s.checksum_errors() > 0 {
            reasons.push("checksum_errors".into());
        }
        if self.target_parse_errors + self.point_parse_errors + self.other_parse_errors > 0 {
            reasons.push("payload_rejected".into());
        }
        if ctx.link.foreign {
            reasons.push("foreign_firmware".into());
        }
        if let Some(e) = ctx.command_error {
            reasons.push(format!("command_write_failed: {e}"));
        }
        let hex = |t: &u16| format!("0x{t:04X}");
        let frame_counts: BTreeMap<String, u64> =
            self.type_counts.iter().map(|(t, n)| (hex(t), *n)).collect();
        let undecoded_frame_counts: BTreeMap<String, u64> = self
            .type_counts
            .iter()
            .filter(|(t, _)| !decode::DECODED.contains(t))
            .map(|(t, n)| (hex(t), *n))
            .collect();
        let status = std::mem::take(&mut self.status);
        let targets = std::mem::take(&mut self.targets);
        let points = std::mem::take(&mut self.points);
        let proof = if ctx.simulated {
            "SYNTHETIC"
        } else {
            "MEASURED"
        };
        let senml = senml::pack(
            &senml::base_name(&ctx.source_id),
            ctx.end_ms / 1000,
            proof,
            &status,
        )
        .map(|recs| senml::line(&recs));
        let code =
            |v: Option<u8>, name: fn(u8) -> Option<&'static str>| v.map(|c| named(c, name(c)));
        let areas = |v: Option<Vec<Region>>| v.map(|rs| rs.iter().map(region_out).collect());
        let report = Report {
            schema: SCHEMA,
            cog: "ld6004-radar",
            version: env!("CARGO_PKG_VERSION"),
            timestamp: ctx.end_ms / 1000,
            timestamp_ms: ctx.end_ms,
            window_start_ms: ctx.start_ms,
            sensor_class: "radar",
            source: Source {
                kind: SOURCE_KIND,
                device: ctx.device,
                verified,
                simulated: ctx.simulated,
            },
            health,
            quality,
            reasons,
            frame: TARGET_FRAME,
            axes: Axes {
                x: "lateral, positive toward the silkscreen text",
                y: "toward the bottom edge of the board",
                z: "forward, out of the antenna",
                above_radar: "above_radar_m = -y_m",
                camera_above_radar_m: f64::from(decode::CAMERA_ABOVE_RADAR_M),
            },
            targets: targets.iter().map(point_out).collect(),
            targets_dropped: self.targets_dropped,
            target_parse_errors: self.target_parse_errors,
            points: points.iter().map(point_out).collect(),
            points_dropped: self.points_dropped,
            point_parse_errors: self.point_parse_errors,
            zones: self.zones,
            noise_areas: areas(self.noise),
            detect_areas: areas(self.detect),
            dwell_areas: areas(self.dwell),
            work_mode: code(status.work_mode, decode::work_mode_name),
            low_power_state: code(status.low_power_state, decode::low_power_state_name),
            sensitivity: code(status.sensitivity, decode::sensitivity_name),
            trigger_speed: code(status.trigger_speed, decode::trigger_name),
            install: code(status.install, decode::install_name),
            gpio_mode: code(status.gpio_mode, decode::gpio_name),
            hold_delay_s: status.hold_delay_s,
            sleep_ms: status.sleep_ms,
            stay_life: status.stay_life,
            output_interval: status.output_interval,
            z_min_m: status.z_min_m.map(r3),
            z_max_m: status.z_max_m.map(r3),
            firmware: ctx.link.firmware.as_ref().map(|fw| FirmwareOut {
                project_code: fw.project_code,
                project_name: fw.project_name,
                major: fw.major,
                sub: fw.sub,
                modified: fw.modified,
                presence_project: fw.is_presence(),
            }),
            foreign_firmware: ctx.link.foreign,
            writes: ctx.writes,
            vitals: "not_defined",
            acks: self.acks,
            presence_refused: self.presence_refused,
            frames: s.frames,
            checksum_errors: s.checksum_errors(),
            oversize_frames: s.oversize_frames,
            resync_bytes: s.resync_bytes,
            bytes_read: ctx.bytes_read,
            frame_counts,
            undecoded_frame_counts,
            #[cfg(feature = "spatial-evidence")]
            spatial: None,
        };
        (report, senml)
    }
}

enum Area {
    Noise,
    Detect,
    Dwell,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{encode, Framer, VENDOR_FW_STATUS};

    fn ctx(stats: FramerStats, link: LinkState, simulated: bool) -> WindowContext {
        WindowContext {
            start_ms: 1_700_000_000_000,
            end_ms: 1_700_000_001_000,
            device: "/dev/ttyUSB0".into(),
            stats,
            bytes_read: stats.frame_bytes,
            device_error: None,
            simulated,
            link,
            writes: "none",
            source_id: "ld6004-radar".into(),
            command_error: None,
        }
    }

    fn run(bytes: &[u8]) -> (Report, Option<String>, LinkState) {
        let mut framer = Framer::new();
        let before = framer.stats();
        let frames = framer.feed(bytes);
        let mut win = Window::default();
        let mut link = LinkState::default();
        for f in &frames {
            win.ingest(f, &mut link);
        }
        let (report, senml) = win.finish(ctx(framer.stats().since(&before), link.clone(), false));
        (report, senml, link)
    }

    #[test]
    fn foreign_firmware_clears_and_refuses_later_targets() {
        let mut payload = vec![1, 0, 0, 0];
        payload.extend_from_slice(&1.0f32.to_le_bytes());
        payload.extend_from_slice(&2.0f32.to_le_bytes());
        payload.extend_from_slice(&3.0f32.to_le_bytes());
        payload.extend_from_slice(&0i32.to_le_bytes());
        payload.extend_from_slice(&4i32.to_le_bytes());
        let target = encode(1, decode::T_TARGET, &payload);
        let foreign = encode(2, decode::T_FIRMWARE, &[1, 0, 0, 0]);
        let mut bytes = target.clone();
        bytes.extend_from_slice(&foreign);
        bytes.extend_from_slice(&target);
        let (report, senml, link) = run(&bytes);
        assert!(link.foreign);
        assert!(
            report.targets.is_empty(),
            "a target before the bad status is cleared"
        );
        assert!(report.presence_refused >= 1);
        assert!(report.reasons.iter().any(|r| r == "foreign_firmware"));
        assert_eq!(report.firmware.as_ref().unwrap().project_code, 1);
        assert!(!report.firmware.as_ref().unwrap().presence_project);
        assert!(senml.is_none());
        let text = serde_json::to_string(&report).unwrap();
        assert!(!text.contains("bpm"), "{text}");
        assert!(!text.contains("heart_rate"));
        assert!(!text.contains("breathing_rate"));
    }

    #[test]
    fn protocol_example_project_6_keeps_the_following_target() {
        let mut payload = vec![1, 0, 0, 0];
        payload.extend_from_slice(&0.44f32.to_le_bytes());
        payload.extend_from_slice(&(-0.32f32).to_le_bytes());
        payload.extend_from_slice(&0.92f32.to_le_bytes());
        payload.extend_from_slice(&0i32.to_le_bytes());
        payload.extend_from_slice(&1i32.to_le_bytes());
        let mut bytes = VENDOR_FW_STATUS.to_vec();
        bytes.extend(encode(1, decode::T_TARGET, &payload));
        let (report, _, link) = run(&bytes);
        assert!(!link.foreign);
        assert!(report.firmware.as_ref().unwrap().presence_project);
        assert_eq!(report.targets.len(), 1);
        assert!((report.targets[0].x_m - 0.44).abs() < 1e-6);
        assert!((report.targets[0].above_radar_m - 0.32).abs() < 1e-6);
        assert!((report.targets[0].z_m - 0.92).abs() < 1e-6);
        assert_eq!(
            report.axes.camera_above_radar_m,
            f64::from(decode::CAMERA_ABOVE_RADAR_M)
        );
    }

    #[test]
    fn presence_firmware_keeps_section_6() {
        let mut bytes = encode(1, decode::T_FIRMWARE, &[0, 3, 1, 0]);
        bytes.extend_from_slice(&decode::simulated_burst());
        let (report, senml, link) = run(&bytes);
        assert!(!link.foreign);
        assert_eq!(report.targets.len(), 1);
        assert_eq!(report.targets[0].cluster_id, 2);
        assert_eq!(report.targets[0].dop_idx, 7);
        assert!((report.targets[0].z_m - 2.5).abs() < 1e-9);
        assert!((report.targets[0].above_radar_m - 0.5).abs() < 1e-9);
        assert!(report.points.is_empty());
        assert!(report.zones.is_none());
        assert!(report.source.verified);
        assert!(!report.source.simulated);
        assert_eq!(report.low_power_state.as_ref().unwrap().code, 1);
        assert_eq!(
            report.low_power_state.as_ref().unwrap().name,
            Some("normal")
        );
        assert_eq!(report.trigger_speed.as_ref().unwrap().code, 2);
        assert_eq!(report.vitals, "not_defined");
        let pack = senml.expect("status scalars");
        assert!(pack.starts_with('['));
        assert!(pack.contains("\"n\":\"low_power_state\""));
        assert!(!pack.contains("bpm"));
        assert_eq!(report.frame, "radar_local");
    }

    #[test]
    fn float_on_0x0a14_is_rejected_and_is_not_a_rate() {
        let (report, senml, _) = run(&encode(1, decode::T_LOW_POWER, &15.0f32.to_le_bytes()));
        assert!(report.low_power_state.is_none());
        assert!(senml.is_none());
        assert!(report.reasons.iter().any(|r| r == "payload_rejected"));
        let text = serde_json::to_string(&report).unwrap();
        assert!(!text.contains("bpm"));
        assert!(!text.contains("heart"));
        assert!(!text.contains("breath"));
    }

    #[test]
    fn empty_known_frame_is_an_ack_and_cloud_is_not_a_track() {
        let (report, _, _) = run(&encode(1, decode::T_TARGET, &[]));
        assert_eq!(report.acks, 1);
        assert!(report.targets.is_empty());
        assert_eq!(report.target_parse_errors, 0);
    }

    #[test]
    fn no_frames_is_an_honest_null_report() {
        let (report, senml) =
            Window::default().finish(ctx(FramerStats::default(), LinkState::default(), false));
        assert_eq!(report.health, "no_source");
        assert!(report.quality.is_none());
        assert!(!report.source.verified);
        assert!(report.zones.is_none());
        assert!(report.targets.is_empty());
        assert!(senml.is_none());
        assert_eq!(report.vitals, "not_defined");
        assert_eq!(report.timestamp, report.timestamp_ms / 1000);
    }

    #[test]
    fn ota_is_counted_and_not_decoded() {
        let (report, _, _) = run(&encode(1, 0x3000, &[0xFF, 0x00]));
        assert_eq!(report.undecoded_frame_counts.get("0x3000"), Some(&1));
        assert!(report.targets.is_empty());
    }
}
