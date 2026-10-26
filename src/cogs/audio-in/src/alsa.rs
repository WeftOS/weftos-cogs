//! Which ALSA capture device the cog opens, how it is read, and the chunk level.
//!
//! The device comes from the node: the host hands the owner `WEFTOS_DEVICES`
//! (`:`-separated, ADR-122 4b), and the capture nodes in it (`/dev/snd/pcmC<c>D<d>c`)
//! become `plughw:<c>,<d>`. `--device` names one by hand for a bench run. The cog
//! opens nothing it was not handed or told.

use std::io::Read;
use std::process::{Child, Command, Stdio};

pub const RATE_HZ: u32 = 16_000;
pub const CHUNK_MS: u32 = 20;
/// Bytes of one chunk: 16 kHz x 20 ms x 2 bytes, mono.
pub const CHUNK_BYTES: usize = (RATE_HZ * CHUNK_MS / 1000 * 2) as usize;

/// `/dev/snd/pcmC1D0c` -> `(1, 0)`.
pub fn capture_node(path: &str) -> Option<(u32, u32)> {
    let rest = path.strip_prefix("/dev/snd/pcmC")?.strip_suffix('c')?;
    let (c, d) = rest.split_once('D')?;
    Some((c.parse().ok()?, d.parse().ok()?))
}

/// Card number to the name `/proc/asound/cards` gives it.
fn card_names(cards: &str) -> Vec<(u32, String)> {
    cards
        .lines()
        .filter_map(|l| {
            let (n, rest) = l.trim_start().split_once(' ')?;
            Some((n.parse().ok()?, rest.to_string()))
        })
        .collect()
}

/// Pick the ALSA device from the handed capture nodes. `want` is a substring of the
/// card's name in `/proc/asound/cards`; without it the lowest card is used.
pub fn pick_capture(devices: &[String], cards: &str, want: Option<&str>) -> Result<String, String> {
    let mut nodes: Vec<(u32, u32)> = devices.iter().filter_map(|d| capture_node(d)).collect();
    nodes.sort();
    if nodes.is_empty() {
        return Err(
            "no capture node in WEFTOS_DEVICES (the host hands the owner /dev/snd/pcmC*D*c)".into(),
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
            "no handed capture card matches {:?}; cards: {}",
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
    pub clipped: u32,
}

pub fn level(chunk: &[u8]) -> Level {
    let (mut sum, mut peak, mut clipped, mut n) = (0f64, 0f64, 0u32, 0u32);
    for s in chunk.chunks_exact(2) {
        let v = i16::from_le_bytes([s[0], s[1]]) as f64;
        sum += v * v;
        peak = peak.max(v.abs());
        clipped += (v.abs() >= 32767.0) as u32;
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
        clipped,
    }
}

/// Chunks from `arecord`; it dies with this value.
pub struct Arecord {
    child: Child,
}

impl Arecord {
    pub fn start(dev: &str) -> Result<Self, String> {
        let child = Command::new("arecord")
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
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("arecord: {e}"))?;
        Ok(Self { child })
    }

    /// Read exactly one chunk; `Ok(false)` when the capture ended.
    pub fn read_chunk(&mut self, buf: &mut [u8; CHUNK_BYTES]) -> Result<bool, String> {
        let out = self.child.stdout.as_mut().ok_or("no arecord pipe")?;
        match out.read_exact(buf) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
            Err(e) => Err(format!("arecord read: {e}")),
        }
    }
}

impl Drop for Arecord {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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

/// A 440 Hz tone with a little noise, `n` chunks in, for `--simulate`.
pub fn synth_chunk(n: u64) -> [u8; CHUNK_BYTES] {
    let mut out = [0u8; CHUNK_BYTES];
    let mut x = 0x2545_F491_4F6C_DD1Du64 ^ n;
    for (i, s) in out.chunks_exact_mut(2).enumerate() {
        let t = (n * (CHUNK_BYTES as u64 / 2) + i as u64) as f64 / RATE_HZ as f64;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let noise = ((x % 200) as f64) - 100.0;
        let v = (t * 440.0 * std::f64::consts::TAU).sin() * 8000.0 + noise;
        s.copy_from_slice(&(v as i16).to_le_bytes());
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
    fn capture_nodes_parse_and_playback_nodes_do_not() {
        assert_eq!(capture_node("/dev/snd/pcmC1D0c"), Some((1, 0)));
        assert_eq!(capture_node("/dev/snd/pcmC10D2c"), Some((10, 2)));
        assert_eq!(capture_node("/dev/snd/pcmC1D0p"), None);
        assert_eq!(capture_node("/dev/video0"), None);
    }

    #[test]
    fn the_device_is_picked_from_what_the_host_handed() {
        let d = devs(&[
            "/dev/snd/pcmC1D0c",
            "/dev/snd/pcmC0D1c",
            "/dev/snd/pcmC0D0p",
        ]);
        assert_eq!(
            pick_capture(&d, CARDS, None).unwrap(),
            "plughw:0,1",
            "the lowest card"
        );
        assert_eq!(
            pick_capture(&d, CARDS, Some("u20cam")).unwrap(),
            "plughw:1,0",
            "by card name, any case"
        );
        let e = pick_capture(&d, CARDS, Some("nope")).unwrap_err();
        assert!(e.contains("no handed capture card matches"), "{e}");
        let e = pick_capture(&devs(&["/dev/snd/pcmC0D0p"]), CARDS, None).unwrap_err();
        assert!(e.contains("no capture node"), "{e}");
    }

    #[test]
    fn chunk_level_is_in_dbfs() {
        let silent = level(&[0u8; CHUNK_BYTES]);
        assert_eq!(
            (silent.rms_dbfs, silent.peak_dbfs, silent.clipped),
            (-120.0, -120.0, 0)
        );
        let full: Vec<u8> = std::iter::repeat_n(i16::MAX.to_le_bytes(), CHUNK_BYTES / 2)
            .flatten()
            .collect();
        let l = level(&full);
        assert!(l.rms_dbfs > -0.01 && l.peak_dbfs > -0.01, "{l:?}");
        assert_eq!(l.clipped as usize, CHUNK_BYTES / 2);
        let half: Vec<u8> = std::iter::repeat_n(16384i16.to_le_bytes(), CHUNK_BYTES / 2)
            .flatten()
            .collect();
        assert!((level(&half).rms_dbfs + 6.02).abs() < 0.01);
    }

    #[test]
    fn wav_files_are_unwrapped_to_pcm() {
        let mut wav = b"RIFF\x00\x00\x00\x00WAVEfmt \x10\x00\x00\x00".to_vec();
        wav.extend_from_slice(&[1, 0, 1, 0, 0x80, 0x3e, 0, 0, 0, 0x7d, 0, 0, 2, 0, 16, 0]);
        wav.extend_from_slice(b"data\x04\x00\x00\x00\x01\x02\x03\x04");
        assert_eq!(pcm_from_file(&wav), [1, 2, 3, 4]);
        assert_eq!(
            pcm_from_file(&[9, 9]),
            [9, 9],
            "a headerless file is raw pcm"
        );
    }

    #[test]
    fn the_synthetic_tone_is_audible_and_marked_by_the_caller() {
        let l = level(&synth_chunk(3));
        assert!(l.rms_dbfs > -20.0 && l.rms_dbfs < -6.0, "{l:?}");
    }
}
