// SPDX-License-Identifier: GPL-3.0-or-later

//! A step host that is lost without an exit record (s66, s83): what is recorded
//! about it, and the rules for going on.
//!
//! A lost host is an infrastructure fault, not a verdict. Nothing finished, so
//! nothing was judged, and no loss is a failed attempt of the task, a send-back or
//! a `max_rounds` round.
//!
//! * A command or test step is run again on a fresh host (its id ends `-host1`),
//!   and only that completed run is judged. A second loss in a row fails the step
//!   and names both.
//! * An agent step is continued: with `resume` and a note when the journal gave a
//!   session id, otherwise from the start, on a new invocation of the same run
//!   (`<key>-2`, …). A host that never came up (`START_LIMIT`, or a `--host-bin`
//!   that exits) is the same loss. At most [`MAX_AGENT_RESTARTS`] restarts per step
//!   run; the invocation directories under `h/` are the count, so a daemon restart
//!   does not reset it. A third loss fails the step and names them all. The retry
//!   ladder (`agent.ladder`) is for a crashed or silent agent, never for this.

use crate::daemon::{now_ms, Daemon};
use mason_core::{FailClass, StepRunId};
use mason_step_host::registry::LostReport;
use mason_store::HostLossRow;
use std::path::{Path, PathBuf};

/// The id of the fresh host that re-runs a step whose host `id` was lost.
pub fn rerun_id(id: &str) -> String {
    format!("{id}-host1")
}

/// What failed a step whose fresh host was lost too: both reports.
pub fn both(first: &LostReport, second: &LostReport) -> String {
    format!(
        "the command was lost twice in a row (its host died, and the fresh host it was re-run on died too). First: {first}. Second: {second}"
    )
}

/// How many times an agent step run is restarted after a lost host.
pub const MAX_AGENT_RESTARTS: usize = 2;

/// How close to a daemon start (ms) a lost host's last sign of life must be for the
/// loss to be put down to the restart: the daemon going away took it, or its stop
/// came so soon before the start that nothing else can be told apart.
pub const RESTART_WINDOW_MS: u64 = 120_000;

/// The file in the hosts directory's parent that lists every daemon start (ms, one per line).
fn starts_path(hosts_root: &Path) -> PathBuf {
    hosts_root
        .parent()
        .unwrap_or(hosts_root)
        .join("daemon-starts")
}

/// Record that a daemon starts now. Called once per start; the file keeps the last 200.
pub fn note_daemon_start(hosts_root: &Path, at_ms: u64) {
    let p = starts_path(hosts_root);
    let mut lines: Vec<String> = std::fs::read_to_string(&p)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    lines.push(at_ms.to_string());
    let keep = lines.len().saturating_sub(200);
    let _ = std::fs::write(&p, lines[keep..].join("\n") + "\n");
}

/// Whether a host last heard from at `last_ms` (or seen gone at `seen_ms` when it never
/// beat) was lost at a daemon restart. Several hosts lost at one start are one event:
/// each is judged against the same start, none counts against its task.
pub fn is_restart_loss(hosts_root: &Path, last_ms: Option<u64>, seen_ms: u64) -> bool {
    let t = last_ms.unwrap_or(seen_ms);
    let text = std::fs::read_to_string(starts_path(hosts_root)).unwrap_or_default();
    restart_loss_in(&text, t)
}

/// [`is_restart_loss`] over the text of the starts file.
pub fn restart_loss_in(starts: &str, t: u64) -> bool {
    starts
        .lines()
        .filter_map(|l| l.trim().parse::<u64>().ok())
        .any(|s| t + RESTART_WINDOW_MS >= s && t <= s + RESTART_WINDOW_MS / 4)
}

/// What failed an agent step that lost its host more often than it may be restarted.
pub fn agent_lost(reports: &[String]) -> String {
    format!(
        "the step host was lost {} times (an agent step is restarted at most {} times after a lost host): {}",
        reports.len(),
        MAX_AGENT_RESTARTS,
        reports
            .iter()
            .enumerate()
            .map(|(i, r)| format!("{}. {r}", i + 1))
            .collect::<Vec<_>>()
            .join("; ")
    )
}

/// What an agent is told when it is continued after a lost host.
pub fn interrupted_note() -> String {
    "Your session was interrupted: the machine running it was lost (a restart, or the process was killed). \
     Nothing you committed is gone, but work in progress may be. Verify the current state of the tree \
     first (look at the files and run git status and git log), then continue and finish the step."
        .into()
}

/// What an agent is told when its session was stopped for going silent.
pub fn silent_note() -> String {
    "Your session was stopped because it went silent: nothing was heard from it for a long time. \
     Nothing you committed is gone, but work in progress may be. Verify the current state of the tree \
     first (look at the files and run git status and git log), then continue and finish the step. \
     If a command hung, do not run it again the same way."
        .into()
}

/// The alert for an agent that has gone quiet.
pub fn silent_alert(task: u64, step: &str, silent_ms: u64) -> String {
    format!(
        "Task #{task}: the agent in `{step}` has been silent for {} (no output, no tool call running)",
        mason_core::progress_text::duration(silent_ms)
    )
}

/// What failed a step whose only host was lost and not re-run.
pub fn lost_text(rep: &LostReport) -> String {
    format!("the command was lost (its host died): {rep}")
}

/// Where a retry with no step named goes when the failure was not about the code
/// (s168): a lost host, a landing push or fetch error, a CI service error. Nothing
/// was judged, so the work before it stands: the failed step runs again, and no
/// agent redoes a change that already passed. A failure recorded as about the code
/// goes to the first developing step (`None`). A failure recorded without a class
/// (before failures were classified) is judged by its text, as before: only a
/// lost command stays on its step.
pub fn retry_step(failure: Option<&str>, class: Option<FailClass>, step: &str) -> Option<String> {
    let outside = match class {
        Some(c) => c == FailClass::Outside,
        None => failure?.contains("the command was lost"),
    };
    (outside && !step.is_empty()).then(|| step.to_string())
}

/// Keep what is known about a lost host, for `mason task show` and the alerts.
/// `run` is the step run it belonged to; without one (a suite run outside a task)
/// nothing is kept.
pub fn record(d: &Daemon, run: Option<StepRunId>, step: &str, rep: &LostReport, retried: bool) {
    let at = if rep.at_ms > 0 { rep.at_ms } else { now_ms() };
    record_as(d, run, step, &rep.id, at, rep.to_string(), retried)
}

/// [`record`] for a loss with no report: a host that never came up.
pub fn record_as(
    d: &Daemon,
    run: Option<StepRunId>,
    step: &str,
    host_id: &str,
    at_ms: u64,
    detail: String,
    retried: bool,
) {
    let Some(run) = run else { return };
    let Ok(w) = d.world() else { return };
    let Some(r) = w.step_runs.get(&run) else {
        return;
    };
    let row = HostLossRow {
        host_id: host_id.to_string(),
        task: r.task.0,
        run: run.0,
        step: step.to_string(),
        lost_at_ms: at_ms,
        detail,
        retried,
    };
    tracing::warn!(task = row.task, host = %row.host_id, "step host lost: {}", row.detail);
    let _ = d.store.record_host_loss(row);
}

#[cfg(test)]
mod tests {
    use super::{restart_loss_in, retry_step};

    #[test]
    fn a_loss_at_a_daemon_start_is_a_restart_loss_and_a_real_one_is_not() {
        let starts = "1000000\n9000000\n";
        // Died a few seconds before the start: a restart loss.
        assert!(restart_loss_in(starts, 995_000));
        assert!(restart_loss_in(starts, 9_000_000));
        // Died mid-run, long after one start and before the next.
        assert!(!restart_loss_in(starts, 5_000_000));
        assert!(!restart_loss_in(starts, 1_500_000));
        assert!(!restart_loss_in("", 5));
    }

    use mason_core::FailClass;

    #[test]
    fn a_retry_after_a_lost_command_stays_on_its_step() {
        let lost = Some("the command was lost (its host died)");
        assert_eq!(retry_step(lost, None, "test").as_deref(), Some("test"));
        let twice = Some("the command was lost twice in a row (...)");
        assert_eq!(retry_step(twice, None, "test").as_deref(), Some("test"));
        assert_eq!(
            retry_step(Some("`test` failed (exit status 101)"), None, "test"),
            None
        );
        assert_eq!(retry_step(None, None, "test"), None);
    }

    #[test]
    fn a_recorded_class_decides_where_a_retry_goes() {
        let push = Some("landing failed: could not push x: Permission denied");
        assert_eq!(
            retry_step(push, Some(FailClass::Outside), "land").as_deref(),
            Some("land")
        );
        assert_eq!(retry_step(push, Some(FailClass::Outside), ""), None);
        let red = Some("still red after 3 rounds");
        assert_eq!(retry_step(red, Some(FailClass::Code), "test"), None);
        // Recorded without a class: the text rule only.
        assert_eq!(retry_step(push, None, "land"), None);
    }
}
