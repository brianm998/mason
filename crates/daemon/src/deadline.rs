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

    /// Carry on from what an earlier daemon had already granted.
    pub fn resume(mut self, granted: Duration) -> Self {
        self.granted = granted.min(self.cap);
        self
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

/// Write the credit, grants and the GitHub part of the credit to `path` if they changed since
/// `last`; returns what is now on disk. The host counts its limit from when it started, so a
/// restarted daemon that forgot the credit would stop a re-attached agent that had only been
/// queueing.
pub fn save(
    extra: &DeadlineExtra,
    path: &std::path::Path,
    last: (u64, u64, u64),
) -> (u64, u64, u64) {
    let ms = |d: Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
    let now = (
        ms(extra.waited()),
        ms(extra.progressed()),
        ms(extra.remote()),
    );
    if now != last {
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, format!("{} {} {}\n", now.0, now.1, now.2)).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
    now
}

/// Read what [`save`] wrote into `extra`; returns it. Nothing there leaves `extra` as it is.
/// A file from before the GitHub part has two numbers; that part is then zero.
pub fn load(extra: &DeadlineExtra, path: &std::path::Path) -> (u64, u64, u64) {
    let Some((w, p, r)) = std::fs::read_to_string(path).ok().and_then(|t| {
        let mut it = t.split_whitespace().map(|x| x.parse::<u64>().ok());
        let w = it.next()??;
        let p = it.next()??;
        let r = it.next().flatten().unwrap_or(0);
        Some((w, p, r))
    }) else {
        return (0, 0, 0);
    };
    extra.restore(Duration::from_millis(w), Duration::from_millis(p));
    extra.restore_remote(Duration::from_millis(r));
    (w, p, r)
}

fn secs(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}h{:02}m{:02}s", s / 3600, s % 3600 / 60, s % 60)
}

/// The failure note of a step that ran out of time: how the time divided.
/// `elapsed` counts from the step's launch (not from this daemon's start); `restarted` says
/// the daemon started again after the launch. `silent` is how long the agent had produced
/// nothing when it was stopped.
pub fn timeout_note(
    budget: Duration,
    elapsed: Duration,
    restarted: bool,
    extra: &DeadlineExtra,
    silent: Duration,
    commits: usize,
) -> String {
    let waited = extra.waited();
    let github = extra.remote().min(waited);
    let queues = waited - github;
    let working = elapsed.saturating_sub(waited);
    let restart = if restarted {
        " The daemon restarted during the step; the time counts from the step's launch."
    } else {
        ""
    };
    format!(
        "the step ran out of time: {} of the {} budget (+{} wait credit, +{} for progress); \
         waiting in queues (run_check, mason run) {}, waiting on GitHub {}, working {}, \
         silent for the last {}, {} commit(s) on the branch. Lease waits are not credited or \
         measured here.{restart}",
        secs(elapsed),
        secs(budget),
        secs(waited),
        secs(extra.progressed()),
        secs(queues),
        secs(github),
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
    fn credit_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("mason-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.kv");
        let a = DeadlineExtra::default();
        a.add(Duration::from_secs(7200));
        a.add_progress(H);
        save(&a, &f, (0, 0, 0));
        let b = DeadlineExtra::default();
        assert_eq!(load(&b, &f).0, 7_200_000);
        assert_eq!(b.waited(), 2 * H);
        assert_eq!(b.progressed(), H);
        // The extender resumes at what was granted, so the cap still holds.
        let mut e =
            Extender::new(4 * H, Duration::from_secs(60), "c0".into(), 0).resume(b.progressed());
        let mut n = 0;
        loop {
            n += 1;
            let g = e.tick(4 * H + b.get(), &b, || format!("c{n}"), 0);
            if g.is_zero() {
                break;
            }
        }
        assert_eq!(b.progressed(), 4 * H);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_timeout_note_names_waiting_and_working_time() {
        let extra = DeadlineExtra::default();
        extra.add(Duration::from_secs(3600));
        let n = timeout_note(4 * H, 5 * H, false, &extra, Duration::from_secs(90), 3);
        assert!(
            n.contains(
                "waiting in queues (run_check, mason run) 1h00m00s, waiting on GitHub 0h00m00s"
            ),
            "{n}"
        );
        assert!(n.contains("working 4h00m00s"), "{n}");
        assert!(n.contains("silent for the last 0h01m30s"), "{n}");
        assert!(!n.contains("restarted"), "{n}");
    }

    #[test]
    fn the_note_counts_from_the_launch_names_the_restart_and_the_github_wait() {
        let extra = DeadlineExtra::default();
        extra.add(Duration::from_secs(600));
        extra.add_remote(Duration::from_secs(3000));
        // Launched 4 h ago; this daemon started 59 minutes ago. The caller passes the elapsed
        // time from the launch, so the note says 4 h, not 59 min.
        let n = timeout_note(4 * H, 4 * H, true, &extra, Duration::ZERO, 1);
        assert!(n.contains("4h00m00s of the 4h00m00s budget"), "{n}");
        assert!(
            n.contains("waiting in queues (run_check, mason run) 0h10m00s"),
            "{n}"
        );
        assert!(n.contains("waiting on GitHub 0h50m00s"), "{n}");
        assert!(n.contains("working 3h00m00s"), "{n}");
        assert!(n.contains("daemon restarted during the step"), "{n}");
    }

    #[test]
    fn the_github_part_of_the_credit_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("mason-dl-gh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.kv");
        let a = DeadlineExtra::default();
        a.add(Duration::from_secs(5));
        a.add_remote(Duration::from_secs(7));
        save(&a, &path, (0, 0, 0));
        let b = DeadlineExtra::default();
        load(&b, &path);
        assert_eq!(b.waited(), Duration::from_secs(12));
        assert_eq!(b.remote(), Duration::from_secs(7));
        // An older file with two numbers still loads.
        std::fs::write(&path, "9000 0\n").unwrap();
        let c = DeadlineExtra::default();
        assert_eq!(load(&c, &path), (9000, 0, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
