// SPDX-License-Identifier: GPL-3.0-or-later

//! An agent going round in circles inside one develop step (s511, decision 0504): its last
//! several full `run_check` runs all ended red on the same failing tests. The detector is
//! pure over the recorded runs; `alert_for` reads the record and says whether those tests are
//! red on develop too.

use crate::daemon::Daemon;
use mason_core::TaskId;

/// How many full runs in a row with the same failing set make an alert. An alert threshold,
/// not a budget: it stops and refuses nothing.
pub const RUNS: usize = 3;

/// One recorded full run: when, whether it was green, and its failing tests as
/// `suite<TAB>test` entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub at_ms: u64,
    pub green: bool,
    pub tests: Vec<String>,
}

/// The same red set over the latest runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Circle {
    pub tests: Vec<String>,
    /// How many runs in a row, at least `RUNS`.
    pub runs: usize,
    /// When the first of those runs ended.
    pub since_ms: u64,
}

fn set(r: &Run) -> Vec<String> {
    let mut t = r.tests.clone();
    t.sort();
    t.dedup();
    t
}

/// `runs` oldest first. A circle exists when the last `n` runs are all red with the same,
/// non-empty set of failing tests. A green run, a red run whose tests could not be read, or a
/// different set ends it.
pub fn detect(runs: &[Run], n: usize) -> Option<Circle> {
    let last = runs.last()?;
    if last.green {
        return None;
    }
    let tests = set(last);
    if tests.is_empty() {
        return None;
    }
    let streak: Vec<&Run> = runs
        .iter()
        .rev()
        .take_while(|r| !r.green && set(r) == tests)
        .collect();
    (streak.len() >= n.max(1)).then(|| Circle {
        tests,
        runs: streak.len(),
        since_ms: streak.last().map_or(0, |r| r.at_ms),
    })
}

/// The circle of a task's current develop step, from the store.
pub fn of_task(d: &Daemon, task: TaskId) -> Option<Circle> {
    let runs: Vec<Run> = d
        .store
        .check_runs(task.0)
        .ok()?
        .into_iter()
        .map(|(at_ms, green, tests)| Run { at_ms, green, tests })
        .collect();
    detect(&runs, RUNS)
}

fn show(entry: &str) -> String {
    match entry.split_once('\t') {
        Some((suite, test)) if !suite.is_empty() => format!("{suite} :: {test}"),
        Some((_, test)) => test.to_string(),
        None => entry.to_string(),
    }
}

fn span(ms: u64) -> String {
    let m = ms / 60_000;
    if m >= 120 {
        format!("{} hours", m / 60)
    } else {
        format!("{m} minutes")
    }
}

/// The alert text: the task, the tests, how long, and whether they are red on develop.
pub fn text(d: &Daemon, task: TaskId, c: &Circle, now_ms: u64) -> String {
    let known = |e: &str| {
        let (suite, test) = e.split_once('\t').unwrap_or(("", e));
        crate::knownred::known_for(&d.store, suite)
            .iter()
            .any(|k| k.test == test)
    };
    let (on_develop, own): (Vec<&String>, Vec<&String>) = c.tests.iter().partition(|e| known(e));
    let names = c
        .tests
        .iter()
        .map(|e| show(e))
        .collect::<Vec<_>>()
        .join(", ");
    let red = if own.is_empty() {
        "all of them are red on develop, so not this task's".to_string()
    } else if on_develop.is_empty() {
        "none is known to be red on develop".to_string()
    } else {
        format!(
            "{} of {} are red on develop",
            on_develop.len(),
            c.tests.len()
        )
    };
    format!(
        "Task #{} has ended its last {} full run_check runs red on the same tests for {}: {names}; {red}",
        task.0,
        c.runs,
        span(now_ms.saturating_sub(c.since_ms)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn red(at: u64, t: &[&str]) -> Run {
        Run {
            at_ms: at,
            green: false,
            tests: t.iter().map(|s| format!("s\t{s}")).collect(),
        }
    }
    fn green(at: u64) -> Run {
        Run {
            at_ms: at,
            green: true,
            tests: vec![],
        }
    }

    #[test]
    fn three_red_runs_with_the_same_set_make_a_circle() {
        let runs = [red(1, &["a", "b"]), red(2, &["b", "a"]), red(3, &["a", "b"])];
        let c = detect(&runs, 3).unwrap();
        assert_eq!((c.runs, c.since_ms, c.tests.len()), (3, 1, 2));
        assert!(detect(&runs[1..], 3).is_none(), "two runs are not enough");
    }

    #[test]
    fn a_different_set_or_an_unreadable_red_does_not() {
        assert!(detect(&[red(1, &["a"]), red(2, &["a"]), red(3, &["a", "b"])], 3).is_none());
        assert!(detect(&[red(1, &["a"]), red(2, &["b"]), red(3, &["a"])], 3).is_none());
        assert!(detect(&[red(1, &[]), red(2, &[]), red(3, &[])], 3).is_none());
    }

    #[test]
    fn a_green_run_clears_it_and_the_count_starts_again() {
        let mut runs = vec![red(1, &["a"]), red(2, &["a"]), red(3, &["a"])];
        assert!(detect(&runs, 3).is_some());
        runs.push(green(4));
        assert!(detect(&runs, 3).is_none());
        runs.extend([red(5, &["a"]), red(6, &["a"])]);
        assert!(detect(&runs, 3).is_none());
    }

    #[test]
    fn a_longer_streak_counts_all_its_runs() {
        let runs = [green(0), red(1, &["a"]), red(2, &["a"]), red(3, &["a"]), red(4, &["a"])];
        let c = detect(&runs, 3).unwrap();
        assert_eq!((c.runs, c.since_ms), (4, 1));
    }
}
