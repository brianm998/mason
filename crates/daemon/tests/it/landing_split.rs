// SPDX-License-Identifier: GPL-3.0-or-later

//! Decision 0387: a red batch that blame cannot pin on one task is split, and only the
//! task (or the tasks together) that reproduce the red go back; the rest land.

use crate::support;
use support::lw::*;
use support::*;

const TOML: &str =
    "window = \"2s\"\nmax_size = 5\n[[tier]]\nname = \"batch\"\nsuites = [\"integration\"]\n";

#[test]
fn only_the_task_that_breaks_a_suite_goes_back_and_the_other_two_land() {
    let w = world(TOML, &[]);
    let t1 = w.task("one", &[("x/one.txt", "1")]);
    let t2 = w.task("two", &[("x/red.integration", "boom")]);
    let t3 = w.task("three", &[("x/three.txt", "3")]);
    wait_for("the innocent two to land", 120, || {
        w.landed(t1) && w.landed(t3)
    });
    wait_for("the culprit to go back", 60, || w.sent_back(t2).is_some());
    let why = w.sent_back(t2).unwrap();
    assert!(why.contains("integration"), "{why}");
    assert!(w.sent_back(t1).is_none() && w.sent_back(t3).is_none());
    assert!(w.on_main("x/one.txt") && w.on_main("x/three.txt"));
    assert!(!w.on_main("x/red.integration"));
    // The closing note shows the bisection.
    wait_for("the note", 30, || {
        w.d.store
            .batches(Some("scratch"), 10)
            .unwrap()
            .iter()
            .any(|b| b.note.contains("split to find it") && b.note.contains(&format!("#{}", t2.0)))
    });
}

#[test]
fn two_tasks_that_break_it_only_together_both_go_back_named_together() {
    let w = world(TOML, &[]);
    let t1 = w.task("one", &[("x/one.txt", "1")]);
    let a = w.task("left", &[("x/joint.a", "1")]);
    let b = w.task("right", &[("x/joint.b", "1")]);
    wait_for("the innocent one to land", 120, || w.landed(t1));
    wait_for("both to go back", 60, || {
        w.sent_back(a).is_some() && w.sent_back(b).is_some()
    });
    for (t, mate) in [(a, b), (b, a)] {
        let why = w.sent_back(t).unwrap();
        assert!(why.contains(&format!("#{}", mate.0)), "{why}");
    }
    assert!(w.sent_back(t1).is_none());
    assert!(!w.on_main("x/joint.a") && !w.on_main("x/joint.b"));
}

#[test]
fn a_batch_tier_red_the_base_shows_too_blames_no_task() {
    // Decision 0539: the whole suite runs at the batch, so a red the base already has must not
    // send anyone back. `integration` is red at the base (`x/red.integration` is in it) and
    // red the same way in the batch: it is excused as also failing at the merge base.
    let w = world(TOML, &[("x/red.integration", "the base is red")]);
    let t1 = w.task("one", &[("x/one.txt", "1")]);
    let t2 = w.task("two", &[("x/two.txt", "2")]);
    wait_for("both to land", 120, || w.landed(t1) && w.landed(t2));
    assert!(w.sent_back(t1).is_none() && w.sent_back(t2).is_none());
    assert!(w.on_main("x/one.txt") && w.on_main("x/two.txt"));
}

#[test]
fn two_tasks_each_red_alone_over_a_green_base_both_go_back() {
    // On 2026-11-18 batch #372 held two tasks forever: one failed `cargo fmt`, the other
    // clippy, each alone, and "every task reproduces the red alone" was taken as the base's
    // red without running the base. The base here is green, so each task goes back.
    let w = world(TOML, &[]);
    let a = w.task("left", &[("x/left/red.integration", "left's own red")]);
    let b = w.task("right", &[("x/right/red.integration", "right's own red")]);
    wait_for("both to go back", 180, || {
        w.sent_back(a).is_some() && w.sent_back(b).is_some()
    });
    for t in [a, b] {
        let why = w.sent_back(t).unwrap();
        assert!(why.contains("integration"), "{why}");
    }
    assert!(!w.on_main("x/left/red.integration") && !w.on_main("x/right/red.integration"));
}
