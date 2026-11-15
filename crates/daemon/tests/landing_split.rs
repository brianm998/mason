// SPDX-License-Identifier: GPL-3.0-or-later

//! Decision 0387: a red batch that blame cannot pin on one task is split, and only the
//! task (or the tasks together) that reproduce the red go back; the rest land.

mod support;
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
