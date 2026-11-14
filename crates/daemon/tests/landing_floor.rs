// SPDX-License-Identifier: GPL-3.0-or-later

//! 2026-11-13: nine batches landed on a `develop` that did not compile, because nothing built
//! them. The owner had skipped every task's test step, MASON's own landing config has no batch
//! tier, and nothing refused the conflict markers one agent had committed. These tests are the
//! two guards that stop it: landing builds a batch that no suite verifies (the build floor),
//! and a branch that adds conflict markers goes back before it joins a batch.
//!
//! Real git, real suites, the real coordinator, as in `landing.rs`; no timing is asserted.

mod support;
use support::lw::*;
use support::*;

/// A merge that left its markers behind, as an agent commits it. Built from repeated
/// characters so this file holds no marker line itself.
fn conflicted() -> String {
    format!(
        "keep\n{} HEAD\nmine\n{}\ntheirs\n{} origin/develop\n",
        "<".repeat(7),
        "=".repeat(7),
        ">".repeat(7)
    )
}

#[test]
fn a_batch_that_does_not_build_is_not_landed_and_the_task_that_broke_it_goes_back() {
    // No batch tier: the floor is the only thing that builds the union.
    let w = world_built("window = \"2s\"\nmax_size = 5\n", &[]);
    let fine = w.task("beta only", &[("b/ok.txt", "1")]);
    let broken = w.task("alpha does not build", &[("a/red.build.alpha", "x")]);
    wait_for("the task that builds to land", 120, || w.landed(fine));
    wait_for("the one that does not to go back", 60, || {
        w.sent_back(broken).is_some()
    });
    let why = w.sent_back(broken).unwrap();
    assert!(why.contains("alpha"), "{why}");
    assert!(w.on_main("b/ok.txt"));
    assert!(
        !w.on_main("a/red.build.alpha"),
        "a batch that does not build must not land"
    );
    assert!(
        w.sent_back(fine).is_none(),
        "the innocent task is not parked"
    );
}

#[test]
fn two_changes_that_merge_cleanly_and_do_not_build_together_do_not_land() {
    // Each builds alone; the union does not (a call and the definition it calls, changed by
    // two tasks). Nothing can pin it on one of them: both go back, nothing lands.
    let w = world_built("window = \"2s\"\nmax_size = 5\n", &[]);
    let left = w.task("left", &[("x/joint.a", "1")]);
    let right = w.task("right", &[("x/joint.b", "1")]);
    wait_for("both to go back", 120, || {
        w.sent_back(left).is_some() && w.sent_back(right).is_some()
    });
    assert!(!w.on_main("x/joint.a") && !w.on_main("x/joint.b"));
    let why = w.sent_back(left).unwrap();
    assert!(why.contains("alpha") || why.contains("beta"), "{why}");
}

#[test]
fn a_base_that_does_not_build_holds_the_batch_and_blames_nobody() {
    // The base itself is red in `alpha`. A change under `x/` is named by no line of the impact
    // map, so everything is built: the red is the base's, not the task's.
    let w = world_built(
        "window = \"2s\"\n",
        &[("a/red.build.alpha", "the base does not build")],
    );
    let t = w.task("innocent", &[("x/one.txt", "1")]);
    wait_for("the batch to be held", 120, || {
        w.d.store
            .batches(Some("scratch"), 10)
            .unwrap()
            .iter()
            .any(|b| b.phase == "failed" && b.note.contains("does not build"))
    });
    assert!(
        !w.on_main("x/one.txt"),
        "nothing lands on a base that does not build"
    );
    assert!(
        w.sent_back(t).is_none(),
        "nobody is blamed for the base's red"
    );
    let alerts = w.d.landing().attention();
    assert!(
        alerts
            .iter()
            .any(|(k, _, text)| k.ends_with(":redbase") && text.contains("does not build")),
        "the owner is told: {alerts:?}"
    );
}

#[test]
fn a_tree_that_already_passed_the_suite_is_not_built_again() {
    // `alpha` would not build, but this exact tree is on record as having passed it (the
    // evidence a task's own test step leaves): identical code never tests twice.
    let w = world_built("window = \"5s\"\n", &[]);
    let t = w.task("proven", &[("a/red.build.alpha", "x")]);
    let tree = sh(
        &w.s.repo,
        &["rev-parse", &format!("mason/scratch/t{}^{{tree}}", t.0)],
    );
    w.d.store
        .put_evidence(&tree, "alpha", 1, 1, t.0, mason_daemon::daemon::now_ms())
        .unwrap();
    wait_for("it to land without a build", 120, || w.landed(t));
    assert!(w.on_main("a/red.build.alpha"));
}

#[test]
fn a_project_with_a_batch_tier_is_left_to_it() {
    // The owner named what verifies a batch: the floor adds nothing to it.
    let w = world_built(
        "window = \"2s\"\n[[tier]]\nname = \"batch\"\nsuites = [\"integration\"]\n",
        &[],
    );
    let t = w.task("alpha does not build", &[("a/red.build.alpha", "x")]);
    wait_for("it to land: only `integration` is run", 120, || w.landed(t));
    assert!(w.on_main("a/red.build.alpha"));
}

#[test]
fn a_branch_that_adds_conflict_markers_goes_back_before_it_joins_a_batch() {
    let w = world("window = \"2s\"\n", &[]);
    let bad = w.task("a merge left its markers", &[("x/m.txt", &conflicted())]);
    let fine = w.task("clean", &[("y/c.txt", "1")]);
    wait_for("the clean task to land", 90, || w.landed(fine));
    wait_for("the other to go back", 60, || w.sent_back(bad).is_some());
    let why = w.sent_back(bad).unwrap();
    assert!(
        why.contains("conflict markers") && why.contains("x/m.txt:2"),
        "{why}"
    );
    assert!(w.on_main("y/c.txt"));
    assert!(!w.on_main("x/m.txt"), "markers must not reach the base");
    // It was never in a batch: the only batch has the clean one.
    let rows = w.d.store.batches(Some("scratch"), 10).unwrap();
    let never = bad.0.to_string();
    assert!(
        rows.iter().all(|b| !b.tasks.split(',').any(|t| t == never)),
        "{rows:?}"
    );
}
