// SPDX-License-Identifier: GPL-3.0-or-later

//! Daemon bugs and long stalls raise a `daemon_bug` alert (task 94): the pure rules.

use mason_daemon::attention::{bug_alerts, stall_alert, BUG_WINDOW_MS, REPEAT_WINDOW_MS};

const NOW: u64 = 100 * 3600 * 1000;

fn row(at: u64, kind: &str, detail: &str) -> (u64, String, String) {
    (at, kind.into(), detail.into())
}

#[test]
fn each_alerting_kind_raises_one_alert_with_count_and_latest_detail() {
    for kind in ["thread_panic", "job_panic", "job_spawn", "projection_drift"] {
        let rows = vec![
            row(NOW - 2000, kind, "first"),
            row(NOW - 1000, kind, "second"),
        ];
        let a = bug_alerts(&rows, NOW);
        assert_eq!(a.len(), 1, "{kind}");
        assert_eq!(a[0].1, "daemon_bug");
        assert!(a[0].2.contains("2 "), "{}", a[0].2);
        assert!(a[0].2.contains("second") && !a[0].2.contains("first"));
    }
}

#[test]
fn the_window_is_24_hours() {
    let inside = vec![row(NOW - BUG_WINDOW_MS + 1, "job_panic", "x")];
    assert_eq!(bug_alerts(&inside, NOW).len(), 1);
    let outside = vec![row(NOW - BUG_WINDOW_MS, "job_panic", "x")];
    assert!(bug_alerts(&outside, NOW).is_empty());
}

#[test]
fn three_of_the_same_missed_job_in_an_hour_alert_and_two_do_not() {
    let d = "task 7 had an open Develop step with no job; started one";
    let two = vec![
        row(NOW - 3000, "missed_job", d),
        row(NOW - 2000, "missed_job", d),
    ];
    assert!(bug_alerts(&two, NOW).is_empty());
    let mut three = two.clone();
    three.push(row(NOW - 1000, "missed_job", d));
    let a = bug_alerts(&three, NOW);
    assert_eq!(a.len(), 1);
    assert!(a[0].2.contains("task 7"));
    // Different tasks do not add up.
    let mixed = vec![
        row(NOW - 3000, "missed_admit", "task 1 x"),
        row(NOW - 2000, "missed_admit", "task 2 x"),
        row(NOW - 1000, "missed_admit", "task 3 x"),
    ];
    assert!(bug_alerts(&mixed, NOW).is_empty());
    // Three spread over more than an hour do not either.
    let spread = vec![
        row(NOW - REPEAT_WINDOW_MS - 5, "missed_job", d),
        row(NOW - 2000, "missed_job", d),
        row(NOW - 1000, "missed_job", d),
    ];
    assert!(bug_alerts(&spread, NOW).is_empty());
    let admit: Vec<_> = (1..=3)
        .map(|i| row(NOW - i, "missed_admit", "task 4 x"))
        .collect();
    assert_eq!(bug_alerts(&admit, NOW).len(), 1);
}

#[test]
fn housekeeping_kinds_stay_rows() {
    let rows: Vec<_> = (1..=5)
        .flat_map(|i| {
            [
                row(NOW - i, "orphan_host", "task 1 x"),
                row(NOW - i, "leftover_branch", "task 1 x"),
            ]
        })
        .collect();
    assert!(bug_alerts(&rows, NOW).is_empty());
}

#[test]
fn a_long_stall_alerts_for_an_hour() {
    assert!(stall_alert(None, NOW).is_none());
    let a = stall_alert(Some((NOW - 10, 45_000)), NOW).unwrap();
    assert_eq!(a.1, "daemon_bug");
    assert!(a.2.contains("45 seconds"), "{}", a.2);
    assert!(stall_alert(Some((NOW - 3_600_000, 45_000)), NOW).is_none());
    assert!(stall_alert(Some((NOW - 10, 29_999)), NOW).is_none());
}
