// SPDX-License-Identifier: GPL-3.0-or-later

//! How full each agent session's context window got (s1108): kept per session beside the
//! tokens, shown on the task's step rows, and counted against send-backs by `mason report`.
//! The figures are the adapter's, in tokens; the percentages are worked out here.

use crate::daemon::{now_ms, Daemon};
use crate::usage::Attempt;
use mason_adapters::runner::ContextStats;
use mason_store::ContextRun;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The report's buckets, in the order printed. A run that compacted is only in the last.
pub const BUCKETS: [&str; 5] = ["<50%", "50-75%", "75-90%", ">=90%", "compacted"];

/// Keep what one session of a step attempt reported. Nothing reported: no row.
pub fn record(d: &Daemon, who: &Attempt, run: u64, session: &str, c: &ContextStats) {
    if !c.reported() {
        return;
    }
    let row = ContextRun {
        attempt: who.key.into(),
        session: session.into(),
        run,
        task: who.task,
        project: who.project.into(),
        step: who.step.into(),
        peak_used: c.peak_used,
        final_used: c.final_used,
        window: c.window,
        compactions: c.compactions,
        at_ms: now_ms(),
    };
    if let Err(e) = d.store.record_context_run(row) {
        tracing::warn!(task = who.task, "cannot store the step's context fill: {e}");
    }
}

/// `used` of `window`, whole percent rounded down, at most 100. `None` with no window.
fn percent(used: u64, window: u64) -> Option<u64> {
    (window > 0).then(|| (u128::from(used) * 100 / u128::from(window)).min(100) as u64)
}

/// One step run's figures over all its sessions: the highest peak, the latest final
/// figure and every compaction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunFill {
    pub peak_percent: Option<u64>,
    pub final_percent: Option<u64>,
    pub compactions: u32,
}

impl RunFill {
    fn add(&mut self, r: &ContextRun) {
        self.peak_percent = self.peak_percent.max(percent(r.peak_used, r.window));
        // Rows come oldest first.
        self.final_percent = percent(r.final_used, r.window).or(self.final_percent);
        self.compactions += r.compactions;
    }

    /// The step row's line, as `mason task show` and both monitors print it.
    pub fn line(&self) -> String {
        let mut parts = vec![];
        if let Some(p) = self.peak_percent {
            parts.push(format!("peak {p}%"));
        }
        if let Some(p) = self.final_percent {
            parts.push(format!("final {p}%"));
        }
        if self.compactions > 0 {
            let s = if self.compactions == 1 { "" } else { "s" };
            parts.push(format!("{} compaction{s}", self.compactions));
        }
        format!("context {}", parts.join(", "))
    }

    pub fn json(&self) -> Value {
        json!({
            "line": self.line(),
            "peak_percent": self.peak_percent,
            "final_percent": self.final_percent,
            "compactions": self.compactions,
        })
    }

    fn bucket(&self) -> Option<usize> {
        if self.compactions > 0 {
            return Some(4);
        }
        Some(match self.peak_percent? {
            0..=49 => 0,
            50..=74 => 1,
            75..=89 => 2,
            _ => 3,
        })
    }
}

/// The figures per step run of rows ordered oldest first.
pub fn per_run<'a>(rows: impl IntoIterator<Item = &'a ContextRun>) -> BTreeMap<u64, RunFill> {
    let mut out: BTreeMap<u64, RunFill> = BTreeMap::new();
    for r in rows {
        out.entry(r.run).or_default().add(r);
    }
    out
}

/// One task's step runs that reported their context.
pub fn of_task(d: &Daemon, task: u64) -> BTreeMap<u64, RunFill> {
    per_run(&d.store.context_runs_of_task(task).unwrap_or_default())
}

/// The report's section: for each step and bucket with a run in it, how many runs there
/// were and how many had their work sent back (`sent_back` holds those step runs).
pub fn by_fill<'a>(
    rows: impl IntoIterator<Item = &'a ContextRun>,
    sent_back: &BTreeSet<u64>,
) -> Value {
    let rows: Vec<&ContextRun> = rows.into_iter().collect();
    let step_of: BTreeMap<u64, &str> = rows.iter().map(|r| (r.run, r.step.as_str())).collect();
    // (step, bucket) -> (runs, sent back)
    let mut counts: BTreeMap<(&str, usize), (u64, u64)> = BTreeMap::new();
    for (run, fill) in per_run(rows.iter().copied()) {
        let Some(bucket) = fill.bucket() else {
            continue;
        };
        let c = counts.entry((step_of[&run], bucket)).or_default();
        c.0 += 1;
        c.1 += u64::from(sent_back.contains(&run));
    }
    counts
        .into_iter()
        .map(|((step, b), (runs, back))| {
            json!({"step": step, "bucket": BUCKETS[b], "runs": runs, "sent_back": back,
                   "rate_percent": (back * 100 + runs / 2) / runs})
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(peak_used: u64, final_used: u64, window: u64, compactions: u32) -> ContextRun {
        ContextRun {
            run: 1,
            peak_used,
            final_used,
            window,
            compactions,
            ..Default::default()
        }
    }

    #[test]
    fn a_step_row_line_covers_every_session_of_the_run() {
        let rows = [row(124, 100, 200, 1), row(60, 40, 200, 1)];
        assert_eq!(
            per_run(&rows)[&1].line(),
            "context peak 62%, final 20%, 2 compactions"
        );
        // Only a compaction was seen: no percentage is made up.
        let rows = [row(0, 0, 0, 1)];
        assert_eq!(per_run(&rows)[&1].line(), "context 1 compaction");
    }

    #[test]
    fn buckets_split_at_50_75_and_90() {
        let bucket = |peak| per_run(&[row(peak, peak, 100, 0)])[&1].bucket();
        assert_eq!(
            [0, 49, 50, 74, 75, 89, 90, 120].map(bucket),
            [0, 0, 1, 1, 2, 2, 3, 3].map(Some)
        );
    }
}
