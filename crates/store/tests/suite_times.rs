// SPDX-License-Identifier: GPL-3.0-or-later
//! How long a suite's build and run take (s385): what the scheduler's backfill estimates from.

use mason_store::Store;

#[test]
fn a_suite_estimate_needs_two_runs_and_is_the_longest_of_the_last_five_plus_a_quarter() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path().join("mason.db")).unwrap();
    s.record_suite_time("star", "test-desktop", "run", 80_000, 1)
        .unwrap();
    assert_eq!(
        s.suite_estimate("star", "test-desktop", "run").unwrap(),
        None,
        "one run is not enough"
    );
    s.record_suite_time("star", "test-desktop", "run", 100_000, 2)
        .unwrap();
    assert_eq!(
        s.suite_estimate("star", "test-desktop", "run").unwrap(),
        Some(125_000)
    );
    // Phases, suites and projects are kept apart.
    assert_eq!(
        s.suite_estimate("star", "test-desktop", "build").unwrap(),
        None
    );
    assert_eq!(
        s.suite_estimate("mason", "test-desktop", "run").unwrap(),
        None
    );
    // Only the last five count: an old long run stops counting.
    s.record_suite_time("star", "test-gui", "run", 900_000, 10)
        .unwrap();
    for at in 11..16 {
        s.record_suite_time("star", "test-gui", "run", 40_000, at)
            .unwrap();
    }
    assert_eq!(
        s.suite_estimate("star", "test-gui", "run").unwrap(),
        Some(50_000)
    );
}

#[test]
fn only_the_last_ten_times_are_kept() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path().join("mason.db")).unwrap();
    for at in 0..15u64 {
        s.record_suite_time("p", "s", "run", 1_000 + at, at)
            .unwrap();
    }
    let conn = rusqlite::Connection::open(d.path().join("mason.db")).unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM suite_times WHERE project='p' AND suite='s' AND phase='run'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 10);
}
