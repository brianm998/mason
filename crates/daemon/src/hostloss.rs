// SPDX-License-Identifier: GPL-3.0-or-later

//! A step host that is lost without an exit record (s66): what is recorded about
//! it, and the rule that a command step is run once more on a fresh host.
//!
//! A lost host is an infrastructure fault, not a verdict. The command did not
//! finish, so nothing was judged: the step is run again on a fresh host (its id
//! ends `-host1`), and only that completed run is judged. This is not a failed
//! attempt of the task and not a send-back. A second loss in a row fails the
//! step and names both. Agent steps keep their own resume ladder.

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

/// What failed a step whose only host was lost and not re-run.
pub fn lost_text(rep: &LostReport) -> String {
    format!("the command was lost (its host died): {rep}")
}

/// Keep what is known about a lost host, for `mason task show` and the alerts.
/// `run` is the step run it belonged to; without one (a suite run outside a task)
/// nothing is kept.
pub fn record(d: &Daemon, run: Option<StepRunId>, step: &str, rep: &LostReport, retried: bool) {
    let Some(run) = run else { return };
    let Ok(w) = d.world() else { return };
    let Some(r) = w.step_runs.get(&run) else {
        return;
    };
    let row = HostLossRow {
        host_id: rep.id.clone(),
        task: r.task.0,
        run: run.0,
        step: step.to_string(),
        lost_at_ms: if rep.at_ms > 0 { rep.at_ms } else { now_ms() },
        detail: rep.to_string(),
        retried,
    };
    tracing::warn!(task = row.task, host = %row.host_id, "step host lost: {}", row.detail);
    let _ = d.store.record_host_loss(row);
}
