// SPDX-License-Identifier: GPL-3.0-or-later

//! s509: a batch whose every task had its testing skipped is built by the floor and then runs the
//! suites' fast unit tier (`fast_command`). On 2026-11-16 two tasks each added the same line to a
//! list in a test; each was green alone, the merge kept both, and the batch was only rebuilt.
//!
//! Real git, real suites, the real coordinator. Nothing here asserts a duration.

mod support;
use mason_core::*;
use support::lw::*;
use support::*;

fn skip_testing(w: &W, t: TaskId) {
    w.d.store
        .add_debt(
            t.0,
            "scratch",
            "step:test",
            "owner skipped",
            "brian",
            false,
            0,
        )
        .unwrap();
}

#[test]
fn a_skipped_batch_whose_merge_breaks_a_unit_test_is_parked_naming_that_test() {
    // Each change is fine alone; together (`joint.a` and `joint.b`) the fast tier is red.
    // The batch forms when the second task is admitted (max_size), never on the timer.
    let w = world_built_fast("window = \"20m\"\nmax_size = 2\n", &[], true);
    let left = w.task_with("left", &[("x/joint.a", "1")], skip_testing);
    let right = w.task_with("right", &[("x/joint.b", "1")], skip_testing);
    wait_for("both to go back", 120, || {
        w.sent_back(left).is_some() && w.sent_back(right).is_some()
    });
    let why = w.sent_back(left).unwrap();
    assert!(why.contains("matrix_events"), "{why}");
    assert!(!w.on_main("x/joint.a") && !w.on_main("x/joint.b"));
}

#[test]
fn a_skipped_batch_whose_unit_tests_pass_lands_and_a_batch_with_testing_does_not_run_the_tier() {
    let w = world_built_fast("window = \"2s\"\nmax_size = 5\n", &[], true);
    let t = w.task("fine", &[("x/ok.txt", "1")]);
    skip_testing(&w, t);
    wait_for("it to land", 120, || w.landed(t));
    // The push is read back through the origin: look until it shows, not once.
    wait_for("its file on main", 120, || w.on_main("x/ok.txt"));
    assert!(w.on_main("x/ok.txt"));
    // Not skipped: only the floor runs, so the tier's red does not stop it.
    let u = w.task("tested", &[("x/red.fast", "1")]);
    wait_for("the tested task to land", 120, || w.landed(u));
    wait_for("its file on main", 120, || w.on_main("x/red.fast"));
    assert!(w.on_main("x/red.fast"));
}

#[test]
fn an_immune_batch_runs_no_test_tier_and_lands_what_the_fast_tier_would_have_parked() {
    // The same pair as above, red together in the fast tier; both made immune to testing (a
    // fast track whose testing happens later), so no tier runs and the floor's build lands them.
    let w = world_built_fast("window = \"20m\"\nmax_size = 2\n", &[], true);
    let immune = |w: &W, t: TaskId| {
        mason_daemon::suites::skip(
            &w.d,
            t,
            mason_daemon::suites::ALL_TESTS,
            "fast track",
            "brian",
            false,
        )
        .unwrap();
    };
    let left = w.task_with("left", &[("x/joint.a", "1")], immune);
    let right = w.task_with("right", &[("x/joint.b", "1")], immune);
    wait_for("both to land", 120, || w.landed(left) && w.landed(right));
    assert!(w.on_main("x/joint.a") && w.on_main("x/joint.b"));
    // The full run is still owed.
    for t in [left, right] {
        let debts = w.d.store.debts(Some(t.0), false).unwrap();
        assert!(debts.iter().any(|x| x.scope == "tests:all"), "{debts:?}");
    }
}
