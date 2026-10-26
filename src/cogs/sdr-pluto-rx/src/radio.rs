//! The radio the acquisition thread drives: a trait with three implementations, the live Pluto
//! (`pluto.rs`), a synthetic one (`--simulate`) and a replay of a recorded capture (`--replay`).

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use crate::plan::{Band, Gain};

/// Samples per ring record.
pub const DEFAULT_BLOCK: usize = 8192;
/// Samples per libiio read. Small reads cost a round trip each over the network backend and
/// cannot keep up with 5 MS/s; the acquisition buffer is cut into `DEFAULT_BLOCK` records.
pub const DEFAULT_IIO_BUFFER: usize = 65_536;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HwStatus {
    pub lo_hz: Option<u64>,
    pub rate_sps: Option<u32>,
    pub bw_hz: Option<u32>,
    pub gain_db: Option<f64>,
    pub gain_mode: Option<String>,
    pub rssi_db: Option<f64>,
    pub temp_phy_c: Option<f64>,
    pub temp_zynq_c: Option<f64>,
}

/// What the cog found and set when it took the radio, so the state is on record.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SafeState {
    pub loopback_was: Option<String>,
    pub loopback_now: Option<String>,
    pub tx_atten_was_db: Option<f64>,
    pub tx_atten_now_db: Option<f64>,
    pub dds_tones_off: bool,
    pub notes: Vec<String>,
}

#[derive(Debug)]
pub enum RefillError {
    /// A replay ran out of data.
    Eof,
    /// The radio is gone (unplugged, power, USB reset): the cog waits and reopens.
    Lost(String),
    Other(String),
}

pub trait Radio: Send {
    fn describe(&self) -> String;
    fn block_samples(&self) -> usize;
    /// Retune: set centre, rate, bandwidth and gain, and restart the capture buffer so no sample
    /// of the old tuning is delivered after this returns.
    fn apply(&mut self, b: &Band) -> Result<(), String>;
    /// Wait for the next block and copy it (interleaved little-endian i16 I, Q) into `out`.
    fn refill(&mut self, out: &mut Vec<u8>) -> Result<(), RefillError>;
    fn hw_status(&mut self) -> HwStatus;
    /// What the cog found and set when it took the radio (the live radio only).
    fn safe_state(&self) -> SafeState {
        SafeState::default()
    }
}

pub fn mono_ns() -> u64 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1_000_000_000
}

/// A synthetic receiver: a tone a quarter of the way up the band, a pulse train and noise,
/// paced to the sample rate so the cog behaves as it would on a live stream.
pub struct Sim {
    block: usize,
    band: Option<Band>,
    phase: f64,
    rng: u64,
    /// Pace multiplier; 0 delivers blocks as fast as asked.
    speed: f64,
    due: Option<Instant>,
    n: u64,
}

impl Sim {
    pub fn new(block: usize, speed: f64) -> Self {
        Self {
            block,
            band: None,
            phase: 0.0,
            rng: 0x9E37_79B9_7F4A_7C15,
            speed,
            due: None,
            n: 0,
        }
    }

    fn noise(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64 - 0.5
    }
}

impl Radio for Sim {
    fn describe(&self) -> String {
        "simulated receiver (SYNTHETIC)".into()
    }

    fn block_samples(&self) -> usize {
        self.block
    }

    fn apply(&mut self, b: &Band) -> Result<(), String> {
        b.check()?;
        self.band = Some(b.clone());
        self.due = None;
        Ok(())
    }

    fn refill(&mut self, out: &mut Vec<u8>) -> Result<(), RefillError> {
        let b = self
            .band
            .clone()
            .ok_or_else(|| RefillError::Other("not tuned".into()))?;
        if self.speed > 0.0 {
            let step =
                Duration::from_secs_f64(self.block as f64 / f64::from(b.rate_sps) / self.speed);
            let due = self.due.unwrap_or_else(Instant::now) + step;
            let now = Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            }
            self.due = Some(due);
        }
        out.clear();
        let w = 2.0 * std::f64::consts::PI * 0.25;
        for i in 0..self.block {
            let pulse = if (self.n * self.block as u64 + i as u64) % 40_000 < 400 {
                600.0
            } else {
                0.0
            };
            let (s, c) = self.phase.sin_cos();
            let ni = self.noise() * 120.0;
            let nq = self.noise() * 120.0;
            let iv = (c * 1000.0 + pulse + ni).clamp(-2047.0, 2047.0) as i16;
            let qv = (s * 1000.0 + nq).clamp(-2047.0, 2047.0) as i16;
            out.extend_from_slice(&iv.to_le_bytes());
            out.extend_from_slice(&qv.to_le_bytes());
            self.phase = (self.phase + w) % (2.0 * std::f64::consts::PI);
        }
        self.n += 1;
        Ok(())
    }

    fn hw_status(&mut self) -> HwStatus {
        let b = self.band.clone();
        HwStatus {
            lo_hz: b.as_ref().map(|b| b.centre_hz),
            rate_sps: b.as_ref().map(|b| b.rate_sps),
            bw_hz: b.as_ref().map(|b| b.bw_hz),
            gain_db: b.as_ref().map(|b| match b.gain {
                Gain::Manual(g) => g,
                Gain::Auto => 30.0,
            }),
            gain_mode: Some("simulated".into()),
            rssi_db: Some(-60.0),
            temp_phy_c: Some(40.0),
            temp_zynq_c: Some(60.0),
        }
    }
}

const MAGIC: &[u8; 8] = b"PLUTOIQ1";
const HEADER: usize = 32;

/// A capture file: 32 bytes (`PLUTOIQ1`, centre Hz u64, rate u32, bandwidth u32, block samples
/// u32, reserved u32, all little-endian) and then whole blocks of interleaved i16 I, Q.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureHeader {
    pub centre_hz: u64,
    pub rate_sps: u32,
    pub bw_hz: u32,
    pub block: u32,
}

impl CaptureHeader {
    pub fn to_bytes(&self) -> [u8; HEADER] {
        let mut b = [0u8; HEADER];
        b[..8].copy_from_slice(MAGIC);
        b[8..16].copy_from_slice(&self.centre_hz.to_le_bytes());
        b[16..20].copy_from_slice(&self.rate_sps.to_le_bytes());
        b[20..24].copy_from_slice(&self.bw_hz.to_le_bytes());
        b[24..28].copy_from_slice(&self.block.to_le_bytes());
        b
    }

    pub fn parse(b: &[u8]) -> Result<Self, String> {
        if b.len() < HEADER || &b[..8] != MAGIC {
            return Err("not a PLUTOIQ1 capture".into());
        }
        let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let h = Self {
            centre_hz: u64::from_le_bytes(b[8..16].try_into().unwrap_or([0; 8])),
            rate_sps: u32_at(16),
            bw_hz: u32_at(20),
            block: u32_at(24),
        };
        if h.block == 0 || h.block > 1 << 20 || h.rate_sps == 0 {
            return Err("capture header out of range".into());
        }
        Ok(h)
    }
}

/// Writes blocks as a capture file.
pub struct Recorder {
    f: std::fs::File,
    pub blocks: u64,
}

impl Recorder {
    pub fn create(path: &str, h: &CaptureHeader) -> Result<Self, String> {
        let mut f = std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?;
        f.write_all(&h.to_bytes())
            .map_err(|e| format!("{path}: {e}"))?;
        Ok(Self { f, blocks: 0 })
    }

    pub fn push(&mut self, block: &[u8]) -> Result<(), String> {
        self.f
            .write_all(block)
            .map_err(|e| format!("record: {e}"))?;
        self.blocks += 1;
        Ok(())
    }
}

/// Replays a capture file, paced to its rate (`speed` > 0) or as fast as read (0).
pub struct Replay {
    f: std::fs::File,
    pub head: CaptureHeader,
    speed: f64,
    due: Option<Instant>,
    path: String,
}

impl Replay {
    pub fn open(path: &str, speed: f64) -> Result<Self, String> {
        let mut f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
        let mut h = [0u8; HEADER];
        f.read_exact(&mut h).map_err(|e| format!("{path}: {e}"))?;
        Ok(Self {
            head: CaptureHeader::parse(&h)?,
            f,
            speed,
            due: None,
            path: path.to_string(),
        })
    }
}

impl Radio for Replay {
    fn describe(&self) -> String {
        format!("replay of {}", self.path)
    }

    fn block_samples(&self) -> usize {
        self.head.block as usize
    }

    /// A replay has one tuning, the capture's; it accepts only that.
    fn apply(&mut self, b: &Band) -> Result<(), String> {
        if b.centre_hz != self.head.centre_hz || b.rate_sps != self.head.rate_sps {
            return Err(format!(
                "the replay holds {} Hz at {} S/s",
                self.head.centre_hz, self.head.rate_sps
            ));
        }
        Ok(())
    }

    fn refill(&mut self, out: &mut Vec<u8>) -> Result<(), RefillError> {
        let n = self.head.block as usize * 4;
        out.resize(n, 0);
        if let Err(e) = self.f.read_exact(out) {
            return Err(if e.kind() == std::io::ErrorKind::UnexpectedEof {
                RefillError::Eof
            } else {
                RefillError::Other(e.to_string())
            });
        }
        if self.speed > 0.0 {
            let step = Duration::from_secs_f64(
                f64::from(self.head.block) / f64::from(self.head.rate_sps) / self.speed,
            );
            let due = self.due.unwrap_or_else(Instant::now) + step;
            let now = Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            }
            self.due = Some(due);
        }
        Ok(())
    }

    fn hw_status(&mut self) -> HwStatus {
        HwStatus {
            lo_hz: Some(self.head.centre_hz),
            rate_sps: Some(self.head.rate_sps),
            bw_hz: Some(self.head.bw_hz),
            gain_mode: Some("recorded".into()),
            ..HwStatus::default()
        }
    }
}

/// True if every byte is zero (a dead or loopback-muted receive path).
pub fn all_zero(b: &[u8]) -> bool {
    b.iter().all(|&x| x == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band() -> Band {
        Band::new(2_437_000_000, 5_000_000, 100)
    }

    #[test]
    fn the_simulator_makes_blocks_of_the_right_size_with_a_tone() {
        let mut s = Sim::new(1024, 0.0);
        let mut out = Vec::new();
        assert!(matches!(s.refill(&mut out), Err(RefillError::Other(_))));
        s.apply(&band()).unwrap();
        s.refill(&mut out).unwrap();
        assert_eq!(out.len(), 1024 * 4);
        assert!(!all_zero(&out));
        let st = s.hw_status();
        assert_eq!(
            (st.lo_hz, st.rate_sps),
            (Some(2_437_000_000), Some(5_000_000))
        );
        assert!(s.apply(&Band::new(10_000_000, 5_000_000, 100)).is_err());
    }

    #[test]
    fn a_capture_file_round_trips_through_the_replay() {
        let dir = std::env::temp_dir().join(format!("sdr-rec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.iq").to_string_lossy().into_owned();
        let h = CaptureHeader {
            centre_hz: 2_437_000_000,
            rate_sps: 5_000_000,
            bw_hz: 4_000_000,
            block: 64,
        };
        let mut rec = Recorder::create(&path, &h).unwrap();
        let mut sim = Sim::new(64, 0.0);
        sim.apply(&band()).unwrap();
        let mut blocks = Vec::new();
        for _ in 0..3 {
            let mut b = Vec::new();
            sim.refill(&mut b).unwrap();
            rec.push(&b).unwrap();
            blocks.push(b);
        }
        let mut rp = Replay::open(&path, 0.0).unwrap();
        assert_eq!(rp.head, h);
        assert!(rp.apply(&Band::new(1_090_000_000, 5_000_000, 100)).is_err());
        rp.apply(&band()).unwrap();
        let mut out = Vec::new();
        for b in &blocks {
            rp.refill(&mut out).unwrap();
            assert_eq!(&out, b);
        }
        assert!(matches!(rp.refill(&mut out), Err(RefillError::Eof)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_bad_capture_header_is_refused() {
        assert!(CaptureHeader::parse(b"short").is_err());
        let mut b = CaptureHeader {
            centre_hz: 1,
            rate_sps: 1,
            bw_hz: 1,
            block: 1,
        }
        .to_bytes();
        assert!(CaptureHeader::parse(&b).is_ok());
        b[0] = b'X';
        assert!(CaptureHeader::parse(&b).is_err());
        let z = CaptureHeader {
            centre_hz: 1,
            rate_sps: 1,
            bw_hz: 1,
            block: 0,
        }
        .to_bytes();
        assert!(CaptureHeader::parse(&z).is_err());
    }

    #[test]
    fn zero_detection() {
        assert!(all_zero(&[0; 16]));
        assert!(!all_zero(&[0, 0, 1]));
    }
}
