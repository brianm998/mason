// SPDX-License-Identifier: GPL-3.0-or-later

//! s251: the last round the test step will give runs on the escalation model, in a fresh session
//! that is told every earlier round's feedback. Earlier rounds keep their own model.

mod support;
use mason_core::*;
use serde_json::json;
use support::*;

/// A develop turn whose commit differs every time, so each round has something new to submit.
fn round_turn() -> serde_json::Value {
    json!([
        {"sh": "uuidgen > round.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm round"},
        {"call": {"tool": "submit_change", "args": {"summary": "a round", "head": "$HEAD"}}}
    ])
}

fn develop_models(
    d: &std::sync::Arc<mason_daemon::Daemon>,
    t: TaskId,
) -> Vec<(String, String, String)> {
    d.store
        .launches_for_task(t.0)
        .unwrap()
        .into_iter()
        .filter(|l| l.step == "develop")
        .map(|l| (l.model, l.model_from, l.moved_up_from))
        .collect()
}

#[test]
fn the_last_round_runs_on_the_escalation_model_and_earlier_rounds_do_not() {
    let s = Scratch::new(
        "false",
        json!({"steps": {"explore": [explore_turn()], "develop": [round_turn(), round_turn(), round_turn(), round_turn()]}}),
    );
    // Three test rounds, then the task fails: the third develop run is the last one that can pass.
    s.write_project(
        "scratch",
        "false",
        "max_test_rounds = 3\nescalate_model = \"opus\"",
    );
    let d = s.start();
    let t = file_task(&d, "Never green");
    wait_state(&d, t, State::Failed, 600);
    let rows = develop_models(&d, t);
    assert!(rows.len() >= 3, "{rows:?}: {:?}", task_of(&d, t).failure);
    assert_ne!(
        rows[0].0, "opus",
        "the first round is not escalated: {rows:?}"
    );
    assert_ne!(rows[1].0, "opus", "nor the second: {rows:?}");
    let last = &rows[2];
    assert_eq!(last.0, "opus", "{rows:?}");
    assert!(
        last.1.starts_with("escalated for the last round"),
        "{rows:?}"
    );
    assert!(
        !last.2.is_empty() || rows[0].0.is_empty(),
        "the original model is kept: {rows:?}"
    );
}

#[test]
fn an_empty_escalate_model_turns_it_off() {
    let s = Scratch::new(
        "false",
        json!({"steps": {"explore": [explore_turn()], "develop": [round_turn(), round_turn(), round_turn(), round_turn()]}}),
    );
    s.write_project(
        "scratch",
        "false",
        "max_test_rounds = 3\nescalate_model = \"\"",
    );
    let d = s.start();
    let t = file_task(&d, "Never green, no escalation");
    wait_state(&d, t, State::Failed, 600);
    let rows = develop_models(&d, t);
    assert!(rows.len() >= 3, "{rows:?}: {:?}", task_of(&d, t).failure);
    assert!(
        rows.iter()
            .all(|r| r.0 != "opus" && !r.1.starts_with("escalated")),
        "{rows:?}"
    );
}
