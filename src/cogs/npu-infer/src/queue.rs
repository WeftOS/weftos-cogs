//! The priority queue. Higher priority runs first; equal priorities run in
//! arrival order. A waiting job gains one level for every `AGE_SECS` it has
//! waited (capped at 9), so a steady stream of interactive calls cannot hold
//! a background call out forever.

use std::time::{Duration, Instant};

pub const MAX_PRIORITY: u8 = 9;
/// Seconds of waiting that are worth one priority level.
pub const AGE_SECS: u64 = 30;

struct Item<T> {
    priority: u8,
    seq: u64,
    since: Instant,
    job: T,
}

pub struct PriorityQueue<T> {
    items: Vec<Item<T>>,
    next: u64,
    cap: usize,
}

impl<T> PriorityQueue<T> {
    pub fn new(cap: usize) -> Self {
        Self {
            items: Vec::new(),
            next: 0,
            cap,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// `Err(job)` when the queue is full.
    pub fn push(&mut self, priority: u8, job: T, now: Instant) -> Result<(), T> {
        if self.items.len() >= self.cap {
            return Err(job);
        }
        self.next += 1;
        self.items.push(Item {
            priority: priority.min(MAX_PRIORITY),
            seq: self.next,
            since: now,
            job,
        });
        Ok(())
    }

    fn effective(i: &Item<T>, now: Instant) -> u64 {
        let aged = now.saturating_duration_since(i.since).as_secs() / AGE_SECS;
        (i.priority as u64 + aged).min(MAX_PRIORITY as u64)
    }

    /// The job that runs next.
    #[cfg(test)]
    pub fn pop(&mut self, now: Instant) -> Option<T> {
        self.pop_preferring(now, |_| false)
    }

    /// The job that runs next. Priority decides first; among jobs of the same
    /// effective priority, one for which `prefer` is true goes before the
    /// others (a model that is already resident saves a reload), and arrival
    /// order breaks the rest.
    pub fn pop_preferring(&mut self, now: Instant, prefer: impl Fn(&T) -> bool) -> Option<T> {
        let best = self
            .items
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| {
                Self::effective(a, now)
                    .cmp(&Self::effective(b, now))
                    .then(prefer(&a.job).cmp(&prefer(&b.job)))
                    .then(b.seq.cmp(&a.seq))
            })
            .map(|(i, _)| i)?;
        Some(self.items.remove(best).job)
    }

    /// Oldest wait among queued jobs.
    pub fn oldest_wait(&self, now: Instant) -> Duration {
        self.items
            .iter()
            .map(|i| now.saturating_duration_since(i.since))
            .max()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn higher_priority_first_then_arrival_order() {
        let t0 = Instant::now();
        let mut q = PriorityQueue::new(16);
        for (p, name) in [(4, "a"), (9, "b"), (4, "c"), (0, "d"), (9, "e"), (4, "f")] {
            q.push(p, name, t0).unwrap();
        }
        let order: Vec<_> = std::iter::from_fn(|| q.pop(t0)).collect();
        assert_eq!(order, ["b", "e", "a", "c", "f", "d"]);
    }

    #[test]
    fn within_a_priority_the_preferred_job_goes_first_but_never_past_a_higher_one() {
        let t0 = Instant::now();
        let mut q = PriorityQueue::new(16);
        for (p, name) in [(4, "cold1"), (4, "warm"), (4, "cold2"), (9, "urgent-cold")] {
            q.push(p, name, t0).unwrap();
        }
        let warm = |n: &&str| n.starts_with("warm");
        assert_eq!(q.pop_preferring(t0, warm), Some("urgent-cold"));
        assert_eq!(
            q.pop_preferring(t0, warm),
            Some("warm"),
            "arrived second, runs first"
        );
        assert_eq!(q.pop_preferring(t0, warm), Some("cold1"));
        assert_eq!(q.pop_preferring(t0, warm), Some("cold2"));
    }

    #[test]
    fn a_full_queue_hands_the_job_back() {
        let t0 = Instant::now();
        let mut q = PriorityQueue::new(2);
        q.push(4, 1, t0).unwrap();
        q.push(4, 2, t0).unwrap();
        assert_eq!(q.push(9, 3, t0), Err(3));
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn waiting_raises_a_background_job_past_fresh_interactive_ones() {
        let t0 = Instant::now();
        let mut q = PriorityQueue::new(16);
        q.push(0, "old-background", t0).unwrap();
        let later = t0 + Duration::from_secs(AGE_SECS * 5);
        q.push(4, "fresh", later).unwrap();
        // 0 + 5 levels of aging beats a fresh 4.
        assert_eq!(q.pop(later), Some("old-background"));
        assert_eq!(q.pop(later), Some("fresh"));
        assert!(q.is_empty());
    }

    #[test]
    fn priority_is_capped() {
        let t0 = Instant::now();
        let mut q = PriorityQueue::new(4);
        q.push(200, "x", t0).unwrap();
        q.push(9, "y", t0).unwrap();
        assert_eq!(
            q.pop(t0),
            Some("x"),
            "equal after the cap, so arrival order"
        );
    }
}
