// SPDX-License-Identifier: GPL-3.0-or-later

//! The reconcile sweep: a slow pass that compares what the event path
//! believes with reality. It is a detector. If it ever finds something the
//! event path missed it fixes it **and files a bug**, so the miss gets fixed at
//! its source; nothing relies on it for normal progress (Part 3).
//!
//! A condition must be seen on two consecutive passes before the sweep acts,
//! so work that is merely in flight is never mistaken for work that was missed.

use crate::daemon::Daemon;
use mason_core::*;
use mason_landing::git;
use mason_step_host::registry::{HostState, Registry};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a host the daemon asked to stop may stay Running before the sweep doubts it.
pub const STOP_WINDOW: Duration = Duration::from_secs(30);

/// Hosts the daemon has asked to stop, and when.
#[derive(Default)]
pub struct StopLog(HashMap<String, Instant>);

impl StopLog {
    pub fn record(&mut self, host: &str, now: Instant) {
        self.0.insert(host.to_string(), now);
    }

    /// Was `host` asked to stop within `STOP_WINDOW` of `now`?
    pub fn in_flight(&self, host: &str, now: Instant) -> bool {
        self.0
            .get(host)
            .is_some_and(|at| now.saturating_duration_since(*at) < STOP_WINDOW)
    }

    pub fn forget_expired(&mut self, now: Instant) {
        self.0
            .retain(|_, at| now.saturating_duration_since(*at) < STOP_WINDOW);
    }
}

#[derive(Default)]
pub struct SweepState {
    suspects: HashSet<String>,
    passes: u64,
    /// Host ids already reported as `orphan_host`: one bug per host, not one per pass.
    orphans_filed: HashSet<String>,
    /// Zombie children of this process and when each was first seen.
    zombies: std::collections::HashMap<u32, Instant>,
}

impl SweepState {
    /// True the first time `host` is reported, false after.
    fn first_orphan_report(&mut self, host: &str) -> bool {
        self.orphans_filed.insert(host.to_string())
    }
}

/// Zombie children of `parent` right now, from `ps`.
fn zombie_children(parent: u32) -> HashSet<u32> {
    let Ok(out) = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,stat="])
        .output()
    else {
        return HashSet::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (pid, ppid, stat) = (f.next()?, f.next()?, f.next()?);
            (ppid.parse::<u32>().ok()? == parent && stat.starts_with('Z'))
                .then(|| pid.parse::<u32>().ok())
                .flatten()
        })
        .collect()
}

/// How long a zombie child must have been seen before it is reaped.
const ZOMBIE_AGE: Duration = Duration::from_secs(60);

/// Reap children of this process that exited and were never waited for (a step host started
/// by a daemon image before an in-place update has no reaper thread any more). A zombie is
/// reaped only after it has been seen for `age`, so a child that a thread of ours is about to
/// wait for is left to it. `seen` is updated: first-seen times of the zombies still there.
pub fn reap_stale_zombies(seen: &mut std::collections::HashMap<u32, Instant>, age: Duration) {
    #[cfg(unix)]
    {
        let now = Instant::now();
        let current = zombie_children(std::process::id());
        seen.retain(|pid, _| current.contains(pid));
        for pid in current {
            let first = *seen.entry(pid).or_insert(now);
            if now.duration_since(first) >= age {
                let _ = nix::sys::wait::waitpid(
                    nix::unistd::Pid::from_raw(pid as i32),
                    Some(nix::sys::wait::WaitPidFlag::WNOHANG),
                );
                seen.remove(&pid);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (seen, age);
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_host_id, StopLog, STOP_WINDOW};
    use std::time::{Duration, Instant};

    #[test]
    fn a_stop_request_is_recorded_reported_in_window_and_expires() {
        let mut log = StopLog::default();
        let t0 = Instant::now();
        assert!(!log.in_flight("t1r2-test", t0));
        log.record("t1r2-test", t0);
        assert!(log.in_flight("t1r2-test", t0 + Duration::from_secs(1)));
        assert!(!log.in_flight("t1r2-other", t0));
        let late = t0 + STOP_WINDOW + Duration::from_secs(1);
        assert!(!log.in_flight("t1r2-test", late));
        log.forget_expired(late);
        assert!(log.0.is_empty());
    }

    #[test]
    fn an_orphan_host_is_reported_once() {
        let mut st = super::SweepState::default();
        assert!(st.first_orphan_report("t1r1-1"));
        assert!(!st.first_orphan_report("t1r1-1"));
        assert!(st.first_orphan_report("t1r1-2"));
    }

    #[cfg(unix)]
    #[test]
    fn an_exited_child_nobody_waits_for_is_reaped_once_old_enough() {
        let child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let pid = child.id();
        let stat = || {
            let o = std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        while !stat().starts_with('Z') {
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut seen = std::collections::HashMap::new();
        super::reap_stale_zombies(&mut seen, Duration::from_secs(3600));
        assert!(seen.contains_key(&pid));
        assert!(stat().starts_with('Z'), "reaped before its age");
        super::reap_stale_zombies(&mut seen, Duration::ZERO);
        assert!(!seen.contains_key(&pid));
        assert!(stat().is_empty(), "still there: {}", stat());
        drop(child);
    }

    #[test]
    fn host_ids_name_their_task_and_run() {
        assert_eq!(
            super::parse_batch_host_id("b421-r1-bis6-7-u-cargo-workspace"),
            Some(421)
        );
        assert_eq!(super::parse_batch_host_id("t12r34-1"), None);
        assert_eq!(super::parse_batch_host_id("bx-1"), None);
        assert_eq!(super::parse_batch_host_id("b12"), None);
        assert_eq!(parse_host_id("t12r34-1"), Some((12, 34)));
        assert_eq!(parse_host_id("t3r9-test"), Some((3, 9)));
        assert_eq!(parse_host_id("t3r9"), Some((3, 9)));
        assert_eq!(parse_host_id("t3r9-check0"), Some((3, 9)));
        for bad in ["x1r2-1", "t1-2", "tr1", "t1rx-1", ""] {
            assert_eq!(parse_host_id(bad), None, "{bad}");
        }
    }
}
