// SPDX-License-Identifier: GPL-3.0-or-later

//! How long each task has spent held under a CPU cause (the CPU gate or its settle pause).
//! Pure: time is a number the caller passes in.

use std::collections::BTreeMap;

/// The most one observation adds, so a paused or stalled daemon cannot inflate a total.
pub const MAX_GAP_MS: u64 = 10_000;

#[derive(Debug, Default)]
pub struct CpuHold {
    last_ms: Option<u64>,
    held: BTreeMap<u64, u64>,
}

impl CpuHold {
    /// Record one pass at `now_ms`. `waiting` has one `(task, cpu_caused)` entry per task that
    /// has a wait. A task under a CPU cause gains the time since the last pass (capped); a task
    /// with no wait, or a wait of another cause, is dropped: the total is per wait episode.
    pub fn observe(&mut self, now_ms: u64, waiting: impl Iterator<Item = (u64, bool)>) {
        let gap = self
            .last_ms
            .map_or(0, |l| now_ms.saturating_sub(l).min(MAX_GAP_MS));
        let mut next = BTreeMap::new();
        for (id, cpu) in waiting {
            if cpu {
                let was = self.held.get(&id).copied().unwrap_or(0);
                next.insert(id, was.saturating_add(gap));
            }
        }
        self.held = next;
        self.last_ms = Some(now_ms);
    }

    /// The largest time any one task has spent under a CPU cause.
    pub fn max_held_ms(&self) -> u64 {
        self.held.values().copied().max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60_000;

    /// Observe every 5 s from `from` for `ms`.
    fn run(h: &mut CpuHold, from: u64, ms: u64, cpu: bool) -> u64 {
        let mut t = from;
        while t < from + ms {
            t += 5_000;
            h.observe(t, [(1, cpu)].into_iter());
        }
        t
    }

    #[test]
    fn a_short_settle_after_a_long_queue_does_not_count() {
        let mut h = CpuHold::default();
        let t = run(&mut h, 0, 90 * MIN, false);
        run(&mut h, t, 21_000, true);
        assert!(h.max_held_ms() <= 25_000);
        assert!(h.max_held_ms() < 30 * MIN);
    }

    #[test]
    fn forty_minutes_under_cpu_counts() {
        let mut h = CpuHold::default();
        run(&mut h, 0, 40 * MIN, true);
        assert!(h.max_held_ms() >= 30 * MIN);
    }

    #[test]
    fn other_causes_add_nothing_and_end_the_episode() {
        let mut h = CpuHold::default();
        let t = run(&mut h, 0, 10 * MIN, true);
        h.observe(t + 5_000, [(1, false)].into_iter());
        assert_eq!(h.max_held_ms(), 0);
        let t = run(&mut h, t + 5_000, 5 * MIN, true);
        assert!(h.max_held_ms() <= 5 * MIN);
        h.observe(t + 5_000, std::iter::empty());
        assert_eq!(h.max_held_ms(), 0);
    }

    #[test]
    fn a_long_gap_adds_only_the_cap() {
        let mut h = CpuHold::default();
        h.observe(0, [(1, true)].into_iter());
        h.observe(60 * MIN, [(1, true)].into_iter());
        assert_eq!(h.max_held_ms(), MAX_GAP_MS);
    }
}
