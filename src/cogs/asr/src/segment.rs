//! Cuts the 20 ms chunk stream into utterances by energy.
//!
//! The noise floor follows the quietest recent chunk (it falls at once and rises slowly), and
//! speech is anything `margin_db` above it and above `min_db`. An utterance starts at the first
//! speech chunk (with `preroll_ms` of the chunks before it), ends after `hang_ms` of quiet, and
//! is dropped if shorter than `min_ms`; one longer than `max_ms` is cut there and a new one
//! starts. This is the first stage only; the transcript decides whether it was speech.

use std::collections::VecDeque;

pub const CHUNK_MS: u64 = 20;
pub const CHUNK_BYTES: usize = 640;

#[derive(Debug, Clone, PartialEq)]
pub struct Params {
    pub margin_db: f64,
    pub min_db: f64,
    pub hang_ms: u64,
    pub min_ms: u64,
    pub max_ms: u64,
    pub preroll_ms: u64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            margin_db: 10.0,
            min_db: -45.0,
            hang_ms: 600,
            min_ms: 300,
            max_ms: 15_000,
            preroll_ms: 240,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Utterance {
    pub pcm: Vec<u8>,
    pub start_ns: u64,
    pub end_ns: u64,
    pub peak_db: f64,
}

impl Utterance {
    pub fn ms(&self) -> u64 {
        (self.pcm.len() / 2) as u64 * 1000 / 16_000
    }
}

/// RMS of one chunk in dBFS (-120 for silence).
pub fn rms_db(chunk: &[u8]) -> f64 {
    let (mut sum, mut n) = (0f64, 0u32);
    for s in chunk.chunks_exact(2) {
        let v = i16::from_le_bytes([s[0], s[1]]) as f64;
        sum += v * v;
        n += 1;
    }
    if n == 0 || sum == 0.0 {
        return -120.0;
    }
    (10.0 * (sum / n as f64 / (32768.0 * 32768.0)).log10()).max(-120.0)
}

pub struct Segmenter {
    p: Params,
    floor_db: f64,
    preroll: VecDeque<(Vec<u8>, u64)>,
    cur: Option<Cur>,
    quiet_ms: u64,
    seen: u64,
}

/// Chunks (1 s) spent learning the room before any speech is declared.
const WARMUP: u64 = 50;

struct Cur {
    pcm: Vec<u8>,
    start_ns: u64,
    last_ns: u64,
    peak_db: f64,
    speech_ms: u64,
}

impl Segmenter {
    pub fn new(p: Params) -> Self {
        Self {
            p,
            floor_db: -60.0,
            preroll: VecDeque::new(),
            cur: None,
            quiet_ms: 0,
            seen: 0,
        }
    }

    pub fn floor_db(&self) -> f64 {
        self.floor_db
    }

    pub fn speaking(&self) -> bool {
        self.cur.is_some()
    }

    fn threshold(&self) -> f64 {
        (self.floor_db + self.p.margin_db).max(self.p.min_db)
    }

    fn finish(&mut self, force: bool) -> Option<Utterance> {
        let c = self.cur.take()?;
        self.quiet_ms = 0;
        let u = Utterance {
            pcm: c.pcm,
            start_ns: c.start_ns,
            end_ns: c.last_ns + CHUNK_MS * 1_000_000,
            peak_db: c.peak_db,
        };
        (force || c.speech_ms >= self.p.min_ms).then_some(u)
    }

    /// Feed one chunk captured at `t_ns`; returns a finished utterance, if any.
    pub fn feed(&mut self, chunk: &[u8], t_ns: u64) -> Option<Utterance> {
        let db = rms_db(chunk);
        if self.seen < WARMUP {
            // The mean level of the first second is the room.
            self.floor_db = if self.seen == 0 {
                db
            } else {
                self.floor_db + (db - self.floor_db) / (self.seen + 1) as f64
            };
            self.seen += 1;
            return None;
        }
        if db < self.floor_db {
            self.floor_db = db.max(-90.0);
        } else if self.cur.is_none() {
            self.floor_db = (self.floor_db + 0.05).min(db);
        }
        let speech = db >= self.threshold();
        match self.cur.as_mut() {
            None => {
                if speech {
                    let mut pcm = Vec::new();
                    let mut start_ns = t_ns;
                    for (c, t) in self.preroll.drain(..) {
                        start_ns = start_ns.min(t);
                        pcm.extend_from_slice(&c);
                    }
                    pcm.extend_from_slice(chunk);
                    self.cur = Some(Cur {
                        pcm,
                        start_ns,
                        last_ns: t_ns,
                        peak_db: db,
                        speech_ms: CHUNK_MS,
                    });
                    self.quiet_ms = 0;
                } else {
                    self.preroll.push_back((chunk.to_vec(), t_ns));
                    while self.preroll.len() as u64 * CHUNK_MS > self.p.preroll_ms {
                        self.preroll.pop_front();
                    }
                }
                None
            }
            Some(c) => {
                c.pcm.extend_from_slice(chunk);
                c.last_ns = t_ns;
                c.peak_db = c.peak_db.max(db);
                if speech {
                    c.speech_ms += CHUNK_MS;
                    self.quiet_ms = 0;
                } else {
                    self.quiet_ms += CHUNK_MS;
                }
                let long = (c.pcm.len() / 2) as u64 * 1000 / 16_000 >= self.p.max_ms;
                if self.quiet_ms >= self.p.hang_ms {
                    self.finish(false)
                } else if long {
                    self.finish(true)
                } else {
                    None
                }
            }
        }
    }

    /// The stream ended: what is being spoken is an utterance.
    pub fn flush(&mut self) -> Option<Utterance> {
        self.finish(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(amp: f64) -> Vec<u8> {
        (0..320)
            .map(|i| (((i as f64) * 0.3).sin() * amp) as i16)
            .flat_map(i16::to_le_bytes)
            .collect()
    }

    fn feed_n(s: &mut Segmenter, chunk: &[u8], n: usize, t: &mut u64) -> Vec<Utterance> {
        let mut out = Vec::new();
        for _ in 0..n {
            if let Some(u) = s.feed(chunk, *t) {
                out.push(u);
            }
            *t += CHUNK_MS * 1_000_000;
        }
        out
    }

    #[test]
    fn rms_is_in_dbfs() {
        assert_eq!(rms_db(&[0u8; 640]), -120.0);
        let half: Vec<u8> = std::iter::repeat_n(16384i16.to_le_bytes(), 320)
            .flatten()
            .collect();
        assert!((rms_db(&half) + 6.02).abs() < 0.01);
    }

    #[test]
    fn a_burst_between_silences_is_one_utterance_with_preroll_and_hangover() {
        let mut s = Segmenter::new(Params::default());
        let (quiet, loud) = (tone(30.0), tone(8000.0));
        let mut t = 0;
        assert!(feed_n(&mut s, &quiet, 100, &mut t).is_empty());
        assert!(
            feed_n(&mut s, &loud, 50, &mut t).is_empty(),
            "no end yet while it is loud"
        );
        assert!(s.speaking());
        let got = feed_n(&mut s, &quiet, 40, &mut t);
        assert_eq!(got.len(), 1, "ends after the hangover");
        let u = &got[0];
        // 12 preroll + 50 loud + 30 hangover chunks (600 ms) = 92 chunks.
        assert_eq!(u.pcm.len(), 92 * 640);
        assert_eq!(u.ms(), 92 * 20);
        assert!(u.start_ns < 100 * 20_000_000 + 1_000_000_000 && u.end_ns > u.start_ns);
        assert!(u.peak_db > -20.0);
        assert!(!s.speaking());
    }

    #[test]
    fn a_click_shorter_than_the_minimum_is_dropped() {
        let mut s = Segmenter::new(Params::default());
        let (quiet, loud) = (tone(30.0), tone(8000.0));
        let mut t = 0;
        feed_n(&mut s, &quiet, 100, &mut t);
        feed_n(&mut s, &loud, 5, &mut t);
        assert!(feed_n(&mut s, &quiet, 40, &mut t).is_empty());
    }

    #[test]
    fn a_pause_shorter_than_the_hangover_does_not_split_an_utterance() {
        let mut s = Segmenter::new(Params::default());
        let (quiet, loud) = (tone(30.0), tone(8000.0));
        let mut t = 0;
        feed_n(&mut s, &quiet, 100, &mut t);
        feed_n(&mut s, &loud, 20, &mut t);
        feed_n(&mut s, &quiet, 15, &mut t);
        feed_n(&mut s, &loud, 20, &mut t);
        assert_eq!(feed_n(&mut s, &quiet, 40, &mut t).len(), 1);
    }

    #[test]
    fn a_long_utterance_is_cut_at_the_maximum() {
        let mut s = Segmenter::new(Params {
            max_ms: 1000,
            ..Params::default()
        });
        let (quiet, loud) = (tone(30.0), tone(8000.0));
        let mut t = 0;
        feed_n(&mut s, &quiet, 100, &mut t);
        let got = feed_n(&mut s, &loud, 120, &mut t);
        assert!(got.len() >= 2, "{}", got.len());
        assert!(got.iter().all(|u| u.ms() <= 1100));
    }

    #[test]
    fn the_floor_follows_the_room_and_raises_the_bar() {
        let mut s = Segmenter::new(Params::default());
        let noisy = tone(600.0);
        let mut t = 0;
        feed_n(&mut s, &noisy, 200, &mut t);
        assert!(s.floor_db() > -40.0, "{}", s.floor_db());
        assert!(
            feed_n(&mut s, &noisy, 100, &mut t).is_empty(),
            "steady noise is not speech"
        );
        assert!(!s.speaking());
    }

    #[test]
    fn flush_returns_what_is_being_spoken() {
        let mut s = Segmenter::new(Params::default());
        let (quiet, loud) = (tone(30.0), tone(8000.0));
        let mut t = 0;
        feed_n(&mut s, &quiet, 50, &mut t);
        feed_n(&mut s, &loud, 30, &mut t);
        assert!(s.flush().is_some());
        assert!(s.flush().is_none());
    }
}
