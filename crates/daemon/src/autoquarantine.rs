// SPDX-License-Identifier: GPL-3.0-or-later

//! Quarantining repeat flakes by itself (s253, decision 0248).
//!
//! A test with flaky records from three or more distinct tasks within seven days, that is not
//! quarantined and that its suite has been seen to pass, is quarantined for 14 days (owner
//! `mason`, reason `auto: flaky in N tasks since <date>`), and one task, "flaky: <suite> <test>",
//! is filed (normal priority, not filed again while one with that title is open). The
//! quarantine is lifted when that task lands, and expires by itself otherwise. A quarantined test
//! still runs and is reported; it only stops deciding the step (`RedClass::Quarantined`).
//!
//! "Seen passing" is the guard against a parser that takes a log line for a test name: such a
//! name is never seen passing in a green run or at a base, so it is never quarantined.

use crate::daemon::{now_ms, Daemon};
use mason_core::*;
use mason_testing::gates::{check_quarantine, Quarantine};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

/// Distinct tasks that must have seen the flake.
pub const MIN_TASKS: usize = 3;
/// ... within this many days.
pub const WINDOW_DAYS: u64 = 7;
/// How long the quarantine lasts.
pub const QUARANTINE_DAYS: u64 = 14;
/// The owner an automatic quarantine carries.
pub const OWNER: &str = "mason";

const DAY_MS: u64 = 86_400_000;

/// One filing at a time: two tasks that flake together file one task.
static GATE: Mutex<()> = Mutex::new(());

/// The tasks that saw `(suite, test)` flake within the window, oldest first with the time of the
/// first. `rows` is `(suite, test, task, at_ms)`.
pub fn flaked_in(
    rows: &[(String, String, u64, u64)],
    suite: &str,
    test: &str,
    now: u64,
) -> (BTreeSet<u64>, Option<u64>) {
    let since = now.saturating_sub(WINDOW_DAYS * DAY_MS);
    let mine: Vec<&(String, String, u64, u64)> = rows
        .iter()
        .filter(|r| r.0 == suite && r.1 == test && r.3 >= since)
        .collect();
    (
        mine.iter().map(|r| r.2).collect(),
        mine.iter().map(|r| r.3).min(),
    )
}

/// `YYYY-MM-DD` of a time in ms (UTC).
pub fn date_of(ms: u64) -> String {
    let days = (ms / DAY_MS) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

/// The reason an automatic quarantine carries (the task that tracks it follows).
pub fn reason(tasks: usize, since_ms: u64) -> String {
    format!("auto: flaky in {tasks} tasks since {}", date_of(since_ms))
}

/// The title of the task filed for a flake.
pub fn title(suite: &str, test: &str) -> String {
    format!("flaky: {suite} {test}")
}

/// The task number at the end of an automatic quarantine's reason (`; task #12`).
pub fn task_in(reason: &str) -> Option<u64> {
    reason
        .rsplit_once("; task #")?
        .1
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// Called after flakes were recorded for `tests` of `scope` (a suite, or a test step's name):
/// quarantines each one that has reached the threshold.
pub fn consider(d: &Arc<Daemon>, project: ProjectId, scope: &str, tests: &[String]) {
    let _g = GATE.lock().unwrap_or_else(|e| e.into_inner());
    let now = now_ms();
    let Ok(rows) = d
        .store
        .flaky_tasks_since(now.saturating_sub(WINDOW_DAYS * DAY_MS))
    else {
        return;
    };
    let active = d.store.quarantined(now).unwrap_or_default();
    for test in tests {
        if active.iter().any(|q| q.0 == scope && &q.1 == test) {
            continue;
        }
        let (tasks, first) = flaked_in(&rows, scope, test, now);
        let Some(first) = first.filter(|_| tasks.len() >= MIN_TASKS) else {
            continue;
        };
        // A name no run of the suite has ever passed is a parser's mistake, not a test.
        if !d.store.seen_passing(scope, test).unwrap_or(false) {
            tracing::info!(%scope, %test, "flaky in {} tasks, but never seen passing: not quarantined", tasks.len());
            continue;
        }
        let q = Quarantine {
            suite: scope.into(),
            test: test.clone(),
            owner: OWNER.into(),
            expires_at_ms: now + QUARANTINE_DAYS * DAY_MS,
            reason: reason(tasks.len(), first),
        };
        if let Err(e) = check_quarantine(&q, now, 30) {
            tracing::warn!(%scope, %test, "automatic quarantine refused: {e}");
            continue;
        }
        let task = file_once(d, project, scope, test, &q.reason);
        let full = match task {
            Some(t) => format!("{}; task #{}", q.reason, t.0),
            None => q.reason.clone(),
        };
        let note = format!(
            "flaked in tasks {}; first on {}",
            tasks
                .iter()
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join(", "),
            date_of(first)
        );
        let _ =
            d.store
                .put_flaky_quarantine(scope, test, OWNER, q.expires_at_ms, &full, &note, now);
        tracing::info!(%scope, %test, "quarantined for {QUARANTINE_DAYS} days: {full}");
    }
}

/// The open task for this flake, or a new one at normal priority.
fn file_once(
    d: &Arc<Daemon>,
    project: ProjectId,
    suite: &str,
    test: &str,
    why: &str,
) -> Option<TaskId> {
    let title = title(suite, test);
    if let Ok(w) = d.world() {
        let mut open: Vec<TaskId> = w
            .tasks
            .values()
            .filter(|t| t.project == project && !t.state.is_terminal() && t.title == title)
            .map(|t| t.id)
            .collect();
        open.sort_by_key(|t| t.0);
        if let Some(t) = open.first() {
            return Some(*t);
        }
    }
    let body = format!(
        "`{test}` of suite `{suite}` flaked in tasks that did not touch it ({why}). It is \
         quarantined for {QUARANTINE_DAYS} days: it still runs and is reported, but it no longer \
         decides a test step. Find why it flakes and fix it; the quarantine is lifted when this \
         task lands.\n\nFiled by the automatic quarantine."
    );
    let a = d
        .emit(EventKind::TaskFiled {
            project,
            title,
            body,
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            explored: false,
            proposed: false,
            blockers: vec![],
            not_before: None,
        })
        .ok()?;
    a.effects.iter().find_map(|e| match e {
        Effect::TaskCreated { task } => Some(*task),
        _ => None,
    })
}

/// The automatic quarantines (`(suite, test, owner, expires, reason)`) whose task has landed.
pub fn to_lift(
    quarantined: &[(String, String, String, u64, String)],
    landed: impl Fn(u64) -> bool,
) -> Vec<(String, String)> {
    quarantined
        .iter()
        .filter(|q| q.2 == OWNER)
        .filter(|q| task_in(&q.4).is_some_and(&landed))
        .map(|q| (q.0.clone(), q.1.clone()))
        .collect()
}

/// Lift the automatic quarantines whose task has landed. Called from the sweep.
pub fn lift_landed(d: &Arc<Daemon>) {
    let Ok(w) = d.world() else { return };
    let q = d.store.quarantined(now_ms()).unwrap_or_default();
    let lift = to_lift(&q, |t| {
        w.tasks
            .get(&TaskId(t))
            .is_some_and(|t| t.state == State::Landed)
    });
    for (suite, test) in lift {
        let _ = d.store.remove_quarantine(&suite, &test);
        tracing::info!(%suite, %test, "quarantine lifted: its task landed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(suite: &str, test: &str, task: u64, at: u64) -> (String, String, u64, u64) {
        (suite.into(), test.into(), task, at)
    }

    #[test]
    fn three_distinct_tasks_within_seven_days_reach_the_threshold_and_one_task_flaking_three_times_does_not(
    ) {
        let now = 100 * DAY_MS;
        let rows = vec![
            row("s", "a", 1, now - DAY_MS),
            row("s", "a", 2, now - 2 * DAY_MS),
            row("s", "a", 3, now - 6 * DAY_MS),
            // The same task three times, and a record that is too old.
            row("s", "b", 7, now - DAY_MS),
            row("s", "b", 7, now - DAY_MS),
            row("s", "b", 7, now - 2 * DAY_MS),
            row("s", "c", 1, now - 8 * DAY_MS),
            row("s", "c", 2, now - DAY_MS),
            row("s", "c", 3, now - DAY_MS),
            // Another suite's test of the same name.
            row("t", "a", 9, now - DAY_MS),
        ];
        let (tasks, first) = flaked_in(&rows, "s", "a", now);
        assert_eq!(tasks.len(), 3);
        assert_eq!(first, Some(now - 6 * DAY_MS));
        assert_eq!(flaked_in(&rows, "s", "b", now).0.len(), 1);
        assert_eq!(
            flaked_in(&rows, "s", "c", now).0.len(),
            2,
            "the old one is out of the window"
        );
    }

    #[test]
    fn the_reason_names_the_count_and_the_first_date_and_the_task_is_read_back() {
        let r = reason(3, 20_040 * DAY_MS);
        assert_eq!(r, "auto: flaky in 3 tasks since 2024-11-13");
        assert_eq!(task_in(&format!("{r}; task #42")), Some(42));
        assert_eq!(task_in(&r), None);
        assert_eq!(
            title("cargo-workspace", "a::b"),
            "flaky: cargo-workspace a::b"
        );
    }

    #[test]
    fn a_quarantine_is_lifted_when_its_task_has_landed_and_only_an_automatic_one() {
        let q = |owner: &str, why: &str| {
            (
                "s".to_string(),
                "t".to_string(),
                owner.to_string(),
                1,
                why.to_string(),
            )
        };
        let landed = |n: u64| n == 5;
        assert_eq!(
            to_lift(
                &[q(
                    "mason",
                    "auto: flaky in 3 tasks since 2026-11-01; task #5"
                )],
                landed
            )
            .len(),
            1
        );
        assert!(to_lift(
            &[q(
                "mason",
                "auto: flaky in 3 tasks since 2026-11-01; task #6"
            )],
            landed
        )
        .is_empty());
        // The owner's own quarantine, even if it names a landed task, stays.
        assert!(to_lift(&[q("brian", "fixed by; task #5")], landed).is_empty());
        assert!(to_lift(
            &[q("mason", "auto: flaky in 3 tasks since 2026-11-01")],
            landed
        )
        .is_empty());
    }

    #[test]
    fn the_automatic_quarantine_is_one_the_gate_accepts() {
        let now = 1_000 * DAY_MS;
        let q = Quarantine {
            suite: "s".into(),
            test: "t".into(),
            owner: OWNER.into(),
            expires_at_ms: now + QUARANTINE_DAYS * DAY_MS,
            reason: reason(3, now),
        };
        assert!(check_quarantine(&q, now, 30).is_ok());
    }
}
