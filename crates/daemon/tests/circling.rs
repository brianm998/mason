// SPDX-License-Identifier: GPL-3.0-or-later

//! An agent going round in circles (s511): three full runs of a develop step that end red on
//! the same tests raise one `circling:<task>` alert; a different set raises none; a green run
//! clears it. The runs are written to the record the way `run_check` writes them.

mod support;
use mason_core::*;
use mason_daemon::attention::alerts_now;
use serde_json::json;
use std::sync::Arc;
use support::*;

type D = Arc<mason_daemon::daemon::Daemon>;

fn circling(d: &D, t: TaskId) -> Vec<String> {
    alerts_now(d)
        .into_iter()
        .filter(|(k, _, _)| *k == format!("circling:{}", t.0))
        .map(|(_, _, text)| text)
        .collect()
}

fn red(d: &D, t: TaskId, tests: &[&str]) {
    let names: Vec<String> = tests.iter().map(|n| format!("test\t{n}")).collect();
    d.store.put_check_run(t.0, 1, 1_000, false, &names).unwrap();
}

fn developing(title: &str) -> (Scratch, D, TaskId) {
    let slow = json!([{"sh": "sleep 120"}]);
    let s = Scratch::new(
        "true",
        json!({"steps": {"explore": [explore_turn()], "develop": [slow]}}),
    );
    let d = s.start();
    let t = file_task(&d, title);
    wait_state(&d, t, State::Developing, 30);
    (s, d, t)
}

#[test]
fn the_same_failing_set_three_times_raises_one_item_and_a_green_run_clears_it() {
    let (_s, d, t) = developing("Goes round");
    red(&d, t, &["a::one"]);
    red(&d, t, &["a::one"]);
    assert!(circling(&d, t).is_empty(), "two runs are not enough");
    red(&d, t, &["a::one"]);
    let got = circling(&d, t);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].contains(&format!("Task #{}", t.0)), "{}", got[0]);
    assert!(got[0].contains("a::one"), "{}", got[0]);
    assert!(
        got[0].contains("none is known to be red on develop"),
        "{}",
        got[0]
    );

    d.store.put_check_run(t.0, 1, 2_000, true, &[]).unwrap();
    assert!(circling(&d, t).is_empty(), "a green run clears it");
}

#[test]
fn different_failing_sets_raise_nothing() {
    let (_s, d, t) = developing("Varies");
    red(&d, t, &["a::one"]);
    red(&d, t, &["a::two"]);
    red(&d, t, &["a::one"]);
    assert!(circling(&d, t).is_empty());
}

#[test]
fn tests_red_on_develop_are_named_as_such() {
    let (_s, d, t) = developing("Not mine");
    d.store
        .put_quarantine(
            "test",
            "a::one",
            "mason:exhausted",
            u64::MAX / 4,
            "task #9",
            1,
        )
        .unwrap();
    for _ in 0..3 {
        red(&d, t, &["a::one"]);
    }
    let got = circling(&d, t);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0].contains("red on develop, so not this task's"),
        "{}",
        got[0]
    );
}
