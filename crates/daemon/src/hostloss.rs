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
use mason_core::StepRunId;
use mason_step_host::registry::LostReport;
use mason_store::HostLossRow;

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

/// What failed a step whose only host was lost and not re-run.
pub fn lost_text(rep: &LostReport) -> String {
    format!("the command was lost (its host died): {rep}")
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
