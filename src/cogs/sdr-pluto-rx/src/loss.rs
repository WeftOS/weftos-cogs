//! Loss accounting: how many samples the stream did not deliver, counted from the clock.
//!
//! The Pluto's IIO stream carries no hardware timestamps (two 12-bit channels, I and Q, and
//! nothing else), and an overflow in the kernel's DMA buffer is silent: `iio_readdev` and a plain
//! libiio refill both report success. So the cog counts from the host's monotonic clock: every
//! block arrives at a time, and `rate x elapsed` is the number of samples that should have been
//! captured by then. The difference to what was delivered is the deficit.
//!
//! A deficit is not yet a loss. A scheduling stall delays blocks and then they arrive in a burst,
//! and the crystal's error (tens of ppm) makes the deficit drift by itself. So the deficit is
//! measured against a baseline (the smallest deficit of the last 5 to 10 seconds), and an excess
//! over the baseline counts as lost only when it has stayed above the slack for the whole
//! confirmation time; what is counted is the smallest excess seen in that time, which is the part
//! that provably did not come back. Resolution, stated rather than hidden:
//!
//! - a loss shorter than the slack (20 ms of data, or four blocks) is not told apart from jitter;
//! - a loss rate below about 0.1 percent is not told apart from clock drift;
//! - a loss is declared about the confirmation time (250 ms) after it happened, and a loss in
//!   progress is visible meanwhile as `pending`.

/// Tunables, in nanoseconds and samples.
#[derive(Debug, Clone, Copy)]
pub struct Params {
    /// Excess that must persist before it counts, in nanoseconds of data.
    pub slack_ns: u64,
    /// How long the excess must persist.
    pub confirm_ns: u64,
    /// Length of one baseline window; the baseline spans two.
    pub window_ns: u64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            slack_ns: 20_000_000,
            confirm_ns: 250_000_000,
            window_ns: 5_000_000_000,
        }
    }
}

/// What one block arrival tells the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Arrival {
    /// Samples declared lost immediately before this block (0 if none).
    pub lost_before: u64,
    /// Index of this block's first sample on the segment's sample clock, lost samples included.
    pub sample_index: u64,
}

/// One dwell: a stretch of the stream at one centre, rate and bandwidth.
#[derive(Debug, Clone)]
pub struct Segment {
    rate: f64,
    block: u32,
    p: Params,
    t0_ns: Option<u64>,
    delivered: u64,
    lost: u64,
    win_start: u64,
    win_min: i64,
    prev_min: i64,
    over_since: Option<u64>,
    over_min: i64,
    deficit: i64,
}

impl Segment {
    pub fn new(rate_sps: u32, block_samples: u32, p: Params) -> Self {
        Self {
            rate: f64::from(rate_sps),
            block: block_samples,
            p,
            t0_ns: None,
            delivered: 0,
            lost: 0,
            win_start: 0,
            win_min: i64::MAX,
            prev_min: i64::MAX,
            over_since: None,
            over_min: i64::MAX,
            deficit: 0,
        }
    }

    fn slack_samples(&self) -> i64 {
        let by_time = (self.p.slack_ns as f64 * self.rate / 1e9) as i64;
        by_time.max(4 * i64::from(self.block))
    }

    /// Account one block of `n` samples that arrived at `now_ns` (monotonic).
    pub fn on_block(&mut self, now_ns: u64, n: u32) -> Arrival {
        // The first block arrives when its last sample was captured, so the segment began one
        // block earlier.
        let t0 = *self
            .t0_ns
            .get_or_insert_with(|| now_ns.saturating_sub((f64::from(n) * 1e9 / self.rate) as u64));
        if self.win_start == 0 {
            self.win_start = now_ns;
        }
        let expected = ((now_ns - t0) as f64 * self.rate / 1e9) as i64;
        let have = (self.delivered + u64::from(n)) as i64;
        // Deficit net of the losses already declared.
        let e = expected - have - self.lost as i64;
        self.deficit = e;
        self.win_min = self.win_min.min(e);
        let baseline = self.win_min.min(self.prev_min);
        let excess = e - baseline;
        let mut declared = 0u64;
        if excess > self.slack_samples() {
            let since = *self.over_since.get_or_insert(now_ns);
            self.over_min = if since == now_ns {
                excess
            } else {
                self.over_min.min(excess)
            };
            if now_ns - since >= self.p.confirm_ns {
                declared = self.over_min.max(0) as u64;
                self.lost += declared;
                // The baseline keeps its value: it was measured before the loss, and the deficit
                // net of the declared loss comes back to it.
                self.over_since = None;
                self.deficit -= declared as i64;
            }
        } else {
            self.over_since = None;
        }
        if now_ns - self.win_start >= self.p.window_ns {
            self.prev_min = self.win_min;
            self.win_min = i64::MAX;
            self.win_start = now_ns;
        }
        let sample_index = self.delivered + self.lost;
        self.delivered += u64::from(n);
        Arrival {
            lost_before: declared,
            sample_index,
        }
    }

    #[cfg(test)]
    pub fn delivered(&self) -> u64 {
        self.delivered
    }

    #[cfg(test)]
    pub fn lost(&self) -> u64 {
        self.lost
    }

    /// Samples the clock says should have arrived by `now_ns`.
    pub fn expected(&self, now_ns: u64) -> u64 {
        match self.t0_ns {
            Some(t0) => ((now_ns.saturating_sub(t0)) as f64 * self.rate / 1e9) as u64,
            None => 0,
        }
    }

    /// Excess over the baseline that has not (yet) been confirmed as a loss.
    pub fn pending(&self) -> u64 {
        let baseline = self.win_min.min(self.prev_min);
        if baseline == i64::MAX {
            return 0;
        }
        (self.deficit - baseline).max(0) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 5_000_000;
    const BLOCK: u32 = 8192;

    fn block_ns() -> f64 {
        f64::from(BLOCK) * 1e9 / f64::from(RATE)
    }

    /// Drive a segment with arrival times from `at(k)` (ns) for `blocks` blocks.
    fn run(blocks: u64, mut at: impl FnMut(u64) -> u64) -> (Segment, u64) {
        let mut s = Segment::new(RATE, BLOCK, Params::default());
        let mut events = 0;
        for k in 1..=blocks {
            if s.on_block(at(k), BLOCK).lost_before > 0 {
                events += 1;
            }
        }
        (s, events)
    }

    #[test]
    fn steady_arrival_loses_nothing() {
        let (s, ev) = run(20_000, |k| 1_000_000_000 + (k as f64 * block_ns()) as u64);
        assert_eq!((s.lost(), ev), (0, 0));
        assert_eq!(s.delivered(), 20_000 * u64::from(BLOCK));
    }

    #[test]
    fn jitter_of_a_few_milliseconds_is_not_loss() {
        // +-4 ms of arrival jitter around the ideal time.
        let (s, ev) = run(20_000, |k| {
            let j = ((k * 2_654_435_761) % 8_000_000) as i64 - 4_000_000;
            (1_000_000_000i64 + (k as f64 * block_ns()) as i64 + j) as u64
        });
        assert_eq!((s.lost(), ev), (0, 0));
    }

    #[test]
    fn crystal_drift_over_an_hour_is_not_loss() {
        // The device clock runs 30 ppm slow: 3.6 s of data is 108 ms short at the end of an
        // hour, and every block arrives that much later than the nominal rate says.
        let blocks = (3600.0 * f64::from(RATE) / f64::from(BLOCK)) as u64;
        let slow = 1.0 + 30e-6;
        let (s, ev) = run(blocks, |k| {
            1_000_000_000 + (k as f64 * block_ns() * slow) as u64
        });
        assert_eq!((s.lost(), ev), (0, 0), "pending {}", s.pending());
    }

    #[test]
    fn a_stall_that_recovers_by_a_burst_is_not_loss() {
        // 150 ms stall at block 5000, then the queued blocks arrive back to back.
        let (s, ev) = run(10_000, |k| {
            let ideal = 1_000_000_000 + (k as f64 * block_ns()) as u64;
            if !(5000..=5000 + 92).contains(&k) {
                ideal
            } else {
                // arrive in a burst at the end of the stall, 20 us apart
                1_000_000_000 + (5000.0 * block_ns()) as u64 + 150_000_000 + (k - 5000) * 20_000
            }
        });
        assert_eq!((s.lost(), ev), (0, 0));
    }

    #[test]
    fn a_lost_stretch_is_counted_after_the_confirmation_time() {
        // 400 ms of data never arrives: every block after block 5000 arrives 400 ms "late" and
        // the data that would have caught it up is gone.
        let lost_ns = 400_000_000u64;
        let (s, ev) = run(20_000, |k| {
            let ideal = 1_000_000_000 + (k as f64 * block_ns()) as u64;
            if k <= 5000 {
                ideal
            } else {
                ideal + lost_ns
            }
        });
        let expect = (f64::from(RATE) * 0.4) as u64;
        assert_eq!(ev, 1, "one gap event");
        let err = s.lost().abs_diff(expect);
        assert!(err < expect / 50, "lost {} vs {expect}", s.lost());
    }

    #[test]
    fn a_stream_limited_to_three_quarters_of_the_rate_loses_a_quarter() {
        // 7.5 MS/s delivered of 10 requested.
        let rate = 10_000_000u32;
        let block = 16_384u32;
        let mut s = Segment::new(rate, block, Params::default());
        let step_ns = f64::from(block) * 1e9 / 7.5e6;
        let mut gaps = 0;
        let blocks = 4000u64;
        for k in 1..=blocks {
            let t = 1_000_000_000 + (k as f64 * step_ns) as u64;
            if s.on_block(t, block).lost_before > 0 {
                gaps += 1;
            }
        }
        let elapsed = blocks as f64 * step_ns / 1e9;
        let expect = (elapsed * 2.5e6) as u64;
        // within the slack plus the confirmation lag
        let tolerance = (0.02 * 10e6 + 0.3 * 10e6) as u64;
        assert!(gaps >= 2);
        assert!(
            s.lost() <= expect && expect - s.lost() < tolerance,
            "lost {} expected about {expect}",
            s.lost()
        );
        // what is not yet confirmed is shown as pending, so lost + pending is close
        assert!(expect.abs_diff(s.lost() + s.pending()) < (0.05 * 10e6) as u64);
    }

    #[test]
    fn the_sample_clock_counts_lost_samples() {
        let lost_ns = 400_000_000u64;
        let mut s = Segment::new(RATE, BLOCK, Params::default());
        let mut last = Arrival::default();
        let mut n = 0u64;
        for k in 1..=20_000u64 {
            let ideal = 1_000_000_000 + (k as f64 * block_ns()) as u64;
            let t = if k <= 5000 { ideal } else { ideal + lost_ns };
            last = s.on_block(t, BLOCK);
            n += 1;
        }
        assert_eq!(last.sample_index, (n - 1) * u64::from(BLOCK) + s.lost());
    }

    #[test]
    fn expected_follows_the_clock() {
        let mut s = Segment::new(RATE, BLOCK, Params::default());
        assert_eq!(s.expected(5), 0);
        s.on_block(1_000_000_000, BLOCK);
        let e = s.expected(2_000_000_000);
        assert!(e.abs_diff(u64::from(RATE) + u64::from(BLOCK)) < 10, "{e}");
    }
}
