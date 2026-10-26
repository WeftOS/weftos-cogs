//! Welch power spectrum and band occupancy over a stream of interleaved i16 I, Q samples.
//!
//! Frames of `n` samples (Hann window, 50 percent overlap) are transformed, their power added into
//! `acc`, and every `stride`-th frame is also classified for occupancy: the frame's noise floor
//! is the median bin power, a bin is busy when it is more than `thresh_db` above it, and the
//! usable bins (the analog bandwidth, not the whole sample rate) are cut into equal slices that
//! count busy and total cells. Levels are dBFS with a full-scale complex tone (I and Q amplitude
//! 2048, the 12-bit range) in one bin reading 0.

use crate::fft::{hann, Fft};

pub const FULL_SCALE: f64 = 2048.0;

pub struct Welch {
    n: usize,
    hop: usize,
    fft: Fft,
    win: Vec<f32>,
    /// `(FULL_SCALE * sum(window))^2`: the power of a full-scale tone's bin.
    ref_pow: f64,
    pend: Vec<(f32, f32)>,
    start: usize,
    re: Vec<f32>,
    im: Vec<f32>,
    acc: Vec<f64>,
    frames: u64,
    occ: Occupancy,
    stride: u64,
    seen: u64,
    pw: Vec<f32>,
    scratch: Vec<f32>,
}

/// Busy and total cells per slice, and the floor and peak seen.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Occupancy {
    pub busy: Vec<u64>,
    pub total: Vec<u64>,
    pub floor_db_sum: f64,
    pub frames: u64,
    pub peak_db: f64,
}

impl Occupancy {
    fn new(slices: usize) -> Self {
        Self {
            busy: vec![0; slices],
            total: vec![0; slices],
            floor_db_sum: 0.0,
            frames: 0,
            peak_db: -300.0,
        }
    }

    pub fn merge(&mut self, o: &Occupancy) {
        if self.busy.len() != o.busy.len() {
            *self = o.clone();
            return;
        }
        for i in 0..self.busy.len() {
            self.busy[i] += o.busy[i];
            self.total[i] += o.total[i];
        }
        self.floor_db_sum += o.floor_db_sum;
        self.frames += o.frames;
        self.peak_db = self.peak_db.max(o.peak_db);
    }

    pub fn slice(&self, i: usize) -> f64 {
        if self.total[i] == 0 {
            0.0
        } else {
            self.busy[i] as f64 / self.total[i] as f64
        }
    }

    pub fn overall(&self) -> f64 {
        let t: u64 = self.total.iter().sum();
        if t == 0 {
            0.0
        } else {
            self.busy.iter().sum::<u64>() as f64 / t as f64
        }
    }

    pub fn floor_db(&self) -> f64 {
        if self.frames == 0 {
            -300.0
        } else {
            self.floor_db_sum / self.frames as f64
        }
    }
}

/// What a window of frames produced.
#[derive(Debug, Clone)]
pub struct Spectrum {
    /// Mean power per bin in dBFS, fftshifted: lowest frequency first, `n` bins.
    pub bins_db: Vec<f32>,
    pub frames: u64,
    pub occ: Occupancy,
}

pub struct Params {
    pub n: usize,
    pub slices: usize,
    pub thresh_db: f64,
    /// Fraction of the bins that is usable (the analog bandwidth over the sample rate).
    pub usable: f64,
    /// Classify every `stride`-th frame for occupancy.
    pub stride: u64,
}

impl Welch {
    pub fn new(p: &Params) -> Result<Welch, String> {
        let fft = Fft::new(p.n)?;
        let (win, s2) = hann(p.n);
        let sw: f64 = win.iter().map(|&w| f64::from(w)).sum();
        let _ = s2;
        Ok(Welch {
            n: p.n,
            hop: p.n / 2,
            fft,
            win,
            ref_pow: (FULL_SCALE * sw).powi(2),
            pend: Vec::new(),
            start: 0,
            re: vec![0.0; p.n],
            im: vec![0.0; p.n],
            acc: vec![0.0; p.n],
            frames: 0,
            occ: Occupancy::new(p.slices.max(1)),
            stride: p.stride.max(1),
            seen: 0,
            pw: vec![0.0; p.n],
            scratch: Vec::with_capacity(p.n),
        })
    }

    /// Forget the overlap buffer: the next samples do not continue the previous ones.
    pub fn break_continuity(&mut self) {
        self.pend.clear();
        self.start = 0;
    }

    #[cfg(test)]
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Add samples (interleaved little-endian i16 I, Q) and transform every frame they complete.
    pub fn push(&mut self, iq: &[u8], p: &Params) {
        for c in iq.chunks_exact(4) {
            self.pend.push((
                f32::from(i16::from_le_bytes([c[0], c[1]])),
                f32::from(i16::from_le_bytes([c[2], c[3]])),
            ));
        }
        while self.pend.len() - self.start >= self.n {
            self.frame(p);
            self.start += self.hop;
        }
        if self.start > 0 {
            self.pend.drain(..self.start);
            self.start = 0;
        }
    }

    fn frame(&mut self, p: &Params) {
        let n = self.n;
        for i in 0..n {
            let (a, b) = self.pend[self.start + i];
            self.re[i] = a * self.win[i];
            self.im[i] = b * self.win[i];
        }
        self.fft.forward(&mut self.re, &mut self.im);
        let h = n / 2;
        // fftshift: output bin k is FFT index (k + n/2) mod n, so the upper half comes first
        let mut pw = std::mem::take(&mut self.pw);
        for k in 0..h {
            let src = k + h;
            let v = self.re[src] * self.re[src] + self.im[src] * self.im[src];
            pw[k] = v;
            self.acc[k] += f64::from(v);
            let v = self.re[k] * self.re[k] + self.im[k] * self.im[k];
            pw[k + h] = v;
            self.acc[k + h] += f64::from(v);
        }
        self.frames += 1;
        self.seen += 1;
        if self.seen.is_multiple_of(self.stride) {
            self.classify(&pw, p);
        }
        self.pw = pw;
    }

    fn classify(&mut self, pw: &[f32], p: &Params) {
        let n = self.n;
        let usable = ((n as f64 * p.usable).round() as usize).clamp(8, n);
        let lo = (n - usable) / 2;
        let band = &pw[lo..lo + usable];
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.clear();
        scratch.extend_from_slice(band);
        let mid = scratch.len() / 2;
        let (_, med, _) = scratch.select_nth_unstable_by(mid, f32::total_cmp);
        let floor = (*med).max(1e-12);
        self.scratch = scratch;
        let thr = floor * 10f32.powf(p.thresh_db as f32 / 10.0);
        let slices = self.occ.busy.len();
        let mut peak = 0f32;
        for (i, &v) in band.iter().enumerate() {
            let s = i * slices / usable;
            self.occ.total[s] += 1;
            if v > thr {
                self.occ.busy[s] += 1;
            }
            peak = peak.max(v);
        }
        self.occ.frames += 1;
        self.occ.floor_db_sum += self.to_db(f64::from(floor));
        self.occ.peak_db = self.occ.peak_db.max(self.to_db(f64::from(peak)));
    }

    fn to_db(&self, p: f64) -> f64 {
        10.0 * (p.max(1e-30) / self.ref_pow).log10()
    }

    /// The window's result; resets the accumulators (the overlap buffer is kept).
    pub fn take(&mut self) -> Option<Spectrum> {
        if self.frames == 0 {
            return None;
        }
        let f = self.frames as f64;
        let bins_db = self.acc.iter().map(|&a| self.to_db(a / f) as f32).collect();
        let slices = self.occ.busy.len();
        let s = Spectrum {
            bins_db,
            frames: self.frames,
            occ: std::mem::replace(&mut self.occ, Occupancy::new(slices)),
        };
        self.acc.iter_mut().for_each(|a| *a = 0.0);
        self.frames = 0;
        Some(s)
    }
}

/// Reduce `bins` (power in dB) to `out` bins by averaging power over equal groups.
pub fn decimate_db(bins: &[f32], out: usize) -> Vec<f32> {
    if out == 0 || out >= bins.len() {
        return bins.to_vec();
    }
    (0..out)
        .map(|i| {
            let (a, b) = (i * bins.len() / out, (i + 1) * bins.len() / out);
            let mean: f64 = bins[a..b]
                .iter()
                .map(|&d| 10f64.powf(f64::from(d) / 10.0))
                .sum::<f64>()
                / (b - a) as f64;
            (10.0 * mean.max(1e-30).log10()) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(slices: usize) -> Params {
        Params {
            n: 1024,
            slices,
            thresh_db: 10.0,
            usable: 0.8,
            stride: 1,
        }
    }

    /// A tone `bin` bins above the centre (the shifted bin index is n/2 + offset), amplitude `a`,
    /// plus deterministic noise of amplitude `noise`.
    fn tone(samples: usize, offset: i32, a: f64, noise: f64, seed: u64) -> Vec<u8> {
        let mut r = seed | 1;
        let mut out = Vec::with_capacity(samples * 4);
        for t in 0..samples {
            r ^= r << 13;
            r ^= r >> 7;
            r ^= r << 17;
            let ni = ((r >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * noise;
            r ^= r << 13;
            r ^= r >> 7;
            r ^= r << 17;
            let nq = ((r >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * noise;
            let ph = 2.0 * std::f64::consts::PI * f64::from(offset) * t as f64 / 1024.0;
            out.extend_from_slice(&((a * ph.cos() + ni) as i16).to_le_bytes());
            out.extend_from_slice(&((a * ph.sin() + nq) as i16).to_le_bytes());
        }
        out
    }

    #[test]
    fn a_full_scale_tone_reads_zero_dbfs_in_its_bin() {
        let p = params(8);
        let mut w = Welch::new(&p).unwrap();
        w.push(&tone(16_384, 100, 2047.0, 0.0, 1), &p);
        let s = w.take().unwrap();
        assert_eq!(s.bins_db.len(), 1024);
        let (peak, db) = s
            .bins_db
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        assert_eq!(peak, 512 + 100, "the bin above the centre");
        assert!(db.abs() < 0.2, "{db} dBFS");
        // 31 frames from 16384 samples at hop 512
        assert_eq!(s.frames, 16_384 / 512 - 1);
    }

    #[test]
    fn a_half_scale_tone_is_six_db_down_and_negative_offsets_are_below_the_centre() {
        let p = params(8);
        let mut w = Welch::new(&p).unwrap();
        w.push(&tone(8192, -200, 1024.0, 0.0, 1), &p);
        let s = w.take().unwrap();
        let (peak, db) = s
            .bins_db
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        assert_eq!(peak, 512 - 200);
        assert!((db + 6.02).abs() < 0.3, "{db}");
    }

    #[test]
    fn frames_continue_across_pushes_and_restart_after_a_break() {
        let p = params(4);
        let data = tone(4096, 10, 1000.0, 0.0, 3);
        let mut a = Welch::new(&p).unwrap();
        a.push(&data, &p);
        let mut b = Welch::new(&p).unwrap();
        for chunk in data.chunks(4 * 700) {
            b.push(chunk, &p);
        }
        assert_eq!(a.frames(), b.frames());
        let (x, y) = (a.take().unwrap(), b.take().unwrap());
        assert!(x
            .bins_db
            .iter()
            .zip(&y.bins_db)
            .all(|(p, q)| (p - q).abs() < 1e-3));
        // after a break the overlap buffer is empty: 1000 samples make no frame
        let mut c = Welch::new(&p).unwrap();
        c.push(&data[..4 * 1000], &p);
        c.break_continuity();
        c.push(&data[..4 * 1000], &p);
        assert_eq!(
            c.frames(),
            0,
            "two 1000-sample halves are not one frame run"
        );
    }

    #[test]
    fn noise_alone_is_not_busy_and_a_strong_tone_is() {
        let p = params(8);
        let mut w = Welch::new(&p).unwrap();
        w.push(&tone(65_536, 0, 0.0, 200.0, 7), &p);
        let quiet = w.take().unwrap();
        assert!(
            quiet.occ.overall() < 0.01,
            "noise occupancy {}",
            quiet.occ.overall()
        );
        // a tone in the third slice of the usable band, 40 dB over the floor
        w.push(&tone(65_536, -150, 1500.0, 200.0, 7), &p);
        let busy = w.take().unwrap();
        let slices: Vec<f64> = (0..8).map(|i| busy.occ.slice(i)).collect();
        let loud = slices.iter().cloned().fold(0.0, f64::max);
        assert!(loud > 0.0, "{slices:?}");
        assert!(
            slices.iter().filter(|&&s| s > 0.01).count() <= 2,
            "{slices:?}"
        );
        assert!(busy.occ.peak_db > busy.occ.floor_db() + 30.0);
    }

    #[test]
    fn occupancy_covers_the_usable_bandwidth_only() {
        let p = Params {
            usable: 0.5,
            ..params(4)
        };
        let mut w = Welch::new(&p).unwrap();
        w.push(&tone(8192, 0, 0.0, 100.0, 5), &p);
        let s = w.take().unwrap();
        // 512 usable bins in 4 slices of 128, one cell per bin per classified frame
        assert!(
            s.occ.total.iter().all(|&t| t == 128 * s.occ.frames),
            "{:?}",
            s.occ.total
        );
    }

    #[test]
    fn decimation_keeps_the_peak_level_of_a_narrow_tone_within_the_bin_ratio() {
        let mut bins = vec![-90.0f32; 1024];
        bins[300] = -10.0;
        let d = decimate_db(&bins, 256);
        assert_eq!(d.len(), 256);
        // one -10 dB bin averaged with three at -90 reads about 6 dB lower
        assert!((d[75] + 16.0).abs() < 0.2, "{}", d[75]);
        assert!((d[0] + 90.0).abs() < 1e-3);
        assert_eq!(decimate_db(&bins, 2048).len(), 1024);
    }

    #[test]
    fn occupancy_merges_by_summing_cells() {
        let mut a = Occupancy {
            busy: vec![1, 2],
            total: vec![10, 10],
            floor_db_sum: -100.0,
            frames: 2,
            peak_db: -20.0,
        };
        let b = Occupancy {
            busy: vec![3, 0],
            total: vec![10, 10],
            floor_db_sum: -50.0,
            frames: 1,
            peak_db: -10.0,
        };
        a.merge(&b);
        assert_eq!(
            (a.busy.clone(), a.total.clone(), a.frames, a.peak_db),
            (vec![4, 2], vec![20, 20], 3, -10.0)
        );
        assert!((a.overall() - 0.15).abs() < 1e-9);
        assert!((a.floor_db() + 50.0).abs() < 1e-9);
    }
}
