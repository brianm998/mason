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
    let w = world_built_fast("window = \"2s\"\nmax_size = 5\n", &[], true);
    let left = w.task("left", &[("x/joint.a", "1")]);
    let right = w.task("right", &[("x/joint.b", "1")]);
    skip_testing(&w, left);
    skip_testing(&w, right);
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
    assert!(w.on_main("x/ok.txt"));
    // Not skipped: only the floor runs, so the tier's red does not stop it.
    let u = w.task("tested", &[("x/red.fast", "1")]);
    wait_for("the tested task to land", 120, || w.landed(u));
    assert!(w.on_main("x/red.fast"));
}
