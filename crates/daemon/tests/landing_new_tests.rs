// SPDX-License-Identifier: GPL-3.0-or-later

//! #812: the landing build floor also runs the tests the batch's skipped tasks add, by name. On
//! 2026-11-21 two fast-tracked tasks landed with their new tests failing, because their code edits
//! had not applied and the floor only proves the merged tree compiles.
//!
//! Real git, real suites, the real coordinator. Nothing here asserts a duration.

mod support;
use mason_core::*;
use support::lw::*;
use support::*;

fn fast_track(w: &W, t: TaskId) {
    w.d.store
        .add_debt(
            t.0,
            "scratch",
            mason_daemon::suites::ALL_TESTS,
            "owner skipped",
            "brian",
            false,
            0,
        )
        .unwrap();
}

#[test]
fn a_skipped_task_whose_new_test_fails_is_parked_and_the_others_land() {
    let w = world_new_tests("window = \"20m\"\nmax_size = 2\n");
    let good = w.task_with(
        "good",
        &[("x/good.rs", "#[test]\nfn adds_fine() {}\n")],
        fast_track,
    );
    let bad = w.task_with(
        "bad",
        &[("x/bad.rs", "#[test]\nfn adds_bad() {}\n")],
        fast_track,
    );
    wait_for("the failing one to go back", 180, || {
        w.sent_back(bad).is_some()
    });
    wait_for("the other to land", 180, || w.landed(good));
    assert!(w.on_main("x/good.rs"));
    assert!(!w.on_main("x/bad.rs"));
    assert!(w.sent_back(good).is_none(), "{:?}", w.sent_back(good));
}

#[test]
fn a_skipped_task_whose_new_test_passes_lands() {
    let w = world_new_tests("window = \"20m\"\nmax_size = 1\n");
    let t = w.task_with(
        "fine",
        &[("x/fine.rs", "#[test]\nfn adds_fine() {}\n")],
        fast_track,
    );
    wait_for("it to land", 180, || w.landed(t));
    assert!(w.on_main("x/fine.rs"));
}

fn land_now_running(w: &W, t: TaskId) {
    fast_track(w, t);
    w.d.store
        .set_landing_flag(t.0, mason_daemon::suites::LAND_NOW_RUNNING, "brian", 0)
        .unwrap();
}

/// s943: Land now stops the testing, the new tests of the skipped tasks included. Before, the
/// new-tests check still ran after the floor (and on 2026-11-22 went to GitHub, was not picked up,
/// and failed the whole batch). The task whose new test is red lands, and the suite the check
/// would have run is owed after the merge, as a suite Land now stopped is.
#[test]
fn land_now_does_not_run_the_new_tests_check_and_owes_it_after_the_merge() {
    let w = world_new_tests("window = \"20m\"\nmax_size = 1\n");
    let t = w.task_with(
        "bad but land now",
        &[("x/bad.rs", "#[test]\nfn adds_bad() {}\n")],
        land_now_running,
    );
    wait_for("it to land past its red new test", 180, || w.landed(t));
    assert!(w.on_main("x/bad.rs"));
    assert!(w.sent_back(t).is_none(), "{:?}", w.sent_back(t));
    wait_for("the new tests to be owed", 60, || {
        w.d.store
            .debts(Some(t.0), true)
            .unwrap()
            .iter()
            .any(|x| x.scope == "suite:alpha" && x.deferred)
    });
}
