// SPDX-License-Identifier: GPL-3.0-or-later
//! A develop agent's deadline grows while it keeps making progress (s401), up to a
//! hard cap, and a step that still ran out of time says how its time was spent.

use mason_adapter_api::DeadlineExtra;
use std::time::Duration;

/// Decides when to extend a step's deadline. Pure: the caller supplies the clock,
/// the branch head and whether output arrived, so tests need no timing.
pub struct Extender {
    chunk: Duration,
    cap: Duration,
    lead: Duration,
    budget: Duration,
    granted: Duration,
    last_head: String,
    last_output: u64,
}

impl Extender {
    /// `budget` is the step's deadline. Each grant is a quarter of it, the total
    /// never passes the budget again (so working time is at most twice the budget,
    /// plus queue credit), and a grant is made within `lead` of the end.
    pub fn new(budget: Duration, lead: Duration, head: String, output: u64) -> Self {
        Extender {
            chunk: budget / 4,
            cap: budget,
            lead,
            budget,
            granted: Duration::ZERO,
            last_head: head,
            last_output: output,
        }
    }

    /// Called now and then. When the deadline is near and the agent committed or
    /// produced output since the last grant, adds time to `extra`; returns what it added.
    pub fn tick(
        &mut self,
        elapsed: Duration,
        extra: &DeadlineExtra,
        head: impl FnOnce() -> String,
        output: u64,
    ) -> Duration {
        let end = self.budget + extra.get();
        if elapsed + self.lead < end || self.granted >= self.cap {
            return Duration::ZERO;
        }
        let head = head();
        if head == self.last_head && output == self.last_output {
            return Duration::ZERO;
        }
        self.last_head = head;
        self.last_output = output;
        let g = self.chunk.min(self.cap - self.granted);
        self.granted += g;
        extra.add_progress(g);
        g
    }
}

fn secs(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}h{:02}m{:02}s", s / 3600, s % 3600 / 60, s % 60)
}

/// The failure note of a step that ran out of time: how the time divided.
/// `silent` is how long the agent had produced nothing when it was stopped.
pub fn timeout_note(
    budget: Duration,
    elapsed: Duration,
    extra: &DeadlineExtra,
    silent: Duration,
    commits: usize,
) -> String {
    let waited = extra.waited();
    let working = elapsed.saturating_sub(waited);
    format!(
        "the step ran out of time: {} of the {} budget (+{} queue credit, +{} for progress); \
         waiting in queues (run_check, mason run) {}, working {}, silent for the last {}, \
         {} commit(s) on the branch. Lease waits are not credited or measured here.",
        secs(elapsed),
        secs(budget),
        secs(waited),
        secs(extra.progressed()),
        secs(waited),
        secs(working),
        secs(silent),
        commits
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: Duration = Duration::from_secs(3600);

    #[test]
    fn a_committing_agent_is_extended_to_the_cap_and_no_further() {
        let extra = DeadlineExtra::default();
        let mut e = Extender::new(4 * H, Duration::from_secs(60), "c0".into(), 0);
        let mut n = 0;
        loop {
            let t = 4 * H + extra.get() - Duration::from_secs(30);
            n += 1;
            let g = e.tick(t, &extra, || format!("c{n}"), 0);
            if g.is_zero() {
                break;
            }
        }
        assert_eq!(extra.progressed(), 4 * H);
    }

    #[test]
    fn nothing_is_granted_early_or_without_progress() {
        let extra = DeadlineExtra::default();
        let mut e = Extender::new(4 * H, Duration::from_secs(60), "c0".into(), 5);
        assert!(e.tick(H, &extra, || "c9".into(), 5).is_zero());
        assert!(e.tick(4 * H, &extra, || "c0".into(), 5).is_zero());
        assert!(!e.tick(4 * H, &extra, || "c1".into(), 5).is_zero());
    }

    #[test]
    fn queue_credit_moves_the_end_the_extender_watches() {
        let extra = DeadlineExtra::default();
        extra.add(H);
        let mut e = Extender::new(4 * H, Duration::from_secs(60), "c0".into(), 0);
        assert!(e.tick(4 * H, &extra, || "c1".into(), 0).is_zero());
    }

    #[test]
    fn the_timeout_note_names_waiting_and_working_time() {
        let extra = DeadlineExtra::default();
        extra.add(Duration::from_secs(3600));
        let n = timeout_note(4 * H, 5 * H, &extra, Duration::from_secs(90), 3);
        assert!(
            n.contains("waiting in queues (run_check, mason run) 1h00m00s"),
            "{n}"
        );
        assert!(n.contains("working 4h00m00s"), "{n}");
        assert!(n.contains("silent for the last 0h01m30s"), "{n}");
    }
}
