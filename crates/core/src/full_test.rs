// SPDX-License-Identifier: GPL-3.0-or-later

//! The rules of the full test (s782, decision 0775): when the whole test is worth running, when
//! a project is quiet enough to run it, and what the status says about it. No I/O and no clock:
//! the record, the repository's answers and the time all come in as values.
//!
//! A full test is run at a time of day (`at`) and when the project has been quiet for a while
//! (`when_quiet`). Both ask the same question, [`decide`]: is the base branch's tip already
//! fully tested? The record is the `full_tests` table; a row is a start, a finish and a verdict.

use crate::task::State;

pub const GREEN: &str = "green";
pub const RED: &str = "red";
pub const CUT_OFF: &str = "cut off";

/// The three ways a full test starts.
pub const NIGHTLY: &str = "nightly";
pub const QUIET: &str = "quiet";
pub const MANUAL: &str = "manual";

/// One row of the record, as the decision needs it. Newest first when passed in a slice.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tested {
    pub commit: String,
    pub trigger: String,
    pub started_ms: u64,
    /// 0 while it runs.
    pub finished_ms: u64,
    /// `green`, `red`, `cut off`, or empty while it runs.
    pub verdict: String,
}

impl Tested {
    pub fn running(&self) -> bool {
        self.finished_ms == 0
    }
    /// A finished test that said something about its commit. A cut-off one did not.
    pub fn counts(&self) -> bool {
        !self.running() && (self.verdict == GREEN || self.verdict == RED)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub start: bool,
    pub reason: String,
}

pub fn short(sha: &str) -> &str {
    sha.get(..10).unwrap_or(sha)
}

/// Start a full test of `tip` or skip, saying why. `runs` is the record, newest first.
/// `changed(sha)` lists the files that differ from `sha` to the tip (`None` when the commit is
/// not in the repository); `selects(files)` says whether any suite is selected by them (false
/// for README.md and docs); `when(ms)` writes a time for the reason.
pub fn decide(
    tip: &str,
    runs: &[Tested],
    changed: &dyn Fn(&str) -> Option<Vec<String>>,
    selects: &dyn Fn(&[String]) -> bool,
    when: &dyn Fn(u64) -> String,
) -> Decision {
    let out = |start, reason: String| Decision { start, reason };
    if let Some(r) = runs.iter().find(|r| r.running()) {
        return out(
            false,
            format!("a full test of {} is running", short(&r.commit)),
        );
    }
    let Some(last) = runs.iter().find(|r| r.counts()) else {
        return out(true, "no full test is on record".into());
    };
    if last.commit == tip {
        return out(
            false,
            format!(
                "the tip {} was fully tested at {} ({})",
                short(tip),
                when(last.finished_ms),
                last.verdict
            ),
        );
    }
    match changed(&last.commit) {
        None => out(
            true,
            format!(
                "the last fully tested commit {} is not in the repository; cannot tell what changed",
                short(&last.commit)
            ),
        ),
        Some(files) if !selects(&files) => out(
            false,
            format!(
                "the tip {} differs from the fully tested {} ({} at {}) only in paths that select no suite ({} file(s))",
                short(tip),
                short(&last.commit),
                last.verdict,
                when(last.finished_ms),
                files.len()
            ),
        ),
        Some(files) => out(
            true,
            format!(
                "{} file(s) changed since the last full test {} ({} at {})",
                files.len(),
                short(&last.commit),
                last.verdict,
                when(last.finished_ms)
            ),
        ),
    }
}

// ---- what proves a batch (s804) ---------------------------------------------------------------

/// How long a green full test may stand before untested commits send batches back to the whole
/// suite, when the project does not say (`full_test_max_age`).
pub const DEFAULT_MAX_AGE_MS: u64 = 36 * 3_600_000;

/// What a batch with `narrow = true` runs, now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gate {
    /// The narrowed selection; the full test is the backstop.
    Narrow,
    /// The whole suite, for this reason.
    Whole(String),
}

/// Is the backstop standing? `runs` is the record, newest first. `commits_since(sha)` counts the
/// commits on the base after `sha` (`None`: cannot tell, taken as some). The last full test must
/// be green, and a green one older than `max_age_ms` must have no untested commits since. With
/// no full test on record the whole suite runs. A red stays in force until a green full test is
/// recorded, because the newest finished test is the one that counts.
pub fn narrow_gate(
    runs: &[Tested],
    now: u64,
    max_age_ms: u64,
    commits_since: &dyn Fn(&str) -> Option<u64>,
    when: &dyn Fn(u64) -> String,
) -> Gate {
    let Some(last) = runs.iter().find(|r| r.counts()) else {
        return Gate::Whole("no full test is on record, so nothing backs a narrowed batch".into());
    };
    if last.verdict == RED {
        return Gate::Whole(format!(
            "the last full test ({} at {}) was red; batches run the whole suite until a full test is green",
            short(&last.commit),
            when(last.finished_ms)
        ));
    }
    if now.saturating_sub(last.finished_ms) > max_age_ms && commits_since(&last.commit) != Some(0) {
        return Gate::Whole(format!(
            "the last green full test ({} at {}) is older than {} h and commits have landed since; batches run the whole suite until a full test is green",
            short(&last.commit),
            when(last.finished_ms),
            max_age_ms / 3_600_000
        ));
    }
    Gate::Narrow
}

// ---- quiet ---------------------------------------------------------------------------------

/// Does a task in this state keep the project busy? Queued, exploring, developing, testing,
/// review and landing do (a landing batch is tasks in landing). Tasks waiting for the owner
/// (proposed, deferred, needs attention, failed) and finished ones do not.
pub fn busy_state(s: State) -> bool {
    matches!(
        s,
        State::Queued
            | State::Exploring
            | State::Developing
            | State::Testing
            | State::Review
            | State::Landing
    )
}

/// The quiet timer of one schedule. It starts when the project goes quiet, restarts whenever the
/// project stops being quiet, and fires once when it reaches the duration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QuietTimer {
    /// When the project went quiet; `None` while it is busy (or not looked at yet).
    pub since: Option<u64>,
    /// It fired in this quiet spell.
    pub fired: bool,
}

impl QuietTimer {
    /// One look. Returns true exactly once per quiet spell, when it has lasted `dur_ms`.
    pub fn look(&mut self, busy: bool, now: u64, dur_ms: u64) -> bool {
        if busy {
            *self = QuietTimer::default();
            return false;
        }
        let since = *self.since.get_or_insert(now);
        if !self.fired && now.saturating_sub(since) >= dur_ms {
            self.fired = true;
            return true;
        }
        false
    }

    /// Whole minutes quiet so far; `None` while busy.
    pub fn quiet_min(&self, now: u64) -> Option<u64> {
        self.since.map(|s| now.saturating_sub(s) / 60_000)
    }
}

// ---- showing ---------------------------------------------------------------------------------

/// `nightly at 01:00`, `quiet for 12 of 30 min`, or both joined by `; `. `quiet_min` is `None`
/// while the project is busy.
pub fn next_text(
    at_min: Option<u32>,
    when_quiet_ms: Option<u64>,
    quiet_min: Option<u64>,
) -> String {
    let mut parts = vec![];
    if let Some(a) = at_min {
        parts.push(format!("nightly at {:02}:{:02}", a / 60, a % 60));
    }
    if let Some(d) = when_quiet_ms {
        let of = d / 60_000;
        parts.push(match quiet_min {
            Some(m) if m >= of => format!("quiet for {of} min: fires once"),
            Some(m) => format!("quiet for {m} of {of} min"),
            None => format!("when quiet for {of} min (busy now)"),
        });
    }
    if parts.is_empty() {
        "no trigger".into()
    } else {
        parts.join("; ")
    }
}

// ---- the tasks in a range -------------------------------------------------------------------

/// The task numbers named by merge subjects such as `Merge branch 'mason/mason/t787'`: a branch
/// `mason/<project>/t<N>`. Sorted, each once. Batch branches (`mason/int/...`) name no task.
pub fn tasks_in(subjects: &[String]) -> Vec<u64> {
    let mut out: Vec<u64> = vec![];
    for s in subjects {
        let mut rest = s.as_str();
        while let Some(i) = rest.find("mason/") {
            rest = &rest[i + 6..];
            let token: &str = rest
                .split(|c: char| c == '\'' || c == '"' || c.is_whitespace())
                .next()
                .unwrap_or("");
            let mut segs = token.split('/');
            let (project, last) = (segs.next(), segs.next_back());
            if let (Some(p), Some(l)) = (project, last) {
                if p != "int" {
                    if let Some(n) = l.strip_prefix('t').and_then(|n| n.parse::<u64>().ok()) {
                        out.push(n);
                    }
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60_000;

    #[test]
    fn the_quiet_timer_fires_once_per_quiet_spell() {
        let mut t = QuietTimer::default();
        let dur = 30 * MIN;
        // Busy, then quiet for 29 minutes: nothing.
        assert!(!t.look(true, 0, dur));
        assert!(!t.look(false, MIN, dur));
        assert!(!t.look(false, 30 * MIN, dur));
        assert_eq!(t.quiet_min(30 * MIN), Some(29));
        // A task is admitted: the timer restarts.
        assert!(!t.look(true, 31 * MIN, dur));
        assert_eq!(t.quiet_min(31 * MIN), None);
        assert!(!t.look(false, 32 * MIN, dur));
        assert!(!t.look(false, 61 * MIN, dur));
        // Quiet for 30: it fires.
        assert!(t.look(false, 62 * MIN, dur));
        // Quiet for another hour: it does not fire again.
        for m in [63, 90, 122] {
            assert!(!t.look(false, m * MIN, dur));
        }
        // Busy, then quiet again: it can fire again.
        assert!(!t.look(true, 130 * MIN, dur));
        assert!(!t.look(false, 131 * MIN, dur));
        assert!(t.look(false, 161 * MIN, dur));
    }

    #[test]
    fn only_work_in_the_pipeline_is_busy() {
        for s in State::ALL {
            let waiting = matches!(
                s,
                State::Proposed
                    | State::Deferred
                    | State::NeedsAttention
                    | State::Failed
                    | State::Canceled
                    | State::Landed
            );
            assert_eq!(busy_state(s), !waiting, "{s:?}");
        }
    }

    fn row(commit: &str, trigger: &str, finished: u64, verdict: &str) -> Tested {
        Tested {
            commit: commit.into(),
            trigger: trigger.into(),
            started_ms: 1,
            finished_ms: finished,
            verdict: verdict.into(),
        }
    }

    fn files(f: &[&str]) -> Option<Vec<String>> {
        Some(f.iter().map(|s| s.to_string()).collect())
    }

    fn selects(f: &[String]) -> bool {
        !f.iter()
            .all(|p| p.ends_with(".md") || p.starts_with("docs/"))
    }

    fn when(_: u64) -> String {
        "01:00".into()
    }

    #[test]
    fn the_decision_starts_only_for_a_tip_not_fully_tested() {
        let done = [row("abc123", NIGHTLY, 5, GREEN)];
        // The tip was tested.
        let d = decide("abc123", &done, &|_| files(&[]), &selects, &when);
        assert!(!d.start);
        assert_eq!(d.reason, "the tip abc123 was fully tested at 01:00 (green)");
        // Only the README changed.
        let d = decide("def456", &done, &|_| files(&["README.md"]), &selects, &when);
        assert!(!d.start, "{}", d.reason);
        assert!(d.reason.contains("select no suite"));
        // A crate file changed.
        let d = decide(
            "def456",
            &done,
            &|_| files(&["README.md", "crates/core/src/lib.rs"]),
            &selects,
            &when,
        );
        assert!(d.start, "{}", d.reason);
        // A full test in flight (for any commit): no second one.
        let running = [row("def456", QUIET, 0, ""), done[0].clone()];
        let d = decide("def456", &running, &|_| files(&["a.rs"]), &selects, &when);
        assert!(!d.start);
        assert_eq!(d.reason, "a full test of def456 is running");
        // Nothing on record, or a commit the repository lost: start.
        assert!(decide("t", &[], &|_| None, &selects, &when).start);
        assert!(decide("t", &done, &|_| None, &selects, &when).start);
        // A cut-off test says nothing about its commit.
        let cut = [row("t", QUIET, 9, CUT_OFF)];
        assert!(decide("t", &cut, &|_| None, &selects, &when).start);
    }

    #[test]
    fn nightly_after_a_quiet_run_on_the_same_tip_skips_with_the_reason() {
        let done = [row("abc123", QUIET, 5, RED)];
        let d = decide("abc123", &done, &|_| files(&[]), &selects, &when);
        assert!(!d.start);
        assert!(d.reason.contains("fully tested") && d.reason.contains("(red)"));
    }

    #[test]
    fn the_next_trigger_is_written_out() {
        assert_eq!(
            next_text(Some(60), Some(30 * MIN), Some(12)),
            "nightly at 01:00; quiet for 12 of 30 min"
        );
        assert_eq!(
            next_text(None, Some(30 * MIN), None),
            "when quiet for 30 min (busy now)"
        );
        assert_eq!(next_text(Some(90), None, None), "nightly at 01:30");
        assert_eq!(next_text(None, None, None), "no trigger");
    }

    #[test]
    fn landed_tasks_are_read_from_merge_subjects() {
        let s: Vec<String> = [
            "Merge branch 'mason/mason/t787'",
            "Regenerate the README status block",
            "Merge remote-tracking branch 'origin/develop' into mason/int/mason/b488",
            "Merge branch 'mason/star/t12' into mason/int/star/b9",
            "Merge branch 'mason/mason/t787'",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(tasks_in(&s), vec![12, 787]);
    }

    fn run(verdict: &str, finished: u64) -> Tested {
        Tested {
            commit: "abcdef0123456789".into(),
            trigger: NIGHTLY.into(),
            started_ms: 1,
            finished_ms: finished,
            verdict: verdict.into(),
        }
    }

    fn gate(runs: &[Tested], now: u64, since: Option<u64>) -> Gate {
        narrow_gate(runs, now, DEFAULT_MAX_AGE_MS, &|_| since, &|ms| {
            ms.to_string()
        })
    }

    #[test]
    fn a_fresh_green_narrows_and_a_red_or_missing_one_does_not() {
        assert_eq!(gate(&[run(GREEN, 1000)], 2000, Some(5)), Gate::Narrow);
        assert!(matches!(gate(&[], 2000, None), Gate::Whole(w) if w.contains("no full test")));
        let red = gate(&[run(RED, 1000)], 2000, Some(0));
        assert!(matches!(red, Gate::Whole(w) if w.contains("was red")));
    }

    #[test]
    fn a_red_is_in_force_until_a_green_is_newer_and_cut_off_runs_do_not_count() {
        let runs = [run(GREEN, 3000), run(RED, 2000)];
        assert_eq!(gate(&runs, 4000, Some(1)), Gate::Narrow);
        let runs = [run(CUT_OFF, 3000), run(RED, 2000), run(GREEN, 1000)];
        assert!(matches!(gate(&runs, 4000, Some(1)), Gate::Whole(_)));
    }

    #[test]
    fn an_old_green_with_untested_commits_goes_whole() {
        let now = DEFAULT_MAX_AGE_MS + 5000;
        assert!(matches!(
            gate(&[run(GREEN, 1000)], now, Some(3)),
            Gate::Whole(w) if w.contains("older than 36 h")
        ));
        assert!(matches!(
            gate(&[run(GREEN, 1000)], now, None),
            Gate::Whole(_)
        ));
        assert_eq!(gate(&[run(GREEN, 1000)], now, Some(0)), Gate::Narrow);
    }
}
