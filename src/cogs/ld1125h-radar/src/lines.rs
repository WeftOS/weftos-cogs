//! ASCII line parser for the HLK-LD1125H-24G.
//!
//! Hi-Link, "Human Presence Detection Radar HLK-LD1125H-24G Application Manual",
//! V1.2, 2021-9-16, section "Radar output": a larger motion is the line
//! `mov, dis=***`; a still body or a small motion is `occ, dis=***`. `dis` is
//! the target distance in metres. The asterisks are the manual's placeholder,
//! not a worked numeric example. The manual defines no checksum.
//!
//! The manual does not name the byte that ends a report line. Commands are
//! specified to need CR+LF. This parser treats LF as the end of a line and
//! strips one preceding CR. That terminator is an assumption, stated in ADR-172.

/// Longest accepted line, not counting the LF. A longer line is rejected and
/// does not stay in the buffer. Noise with no LF is dropped one byte at a time
/// once the buffer passes this, so the buffer cannot grow without bound.
pub const MAX_LINE: usize = 128;

/// Refusal bound for `dis`, metres. Not a datasheet maximum. The manual's
/// longest discussed figure is "above 8 m" (the third threshold segment) and
/// the default `rmax` is 6. Values outside `(0, MAX_RANGE_M]` are not readings.
pub const MAX_RANGE_M: f64 = 100.0;

/// Which documented line the module sent. These words are the module's, not a
/// person and not a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Larger motion. Manual text: `mov`.
    Mov,
    /// Still, or a small motion. Manual text: `occ`.
    Occ,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Mov => "mov",
            Kind::Occ => "occ",
        }
    }
}

/// One accepted report line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    pub kind: Kind,
    pub distance_m: f64,
}

/// Cumulative parser counters. A window takes [`Stats::since`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub frames: u64,
    pub resync_bytes: u64,
    pub undecoded_lines: u64,
    pub rejected_lines: u64,
    /// The manual defines no checksum. This counter stays 0.
    pub checksum_errors: u64,
}

impl Stats {
    pub fn since(self, earlier: Stats) -> Stats {
        Stats {
            frames: self.frames.saturating_sub(earlier.frames),
            resync_bytes: self.resync_bytes.saturating_sub(earlier.resync_bytes),
            undecoded_lines: self.undecoded_lines.saturating_sub(earlier.undecoded_lines),
            rejected_lines: self.rejected_lines.saturating_sub(earlier.rejected_lines),
            checksum_errors: self.checksum_errors.saturating_sub(earlier.checksum_errors),
        }
    }
}

/// Long-lived accumulator. Frames split across reads stay in `buf`.
#[derive(Debug, Default)]
pub struct Parser {
    buf: Vec<u8>,
    pub stats: Stats,
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bytes still waiting for a newline. Tests use this to check a partial line.
    #[cfg(test)]
    pub fn pending(&self) -> &[u8] {
        &self.buf
    }

    /// Push a chunk and return every complete, accepted report line in it.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<Reading> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        loop {
            if let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
                let mut line: Vec<u8> = self.buf.drain(..=nl).collect();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                if line.len() > MAX_LINE {
                    self.stats.rejected_lines += 1;
                    self.stats.resync_bytes += line.len() as u64;
                    continue;
                }
                if line.iter().all(|b| b.is_ascii_whitespace()) {
                    continue;
                }
                match decode_line(&line) {
                    Decode::Reading(r) => {
                        self.stats.frames += 1;
                        out.push(r);
                    }
                    Decode::Undecoded => self.stats.undecoded_lines += 1,
                    Decode::Rejected => self.stats.rejected_lines += 1,
                }
            } else if self.buf.len() > MAX_LINE {
                // No newline, and longer than a legal line. Keep a report prefix
                // if one is in the tail; otherwise the bytes are noise and must
                // not stick to the next line.
                match report_prefix(&self.buf) {
                    Some(0) => {
                        self.buf.drain(..1);
                        self.stats.resync_bytes += 1;
                    }
                    Some(i) => {
                        self.stats.resync_bytes += i as u64;
                        self.buf.drain(..i);
                    }
                    None => {
                        self.stats.resync_bytes += self.buf.len() as u64;
                        self.buf.clear();
                    }
                }
            } else {
                break;
            }
        }
        out
    }
}

/// Index of `mov, dis=` or `occ, dis=` in `buf`, if either is present.
fn report_prefix(buf: &[u8]) -> Option<usize> {
    buf.windows(9)
        .position(|w| w == b"mov, dis=" || w == b"occ, dis=")
}

enum Decode {
    Reading(Reading),
    /// Not the documented `mov, dis=` / `occ, dis=` form. No meaning is assigned.
    Undecoded,
    /// The documented prefix, but the distance is not a finite metres value in range.
    Rejected,
}

/// `mov, dis=<decimal>` or `occ, dis=<decimal>`, nothing else on the line.
/// The manual prints one space after the comma and no space around `=`.
fn decode_line(line: &[u8]) -> Decode {
    let Ok(text) = std::str::from_utf8(line) else {
        return Decode::Undecoded;
    };
    let (kind, num) = if let Some(n) = text.strip_prefix("mov, dis=") {
        (Kind::Mov, n)
    } else if let Some(n) = text.strip_prefix("occ, dis=") {
        (Kind::Occ, n)
    } else {
        return Decode::Undecoded;
    };
    if !plain_decimal(num) {
        return Decode::Rejected;
    }
    let Ok(distance_m) = num.parse::<f64>() else {
        return Decode::Rejected;
    };
    if !distance_m.is_finite() || distance_m <= 0.0 || distance_m > MAX_RANGE_M {
        return Decode::Rejected;
    }
    Decode::Reading(Reading { kind, distance_m })
}

/// Digits, or digits, a dot, and digits. No sign, no exponent, no trailing junk.
fn plain_decimal(s: &str) -> bool {
    let mut parts = s.split('.');
    let Some(whole) = parts.next() else {
        return false;
    };
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    match parts.next() {
        None => true,
        Some(frac) => {
            !frac.is_empty() && frac.bytes().all(|b| b.is_ascii_digit()) && parts.next().is_none()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(bytes: &[u8]) -> (Vec<Reading>, Stats, Vec<u8>) {
        let mut p = Parser::new();
        let got = p.feed(bytes);
        (got, p.stats, p.buf.clone())
    }

    #[test]
    fn documented_grammar_with_a_synthetic_decimal() {
        // The manual's own text is `mov, dis=***` and `occ, dis=***`. It gives
        // no numeric worked example. The decimals below are synthetic stand-ins
        // for the placeholder, in the manual's spelling (comma, one space, `dis=`).
        let (got, stats, rest) = feed(b"mov, dis=1.25\nocc, dis=0.75\n");
        assert_eq!(
            got,
            vec![
                Reading {
                    kind: Kind::Mov,
                    distance_m: 1.25
                },
                Reading {
                    kind: Kind::Occ,
                    distance_m: 0.75
                }
            ]
        );
        assert!(rest.is_empty());
        assert_eq!(stats.frames, 2);
        assert_eq!(stats.checksum_errors, 0);
    }

    #[test]
    fn manual_placeholder_asterisks_are_not_a_distance() {
        let (got, stats, _) = feed(b"mov, dis=***\r\nocc, dis=***\n");
        assert!(got.is_empty());
        assert_eq!(stats.frames, 0);
        assert_eq!(stats.rejected_lines, 2);
        assert_eq!(stats.checksum_errors, 0);
    }

    #[test]
    fn crlf_and_integer_metres_are_accepted() {
        let (got, _, rest) = feed(b"mov, dis=2\r\n");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].distance_m, 2.0);
        assert!(rest.is_empty());
    }

    #[test]
    fn extra_tokens_and_wrong_spelling_are_not_decoded() {
        // `str` is named in the test_mode prose and is not given a grammar.
        // A suffix is refused rather than stored as a signal strength.
        let (got, stats, _) = feed(b"mov, dis=1.2, str=80\nMOV, dis=1.2\nmov,dis=1.2\n");
        assert!(got.is_empty());
        assert_eq!(stats.rejected_lines, 1, "prefix matched, suffix did not");
        assert_eq!(stats.undecoded_lines, 2);
        assert_eq!(stats.frames, 0);
    }

    #[test]
    fn non_finite_zero_and_oversize_distance_are_rejected() {
        let (got, stats, _) = feed(
            b"mov, dis=0\nmov, dis=0.0\nmov, dis=-1\nmov, dis=inf\nmov, dis=NaN\nmov, dis=101\nmov, dis=1e2\n",
        );
        assert!(got.is_empty());
        assert_eq!(stats.frames, 0);
        assert_eq!(stats.rejected_lines, 7);
    }

    #[test]
    fn garbage_line_then_a_good_line() {
        let (got, stats, _) = feed(b"noise\nmov, dis=3.5\n");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, Kind::Mov);
        assert_eq!(stats.undecoded_lines, 1);
        assert_eq!(stats.frames, 1);
    }

    #[test]
    fn partial_line_is_kept_until_the_newline() {
        let mut p = Parser::new();
        assert!(p.feed(b"mov, dis=1.").is_empty());
        assert_eq!(p.pending(), b"mov, dis=1.");
        let got = p.feed(b"5\n");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].distance_m, 1.5);
        assert!(p.pending().is_empty());
    }

    #[test]
    fn every_split_point_and_byte_at_a_time_match_the_bulk_parse() {
        let bulk = b"xx\nmov, dis=1.25\nocc, dis=0.75\n";
        let (want, _, _) = feed(bulk);
        assert_eq!(want.len(), 2);
        for i in 0..=bulk.len() {
            let mut p = Parser::new();
            let mut got = p.feed(&bulk[..i]);
            got.extend(p.feed(&bulk[i..]));
            assert_eq!(got, want, "split at {i}");
        }
        let mut p = Parser::new();
        let mut got = Vec::new();
        for b in bulk {
            got.extend(p.feed(std::slice::from_ref(b)));
        }
        assert_eq!(got, want);
        assert!(p.pending().is_empty());
    }

    #[test]
    fn oversize_line_is_dropped_and_the_next_line_parses() {
        let mut big = vec![b'x'; MAX_LINE + 10];
        big.push(b'\n');
        big.extend_from_slice(b"occ, dis=4\n");
        let (got, stats, rest) = feed(&big);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, Kind::Occ);
        assert_eq!(stats.rejected_lines, 1);
        assert!(stats.resync_bytes > 0);
        assert!(rest.is_empty());
    }

    #[test]
    fn noise_without_a_newline_does_not_grow_the_buffer() {
        let mut p = Parser::new();
        let noise = vec![0xFFu8; 64 * 1024];
        assert!(p.feed(&noise).is_empty());
        assert!(p.pending().len() <= MAX_LINE);
        assert!(p.stats.resync_bytes > 0);
        assert_eq!(p.stats.frames, 0);
        // A real line that arrives after the noise still parses.
        let got = p.feed(b"mov, dis=1.25\n");
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn command_feedback_is_not_a_report() {
        // Shapes quoted in the manual's configuration demo. This cog never
        // sends those commands. If they appear, they are not distances.
        let (got, stats, _) =
            feed(b"received message: rmax=8\nrmax is 8.00\nall vars have been saved\n");
        assert!(got.is_empty());
        assert_eq!(stats.undecoded_lines, 3);
        assert_eq!(stats.frames, 0);
    }
}
