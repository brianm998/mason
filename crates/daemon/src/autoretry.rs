// SPDX-License-Identifier: GPL-3.0-or-later

//! Retrying failures outside the code by itself (s247, decision 0242).
//!
//! A task that failed with class `outside` (a lost host, a fetch that timed out, a refused push)
//! is retried from the failed step: first 10 minutes after the failure, then 60 minutes after the
//! next one. A third outside failure within 24 hours stays failed and raises the usual alert.
//! The retries are counted in the store (`auto_retries`), so a restart neither forgets nor
//! repeats one. Failures about the code are never retried here.
//!
//! Three or more tasks failing outside with the same normalised reason within ten minutes are
//! one host-wide event: one alert (attention.rs asks [`bursts`]), and when the reason is a failed
//! fetch their retries wait until a fetch works.

use crate::config::ProjectCfg;
use crate::daemon::{now_ms, Daemon};
use mason_core::*;
use mason_engine::World;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

/// What is due for a task that failed outside the code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Due {
    /// Retry at this time (ms); it has not come yet.
    At(u64),
    /// Retry now.
    Now,
    /// Enough retries were made in the window: the task stays failed.
    GiveUp,
}

/// `done`: the automatic retries made inside the window. The n-th retry comes `delays[n]` after
/// the failure it follows.
pub fn due(delays: &[Duration], done: usize, failed_at_ms: u64, now: u64) -> Due {
    let Some(delay) = delays.get(done) else {
        return Due::GiveUp;
    };
    let at = failed_at_ms + delay.as_millis() as u64;
    if now >= at {
        Due::Now
    } else {
        Due::At(at)
    }
}

/// A failure outside the code, as far as grouping needs.
#[derive(Clone, Debug)]
pub struct Failed {
    pub task: u64,
    pub project: ProjectId,
    pub reason: String,
    pub at_ms: u64,
}

/// A group of `min` or more failures with the same normalised reason inside `window`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Burst {
    pub reason: String,
    pub tasks: Vec<u64>,
}

pub fn bursts(failed: &[Failed], now: u64, window: Duration, min: usize) -> Vec<Burst> {
    let since = now.saturating_sub(window.as_millis() as u64);
    let mut groups: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for f in failed.iter().filter(|f| f.at_ms >= since) {
        groups
            .entry(mason_testing::basered::normalise(&f.reason))
            .or_default()
            .push(f.task);
    }
    groups
        .into_iter()
        .filter(|(_, t)| t.len() >= min)
        .map(|(reason, mut tasks)| {
            tasks.sort_unstable();
            Burst { reason, tasks }
        })
        .collect()
}

/// A burst whose reason is a failed fetch is held until a fetch works.
fn is_fetch(reason: &str) -> bool {
    reason.contains("fetch")
}

/// The tasks that failed outside the code, newest state: class `outside`, in a project that has
/// not turned automatic retries off.
fn candidates(d: &Daemon, w: &World) -> Vec<(Failed, bool)> {
    w.tasks
        .values()
        .filter(|t| t.state == State::Failed && t.failure_class == Some(FailClass::Outside))
        .filter_map(|t| {
            let on = d
                .project(t.project)
                .is_some_and(|p: &ProjectCfg| p.auto_retry);
            Some((
                Failed {
                    task: t.id.0,
                    project: t.project,
                    reason: t.failure.clone()?,
                    at_ms: t.state_entered_at.0,
                },
                on,
            ))
        })
        .collect()
}

/// Every burst among the tasks failed outside the code right now (for the alert).
pub fn current_bursts(d: &Daemon, w: &World) -> Vec<Burst> {
    let failed: Vec<Failed> = candidates(d, w).into_iter().map(|(f, _)| f).collect();
    bursts(&failed, now_ms(), d.cfg.burst_window, d.cfg.burst_min)
}

/// What `mason task show` says about a failed task: retries made in the window, and when the
/// next one is due (`None`: none will be made).
pub fn status(d: &Daemon, t: &Task) -> Option<(usize, Option<u64>)> {
    if t.state != State::Failed || t.failure_class != Some(FailClass::Outside) {
        return None;
    }
    let on = d.project(t.project).is_some_and(|p| p.auto_retry);
    let since = now_ms().saturating_sub(d.cfg.auto_retry_window.as_millis() as u64);
    let done = d
        .store
        .auto_retries_since(t.id.0, since)
        .unwrap_or_default()
        .len();
    if !on {
        return Some((done, None));
    }
    match due(
        &d.cfg.auto_retry_delays,
        done,
        t.state_entered_at.0,
        now_ms(),
    ) {
        Due::GiveUp => Some((done, None)),
        Due::At(at) => Some((done, Some(at))),
        Due::Now => Some((done, Some(now_ms()))),
    }
}

/// Whether a failed task has an automatic retry coming (so the "failed" alert waits).
pub fn pending(d: &Daemon, t: &Task) -> bool {
    status(d, t).is_some_and(|(_, next)| next.is_some())
}

/// One pass: retry what is due.
pub fn pass(d: &Arc<Daemon>) {
    let Ok(w) = d.world() else { return };
    let now = now_ms();
    let cands = candidates(d, &w);
    let failed: Vec<Failed> = cands.iter().map(|(f, _)| f.clone()).collect();
    // A burst of failed fetches is held until one fetch works.
    let mut held: Vec<u64> = vec![];
    for b in bursts(&failed, now, d.cfg.burst_window, d.cfg.burst_min) {
        if !is_fetch(&b.reason) {
            continue;
        }
        let Some(f) = failed.iter().find(|f| f.task == b.tasks[0]) else {
            continue;
        };
        if d.fetch_remote(f.project).is_err() {
            held.extend(b.tasks);
        }
    }
    let since = now.saturating_sub(d.cfg.auto_retry_window.as_millis() as u64);
    for (f, on) in cands {
        if !on || held.contains(&f.task) {
            continue;
        }
        let done = d
            .store
            .auto_retries_since(f.task, since)
            .unwrap_or_default()
            .len();
        if due(&d.cfg.auto_retry_delays, done, f.at_ms, now) != Due::Now {
            continue;
        }
        let latest = crate::http::latest_run(&w, TaskId(f.task));
        // A retry the guard refuses (the owner got there first) is not counted. It is counted
        // right after it is made; a crash between the two costs at most one retry more.
        let ask = crate::http::RetryAsk {
            seen: Some(latest),
            auto: true,
            ..Default::default()
        };
        if let Ok((to, step)) = crate::http::retry_task(d, f.task, ask) {
            let _ = d.store.record_auto_retry(f.task, now, &f.reason);
            tracing::info!(task = f.task, ?to, %step, "automatic retry after a failure outside the code");
        }
    }
}

pub fn spawn(d: &Arc<Daemon>) {
    crate::bg::spawn_loop(d, "auto-retry", move |weak| {
        let Some(d) = weak.upgrade() else {
            return false;
        };
        let tick = d.cfg.auto_retry_tick;
        pass(&d);
        drop(d);
        std::thread::sleep(tick);
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: &[u64]) -> Vec<Duration> {
        s.iter().map(|n| Duration::from_secs(*n)).collect()
    }

    #[test]
    fn the_first_retry_waits_ten_minutes_the_second_sixty_and_a_third_failure_stays() {
        let delays = secs(&[600, 3600]);
        let t0 = 1_000_000;
        assert_eq!(due(&delays, 0, t0, t0 + 599_000), Due::At(t0 + 600_000));
        assert_eq!(due(&delays, 0, t0, t0 + 600_000), Due::Now);
        assert_eq!(due(&delays, 1, t0, t0 + 3_599_000), Due::At(t0 + 3_600_000));
        assert_eq!(due(&delays, 1, t0, t0 + 3_600_000), Due::Now);
        assert_eq!(due(&delays, 2, t0, t0 + 9_999_999_999), Due::GiveUp);
    }

    #[test]
    fn three_failures_with_one_reason_inside_the_window_are_one_burst() {
        let f = |task: u64, reason: &str, at_ms: u64| Failed {
            task,
            project: ProjectId(0),
            reason: reason.into(),
            at_ms,
        };
        let now = 10_000_000;
        let list = vec![
            f(
                1,
                "fetching the remote failed (git fetch --prune origin timed out after 120 s)",
                now - 1000,
            ),
            f(
                2,
                "fetching the remote failed (git fetch --prune origin timed out after 90 s)",
                now - 2000,
            ),
            f(
                3,
                "fetching the remote failed (git fetch --prune origin timed out after 300 s)",
                now - 3000,
            ),
            f(4, "the step host was lost 2 times", now - 1000),
            // Too old to count.
            f(
                5,
                "fetching the remote failed (git fetch --prune origin timed out after 5 s)",
                now - 700_000,
            ),
        ];
        let b = bursts(&list, now, Duration::from_secs(600), 3);
        assert_eq!(b.len(), 1, "{b:?}");
        assert_eq!(b[0].tasks, vec![1, 2, 3]);
        assert!(is_fetch(&b[0].reason));
        assert!(bursts(&list, now, Duration::from_secs(600), 4).is_empty());
    }
}
