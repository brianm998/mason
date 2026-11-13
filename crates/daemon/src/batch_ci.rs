// SPDX-License-Identifier: GPL-3.0-or-later

//! What a landing batch knows about its CI, kept in memory for the views: the ref, the run's
//! address, when it started, and each workflow's job counts as of the last poll. It records and
//! reports only; nothing here (or in the calls that feed it) decides a verdict.
//!
//! Kept for the daemon's lifetime, so a finished batch still shows its last state and link.
//! After a restart the batch has none.

use mason_adapter_api::service::{CiProgress, CiWorkflowJobs};
use mason_protocol::{BatchCi, WorkflowJobs};
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CiNow {
    pub ci_ref: String,
    pub started_ms: u64,
    /// queued, running, green or red.
    pub state: String,
    pub workflows: Vec<CiWorkflowJobs>,
}

#[derive(Default)]
pub struct CiBook {
    m: Mutex<BTreeMap<u64, CiNow>>,
}

impl CiBook {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, CiNow>> {
        self.m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// CI was started for `batch` on `ci_ref`.
    pub fn start(&self, batch: u64, ci_ref: &str, now_ms: u64) {
        self.lock().insert(
            batch,
            CiNow {
                ci_ref: ci_ref.into(),
                started_ms: now_ms,
                state: "queued".into(),
                workflows: vec![],
            },
        );
    }

    /// The latest counts from a poll.
    pub fn update(&self, batch: u64, p: &CiProgress) {
        if let Some(c) = self.lock().get_mut(&batch) {
            c.state = running_state(&p.workflows).into();
            c.workflows = p.workflows.clone();
        }
    }

    /// The verdict is in: `green` or `red`. The counts stay as last polled.
    pub fn finish(&self, batch: u64, state: &str) {
        if let Some(c) = self.lock().get_mut(&batch) {
            c.state = state.into();
        }
    }

    pub fn get(&self, batch: u64) -> Option<CiNow> {
        self.lock().get(&batch).cloned()
    }
}

/// `queued` while no job has started (or none is known yet), `running` after.
pub fn running_state(w: &[CiWorkflowJobs]) -> &'static str {
    if w.iter().any(|w| w.running + w.passed + w.failed > 0) {
        "running"
    } else {
        "queued"
    }
}

/// An http(s) address, or nothing: a client opens whatever it is given.
pub fn safe_url(u: &str) -> String {
    if u.starts_with("https://") || u.starts_with("http://") {
        u.to_string()
    } else {
        String::new()
    }
}

/// The run's address: the first workflow's that has one.
pub fn run_url(c: &CiNow) -> String {
    c.workflows
        .iter()
        .map(|w| safe_url(&w.url))
        .find(|u| !u.is_empty())
        .unwrap_or_default()
}

pub fn to_proto(c: &CiNow) -> BatchCi {
    BatchCi {
        ci_ref: c.ci_ref.clone(),
        url: run_url(c),
        started_ms: c.started_ms,
        state: c.state.clone(),
        workflows: c
            .workflows
            .iter()
            .map(|w| WorkflowJobs {
                workflow: w.workflow.clone(),
                queued: w.queued,
                running: w.running,
                passed: w.passed,
                failed: w.failed,
                running_names: w.running_names.clone(),
            })
            .collect(),
    }
}

/// For the update banner: "CI running: 2 of 11 jobs left", "CI queued for a runner".
pub fn wait_text(c: &CiNow) -> String {
    let total: u32 = c
        .workflows
        .iter()
        .map(|w| w.queued + w.running + w.passed + w.failed)
        .sum();
    let left: u32 = c.workflows.iter().map(|w| w.queued + w.running).sum();
    let failed: u32 = c.workflows.iter().map(|w| w.failed).sum();
    match c.state.as_str() {
        "queued" => "CI queued for a runner".into(),
        "running" if failed > 0 => {
            format!("CI running: {left} of {total} jobs left, {failed} failed")
        }
        "running" => format!("CI running: {left} of {total} jobs left"),
        other => format!("CI {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(q: u32, r: u32, p: u32, f: u32, url: &str) -> CiWorkflowJobs {
        CiWorkflowJobs {
            workflow: "pr-gate".into(),
            queued: q,
            running: r,
            passed: p,
            failed: f,
            running_names: vec![],
            url: url.into(),
        }
    }

    #[test]
    fn the_state_follows_the_polls_and_the_last_one_is_kept() {
        let b = CiBook::default();
        b.start(7, "mason/ci/star-b7-1", 1000);
        assert_eq!(b.get(7).unwrap().state, "queued");
        b.update(
            7,
            &CiProgress {
                workflows: vec![w(3, 0, 0, 0, "https://gh/run/1")],
            },
        );
        assert_eq!(b.get(7).unwrap().state, "queued");
        b.update(
            7,
            &CiProgress {
                workflows: vec![w(1, 1, 1, 0, "https://gh/run/1")],
            },
        );
        assert_eq!(b.get(7).unwrap().state, "running");
        b.update(
            7,
            &CiProgress {
                workflows: vec![w(0, 1, 1, 1, "https://gh/run/1")],
            },
        );
        b.finish(7, "red");
        let c = b.get(7).unwrap();
        assert_eq!(c.state, "red");
        assert_eq!(c.workflows[0].failed, 1);
        assert_eq!(to_proto(&c).url, "https://gh/run/1");
        assert!(b.get(8).is_none());
    }

    #[test]
    fn only_http_addresses_are_sent() {
        assert_eq!(safe_url("javascript:alert(1)"), "");
        assert_eq!(safe_url("file:///etc/passwd"), "");
        assert_eq!(safe_url("http://x/y"), "http://x/y");
        let c = CiNow {
            workflows: vec![w(0, 0, 1, 0, "ftp://x"), w(0, 0, 1, 0, "https://ok/1")],
            ..Default::default()
        };
        assert_eq!(run_url(&c), "https://ok/1");
    }

    #[test]
    fn the_wait_text_names_the_ci_state() {
        let mut c = CiNow {
            state: "queued".into(),
            ..Default::default()
        };
        assert_eq!(wait_text(&c), "CI queued for a runner");
        c.state = "running".into();
        c.workflows = vec![w(1, 1, 9, 0, "")];
        assert_eq!(wait_text(&c), "CI running: 2 of 11 jobs left");
        c.workflows = vec![w(0, 1, 9, 1, "")];
        assert_eq!(wait_text(&c), "CI running: 1 of 11 jobs left, 1 failed");
    }
}
