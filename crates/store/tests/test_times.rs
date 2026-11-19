// SPDX-License-Identifier: GPL-3.0-or-later
//! How long each test took (s634): the stored median follows the last runs, and the slowest
//! tests come out longest first.

use mason_store::{test_median, Store, TEST_TIME_RUNS};

fn times(v: &[(&str, u64)]) -> Vec<(String, u64)> {
    v.iter().map(|(n, m)| (n.to_string(), *m)).collect()
}

#[test]
fn the_median_follows_the_last_runs_only() {
    // Newest first: only the first TEST_TIME_RUNS count.
    let mut v = vec![10u64; TEST_TIME_RUNS];
    v.extend([9_999, 9_999, 9_999]);
    assert_eq!(test_median(&v), 10);
    assert_eq!(test_median(&[]), 0);
    assert_eq!(test_median(&[5, 1, 3]), 3);
    assert_eq!(test_median(&[4, 2]), 3);
}

#[test]
fn recorded_times_keep_the_last_runs_and_rank_the_slowest_first() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path().join("mason.db")).unwrap();
    // Twelve runs of `slow`: 1000 ms for the first two, then 100 ms; the first two fall off.
    for at in 0..12u64 {
        let ms = if at < 2 { 1_000 } else { 100 };
        s.record_test_times("p", "cargo", &times(&[("slow", ms), ("fast", 10)]), at + 1)
            .unwrap();
    }
    s.record_test_times("p", "cargo", &times(&[("mid", 50)]), 100)
        .unwrap();
    s.record_test_times("q", "other", &times(&[("huge", 9_000)]), 100)
        .unwrap();
    let all = s.slowest_tests(None, 20).unwrap();
    let names: Vec<_> = all.iter().map(|t| t.test.as_str()).collect();
    assert_eq!(names, ["huge", "slow", "mid", "fast"]);
    let slow = &all[1];
    assert_eq!((slow.median_ms, slow.runs), (100, TEST_TIME_RUNS));
    assert_eq!(slow.suite_total_ms, 100 + 50 + 10);
    let p = s.slowest_tests(Some("p"), 2).unwrap();
    assert_eq!(p.len(), 2);
    assert_eq!(p[0].test, "slow");
}

#[test]
fn target_times_keep_the_last_runs_and_give_a_median_per_target() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path().join("mason.db")).unwrap();
    for at in 0..12u64 {
        let ms = if at < 2 { 9_000 } else { 100 };
        s.record_target_times("p", "cargo", &times(&[("it", ms), ("lib", 5)]), at + 1)
            .unwrap();
    }
    s.record_target_times("q", "cargo", &times(&[("it", 1)]), 50)
        .unwrap();
    let m = s.target_medians("p", "cargo").unwrap();
    assert_eq!(m.get("it"), Some(&100));
    assert_eq!(m.get("lib"), Some(&5));
    assert_eq!(m.len(), 2);
    assert!(s.target_medians("p", "other").unwrap().is_empty());
}
