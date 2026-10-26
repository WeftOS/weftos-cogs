//! Sample-rate conversion to the stream's 16 kHz, and float to s16le.
//!
//! Windowed-sinc (Hann) interpolation, with the cutoff lowered when going down so nothing above
//! the new Nyquist folds back. Direct convolution: speech is a few seconds, so cost is nothing.

const HALF_TAPS: f64 = 16.0;

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-9 {
        1.0
    } else {
        (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
    }
}

/// Convert `input` from `from` Hz to `to` Hz.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let cutoff = (to as f64 / from as f64).min(1.0);
    let w = HALF_TAPS / cutoff;
    let n_out = (input.len() as f64 / ratio) as usize;
    let mut out = Vec::with_capacity(n_out);
    for n in 0..n_out {
        let pos = n as f64 * ratio;
        let c = pos.floor() as isize;
        let (lo, hi) = (
            (c - w.ceil() as isize + 1).max(0),
            (c + w.ceil() as isize).min(input.len() as isize - 1),
        );
        let mut acc = 0.0f64;
        for k in lo..=hi {
            let x = pos - k as f64;
            if x.abs() >= w {
                continue;
            }
            let window = 0.5 * (1.0 + (std::f64::consts::PI * x / w).cos());
            acc += input[k as usize] as f64 * cutoff * sinc(cutoff * x) * window;
        }
        out.push(acc as f32);
    }
    out
}

/// Float samples in [-1, 1] to s16le bytes, clipped.
pub fn to_s16le(x: &[f32]) -> Vec<u8> {
    x.iter()
        .flat_map(|v| ((v.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64, rate: u32, secs: f64) -> Vec<f32> {
        (0..(rate as f64 * secs) as usize)
            .map(|i| (i as f64 / rate as f64 * hz * std::f64::consts::TAU).sin() as f32 * 0.5)
            .collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    fn crossings(x: &[f32]) -> usize {
        x.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count()
    }

    #[test]
    fn lengths_follow_the_ratio() {
        assert_eq!(resample(&vec![0.0; 24_000], 24_000, 16_000).len(), 16_000);
        assert_eq!(resample(&vec![0.0; 22_050], 22_050, 16_000).len(), 16_000);
        assert_eq!(resample(&[1.0, 2.0], 16_000, 16_000), [1.0, 2.0]);
        assert!(resample(&[], 24_000, 16_000).is_empty());
    }

    #[test]
    fn a_tone_keeps_its_pitch_and_level() {
        for rate in [24_000u32, 22_050] {
            let out = resample(&tone(1000.0, rate, 1.0), rate, 16_000);
            let mid = &out[400..out.len() - 400];
            assert!(
                (crossings(mid) as f64 - 1000.0 * mid.len() as f64 / 16_000.0).abs() <= 2.0,
                "{rate}: {}",
                crossings(mid)
            );
            assert!(
                (rms(mid) - 0.5 / 2f64.sqrt()).abs() < 0.01,
                "{rate}: {}",
                rms(mid)
            );
        }
    }

    #[test]
    fn what_cannot_exist_at_16k_is_removed_not_folded_back() {
        let out = resample(&tone(10_000.0, 24_000, 1.0), 24_000, 16_000);
        let mid = &out[400..out.len() - 400];
        assert!(
            rms(mid) < 0.5 / 2f64.sqrt() * 0.03,
            "10 kHz leaked: {}",
            rms(mid)
        );
    }

    #[test]
    fn s16_conversion_clips_and_rounds() {
        assert_eq!(
            to_s16le(&[0.0, 1.0, -1.0, 2.0, -3.0, 0.5]),
            [0, 0, 0xff, 0x7f, 0x01, 0x80, 0xff, 0x7f, 0x01, 0x80, 0x00, 0x40]
        );
    }
}
