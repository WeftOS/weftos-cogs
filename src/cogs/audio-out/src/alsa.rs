//! Which ALSA playback device the cog opens, how it is fed, and the chunk level.
//!
//! The device comes from the node: the host hands the owner `WEFTOS_DEVICES`
//! (`:`-separated, ADR-122 4b), and the playback nodes in it (`/dev/snd/pcmC<c>D<d>p`)
//! become `plughw:<c>,<d>`. `--device` names one by hand for a bench run. The cog
//! opens nothing it was not handed or told.

use std::io::Write;
use std::process::{Child, Command, Stdio};

pub const RATE_HZ: u32 = 16_000;
pub const CHUNK_MS: u32 = 20;
/// Bytes of one chunk: 16 kHz x 20 ms x 2 bytes, mono.
pub const CHUNK_BYTES: usize = (RATE_HZ * CHUNK_MS / 1000 * 2) as usize;

/// `/dev/snd/pcmC0D1p` -> `(0, 1)`.
pub fn playback_node(path: &str) -> Option<(u32, u32)> {
    let rest = path.strip_prefix("/dev/snd/pcmC")?.strip_suffix('p')?;
    let (c, d) = rest.split_once('D')?;
    Some((c.parse().ok()?, d.parse().ok()?))
}

fn card_names(cards: &str) -> Vec<(u32, String)> {
    cards
        .lines()
        .filter_map(|l| {
            let (n, rest) = l.trim_start().split_once(' ')?;
            Some((n.parse().ok()?, rest.to_string()))
        })
        .collect()
}

/// Pick the ALSA device from the handed playback nodes. `want` is a substring of the
/// card's name in `/proc/asound/cards`; without it the lowest card is used.
pub fn pick_playback(
    devices: &[String],
    cards: &str,
    want: Option<&str>,
) -> Result<String, String> {
    let mut nodes: Vec<(u32, u32)> = devices.iter().filter_map(|d| playback_node(d)).collect();
    nodes.sort();
    if nodes.is_empty() {
        return Err(
            "no playback node in WEFTOS_DEVICES (the host hands the owner /dev/snd/pcmC*D*p)"
                .into(),
        );
    }
    let names = card_names(cards);
    let hit = match want {
        None => nodes.first().copied(),
        Some(w) => {
            let w = w.to_lowercase();
            nodes.iter().copied().find(|(c, _)| {
                names
                    .iter()
                    .any(|(n, name)| n == c && name.to_lowercase().contains(&w))
            })
        }
    };
    hit.map(|(c, d)| format!("plughw:{c},{d}")).ok_or_else(|| {
        format!(
            "no handed playback card matches {:?}; cards: {}",
            want.unwrap_or(""),
            cards.trim().replace('\n', " | ")
        )
    })
}

/// Level of one chunk of s16le samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    pub rms_dbfs: f64,
    pub peak_dbfs: f64,
}

pub fn level(chunk: &[u8]) -> Level {
    let (mut sum, mut peak, mut n) = (0f64, 0f64, 0u32);
    for s in chunk.chunks_exact(2) {
        let v = i16::from_le_bytes([s[0], s[1]]) as f64;
        sum += v * v;
        peak = peak.max(v.abs());
        n += 1;
    }
    let db = |x: f64| {
        if x <= 0.0 {
            -120.0
        } else {
            (20.0 * (x / 32768.0).log10()).max(-120.0)
        }
    };
    Level {
        rms_dbfs: db(if n == 0 { 0.0 } else { (sum / n as f64).sqrt() }),
        peak_dbfs: db(peak),
    }
}

/// Where played chunks go.
pub enum Sink {
    /// `aplay` reading raw PCM on its stdin; it dies with this value.
    Aplay(Child, String),
    /// Counts and discards (`--simulate`).
    Null,
}

impl Sink {
    pub fn aplay(dev: &str) -> Result<Self, String> {
        let child = Command::new("aplay")
            .args([
                "-q",
                "-D",
                dev,
                "-t",
                "raw",
                "-f",
                "S16_LE",
                "-r",
                &RATE_HZ.to_string(),
                "-c",
                "1",
                "-",
            ])
            .stdin(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("aplay: {e}"))?;
        Ok(Sink::Aplay(child, dev.to_string()))
    }

    pub fn label(&self) -> String {
        match self {
            Sink::Aplay(_, d) => format!("alsa:{d}"),
            Sink::Null => "null".into(),
        }
    }

    /// Hand one chunk to the device; blocks while the device's buffer is full, which is
    /// what keeps playback at real time.
    pub fn write(&mut self, chunk: &[u8]) -> Result<(), String> {
        match self {
            Sink::Aplay(c, _) => c
                .stdin
                .as_mut()
                .ok_or("no aplay pipe")?
                .write_all(chunk)
                .map_err(|e| format!("aplay write: {e}")),
            Sink::Null => Ok(()),
        }
    }

    /// Close the pipe and let the device drain.
    pub fn finish(mut self) {
        if let Sink::Aplay(c, _) = &mut self {
            drop(c.stdin.take());
            let _ = c.wait();
        }
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        if let Sink::Aplay(c, _) = self {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Raw s16le from a WAV (header skipped to the `data` chunk) or a headerless file.
pub fn pcm_from_file(bytes: &[u8]) -> Vec<u8> {
    if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        let mut i = 12;
        while i + 8 <= bytes.len() {
            let n = u32::from_le_bytes([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]])
                as usize;
            if &bytes[i..i + 4] == b"data" {
                return bytes[i + 8..(i + 8 + n).min(bytes.len())].to_vec();
            }
            i += 8 + n + (n & 1);
        }
        return Vec::new();
    }
    bytes.to_vec()
}

/// A 440 Hz test tone, chunk `n` of it.
pub fn tone_chunk(n: u64) -> [u8; CHUNK_BYTES] {
    let mut out = [0u8; CHUNK_BYTES];
    for (i, s) in out.chunks_exact_mut(2).enumerate() {
        let t = (n * (CHUNK_BYTES as u64 / 2) + i as u64) as f64 / RATE_HZ as f64;
        s.copy_from_slice(
            &(((t * 440.0 * std::f64::consts::TAU).sin() * 8000.0) as i16).to_le_bytes(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARDS: &str = " 0 [Codec         ]: sc8280xp - SC8280XP\n                      Ventuno\n 1 [U20CAM         ]: USB-Audio - U20CAM-1080p\n                      Innomaker U20CAM-1080p at usb-0000:01:00.0-1\n";

    fn devs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn playback_nodes_parse_and_capture_nodes_do_not() {
        assert_eq!(playback_node("/dev/snd/pcmC0D1p"), Some((0, 1)));
        assert_eq!(playback_node("/dev/snd/pcmC1D0c"), None);
        assert_eq!(playback_node("/dev/snd/controlC0"), None);
    }

    #[test]
    fn the_device_is_picked_from_what_the_host_handed() {
        let d = devs(&[
            "/dev/snd/pcmC1D0p",
            "/dev/snd/pcmC0D1p",
            "/dev/snd/pcmC0D0c",
        ]);
        assert_eq!(pick_playback(&d, CARDS, None).unwrap(), "plughw:0,1");
        assert_eq!(pick_playback(&d, CARDS, Some("u20")).unwrap(), "plughw:1,0");
        assert!(pick_playback(&d, CARDS, Some("nope"))
            .unwrap_err()
            .contains("no handed playback card matches"));
        assert!(pick_playback(&devs(&["/dev/snd/pcmC0D0c"]), CARDS, None)
            .unwrap_err()
            .contains("no playback node"));
    }

    #[test]
    fn chunk_level_is_in_dbfs() {
        assert_eq!(
            level(&[0u8; CHUNK_BYTES]),
            Level {
                rms_dbfs: -120.0,
                peak_dbfs: -120.0
            }
        );
        let half: Vec<u8> = std::iter::repeat_n(16384i16.to_le_bytes(), CHUNK_BYTES / 2)
            .flatten()
            .collect();
        assert!((level(&half).rms_dbfs + 6.02).abs() < 0.01);
        let t = level(&tone_chunk(2));
        assert!(t.rms_dbfs > -17.0 && t.rms_dbfs < -14.0, "{t:?}");
    }

    #[test]
    fn wav_files_are_unwrapped_to_pcm() {
        let mut wav = b"RIFF\x00\x00\x00\x00WAVEfmt \x10\x00\x00\x00".to_vec();
        wav.extend_from_slice(&[1, 0, 1, 0, 0x80, 0x3e, 0, 0, 0, 0x7d, 0, 0, 2, 0, 16, 0]);
        wav.extend_from_slice(b"data\x04\x00\x00\x00\x01\x02\x03\x04");
        assert_eq!(pcm_from_file(&wav), [1, 2, 3, 4]);
        assert_eq!(pcm_from_file(&[9, 9]), [9, 9]);
    }

    #[test]
    fn the_null_sink_accepts_chunks() {
        let mut s = Sink::Null;
        s.write(&[0u8; CHUNK_BYTES]).unwrap();
        assert_eq!(s.label(), "null");
        s.finish();
    }
}
