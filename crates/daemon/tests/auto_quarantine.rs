// SPDX-License-Identifier: GPL-3.0-or-later

//! s253: a test that flaked in three different tasks within a week, and that its suite has been
//! seen to pass, is quarantined for 14 days by `mason` and gets one task; a name never seen
//! passing is not, one task flaking three times is not, and a quarantine that is in force is not
//! made twice.

mod support;
use mason_core::*;
use mason_daemon::autoquarantine::{consider, OWNER};
use mason_daemon::daemon::now_ms;
use support::*;

const DAY: u64 = 86_400_000;

fn flake(
    d: &std::sync::Arc<mason_daemon::Daemon>,
    suite: &str,
    test: &str,
    task: u64,
    days_ago: u64,
) {
    d.store
        .record_flaky(suite, test, task, now_ms() - days_ago * DAY)
        .unwrap();
}

fn titled(d: &std::sync::Arc<mason_daemon::Daemon>, title: &str) -> Vec<TaskId> {
    d.world()
        .unwrap()
        .tasks
        .values()
        .filter(|t| t.title == title)
        .map(|t| t.id)
        .collect()
}

#[test]
fn a_test_that_flaked_in_three_tasks_is_quarantined_for_fourteen_days_with_one_task() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let pid = d.project_id_by_name("scratch").unwrap();
    // Seen passing, and flaked in three different tasks.
    d.store
        .note_passing("unit", &["flaky_one".into()], now_ms())
        .unwrap();
    for (task, ago) in [(1, 1), (2, 2), (3, 5)] {
        flake(&d, "unit", "flaky_one", task, ago);
    }
    consider(&d, pid, "unit", &["flaky_one".into()]);
    let q = d.store.quarantined(now_ms()).unwrap();
    assert_eq!(q.len(), 1, "{q:?}");
    let (suite, test, owner, expires, reason) = q[0].clone();
    assert_eq!(
        (suite.as_str(), test.as_str(), owner.as_str()),
        ("unit", "flaky_one", OWNER)
    );
    assert!(
        reason.starts_with("auto: flaky in 3 tasks since "),
        "{reason}"
    );
    // 14 days: in force now and in 13 days, gone after 15.
    assert!(
        expires > now_ms() + 13 * DAY && expires <= now_ms() + 14 * DAY + 60_000,
        "{expires}"
    );
    assert!(d.store.quarantined(now_ms() + 15 * DAY).unwrap().is_empty());
    // One task, normal priority, that the reason names.
    let tasks = titled(&d, "flaky: unit flaky_one");
    assert_eq!(tasks.len(), 1);
    assert_eq!(task_of(&d, tasks[0]).priority, Priority::Normal);
    assert!(
        reason.ends_with(&format!("; task #{}", tasks[0].0)),
        "{reason}"
    );
    // Asked again while it is in force: nothing more.
    consider(&d, pid, "unit", &["flaky_one".into()]);
    assert_eq!(d.store.quarantined(now_ms()).unwrap().len(), 1);
    assert_eq!(titled(&d, "flaky: unit flaky_one").len(), 1);
}

#[test]
fn the_threshold_the_window_and_the_seen_passing_guard_hold() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let pid = d.project_id_by_name("scratch").unwrap();
    for t in ["two_tasks", "one_task_often", "too_old"] {
        d.store
            .note_passing("unit", &[t.to_string()], now_ms())
            .unwrap();
    }
    // Only two tasks.
    flake(&d, "unit", "two_tasks", 1, 1);
    flake(&d, "unit", "two_tasks", 2, 1);
    // One task flaking three times.
    for _ in 0..3 {
        flake(&d, "unit", "one_task_often", 4, 1);
    }
    // Three tasks, one of them eight days ago.
    flake(&d, "unit", "too_old", 1, 8);
    flake(&d, "unit", "too_old", 2, 1);
    flake(&d, "unit", "too_old", 3, 1);
    let all: Vec<String> = ["two_tasks", "one_task_often", "too_old"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    consider(&d, pid, "unit", &all);
    let q: Vec<String> = d
        .store
        .quarantined(now_ms())
        .unwrap()
        .into_iter()
        .map(|q| q.1)
        .collect();
    assert!(!q.contains(&"two_tasks".to_string()), "{q:?}");
    assert!(!q.contains(&"one_task_often".to_string()), "{q:?}");
    assert!(!q.contains(&"too_old".to_string()), "{q:?}");
    // Three tasks, but never seen passing: a log line a parser took for a test name.
    for task in [1, 2, 3] {
        flake(&d, "unit", "a log line", task, 1);
    }
    consider(&d, pid, "unit", &["a log line".into()]);
    let q = d.store.quarantined(now_ms()).unwrap();
    assert!(q.iter().all(|q| q.1 != "a log line"), "{q:?}");
    assert!(titled(&d, "flaky: unit a log line").is_empty());
    // A name the base listed and did not fail counts as seen passing.
    d.store
        .put_baseline(
            "b1",
            "unit",
            &[],
            &["from_the_base".to_string()],
            1,
            now_ms(),
        )
        .unwrap();
    for task in [1, 2, 3] {
        flake(&d, "unit", "from_the_base", task, 1);
    }
    consider(&d, pid, "unit", &["from_the_base".into()]);
    assert!(d
        .store
        .quarantined(now_ms())
        .unwrap()
        .iter()
        .any(|q| q.1 == "from_the_base"));
}
