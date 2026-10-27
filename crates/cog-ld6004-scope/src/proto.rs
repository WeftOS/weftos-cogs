//! HLK-LD6004 TinyFrame codec. Vendor: Seengreat LD6004 Serial Communication
//! Protocol, April 2026. DATA is little-endian. Header fields are big-endian.
//! Checksum is XOR of the covered bytes, then bitwise NOT.
//! A frame with LEN 0 is 8 bytes and has no data-checksum byte.

use std::collections::{BTreeMap, VecDeque};

pub const TY_CONTROL: u16 = 0x0201;
pub const TY_ZONE_SET: u16 = 0x0202;
pub const TY_DELAY_SET: u16 = 0x0203;
pub const TY_Z_SET: u16 = 0x0204;
pub const TY_SLEEP_SET: u16 = 0x0205;
pub const TY_LIFE_SET: u16 = 0x0206;
pub const TY_INTERVAL_SET: u16 = 0x0207;
pub const TY_TARGETS: u16 = 0x0A04;
pub const TY_CLOUD: u16 = 0x0A08;
pub const TY_PRESENCE: u16 = 0x0A0A;
pub const TY_INTERFERENCE: u16 = 0x0A0B;
pub const TY_DETECTION: u16 = 0x0A0C;
pub const TY_DELAY: u16 = 0x0A0D;
pub const TY_SENS: u16 = 0x0A0E;
pub const TY_TRIGGER: u16 = 0x0A0F;
pub const TY_Z: u16 = 0x0A10;
pub const TY_INSTALL: u16 = 0x0A11;
pub const TY_MODE: u16 = 0x0A12;
pub const TY_SLEEP: u16 = 0x0A13;
pub const TY_WORK: u16 = 0x0A14;
pub const TY_P20: u16 = 0x0A15;
pub const TY_DWELL: u16 = 0x0A16;
pub const TY_LIFE: u16 = 0x0A17;
pub const TY_INTERVAL: u16 = 0x0A18;
pub const TY_FW: u16 = 0xFFFF;

/// Read commands on type 0x0201. Each is a query, not a saved-config write.
pub const READ_CMDS: &[u32] = &[
    0x02, 0x05, 0x0D, 0x11, 0x12, 0x15, 0x18, 0x19, 0x1F, 0x26, 0x27,
];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Zone {
    pub x_min: f32,
    pub x_max: f32,
    pub y_min: f32,
    pub y_max: f32,
    pub z_min: f32,
    pub z_max: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Track {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub doppler: i32,
    pub id: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct TrailPt {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub id: i32,
}

#[derive(Clone, Debug)]
pub struct Frame {
    #[allow(dead_code)] // frame id is part of the wire header; the viewer keys off type
    pub id: u16,
    pub ty: u16,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub presence: [u32; 4],
    pub tracks: Vec<Track>,
    pub cloud: Vec<Track>,
    pub interference: [Zone; 4],
    pub detection: [Zone; 4],
    pub dwell: [Zone; 4],
    pub delay_s: Option<u32>,
    pub sensitivity: Option<u8>,
    pub trigger: Option<u8>,
    pub z_min: Option<f32>,
    pub z_max: Option<f32>,
    pub install: Option<u8>,
    pub mode: Option<u32>,
    pub sleep_ms: Option<u32>,
    pub work: Option<u32>,
    pub p20: Option<u8>,
    pub life: Option<u32>,
    pub interval: Option<u32>,
    pub firmware: Option<[u8; 4]>,
    pub reports: BTreeMap<u16, u32>,
    pub acks: BTreeMap<u16, u32>,
    pub trail: VecDeque<TrailPt>,
    /// Consecutive target frames with a count of 0. The coordinate holds across brief gaps.
    pub target_miss: u16,
}

/// Tracker clusters closer than this are one person. Live bench reads sit about 0.5 m apart.
pub const BODY_SPLIT_M: f32 = 0.75;
/// The U20 sits about a foot above the module. Radar +Y points toward the bottom edge of the board.
pub const CAMERA_ABOVE_RADAR_M: f32 = 0.30;

/// Height above the radar. Negating Y makes up match the room: the lens-height return was Y ≈ −0.32 m.
pub fn above_radar_m(y: f32) -> f32 {
    -y
}
/// Empty target frames to keep showing the last steady coordinate. At ~12 Hz this is under a second.
pub const TRACK_HOLD: u16 = 8;

#[derive(Clone, Debug)]
pub struct Body {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub range: f32,
    pub spread: f32,
    pub ids: Vec<i32>,
    pub doppler: i32,
}

/// Median of recent tracker points, then one body per person.
/// A one-off cluster beside a steady id is dropped. Eight empty frames clear the hold.
pub fn steady_bodies(trail: &VecDeque<TrailPt>, tracks: &[Track], target_miss: u16) -> Vec<Body> {
    if target_miss >= TRACK_HOLD {
        return Vec::new();
    }
    const WINDOW: usize = 36;
    let recent: Vec<&TrailPt> = trail
        .iter()
        .skip(trail.len().saturating_sub(WINDOW))
        .collect();
    let mut ids: Vec<i32> = Vec::new();
    for pt in &recent {
        if !ids.contains(&pt.id) {
            ids.push(pt.id);
        }
    }
    if ids.is_empty() {
        for t in tracks {
            if !ids.contains(&t.id) {
                ids.push(t.id);
            }
        }
    }
    let mut cents: Vec<Cent> = Vec::new();
    for id in ids {
        let pts: Vec<&TrailPt> = recent.iter().copied().filter(|pt| pt.id == id).collect();
        let (x, y, z, n) = if pts.is_empty() {
            match tracks.iter().find(|t| t.id == id) {
                Some(t) => (t.x, t.y, t.z, 1usize),
                None => continue,
            }
        } else {
            (
                median(&pts.iter().map(|pt| pt.x).collect::<Vec<_>>()),
                median(&pts.iter().map(|pt| pt.y).collect::<Vec<_>>()),
                median(&pts.iter().map(|pt| pt.z).collect::<Vec<_>>()),
                pts.len(),
            )
        };
        let doppler = tracks
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.doppler)
            .unwrap_or(0);
        cents.push(Cent {
            id,
            x,
            y,
            z,
            doppler,
            n,
        });
    }
    if cents.iter().any(|c| c.n >= 3) {
        cents.retain(|c| c.n >= 3);
    }
    cents.sort_by(|a, b| a.range().total_cmp(&b.range()));
    let mut used = vec![false; cents.len()];
    let mut out = Vec::new();
    for i in 0..cents.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut group = vec![i];
        for j in (i + 1)..cents.len() {
            if used[j] {
                continue;
            }
            if dist3(cents[i].xyz(), cents[j].xyz()) <= BODY_SPLIT_M {
                used[j] = true;
                group.push(j);
            }
        }
        out.push(body_from(&cents, &group));
    }
    out.sort_by(|a, b| a.range.total_cmp(&b.range));
    out
}

struct Cent {
    id: i32,
    x: f32,
    y: f32,
    z: f32,
    doppler: i32,
    n: usize,
}

impl Cent {
    fn xyz(&self) -> (f32, f32, f32) {
        (self.x, self.y, self.z)
    }

    fn range(&self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }
}

fn body_from(cents: &[Cent], group: &[usize]) -> Body {
    let n = group.len() as f32;
    let x = group.iter().map(|&k| cents[k].x).sum::<f32>() / n;
    let y = group.iter().map(|&k| cents[k].y).sum::<f32>() / n;
    let z = group.iter().map(|&k| cents[k].z).sum::<f32>() / n;
    let mut ids: Vec<i32> = group.iter().map(|&k| cents[k].id).collect();
    ids.sort_unstable();
    let doppler = group
        .iter()
        .map(|&k| cents[k].doppler)
        .max_by_key(|d| d.abs())
        .unwrap_or(0);
    let mut spread = 0.0f32;
    for a in group {
        for b in group {
            spread = spread.max(dist3(cents[*a].xyz(), cents[*b].xyz()));
        }
    }
    Body {
        x,
        y,
        z,
        range: (x * x + y * y + z * z).sqrt(),
        spread,
        ids,
        doppler,
    }
}

fn dist3(a: (f32, f32, f32), b: (f32, f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    let dz = a.2 - b.2;
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn median(values: &[f32]) -> f32 {
    let mut v: Vec<f32> = values.iter().copied().filter(|n| n.is_finite()).collect();
    v.sort_by(f32::total_cmp);
    let n = v.len();
    if n == 0 {
        return 0.0;
    }
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

impl Snapshot {
    pub fn got(&self, ty: u16) -> bool {
        self.reports.get(&ty).copied().unwrap_or(0) > 0
    }

    /// Queries that already answered on this module, plus the two live streams.
    pub fn required_ok(&self) -> bool {
        const NEED: &[u16] = &[
            TY_PRESENCE,
            TY_TARGETS,
            TY_INTERFERENCE,
            TY_DETECTION,
            TY_DWELL,
            TY_DELAY,
            TY_SENS,
            TY_TRIGGER,
            TY_Z,
            TY_INSTALL,
            TY_MODE,
            TY_SLEEP,
            TY_P20,
            TY_LIFE,
            TY_INTERVAL,
        ];
        NEED.iter().all(|ty| self.got(*ty)) && self.firmware.is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Wait,
    Held,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub name: String,
    pub verdict: Verdict,
    pub detail: String,
}

pub fn rows(snap: &Snapshot) -> Vec<Row> {
    let mut out = Vec::new();
    out.push(stream_row(
        "presence  0x0A0A",
        snap.got(TY_PRESENCE),
        presence_text(&snap.presence),
    ));
    out.push(stream_row(
        "targets   0x0A04",
        snap.got(TY_TARGETS),
        track_text(snap),
    ));
    out.push(stream_row(
        "point cloud 0x0A08",
        snap.got(TY_CLOUD),
        if snap.got(TY_CLOUD) {
            format!("{} points", snap.cloud.len())
        } else {
            "off until Show point cloud".into()
        },
    ));
    let zones_ok = snap.got(TY_INTERFERENCE) && snap.got(TY_DETECTION) && snap.got(TY_DWELL);
    out.push(stream_row(
        "zones     0x02 → 0x0A0B / 0C / 16",
        zones_ok,
        format!("det0 {}", zone_brief(&snap.detection[0])),
    ));
    out.push(value_row(
        "hold delay 0x0A0D",
        snap.delay_s.map(|s| format!("{s} s")),
    ));
    out.push(value_row(
        "sensitivity 0x0A0E",
        snap.sensitivity.map(|v| sens_name(v).to_string()),
    ));
    out.push(value_row(
        "trigger   0x0A0F",
        snap.trigger.map(|v| trigger_name(v).to_string()),
    ));
    out.push(value_row(
        "Z range   0x0A10",
        match (snap.z_min, snap.z_max) {
            (Some(a), Some(b)) => Some(format!("{a:.2} .. {b:.2} m")),
            _ => None,
        },
    ));
    out.push(value_row(
        "install   0x0A11",
        snap.install.map(|v| install_name(v).to_string()),
    ));
    out.push(value_row(
        "mode      0x0A12",
        snap.mode.map(|v| mode_name(v).to_string()),
    ));
    out.push(value_row(
        "sleep     0x0A13",
        snap.sleep_ms.map(|v| format!("{v} ms")),
    ));
    out.push(Row {
        name: "work      0x0A14".into(),
        verdict: if snap.got(TY_WORK) {
            Verdict::Pass
        } else {
            Verdict::Wait
        },
        detail: snap
            .work
            .map(|v| work_name(v).to_string())
            .unwrap_or_else(|| "only while unattended low-power".into()),
    });
    out.push(value_row(
        "P20       0x0A15",
        snap.p20.map(|v| p20_name(v).to_string()),
    ));
    out.push(value_row(
        "dwell life 0x0A17",
        snap.life.map(|v| format!("{v} (drops 20/s)")),
    ));
    out.push(value_row(
        "interval  0x0A18",
        snap.interval.map(|v| format!("{v} (1 = every frame)")),
    ));
    out.push(value_row(
        "firmware  0xFFFF",
        snap.firmware.map(firmware_text),
    ));
    let acks = snap.acks.get(&TY_CONTROL).copied().unwrap_or(0);
    out.push(stream_row(
        "control ACK 0x0201",
        acks > 0,
        format!("{acks} empty replies"),
    ));
    out.push(Row {
        name: "baud      0x0F0F".into(),
        verdict: Verdict::Held,
        detail: "not sent — would drop this link".into(),
    });
    out.push(Row {
        name: "OTA       0x3000".into(),
        verdict: Verdict::Held,
        detail: "not sent — image is unpublished".into(),
    });
    out.push(Row {
        name: "destructive 0x01/03/04/1A/1B/1C/25".into(),
        verdict: Verdict::Held,
        detail: "not sent — clears zones, stops the radar, or resets dwell".into(),
    });
    out
}

fn stream_row(name: &str, ok: bool, detail: String) -> Row {
    Row {
        name: name.into(),
        verdict: if ok { Verdict::Pass } else { Verdict::Wait },
        detail,
    }
}

fn value_row(name: &str, detail: Option<String>) -> Row {
    Row {
        name: name.into(),
        verdict: if detail.is_some() {
            Verdict::Pass
        } else {
            Verdict::Wait
        },
        detail: detail.unwrap_or_else(|| "no reply yet".into()),
    }
}

pub fn cksum(bytes: &[u8]) -> u8 {
    !bytes.iter().fold(0u8, |acc, b| acc ^ b)
}

pub fn encode(id: u16, ty: u16, data: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(9 + data.len());
    frame.push(0x01);
    frame.extend_from_slice(&id.to_be_bytes());
    frame.extend_from_slice(&(data.len() as u16).to_be_bytes());
    frame.extend_from_slice(&ty.to_be_bytes());
    let head = cksum(&frame);
    frame.push(head);
    if !data.is_empty() {
        frame.extend_from_slice(data);
        frame.push(cksum(data));
    }
    frame
}

pub fn control(id: u16, cmd: u32) -> Vec<u8> {
    encode(id, TY_CONTROL, &cmd.to_le_bytes())
}

pub fn sens_command(v: u8) -> u32 {
    match v {
        0 => 0x0A,
        1 => 0x0B,
        _ => 0x0C,
    }
}

pub fn trigger_command(v: u8) -> u32 {
    match v {
        0 => 0x0E,
        1 => 0x0F,
        _ => 0x10,
    }
}

pub fn install_command(v: u8) -> u32 {
    if v == 0 { 0x13 } else { 0x14 }
}

/// Normal, unattended, strong reflection. Radar-off (2, 3) is not offered.
pub fn mode_command(v: u32) -> Option<u32> {
    match v {
        0 => Some(0x17),
        1 => Some(0x16),
        4 => Some(0x24),
        _ => None,
    }
}

pub fn p20_command(v: u8) -> Option<u32> {
    match v {
        0 => Some(0x1D),
        1 => Some(0x1E),
        2 => Some(0x20),
        3 => Some(0x21),
        4 => Some(0x22),
        5 => Some(0x23),
        _ => None,
    }
}

pub fn zone_payload(area: i32, z: Zone) -> Vec<u8> {
    let mut data = Vec::with_capacity(28);
    data.extend_from_slice(&area.to_le_bytes());
    for v in [z.x_min, z.x_max, z.y_min, z.y_max, z.z_min, z.z_max] {
        data.extend_from_slice(&v.to_le_bytes());
    }
    data
}

pub fn z_payload(z_min: f32, z_max: f32) -> Vec<u8> {
    let mut data = Vec::with_capacity(8);
    data.extend_from_slice(&z_min.to_le_bytes());
    data.extend_from_slice(&z_max.to_le_bytes());
    data
}

#[derive(Debug, Default)]
pub struct Parser {
    buf: Vec<u8>,
    pub resyncs: u64,
}

impl Parser {
    pub fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
        if self.buf.len() > 8192 {
            let keep = self.buf.len().saturating_sub(64);
            self.buf.drain(..keep);
            self.resyncs += 1;
        }
    }

    pub fn next_frame(&mut self) -> Option<Frame> {
        loop {
            let Some(start) = self.buf.iter().position(|b| *b == 0x01) else {
                self.buf.clear();
                return None;
            };
            if start > 0 {
                self.buf.drain(..start);
                self.resyncs += 1;
            }
            if self.buf.len() < 8 {
                return None;
            }
            let len = u16::from_be_bytes([self.buf[3], self.buf[4]]) as usize;
            if len > 1024 {
                self.buf.drain(..1);
                self.resyncs += 1;
                continue;
            }
            let total = if len == 0 { 8 } else { 8 + len + 1 };
            if self.buf.len() < total {
                return None;
            }
            if cksum(&self.buf[..7]) != self.buf[7] {
                self.buf.drain(..1);
                self.resyncs += 1;
                continue;
            }
            if len > 0 && cksum(&self.buf[8..8 + len]) != self.buf[8 + len] {
                self.buf.drain(..1);
                self.resyncs += 1;
                continue;
            }
            let id = u16::from_be_bytes([self.buf[1], self.buf[2]]);
            let ty = u16::from_be_bytes([self.buf[5], self.buf[6]]);
            let data = if len == 0 {
                Vec::new()
            } else {
                self.buf[8..8 + len].to_vec()
            };
            self.buf.drain(..total);
            return Some(Frame { id, ty, data });
        }
    }
}

pub fn apply(snap: &mut Snapshot, frame: &Frame) {
    if frame.data.is_empty() {
        *snap.acks.entry(frame.ty).or_default() += 1;
        return;
    }
    *snap.reports.entry(frame.ty).or_default() += 1;
    match frame.ty {
        TY_PRESENCE => snap.presence = presence_from(&frame.data),
        TY_TARGETS => {
            snap.tracks = tracks_from(&frame.data);
            if snap.tracks.is_empty() {
                snap.target_miss = snap.target_miss.saturating_add(1);
            } else {
                snap.target_miss = 0;
                for t in &snap.tracks {
                    snap.trail.push_back(TrailPt {
                        x: t.x,
                        y: t.y,
                        z: t.z,
                        id: t.id,
                    });
                }
                while snap.trail.len() > 240 {
                    snap.trail.pop_front();
                }
            }
        }
        TY_CLOUD => snap.cloud = tracks_from(&frame.data),
        TY_INTERFERENCE => snap.interference = zones_from(&frame.data),
        TY_DETECTION => snap.detection = zones_from(&frame.data),
        TY_DWELL => snap.dwell = zones_from(&frame.data),
        TY_DELAY => snap.delay_s = u32_of(&frame.data),
        TY_SENS => snap.sensitivity = frame.data.first().copied(),
        TY_TRIGGER => snap.trigger = frame.data.first().copied(),
        TY_Z if frame.data.len() >= 8 => {
            snap.z_min = Some(f32_at(&frame.data, 0));
            snap.z_max = Some(f32_at(&frame.data, 4));
        }
        TY_INSTALL => snap.install = frame.data.first().copied(),
        TY_MODE => snap.mode = u32_of(&frame.data),
        TY_SLEEP => snap.sleep_ms = u32_of(&frame.data),
        TY_WORK => snap.work = work_of(&frame.data),
        TY_P20 => snap.p20 = frame.data.first().copied(),
        TY_LIFE => snap.life = u32_of(&frame.data),
        TY_INTERVAL => snap.interval = u32_of(&frame.data),
        TY_FW if frame.data.len() >= 4 => {
            snap.firmware = Some([frame.data[0], frame.data[1], frame.data[2], frame.data[3]]);
        }
        _ => {}
    }
}

pub fn sens_name(v: u8) -> &'static str {
    match v {
        0 => "low",
        1 => "medium",
        2 => "high",
        _ => "unknown",
    }
}

pub fn trigger_name(v: u8) -> &'static str {
    match v {
        0 => "slow",
        1 => "medium",
        2 => "fast",
        _ => "unknown",
    }
}

pub fn install_name(v: u8) -> &'static str {
    match v {
        0 => "top",
        1 => "side",
        _ => "unknown",
    }
}

pub fn mode_name(v: u32) -> &'static str {
    match v {
        0 => "normal",
        1 => "unattended low-power",
        2 => "radar off, P20 high",
        3 => "radar off, P20 low",
        4 => "strong reflection",
        _ => "unknown",
    }
}

pub fn work_name(v: u32) -> &'static str {
    match v {
        0 => "sleeping (unattended)",
        1 => "awake",
        _ => "unknown",
    }
}

pub fn p20_name(v: u8) -> &'static str {
    match v {
        0 => "high when present",
        1 => "low when present",
        2 => "held low",
        3 => "held high",
        4 => "500 ms low jog",
        5 => "500 ms high jog",
        _ => "unknown",
    }
}

pub fn firmware_text(v: [u8; 4]) -> String {
    format!("project {}  {}.{}.{}", v[0], v[1], v[2], v[3])
}

pub fn zone_brief(z: &Zone) -> String {
    format!(
        "x {:.2}..{:.2}  y {:.2}..{:.2}  z {:.2}..{:.2}",
        z.x_min, z.x_max, z.y_min, z.y_max, z.z_min, z.z_max
    )
}

fn presence_text(p: &[u32; 4]) -> String {
    p.iter()
        .enumerate()
        .map(|(i, v)| format!("z{i}:{}", if *v == 0 { "empty" } else { "in" }))
        .collect::<Vec<_>>()
        .join("  ")
}

fn track_text(snap: &Snapshot) -> String {
    match snap.tracks.first() {
        Some(t) => format!(
            "n={}  id {}  x {:.3}  y {:.3}  z {:.3}  dop {}",
            snap.tracks.len(),
            t.id,
            t.x,
            t.y,
            t.z,
            t.doppler
        ),
        None if snap.got(TY_TARGETS) => "count 0".into(),
        None => "no reply yet".into(),
    }
}

fn presence_from(data: &[u8]) -> [u32; 4] {
    let mut out = [0; 4];
    for (i, slot) in out.iter_mut().enumerate() {
        let o = i * 4;
        if data.len() >= o + 4 {
            *slot = u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
        }
    }
    out
}

fn tracks_from(data: &[u8]) -> Vec<Track> {
    if data.len() < 4 {
        return Vec::new();
    }
    let n = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    let body = &data[4..];
    let available = body.len() / 20;
    (0..n.min(available).min(8))
        .map(|i| {
            let c = &body[i * 20..i * 20 + 20];
            Track {
                x: f32_at(c, 0),
                y: f32_at(c, 4),
                z: f32_at(c, 8),
                doppler: i32::from_le_bytes(c[12..16].try_into().unwrap()),
                id: i32::from_le_bytes(c[16..20].try_into().unwrap()),
            }
        })
        .collect()
}

fn zones_from(data: &[u8]) -> [Zone; 4] {
    let mut out = [Zone::default(); 4];
    for (i, chunk) in data.chunks_exact(24).take(4).enumerate() {
        out[i] = Zone {
            x_min: f32_at(chunk, 0),
            x_max: f32_at(chunk, 4),
            y_min: f32_at(chunk, 8),
            y_max: f32_at(chunk, 12),
            z_min: f32_at(chunk, 16),
            z_max: f32_at(chunk, 20),
        };
    }
    out
}

fn u32_of(data: &[u8]) -> Option<u32> {
    if data.len() < 4 {
        return None;
    }
    Some(u32::from_le_bytes(data[0..4].try_into().unwrap()))
}

fn work_of(data: &[u8]) -> Option<u32> {
    if data.len() >= 4 {
        u32_of(data)
    } else {
        data.first().map(|b| *b as u32)
    }
}

fn f32_at(data: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(data[at..at + 4].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn live_probe_bytes_round_trip() {
        assert_eq!(hex(&encode(0x000c, TY_FW, &[])), "01000c0000fffff2");
        assert_eq!(hex(&control(0x0001, 0x02)), "01000100040201f802000000fd");
        assert_eq!(cksum(&[0x01, 0x00, 0x00, 0x00, 0x04, 0x02, 0x01]), 0xF9);
    }

    #[test]
    fn empty_frame_does_not_eat_the_next_start() {
        let mut bytes = encode(0x000c, TY_FW, &[]);
        bytes.extend_from_slice(&control(0x0001, 0x02));
        let mut parser = Parser::default();
        parser.push(&[0x00, 0xFF]);
        parser.push(&bytes);
        let ack = parser.next_frame().unwrap();
        let cmd = parser.next_frame().unwrap();
        assert!(ack.data.is_empty());
        assert_eq!(ack.ty, TY_FW);
        assert_eq!(cmd.data, 2u32.to_le_bytes());
        assert!(parser.next_frame().is_none());
        assert_eq!(parser.resyncs, 1);
    }

    #[test]
    fn live_target_and_presence_payloads() {
        let target = unhex("010000009aeadebdb19bc1bc07b6213f0000000001000000");
        let presence = unhex("01000000000000000000000000000000");
        let mut snap = Snapshot::default();
        apply(
            &mut snap,
            &Frame {
                id: 1,
                ty: TY_TARGETS,
                data: target,
            },
        );
        apply(
            &mut snap,
            &Frame {
                id: 2,
                ty: TY_PRESENCE,
                data: presence,
            },
        );
        assert_eq!(snap.presence, [1, 0, 0, 0]);
        let t = snap.tracks[0];
        assert_eq!(t.id, 1);
        assert_eq!(t.doppler, 0);
        assert!((-0.11..-0.10).contains(&t.x), "{}", t.x);
        assert!((-0.03..-0.02).contains(&t.y), "{}", t.y);
        assert!((0.60..0.65).contains(&t.z), "{}", t.z);
        assert_eq!(snap.trail.len(), 1);
    }

    #[test]
    fn live_scalar_replies() {
        let mut snap = Snapshot::default();
        apply(
            &mut snap,
            &Frame {
                id: 1,
                ty: TY_DELAY,
                data: unhex("1e000000"),
            },
        );
        apply(
            &mut snap,
            &Frame {
                id: 2,
                ty: TY_SENS,
                data: vec![2],
            },
        );
        apply(
            &mut snap,
            &Frame {
                id: 3,
                ty: TY_TRIGGER,
                data: vec![2],
            },
        );
        apply(
            &mut snap,
            &Frame {
                id: 4,
                ty: TY_Z,
                data: unhex("0000003f0000c040"),
            },
        );
        apply(
            &mut snap,
            &Frame {
                id: 5,
                ty: TY_INSTALL,
                data: vec![0],
            },
        );
        apply(
            &mut snap,
            &Frame {
                id: 6,
                ty: TY_MODE,
                data: unhex("00000000"),
            },
        );
        apply(
            &mut snap,
            &Frame {
                id: 7,
                ty: TY_SLEEP,
                data: unhex("64000000"),
            },
        );
        apply(
            &mut snap,
            &Frame {
                id: 8,
                ty: TY_P20,
                data: vec![0],
            },
        );
        apply(
            &mut snap,
            &Frame {
                id: 9,
                ty: TY_LIFE,
                data: unhex("c8000000"),
            },
        );
        apply(
            &mut snap,
            &Frame {
                id: 10,
                ty: TY_INTERVAL,
                data: unhex("01000000"),
            },
        );
        apply(
            &mut snap,
            &Frame {
                id: 11,
                ty: TY_FW,
                data: unhex("06031200"),
            },
        );
        assert_eq!(snap.delay_s, Some(30));
        assert_eq!(snap.sensitivity, Some(2));
        assert_eq!(snap.trigger, Some(2));
        assert_eq!(snap.z_min, Some(0.5));
        assert_eq!(snap.z_max, Some(6.0));
        assert_eq!(snap.install, Some(0));
        assert_eq!(snap.mode, Some(0));
        assert_eq!(snap.sleep_ms, Some(100));
        assert_eq!(snap.p20, Some(0));
        assert_eq!(snap.life, Some(200));
        assert_eq!(snap.interval, Some(1));
        assert_eq!(snap.firmware, Some([6, 3, 18, 0]));
        assert_eq!(firmware_text([6, 3, 18, 0]), "project 6  3.18.0");
    }

    #[test]
    fn setters_match_the_manual_and_skip_radar_off() {
        assert_eq!(sens_command(2), 0x0C);
        assert_eq!(trigger_command(2), 0x10);
        assert_eq!(install_command(0), 0x13);
        assert_eq!(mode_command(0), Some(0x17));
        assert_eq!(mode_command(1), Some(0x16));
        assert_eq!(mode_command(4), Some(0x24));
        assert_eq!(mode_command(2), None);
        assert_eq!(mode_command(3), None);
        assert_eq!(p20_command(0), Some(0x1D));
        assert!(p20_command(9).is_none());
        let z = Zone {
            x_min: -6.0,
            x_max: 6.0,
            y_min: -6.0,
            y_max: 6.0,
            z_min: 0.5,
            z_max: 6.0,
        };
        let data = zone_payload(4, z);
        assert_eq!(data.len(), 28);
        assert_eq!(&data[..4], &4i32.to_le_bytes());
    }

    fn push(trail: &mut VecDeque<TrailPt>, id: i32, x: f32, y: f32, z: f32, n: usize) {
        for _ in 0..n {
            trail.push_back(TrailPt { x, y, z, id });
        }
    }

    #[test]
    fn a_foot_of_camera_height_lands_on_the_lens() {
        let up = above_radar_m(-0.32);
        assert!((0.30..0.34).contains(&up), "{up}");
        assert!((up - CAMERA_ABOVE_RADAR_M).abs() < 0.05);
    }

    #[test]
    fn nearby_tracker_ids_are_one_person_and_a_far_id_stays_separate() {
        let mut trail = VecDeque::new();
        push(&mut trail, 1, -0.48, -0.04, 0.95, 8);
        push(&mut trail, 2, -0.01, -0.16, 0.97, 8);
        let tracks = [
            Track {
                x: -0.48,
                y: -0.04,
                z: 0.95,
                doppler: 0,
                id: 1,
            },
            Track {
                x: -0.01,
                y: -0.16,
                z: 0.97,
                doppler: 0,
                id: 2,
            },
        ];
        let one = steady_bodies(&trail, &tracks, 0);
        assert_eq!(one.len(), 1, "{one:?}");
        assert_eq!(one[0].ids, vec![1, 2]);
        assert!((0.95..1.15).contains(&one[0].range), "{}", one[0].range);
        assert!((0.40..0.60).contains(&one[0].spread), "{}", one[0].spread);
        assert_eq!(one[0].doppler, 0);

        let mut far = VecDeque::new();
        push(&mut far, 1, 0.0, 0.0, 1.0, 8);
        push(&mut far, 2, 0.0, 0.0, 3.0, 8);
        let two = steady_bodies(&far, &[], 0);
        assert_eq!(two.len(), 2);
        assert!((0.9..1.1).contains(&two[0].range));
        assert!((2.9..3.1).contains(&two[1].range));
    }

    #[test]
    fn one_wild_point_does_not_move_the_person_or_add_a_second() {
        let mut trail = VecDeque::new();
        push(&mut trail, 1, 0.0, 0.0, 1.0, 9);
        push(&mut trail, 1, 0.0, 0.0, 4.0, 1);
        push(&mut trail, 9, 2.0, 0.0, 0.5, 1);
        let bodies = steady_bodies(&trail, &[], 0);
        assert_eq!(bodies.len(), 1);
        assert!((0.95..1.05).contains(&bodies[0].z), "{}", bodies[0].z);
        assert_eq!(bodies[0].ids, vec![1]);
    }

    #[test]
    fn eight_empty_target_frames_drop_the_held_coordinate() {
        let mut snap = Snapshot::default();
        apply(
            &mut snap,
            &Frame {
                id: 1,
                ty: TY_TARGETS,
                data: {
                    let mut data = 1u32.to_le_bytes().to_vec();
                    data.extend_from_slice(&0.0f32.to_le_bytes());
                    data.extend_from_slice(&0.0f32.to_le_bytes());
                    data.extend_from_slice(&1.0f32.to_le_bytes());
                    data.extend_from_slice(&0i32.to_le_bytes());
                    data.extend_from_slice(&1i32.to_le_bytes());
                    data
                },
            },
        );
        for _ in 0..5 {
            snap.trail.push_back(TrailPt {
                x: 0.0,
                y: 0.0,
                z: 1.0,
                id: 1,
            });
        }
        let empty = Frame {
            id: 2,
            ty: TY_TARGETS,
            data: 0u32.to_le_bytes().to_vec(),
        };
        for _ in 0..(TRACK_HOLD - 1) {
            apply(&mut snap, &empty);
        }
        assert_eq!(
            steady_bodies(&snap.trail, &snap.tracks, snap.target_miss).len(),
            1
        );
        apply(&mut snap, &empty);
        assert!(steady_bodies(&snap.trail, &snap.tracks, snap.target_miss).is_empty());
    }
}
