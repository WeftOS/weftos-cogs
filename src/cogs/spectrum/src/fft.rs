//! A small in-place radix-2 complex FFT on f32 pairs, with the twiddles and the bit-reversal
//! table computed once. Written here so the cog has no dependency for it; checked against a
//! direct DFT and against known tones in the tests.

pub struct Fft {
    n: usize,
    rev: Vec<u32>,
    /// `cos, -sin` of `2 pi k / n` for k in 0..n/2.
    tw: Vec<(f32, f32)>,
}

impl Fft {
    /// `n` must be a power of two, at least 2.
    pub fn new(n: usize) -> Result<Fft, String> {
        if n < 2 || !n.is_power_of_two() {
            return Err(format!("FFT size {n} is not a power of two"));
        }
        let bits = n.trailing_zeros();
        let rev = (0..n as u32)
            .map(|i| i.reverse_bits() >> (32 - bits))
            .collect();
        let tw = (0..n / 2)
            .map(|k| {
                let a = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .collect();
        Ok(Fft { n, rev, tw })
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.n
    }

    /// Forward transform of `re`, `im` (both of length `n`), in place.
    pub fn forward(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.n;
        assert!(
            re.len() == n && im.len() == n,
            "FFT buffers must have length {n}"
        );
        for i in 0..n {
            let j = self.rev[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = self.tw[k * step];
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len <<= 1;
        }
    }
}

/// A Hann window of length `n` and the sum of its squares.
pub fn hann(n: usize) -> (Vec<f32>, f64) {
    let w: Vec<f32> = (0..n)
        .map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()) as f32)
        .collect();
    let s2 = w.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    (w, s2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_a_direct_dft() {
        let n = 16;
        let f = Fft::new(n).unwrap();
        let x: Vec<(f32, f32)> = (0..n)
            .map(|i| {
                (
                    ((i * 7 + 3) % 11) as f32 - 5.0,
                    ((i * 5 + 1) % 9) as f32 - 4.0,
                )
            })
            .collect();
        let mut re: Vec<f32> = x.iter().map(|p| p.0).collect();
        let mut im: Vec<f32> = x.iter().map(|p| p.1).collect();
        f.forward(&mut re, &mut im);
        for k in 0..n {
            let (mut sr, mut si) = (0.0f64, 0.0f64);
            for (t, p) in x.iter().enumerate() {
                let a = -2.0 * std::f64::consts::PI * (k * t) as f64 / n as f64;
                sr += f64::from(p.0) * a.cos() - f64::from(p.1) * a.sin();
                si += f64::from(p.0) * a.sin() + f64::from(p.1) * a.cos();
            }
            assert!(
                (f64::from(re[k]) - sr).abs() < 1e-3 && (f64::from(im[k]) - si).abs() < 1e-3,
                "bin {k}"
            );
        }
    }

    #[test]
    fn a_complex_tone_lands_in_its_bin() {
        let n = 1024;
        let f = Fft::new(n).unwrap();
        for bin in [0usize, 1, 100, 511, 512, 1000] {
            let mut re: Vec<f32> = (0..n)
                .map(|t| (2.0 * std::f64::consts::PI * (bin * t) as f64 / n as f64).cos() as f32)
                .collect();
            let mut im: Vec<f32> = (0..n)
                .map(|t| (2.0 * std::f64::consts::PI * (bin * t) as f64 / n as f64).sin() as f32)
                .collect();
            f.forward(&mut re, &mut im);
            let peak = (0..n)
                .max_by(|&a, &b| (re[a].hypot(im[a])).total_cmp(&re[b].hypot(im[b])))
                .unwrap();
            assert_eq!(peak, bin);
            assert!((re[peak].hypot(im[peak]) - n as f32).abs() < 0.5);
        }
    }

    #[test]
    fn sizes_must_be_powers_of_two() {
        assert!(Fft::new(1000).is_err());
        assert!(Fft::new(1).is_err());
        assert_eq!(Fft::new(8).unwrap().len(), 8);
    }

    #[test]
    fn the_hann_window_sums_as_expected() {
        let (w, s2) = hann(1024);
        assert!((s2 - 1024.0 * 0.375).abs() < 0.5);
        assert!(w[0] < 1e-6 && (w[512] - 1.0).abs() < 1e-6);
    }
}
