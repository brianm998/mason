// SPDX-License-Identifier: GPL-3.0-or-later

//! A test still red after the last round (s282): is it the change or a known red?
//!
//! Before a test step fails its task on the last red round, the tests that failed on every
//! run of every round are run at the merge base (up to 3 times, targeted). Red there on any
//! run: the test is not this change's, so it goes into the ledger (`knownred::add`), its fix
//! task is filed, and the step goes on without it. Green every time: the change broke it, and
//! the failure says so. No result: nothing is recorded, and the failure says it could not
//! check. A test that fails on every run in the test steps of 3 different tasks within 7 days
//! is added the same way, without a base run. The pure parts are first; the rest uses the
//! daemon.

use crate::basecheck;
use crate::jobs::Ctx;
use mason_core::*;

/// Base runs of the failing tests before they are called green at the base.
pub const BASE_RUNS: usize = 3;
/// Different tasks in which a test must fail on every run before it is added.
pub const MIN_TASKS: usize = 3;
/// The span those tasks must fall in.
pub const WINDOW_MS: u64 = 7 * 86_400_000;

/// The tests that failed on every run of every round. A round whose failing tests are not
/// known (`None`) leaves nothing proved.
pub fn persistent_across(rounds: &[Option<Vec<String>>]) -> Vec<String> {
    let mut it = rounds.iter();
    let Some(Some(first)) = it.next() else {
        return vec![];
    };
    let mut out = first.clone();
    for r in it {
        match r {
            Some(r) => out.retain(|t| r.contains(t)),
            None => return vec![],
        }
    }
    out
}

/// What the runs at the base said of each test.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Judged {
    /// Failed at the base on some run.
    pub red: Vec<String>,
    /// Ran, and passed, on every run.
    pub green: Vec<String>,
    /// At least one run gave no result, and none failed.
    pub unknown: Vec<String>,
}

/// `runs`: for each base run, the names that failed there, or `None` if it gave no result (it
/// could not run, or ran none of the tests).
pub fn judge(tests: &[String], runs: &[Option<Vec<String>>]) -> Judged {
    let mut j = Judged::default();
    for t in tests {
        if runs.iter().flatten().any(|f| f.contains(t)) {
            j.red.push(t.clone());
        } else if !runs.is_empty() && runs.iter().all(Option::is_some) {
            j.green.push(t.clone());
        } else {
            j.unknown.push(t.clone());
        }
    }
    j
}

/// Why the tests that are still the change's are, for the failure text.
pub fn why_not_excused(base: &str, j: &Judged, runs: usize) -> Option<String> {
    let mut parts = vec![];
    if !j.green.is_empty() {
        parts.push(format!(
            "passes at base {} on {runs} run{}; your change breaks `{}`",
            basecheck::short(base),
            if runs == 1 { "" } else { "s" },
            j.green.join("`, `")
        ));
    }
    if !j.unknown.is_empty() {
        parts.push(format!(
            "could not check at the base {}: `{}`",
            basecheck::short(base),
            j.unknown.join("`, `")
        ));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// The tests of `tests` that failed on every run in at least [`MIN_TASKS`] different tasks
/// since `now - WINDOW_MS` (counting the rows in the store, one per task and test).
pub fn across_tasks(
    store: &mason_store::Store,
    scope: &str,
    tests: &[String],
    now_ms: u64,
) -> Vec<String> {
    tests
        .iter()
        .filter(|t| {
            store
                .persistent_red_tasks(scope, t, now_ms.saturating_sub(WINDOW_MS))
                .is_ok_and(|v| v.len() >= MIN_TASKS)
        })
        .cloned()
        .collect()
}

// ---- with the daemon -------------------------------------------------------

/// Is this the last round the step may go back for (so another red fails the task)?
pub fn is_last_round(c: &Ctx) -> bool {
    let limit = c
        .res
        .max_rounds
        .as_ref()
        .map(|a| a.value)
        .or(c.step.max_rounds)
        .unwrap_or(1);
    c.step.on_red.is_some() && c.round() >= limit
}

fn round_file(c: &Ctx, round: u32) -> std::path::PathBuf {
    c.task_dir().join(format!("red-round-{round}.txt"))
}

/// Keep what failed on every run of this round, for the last round to read.
pub fn record_round(c: &Ctx, persistent: &[String]) {
    let _ = std::fs::write(round_file(c, c.round()), persistent.join("\n"));
}

fn rounds_so_far(c: &Ctx) -> Vec<Option<Vec<String>>> {
    (1..=c.round())
        .map(|r| {
            std::fs::read_to_string(round_file(c, r))
                .ok()
                .map(|t| t.lines().map(String::from).collect())
        })
        .collect()
}

/// Note the tests that failed on every run of this task's test step. A test that did so in
/// 3 or more different tasks within 7 days is added to the ledger, with a fix task, the same
/// as one proved red at the base (s282). A retry of the same task counts once.
pub fn record_persistent(c: &Ctx, scope: &str, tests: &[String]) {
    let now = crate::daemon::now_ms();
    for t in tests {
        let _ = c.d.store.record_persistent_red(scope, t, c.task.0, now);
    }
    let known = crate::knownred::known_for(&c.d.store, scope);
    let fresh: Vec<String> = tests
        .iter()
        .filter(|t| !known.iter().any(|k| &k.test == *t))
        .cloned()
        .collect();
    for t in across_tasks(&c.d.store, scope, &fresh, now) {
        let base = basecheck::merge_base(c).unwrap_or_default();
        let body = format!(
            "`{t}` failed on every run in the `{scope}` step of {MIN_TASKS} or more different tasks \
             within 7 days, so it is red for reasons that are not any one task's. It is now known \
             red (tasks that fail only on it are excused) until a run proves it green.\n\n\
             Filed by the test step's repeat check (task #{} was the latest).",
            c.task.0
        );
        if let Some(fix) = file_fix(c, &t, &base, &body) {
            crate::knownred::add(&c.d.store, scope, &t, &fix.text());
        }
    }
}

/// The open fix for `test` (an open task or issue naming it), or a new urgent task.
fn file_fix(c: &Ctx, test: &str, base: &str, body: &str) -> Option<basecheck::Fix> {
    let _g = basecheck::FILING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(f) = basecheck::existing_fix(c, test) {
        return Some(f);
    }
    let a =
        c.d.emit(EventKind::TaskFiled {
            project: c.project_id,
            title: format!("{test} fails on {}", basecheck::short(base)),
            body: body.to_string(),
            priority: Priority::Urgent,
            trust: TrustSource::Owner,
            explored: false,
            proposed: false,
            blockers: vec![],
            not_before: None,
        })
        .ok()?;
    a.effects.iter().find_map(|e| match e {
        Effect::TaskCreated { task } => Some(basecheck::Fix::Task(task.0)),
        _ => None,
    })
}

/// What the last-round check found.
#[derive(Debug, Default)]
pub struct Exhausted {
    /// Tests that are red at the base: in the ledger now, with a fix task. One line each.
    pub excused: Vec<String>,
    pub lines: Vec<String>,
    /// Why the tests that are not excused are the change's (or could not be checked).
    pub why: Option<String>,
}

/// The step is on its last red round. `candidates` are this round's failing tests that the
/// ledger and the base check did not already excuse; those that failed on every run of every
/// round are run at the merge base through `cmd_for` (the command that runs only the tests it
/// is given), after `build` if any. `scope` is the suite or step the ledger keeps them under.
pub fn on_last_round(
    c: &Ctx,
    scope: &str,
    build: Option<&str>,
    cmd_for: &dyn Fn(&[String]) -> String,
    candidates: &[String],
) -> Exhausted {
    let mut out = Exhausted::default();
    let every = persistent_across(&rounds_so_far(c));
    let tests: Vec<String> = candidates
        .iter()
        .filter(|t| every.contains(t))
        .cloned()
        .collect();
    if tests.is_empty() {
        return out;
    }
    let Some(base) = basecheck::merge_base(c) else {
        out.why = Some(format!(
            "could not check at the base (no merge base): `{}`",
            tests.join("`, `")
        ));
        return out;
    };
    let mut runs: Vec<Option<Vec<String>>> = vec![];
    let mut red: Vec<String> = vec![];
    let mut tail = String::new();
    for _ in 0..BASE_RUNS {
        let left: Vec<String> = tests.iter().filter(|t| !red.contains(t)).cloned().collect();
        if left.is_empty() {
            break;
        }
        let r = {
            let _g = basecheck::BASE_RUN
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            basecheck::run_at_base(c, &base, build, &cmd_for(&left), &left)
        }
        .and_then(|(failing, t, ran)| {
            tail = t;
            ran.then_some(failing)
        });
        if let Some(f) = &r {
            red.extend(left.iter().filter(|t| f.contains(t)).cloned());
        }
        runs.push(r);
    }
    let j = judge(&tests, &runs);
    for t in &j.red {
        let body = format!(
            "`{t}` failed on every run of every round of task #{} in `{scope}`, and also fails at \
             `{base}` (the merge base), so it fails without the task's change. It is now known \
             red (tasks that fail only on it are excused) until a run proves it green.\n\n\
             Output tail:\n```\n{tail}\n```\n\nFiled by the test step's last-round check.",
            c.task.0
        );
        let fix = file_fix(c, t, &base, &body);
        if let Some(f) = &fix {
            crate::knownred::add(&c.d.store, scope, t, &f.text());
        }
        out.lines.push(format!(
            "{t}: red at the merge base {} too; added to the known-red ledger, {}",
            basecheck::short(&base),
            fix.map_or("no fix could be filed".to_string(), |f| format!(
                "fix: {}",
                f.text()
            ))
        ));
        out.excused.push(t.clone());
    }
    out.why = why_not_excused(&base, &j, runs.len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn only_a_test_that_failed_in_every_round_is_a_candidate() {
        let r = [Some(s(&["a", "b"])), Some(s(&["a"])), Some(s(&["a", "c"]))];
        assert_eq!(persistent_across(&r), s(&["a"]));
        // A round whose failing tests are not known proves nothing.
        let r = [Some(s(&["a"])), None, Some(s(&["a"]))];
        assert!(persistent_across(&r).is_empty());
        assert!(persistent_across(&[]).is_empty());
    }

    #[test]
    fn red_at_the_base_on_any_run_is_red_and_green_needs_every_run() {
        let runs = [Some(s(&[])), Some(s(&["a"])), Some(s(&[]))];
        let j = judge(&s(&["a", "b"]), &runs);
        assert_eq!(j.red, s(&["a"]));
        assert_eq!(j.green, s(&["b"]));
        assert!(j.unknown.is_empty());
    }

    #[test]
    fn a_run_with_no_result_adds_no_proof() {
        // Green on two runs and one without a result: not proved green.
        let runs = [Some(s(&[])), None, Some(s(&[]))];
        let j = judge(&s(&["a"]), &runs);
        assert_eq!(j.unknown, s(&["a"]));
        assert!(j.red.is_empty() && j.green.is_empty());
        assert_eq!(judge(&s(&["a"]), &[]).unknown, s(&["a"]));
        let why = why_not_excused("0123456789abcdef", &j, 3).unwrap();
        assert!(why.contains("could not check at the base"), "{why}");
    }

    #[test]
    fn green_at_the_base_names_the_base_and_the_runs() {
        let j = judge(&s(&["a"]), &[Some(s(&[])), Some(s(&[])), Some(s(&[]))]);
        let why = why_not_excused("0123456789abcdef", &j, 3).unwrap();
        assert!(
            why.contains("passes at base 0123456789 on 3 runs; your change breaks `a`"),
            "{why}"
        );
        assert!(why_not_excused("b", &Judged::default(), 3).is_none());
    }

    #[test]
    fn three_different_tasks_count_and_three_retries_of_one_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let store = mason_store::Store::open(dir.path().join("s.db")).unwrap();
        let now = 10 * WINDOW_MS;
        let t = s(&["a"]);
        // One task, retried: the table keeps one row.
        for _ in 0..3 {
            store.record_persistent_red("test", "a", 7, now).unwrap();
        }
        assert!(across_tasks(&store, "test", &t, now).is_empty());
        store.record_persistent_red("test", "a", 8, now).unwrap();
        assert!(across_tasks(&store, "test", &t, now).is_empty());
        store.record_persistent_red("test", "a", 9, now).unwrap();
        assert_eq!(across_tasks(&store, "test", &t, now), t);
        // Old failures fall out of the window; another suite's do not count.
        assert!(across_tasks(&store, "test", &t, now + 2 * WINDOW_MS).is_empty());
        assert!(across_tasks(&store, "other", &t, now).is_empty());
    }
}
