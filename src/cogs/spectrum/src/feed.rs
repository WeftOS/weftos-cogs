//! Offline sources for `--once`: a synthetic band and a replay of a capture made by the owner's
//! `--record`. Both produce the same records the owner writes to its ring, with the stream's own
//! time (sample count over rate) as the clock, so a run is deterministic.

use std::io::Read;

use crate::iq::{Header, KIND_SAMPLES};

pub const RECORD: usize = 8192;
const CAPTURE_MAGIC: &[u8; 8] = b"PLUTOIQ1";
const CAPTURE_HEADER: usize = 32;

pub struct Band {
    pub centre_hz: u64,
    pub rate_sps: u32,
    pub bw_hz: u32,
}

pub trait Source {
    fn band(&self) -> &Band;
    /// The next record's samples, or None at the end.
    fn next(&mut self) -> Option<Vec<u8>>;
}

/// A tone a quarter of the way up the band, a wideband burst with a 20 percent duty cycle and
/// noise: enough to see peaks, a floor and a partial occupancy.
pub struct Synthetic {
    band: Band,
    left: u64,
    n: u64,
    rng: u64,
}

impl Synthetic {
    pub fn new(band: Band, secs: f64) -> Self {
        let records = (f64::from(band.rate_sps) * secs / RECORD as f64).ceil() as u64;
        Self {
            band,
            left: records,
            n: 0,
            rng: 0x2545_F491_4F6C_DD1D,
        }
    }

    fn noise(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64 - 0.5
    }
}

impl Source for Synthetic {
    fn band(&self) -> &Band {
        &self.band
    }

    fn next(&mut self) -> Option<Vec<u8>> {
        if self.left == 0 {
            return None;
        }
        self.left -= 1;
        // a burst lasts one record in five
        let burst = self.n.is_multiple_of(5);
        let mut out = Vec::with_capacity(RECORD * 4);
        for i in 0..RECORD as u64 {
            let t = (self.n * RECORD as u64 + i) as f64;
            let ph = 2.0 * std::f64::consts::PI * 0.25 * t;
            let (mut a, mut b) = (1000.0 * ph.cos(), 1000.0 * ph.sin());
            let sigma = if burst { 500.0 } else { 100.0 };
            a += self.noise() * sigma;
            b += self.noise() * sigma;
            out.extend_from_slice(&(a.clamp(-2047.0, 2047.0) as i16).to_le_bytes());
            out.extend_from_slice(&(b.clamp(-2047.0, 2047.0) as i16).to_le_bytes());
        }
        self.n += 1;
        Some(out)
    }
}

/// A capture file from `sdr-pluto-rx --record`: 32 bytes (`PLUTOIQ1`, centre Hz u64, rate u32,
/// bandwidth u32, block samples u32, reserved u32, little-endian), then interleaved i16 I, Q.
pub struct Replay {
    band: Band,
    f: std::fs::File,
}

impl Replay {
    pub fn open(path: &str) -> Result<Replay, String> {
        let mut f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
        let mut h = [0u8; CAPTURE_HEADER];
        f.read_exact(&mut h).map_err(|e| format!("{path}: {e}"))?;
        if &h[..8] != CAPTURE_MAGIC {
            return Err(format!("{path} is not a PLUTOIQ1 capture"));
        }
        let rate = u32::from_le_bytes([h[16], h[17], h[18], h[19]]);
        if rate == 0 {
            return Err(format!("{path}: sample rate 0"));
        }
        Ok(Replay {
            band: Band {
                centre_hz: u64::from_le_bytes(h[8..16].try_into().unwrap_or([0; 8])),
                rate_sps: rate,
                bw_hz: u32::from_le_bytes([h[20], h[21], h[22], h[23]]),
            },
            f,
        })
    }
}

impl Source for Replay {
    fn band(&self) -> &Band {
        &self.band
    }

    fn next(&mut self) -> Option<Vec<u8>> {
        let mut v = vec![0u8; RECORD * 4];
        let mut got = 0;
        while got < v.len() {
            match self.f.read(&mut v[got..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => got += n,
            }
        }
        let whole = got / 4 * 4;
        if whole == 0 {
            return None;
        }
        v.truncate(whole);
        Some(v)
    }
}

/// The header of record number `k` of an offline source, `idx` samples in.
pub fn header(band: &Band, idx: u64, n: usize) -> Header {
    Header {
        kind: KIND_SAMPLES,
        flags: 0,
        segment: 1,
        centre_hz: band.centre_hz,
        rate_sps: band.rate_sps,
        bw_hz: band.bw_hz,
        sample_index: idx,
        lost: 0,
        n_samples: n as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_synthetic_band_has_the_length_asked_for_and_a_tone() {
        let mut s = Synthetic::new(
            Band {
                centre_hz: 2_437_000_000,
                rate_sps: 5_000_000,
                bw_hz: 4_000_000,
            },
            0.1,
        );
        let mut n = 0;
        let mut first = None;
        while let Some(r) = s.next() {
            assert_eq!(r.len(), RECORD * 4);
            first.get_or_insert(r);
            n += 1;
        }
        assert_eq!(n, (5_000_000f64 * 0.1 / RECORD as f64).ceil() as usize);
        assert!(first.unwrap().iter().any(|&b| b != 0));
    }

    #[test]
    fn a_capture_file_is_read_in_records_and_the_tail_is_kept() {
        let dir = std::env::temp_dir().join(format!("spec-feed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("c.iq");
        let mut h = [0u8; CAPTURE_HEADER];
        h[..8].copy_from_slice(CAPTURE_MAGIC);
        h[8..16].copy_from_slice(&1_090_000_000u64.to_le_bytes());
        h[16..20].copy_from_slice(&2_400_000u32.to_le_bytes());
        h[20..24].copy_from_slice(&1_920_000u32.to_le_bytes());
        let mut data = h.to_vec();
        data.extend(vec![1u8; RECORD * 4 + 40]);
        std::fs::write(&p, &data).unwrap();
        let mut r = Replay::open(p.to_str().unwrap()).unwrap();
        assert_eq!(
            (r.band().centre_hz, r.band().rate_sps, r.band().bw_hz),
            (1_090_000_000, 2_400_000, 1_920_000)
        );
        assert_eq!(r.next().unwrap().len(), RECORD * 4);
        assert_eq!(r.next().unwrap().len(), 40);
        assert!(r.next().is_none());
        std::fs::write(
            &p,
            b"nonsense that is long enough to read a header from, yes",
        )
        .unwrap();
        assert!(Replay::open(p.to_str().unwrap()).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
