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
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Default)]
pub struct SweepState {
    suspects: HashSet<String>,
    passes: u64,
}

/// `t<task>r<run>-<n>` or `t<task>r<run>-test` -> (task, run).
pub fn parse_host_id(id: &str) -> Option<(u64, u64)> {
    let rest = id.strip_prefix('t')?;
    let (task, rest) = rest.split_once('r')?;
    let run = rest.split('-').next()?;
    Some((task.parse().ok()?, run.parse().ok()?))
}

/// Run one pass. Returns the kinds of bugs it filed.
pub fn sweep_once(d: &Arc<Daemon>) -> Vec<String> {
    let mut filed = vec![];
    let Ok(w) = d.world() else { return filed };
    let mut st = d.sweep.lock().unwrap_or_else(|e| e.into_inner());
    st.passes += 1;
    let mut seen: HashSet<String> = HashSet::new();
    let mut confirmed = |st: &SweepState, key: String| {
        seen.insert(key.clone());
        st.suspects.contains(&key)
    };

    // 1. A step run with no job working on it.
    for r in w.step_runs.values().filter(|r| r.outcome.is_none()) {
        if !d.has_job(r.id) && confirmed(&st, format!("job:{}", r.id)) {
            d.record_bug(
                "missed_job",
                &format!(
                    "task {} had an open {:?} step with no job; started one",
                    r.task, r.step
                ),
            );
            filed.push("missed_job".to_string());
            d.start_job(r.task, r.step, r.id);
        }
    }

    // 2. A queued task that could start but wasn't admitted.
    let active = w.tasks.values().filter(|t| t.state.is_step()).count();
    if active < d.cfg.max_concurrent {
        for t in w.tasks.values().filter(|t| {
            t.state == State::Queued && t.activity == Activity::Waiting(WaitReason::Slot)
        }) {
            if confirmed(&st, format!("admit:{}", t.id)) {
                d.record_bug(
                    "missed_admit",
                    &format!(
                        "task {} was claimable with a free slot but was not admitted; admitted it",
                        t.id
                    ),
                );
                filed.push("missed_admit".to_string());
                d.admit();
                break;
            }
        }
    }

    // 3. A running step host whose step run is no longer open (a stop that never happened).
    let reg = Registry::new(d.cfg.hosts_dir());
    for h in reg.scan().unwrap_or_default() {
        if h.state != HostState::Running {
            continue;
        }
        let Some((task, run)) = parse_host_id(&h.info.id) else {
            continue;
        };
        let open = w.open_run(TaskId(task)) == Some(StepRunId(run));
        if !open && confirmed(&st, format!("host:{}", h.info.id)) {
            d.record_bug(
                "orphan_host",
                &format!(
                    "step host {} was running for a closed step run; stopped it",
                    h.info.id
                ),
            );
            filed.push("orphan_host".to_string());
            d.stop_hosts(&h.info.id);
        }
    }

    // 4. A cancelled task whose branch was to be deleted but is still there.
    for t in w
        .tasks
        .values()
        .filter(|t| t.state == State::Canceled && !t.cancel_keeps_branch)
    {
        let Some(p) = d.project(t.project) else {
            continue;
        };
        if git::branch_exists(&p.path, &p.branch(t.id.0))
            && confirmed(&st, format!("branch:{}", t.id))
        {
            d.record_bug(
                "leftover_branch",
                &format!(
                    "task {} is finished but its branch remained; deleted it",
                    t.id
                ),
            );
            filed.push("leftover_branch".to_string());
            crate::jobs::cleanup(d, t.id, true);
        }
    }

    // 5. Projections that no longer match the log (every tenth pass; it replays everything).
    if st.passes.is_multiple_of(10) {
        if let Err(e) = d.store.verify() {
            d.record_bug(
                "projection_drift",
                &format!("projections disagreed with the log ({e}); rebuilt them"),
            );
            filed.push("projection_drift".to_string());
            let _ = d.store.rebuild_projections();
        }
    }

    st.suspects = seen;
    filed
}

#[cfg(test)]
mod tests {
    use super::parse_host_id;

    #[test]
    fn host_ids_name_their_task_and_run() {
        assert_eq!(parse_host_id("t12r34-1"), Some((12, 34)));
        assert_eq!(parse_host_id("t3r9-test"), Some((3, 9)));
        assert_eq!(parse_host_id("t3r9"), Some((3, 9)));
        for bad in ["x1r2-1", "t1-2", "tr1", "t1rx-1", ""] {
            assert_eq!(parse_host_id(bad), None, "{bad}");
        }
    }
}
