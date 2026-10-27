//! Payload meanings from the HLK-LD6004 protocol only.
//!
//! Shenzhen Hi-Link, "HLK-LD6004 模组通信协议", cover date 2025-09.
//! Section 6 is the presence project. Type ids are not copied from the LD6002
//! breath protocol: there `0x0A14` is a breathing rate; here §6.18 `0x0A14` is
//! the unattended low-power working state. Heart rate and breathing rate are
//! not defined anywhere in this document, so this module has no rate field.
//!
//! A payload whose length is not the length the same section states returns
//! `None`. The caller counts that and emits null. It does not guess.

/// Personnel target positions. §6.8: `0x0A04` is target data.
pub const T_TARGET: u16 = 0x0A04;
/// Point cloud. §6.8: same layout as targets, not a person track.
pub const T_CLOUD: u16 = 0x0A08;
/// Four detection-area occupancy words. §6.9.
pub const T_ZONES: u16 = 0x0A0A;
/// Interference-area rectangles. §6.10.
pub const T_NOISE_AREA: u16 = 0x0A0B;
/// Detection-area rectangles. §6.10.
pub const T_DETECT_AREA: u16 = 0x0A0C;
/// Dwell-area rectangles. §6.10. Not the LD6002 distance type.
pub const T_DWELL_AREA: u16 = 0x0A16;
/// Hold delay, uint32 seconds. §6.11.
pub const T_HOLD: u16 = 0x0A0D;
/// Sensitivity 0 low, 1 medium, 2 high. §6.12.
pub const T_SENS: u16 = 0x0A0E;
/// Trigger speed 0 slow, 1 medium, 2 fast. §6.13.
pub const T_TRIGGER: u16 = 0x0A0F;
/// Z range, two floats, metres. §6.14.
pub const T_Z_RANGE: u16 = 0x0A10;
/// Install method 0 top, 1 side. §6.15.
pub const T_INSTALL: u16 = 0x0A11;
/// Work mode 0..=4. §6.16. Not a vital sign.
pub const T_WORK: u16 = 0x0A12;
/// Unattended low-power sleep time, uint32 milliseconds. §6.17.
/// The section heading wrongly says `0x0A12`; the TYPE column says `0x0A13`.
pub const T_SLEEP: u16 = 0x0A13;
/// Unattended low-power working state. §6.18. Not a breathing rate.
pub const T_LOW_POWER: u16 = 0x0A14;
/// P20 GPIO output mode 0..=5. §6.19. Not the LD6002 heart-rate type.
pub const T_GPIO: u16 = 0x0A15;
/// Dwell-area life counter. §6.20.
pub const T_STAY: u16 = 0x0A17;
/// Serial output interval (frames, not a duration). §6.21.
pub const T_INTERVAL: u16 = 0x0A18;
/// Firmware status. §5.2.
pub const T_FIRMWARE: u16 = 0xFFFF;

/// Type ids whose payload this module will decode. Anything else is counted
/// and left undecoded, including LD6002 breath-protocol ids this part does not
/// define (`0x0A15` here is GPIO, and there is no heart-rate id).
pub const DECODED: &[u16] = &[
    T_TARGET,
    T_CLOUD,
    T_ZONES,
    T_NOISE_AREA,
    T_DETECT_AREA,
    T_DWELL_AREA,
    T_HOLD,
    T_SENS,
    T_TRIGGER,
    T_Z_RANGE,
    T_INSTALL,
    T_WORK,
    T_SLEEP,
    T_LOW_POWER,
    T_GPIO,
    T_STAY,
    T_INTERVAL,
    T_FIRMWARE,
];

const RECORD_LEN: usize = 20;

/// One position record. `dop_idx` is the protocol's int32; no unit is stated.
#[derive(Debug, Clone, PartialEq)]
pub struct Position {
    pub x_m: f32,
    pub y_m: f32,
    pub z_m: f32,
    pub dop_idx: i32,
    pub cluster_id: i32,
}

/// §5.2 firmware status. `project_name` is `None` when the code is outside 0..=5.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firmware {
    pub project_code: u8,
    pub project_name: Option<&'static str>,
    pub major: u8,
    pub sub: u8,
    pub modified: u8,
}

impl Firmware {
    /// Section 6 applies to project code 0 and to project code 6.
    /// Code 0 is the name-table entry "presence". Code 6 is the code printed in
    /// protocol §5.2 and the code on the bench module (firmware 3.18.0).
    /// Any other code is a different project and must not be read as section 6.
    pub fn is_presence(&self) -> bool {
        self.project_code == 0 || self.project_code == 6
    }
}

/// The bench U20 sits about a foot above the module. Not a correction to X.
pub const CAMERA_ABOVE_RADAR_M: f32 = 0.30;

/// Height above the radar. With the silkscreen readable, +Y points toward the
/// bottom edge of the board, so up is −Y. A lens-height return sat near Y = −0.32 m.
pub fn above_radar_m(y_m: f32) -> f32 {
    -y_m
}

/// Axis-aligned region. Floats, metres, as the §6.10 bullets state.
/// The table cell that types `x_min` as int32 contradicts those bullets and
/// the §6.2 set command (six floats). The bullets are followed.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub x_min_m: f32,
    pub x_max_m: f32,
    pub y_min_m: f32,
    pub y_max_m: f32,
    pub z_min_m: f32,
    pub z_max_m: f32,
}

/// §6.8. `target_num` then N records of x, y, z (float metres), `dop_idx`
/// (int32), `cluster_id` (int32), little-endian. The payload must be exactly
/// `4 + 20 * N`. A non-finite coordinate rejects the whole frame.
pub fn positions(payload: &[u8]) -> Option<Vec<Position>> {
    if payload.len() < 4 {
        return None;
    }
    let n = i32::from_le_bytes(take4(payload, 0)?);
    if n < 0 {
        return None;
    }
    let n = n as usize;
    let need = 4usize.checked_add(n.checked_mul(RECORD_LEN)?)?;
    if payload.len() != need {
        return None;
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let o = 4 + i * RECORD_LEN;
        let x_m = f32::from_le_bytes(take4(payload, o)?);
        let y_m = f32::from_le_bytes(take4(payload, o + 4)?);
        let z_m = f32::from_le_bytes(take4(payload, o + 8)?);
        if !x_m.is_finite() || !y_m.is_finite() || !z_m.is_finite() {
            return None;
        }
        out.push(Position {
            x_m,
            y_m,
            z_m,
            dop_idx: i32::from_le_bytes(take4(payload, o + 12)?),
            cluster_id: i32::from_le_bytes(take4(payload, o + 16)?),
        });
    }
    Some(out)
}

/// §6.9. Four uint32 values, 1 = occupied, 0 = empty. Any other value is
/// `None` for that area (the manual does not define it). Wrong length is
/// `None` for the whole frame.
pub fn zones(payload: &[u8]) -> Option<[Option<bool>; 4]> {
    if payload.len() != 16 {
        return None;
    }
    let mut out = [None; 4];
    for (i, slot) in out.iter_mut().enumerate() {
        let v = u32::from_le_bytes(take4(payload, i * 4)?);
        *slot = match v {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
    }
    Some(out)
}

/// §6.10. Four regions, six float metres each (96 bytes).
pub fn regions(payload: &[u8]) -> Option<Vec<Region>> {
    const ONE: usize = 24;
    if payload.len() != ONE * 4 {
        return None;
    }
    let mut out = Vec::with_capacity(4);
    for i in 0..4 {
        let o = i * ONE;
        let vals = [
            f32::from_le_bytes(take4(payload, o)?),
            f32::from_le_bytes(take4(payload, o + 4)?),
            f32::from_le_bytes(take4(payload, o + 8)?),
            f32::from_le_bytes(take4(payload, o + 12)?),
            f32::from_le_bytes(take4(payload, o + 16)?),
            f32::from_le_bytes(take4(payload, o + 20)?),
        ];
        if vals.iter().any(|v| !v.is_finite()) {
            return None;
        }
        out.push(Region {
            x_min_m: vals[0],
            x_max_m: vals[1],
            y_min_m: vals[2],
            y_max_m: vals[3],
            z_min_m: vals[4],
            z_max_m: vals[5],
        });
    }
    Some(out)
}

/// §5.2. Four bytes: project, major, sub, modified.
pub fn firmware(payload: &[u8]) -> Option<Firmware> {
    if payload.len() != 4 {
        return None;
    }
    let project_code = payload[0];
    Some(Firmware {
        project_code,
        project_name: project_name(project_code),
        major: payload[1],
        sub: payload[2],
        modified: payload[3],
    })
}

/// §5.2 project codes. 1 means the breath-detection *project*, not a rate.
pub fn project_name(code: u8) -> Option<&'static str> {
    Some(match code {
        0 => "presence",
        1 => "breath_detection_project",
        2 => "gesture",
        3 => "ranging",
        4 => "people_count",
        5 => "point_cloud_3d",
        _ => return None,
    })
}

pub fn work_mode_name(code: u8) -> Option<&'static str> {
    Some(match code {
        0 => "normal",
        1 => "unattended_low_power",
        2 => "radar_off_p20_high",
        3 => "radar_off_p20_low",
        4 => "strong_reflection",
        _ => return None,
    })
}

/// §6.18. 0 unattended low-power, 1 normal. Not a rate.
pub fn low_power_state_name(code: u8) -> Option<&'static str> {
    Some(match code {
        0 => "unattended_low_power",
        1 => "normal",
        _ => return None,
    })
}

pub fn sensitivity_name(code: u8) -> Option<&'static str> {
    Some(match code {
        0 => "low",
        1 => "medium",
        2 => "high",
        _ => return None,
    })
}

pub fn trigger_name(code: u8) -> Option<&'static str> {
    Some(match code {
        0 => "slow",
        1 => "medium",
        2 => "fast",
        _ => return None,
    })
}

pub fn install_name(code: u8) -> Option<&'static str> {
    Some(match code {
        0 => "top",
        1 => "side",
        _ => return None,
    })
}

pub fn gpio_name(code: u8) -> Option<&'static str> {
    Some(match code {
        0 => "present_high",
        1 => "present_low",
        2 => "held_low",
        3 => "held_high",
        4 => "pulse_500ms_low",
        5 => "pulse_500ms_high",
        _ => return None,
    })
}

/// One-byte status whose section states a 1-byte payload.
pub fn u8_code(payload: &[u8]) -> Option<u8> {
    if payload.len() == 1 {
        Some(payload[0])
    } else {
        None
    }
}

/// Little-endian uint32. Used for fields whose DATA row says 4 bytes.
pub fn u32_le(payload: &[u8]) -> Option<u32> {
    if payload.len() == 4 {
        Some(u32::from_le_bytes(take4(payload, 0)?))
    } else {
        None
    }
}

/// §6.14. Two little-endian floats, metres.
pub fn z_range(payload: &[u8]) -> Option<(f32, f32)> {
    if payload.len() != 8 {
        return None;
    }
    let z_min = f32::from_le_bytes(take4(payload, 0)?);
    let z_max = f32::from_le_bytes(take4(payload, 4)?);
    if z_min.is_finite() && z_max.is_finite() {
        Some((z_min, z_max))
    } else {
        None
    }
}

fn take4(payload: &[u8], o: usize) -> Option<[u8; 4]> {
    Some([
        *payload.get(o)?,
        *payload.get(o + 1)?,
        *payload.get(o + 2)?,
        *payload.get(o + 3)?,
    ])
}

/// Documented little-endian float example, protocol §7.2: bytes
/// `66 66 A2 41` are 20.3 (the manual rounds the f32).
#[cfg(test)]
pub fn manual_float_20_3() -> f32 {
    f32::from_le_bytes([0x66, 0x66, 0xA2, 0x41])
}

/// Bytes `--simulate` feeds through the real framer.
///
/// Two frames are copied from the manual (`VENDOR_EMPTY_CLOUD`, `VENDOR_TRIGGER_FAST`).
/// The `0x0A04` target and the `0x0A14` state are built with [`crate::frame::encode`]
/// because the manual does not print a full one-target frame. The burst has no
/// `0xFFFF` status. Project code 6 is presence (see [`Firmware::is_presence`]);
/// a breath-project status is not part of this burst.
pub fn simulated_burst() -> Vec<u8> {
    let mut payload = Vec::with_capacity(24);
    // New frame: X lateral toward the silkscreen, Y toward the bottom edge
    // (height above the radar is -Y), Z forward out of the antenna.
    // At yaw 90 this is room [1.5, 2.5, 0.5]: 2.5 m forward, 0.5 m above the radar.
    payload.extend_from_slice(&1i32.to_le_bytes());
    payload.extend_from_slice(&1.5f32.to_le_bytes());
    payload.extend_from_slice(&(-0.5f32).to_le_bytes());
    payload.extend_from_slice(&2.5f32.to_le_bytes());
    payload.extend_from_slice(&7i32.to_le_bytes());
    payload.extend_from_slice(&2i32.to_le_bytes());
    let mut out = Vec::new();
    out.extend_from_slice(crate::frame::VENDOR_EMPTY_CLOUD);
    out.extend_from_slice(crate::frame::VENDOR_TRIGGER_FAST);
    out.extend(crate::frame::encode(1, T_TARGET, &payload));
    out.extend(crate::frame::encode(2, T_LOW_POWER, &[0x01]));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(x: f32, y: f32, z: f32, dop: i32, id: i32) -> [u8; 20] {
        let mut b = [0u8; 20];
        b[0..4].copy_from_slice(&x.to_le_bytes());
        b[4..8].copy_from_slice(&y.to_le_bytes());
        b[8..12].copy_from_slice(&z.to_le_bytes());
        b[12..16].copy_from_slice(&dop.to_le_bytes());
        b[16..20].copy_from_slice(&id.to_le_bytes());
        b
    }

    #[test]
    fn a_foot_of_camera_height_lands_on_the_lens() {
        let up = above_radar_m(-0.32);
        assert!((0.30..0.34).contains(&up), "{up}");
        assert!((up - CAMERA_ABOVE_RADAR_M).abs() < 0.05);
    }

    #[test]
    fn manual_float_example_is_about_20_3() {
        let v = manual_float_20_3();
        assert!((v - 20.3).abs() < 1e-4, "{v}");
        assert_eq!(v.to_le_bytes(), [0x66, 0x66, 0xA2, 0x41]);
    }

    #[test]
    fn empty_cloud_payload_is_zero_targets() {
        let pts = positions(&[0, 0, 0, 0]).unwrap();
        assert!(pts.is_empty());
    }

    #[test]
    fn one_target_uses_little_endian_floats_and_exact_length() {
        let mut p = vec![1, 0, 0, 0];
        p.extend_from_slice(&rec(1.5, 2.5, 0.5, 7, 2));
        let pts = positions(&p).unwrap();
        assert_eq!(pts.len(), 1);
        assert_eq!(pts[0].cluster_id, 2);
        assert_eq!(pts[0].dop_idx, 7);
        assert_eq!(pts[0].x_m, 1.5);
        assert_eq!(pts[0].y_m, 2.5);
        assert_eq!(pts[0].z_m, 0.5);
        p.push(0);
        assert!(
            positions(&p).is_none(),
            "trailing byte is not a partial record"
        );
    }

    #[test]
    fn negative_count_non_finite_and_short_records_are_refused() {
        assert!(positions(&(-1i32).to_le_bytes()).is_none());
        assert!(positions(&[0, 0]).is_none());
        let mut p = vec![1, 0, 0, 0];
        p.extend_from_slice(&rec(f32::NAN, 1.0, 0.0, 0, 1));
        assert!(positions(&p).is_none());
        // N says 2 but only one record is present: no partial frame.
        let mut short = vec![2, 0, 0, 0];
        short.extend_from_slice(&rec(1.0, 1.0, 0.0, 0, 1));
        assert!(positions(&short).is_none());
    }

    #[test]
    fn zones_are_four_u32_words() {
        let mut p = Vec::new();
        for v in [1u32, 0, 1, 0] {
            p.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(
            zones(&p),
            Some([Some(true), Some(false), Some(true), Some(false)])
        );
        p[0] = 2; // defined values are only 0 and 1
        assert_eq!(zones(&p).unwrap()[0], None);
        assert!(zones(&[0, 1]).is_none());
    }

    #[test]
    fn low_power_state_is_not_a_float_rate() {
        assert_eq!(u8_code(&[0x01]), Some(1));
        assert_eq!(low_power_state_name(1), Some("normal"));
        assert_eq!(low_power_state_name(0), Some("unattended_low_power"));
        // A 4-byte float, which is what the LD6002 breath protocol puts on
        // 0x0A14, does not match this part's 1-byte state.
        assert!(u8_code(&15.0f32.to_le_bytes()).is_none());
        assert!(low_power_state_name(9).is_none());
    }

    #[test]
    fn firmware_example_project_6_has_no_name() {
        let fw = firmware(&[0x06, 0x03, 0x0A, 0x00]).unwrap();
        assert_eq!(fw.major, 3);
        assert_eq!(fw.sub, 0x0A);
        assert_eq!(fw.project_name, None);
        assert!(
            fw.is_presence(),
            "protocol example project 6 is this presence module"
        );
        assert!(!Firmware {
            project_code: 1,
            project_name: project_name(1),
            major: 0,
            sub: 0,
            modified: 0,
        }
        .is_presence());
        assert!(firmware(&[0]).is_none());
        assert_eq!(project_name(0), Some("presence"));
        assert_eq!(project_name(1), Some("breath_detection_project"));
    }

    #[test]
    fn dwell_area_is_not_a_four_byte_distance() {
        // LD6002 uses 0x0A16 as a distance. This part's 0x0A16 is four regions.
        assert!(regions(&100.0f32.to_le_bytes()).is_none());
        let mut p = vec![0u8; 96];
        p[0..4].copy_from_slice(&1.0f32.to_le_bytes());
        let regs = regions(&p).unwrap();
        assert_eq!(regs.len(), 4);
        assert_eq!(regs[0].x_min_m, 1.0);
    }

    #[test]
    fn simulated_burst_parses_and_is_not_a_foreign_firmware() {
        let frames = crate::frame::Framer::new().feed(&simulated_burst());
        let types: Vec<u16> = frames.iter().map(|f| f.ty).collect();
        assert_eq!(types, vec![T_CLOUD, T_TRIGGER, T_TARGET, T_LOW_POWER]);
        let pts = positions(&frames[2].payload).unwrap();
        assert_eq!(
            (
                pts[0].x_m,
                pts[0].y_m,
                pts[0].z_m,
                pts[0].dop_idx,
                pts[0].cluster_id
            ),
            (1.5, -0.5, 2.5, 7, 2)
        );
        assert_eq!(u8_code(&frames[1].payload), Some(2));
        assert_eq!(u8_code(&frames[3].payload), Some(1));
        assert!(frames.iter().all(|f| f.ty != T_FIRMWARE));
    }

    #[test]
    fn sleep_time_requires_four_bytes() {
        // §6.17 DATA row is uint32. The example LEN column says 1, which
        // contradicts it. A 1-byte payload is not decoded.
        assert!(u32_le(&[0xF4, 0x01, 0x00, 0x00]).is_some());
        assert!(u32_le(&[0xF4]).is_none());
    }
}
