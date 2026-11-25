// SPDX-License-Identifier: GPL-3.0-or-later

//! s1114: a red round's feedback is kept when it uses up the step's last round, so the diagnosis
//! and the develop round after an owner retry see why the task failed.

use crate::support;
use mason_core::*;
use mason_definition::graph::Outcome;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use support::*;

const REVIEWED: &str = r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "review"

[step.review]
type = "agent"
category = "review"
max_rounds = 2
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

const TESTED: &str = r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
command = "cat marker.txt; exit 1"
max_rounds = 2
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

const SIGNED: &str = r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "sign-off"

[step.sign-off]
type = "approval"
category = "review"
max_rounds = 2
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

const NOTE: &str = "OWNER-NOTE fix what the last review found";

fn record(dir: &Path, name: &str) -> Value {
    // Written beside the name and moved there, so a reader never sees half a prompt.
    let to = dir.join(format!("{name}.txt"));
    json!({"sh": format!("printf '%s' \"$FAKE_PROMPT\" > {0}.part && mv {0}.part {0}", to.display())})
}

/// A develop turn that records its prompt and commits something new, `marker` in `marker.txt`.
fn develop(rec: &Path, name: &str, marker: &str) -> Value {
    json!([
        record(rec, name),
        {"sh": format!("echo {marker} > marker.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm '{name}'")},
        {"call": {"tool": "submit_change", "args": {"summary": name, "head": "$HEAD"}}}
    ])
}

fn changes(finding: &str) -> Value {
    json!([{"call": {"tool": "submit_review", "args": {
        "verdict": "changes", "summary": "not yet", "findings": [{"issue": finding}]}}}])
}

fn approve() -> Value {
    json!({"call": {"tool": "submit_review", "args": {"verdict": "approve", "summary": "fine"}}})
}

fn diagnosis(rec: &Path) -> Value {
    json!([
        record(rec, "diagnosis"),
        {"call": {"tool": "submit_diagnosis", "args": {
            "verdict": "ask", "summary": "s", "question": "q?", "recommendation": "r"}}}
    ])
}

fn start(def: &str, steps: Value, extra: &str) -> (Scratch, Arc<mason_daemon::Daemon>) {
    let s = Scratch::new("true", json!({"steps": steps}));
    s.write_project("scratch", "true", &format!("auto_retry = false\n{extra}"));
    s.push_files(&[(".mason/mason.toml", def)], "project setup");
    let d = s.start();
    (s, d)
}

fn read(d: &mason_daemon::Daemon, t: TaskId, name: &str) -> Option<String> {
    std::fs::read_to_string(d.task_dir(t).join(name)).ok()
}

fn history(d: &mason_daemon::Daemon, t: TaskId) -> String {
    read(d, t, "feedback-history.txt").unwrap_or_default()
}

fn entries(history: &str) -> usize {
    history.matches("## After run ").count()
}

fn recorded(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(format!("{name}.txt"))).unwrap_or_default()
}

/// The diagnosis runs after the task shows as failed.
fn diagnosis_prompt(d: &Arc<mason_daemon::Daemon>, t: TaskId, rec: &Path) -> String {
    wait_for("the diagnosis", 180, || {
        !d.store.diagnoses(Some(t.0)).unwrap().is_empty()
    });
    recorded(rec, "diagnosis")
}

fn retry(d: &Arc<mason_daemon::Daemon>, t: TaskId, to: &str, message: Option<&str>) {
    let ask = mason_daemon::http::RetryAsk {
        to: Some(to.into()),
        message: message.map(str::to_string),
        ..Default::default()
    };
    if let Err((code, body)) = mason_daemon::http::retry_task(d, t.0, ask) {
        panic!("retry: {code} {}", body.0);
    }
}

/// Task #1112's shape: two send-backs with different findings, and the second uses up the rounds.
#[test]
fn the_last_reviews_findings_reach_the_history_the_diagnosis_and_the_retried_develop_round() {
    let rec = tempfile::tempdir().unwrap();
    let r = rec.path();
    let gate = r.join("go");
    let (_s, d) = start(
        REVIEWED,
        json!({
            "develop": [develop(r, "dev1", "one"), develop(r, "dev2", "two"), develop(r, "dev3", "three")],
            "review": [
                changes("FINDING-ONE"),
                changes("FINDING-TWO"),
                [
                    record(r, "review3"),
                    {"sh": format!("while [ ! -f {} ]; do sleep 0.1; done", gate.display())},
                    approve()
                ],
            ],
            "diagnose": [diagnosis(r)],
        }),
        "",
    );
    let t = file_task(&d, "Two reviews say no");
    wait_state(&d, t, State::Failed, 600);

    let h = history(&d, t);
    assert_eq!(entries(&h), 2, "{h}");
    let (one, two) = (h.find("FINDING-ONE"), h.find("FINDING-TWO"));
    assert!(one.is_some() && two.is_some() && one < two, "{h}");
    assert!(
        h.contains("(review): the reviewer asked for changes"),
        "{h}"
    );
    let last = read(&d, t, "feedback.txt").expect("the last findings are kept");
    assert!(
        last.contains("FINDING-TWO") && !last.contains("FINDING-ONE"),
        "{last}"
    );
    assert_eq!(read(&d, t, "round-reason.txt").as_deref(), Some("fix"));

    let p = diagnosis_prompt(&d, t, r);
    assert!(p.contains("FINDING-TWO"), "{p}");

    retry(&d, t, "develop", Some(NOTE));
    wait_for("the review after the retry", 600, || {
        r.join("review3.txt").exists()
    });
    let dev = recorded(r, "dev3");
    let note = dev.find(NOTE).expect("the note is in the prompt");
    let finding = dev.find("FINDING-TWO").expect("the findings are too");
    assert!(note < finding, "{dev}");
    // The develop round was accepted: its feedback is used up, the history is not.
    assert_eq!(read(&d, t, "feedback.txt"), None);
    let h = history(&d, t);
    assert_eq!(entries(&h), 2, "{h}");
    assert!(h.contains("FINDING-TWO"), "{h}");
    std::fs::write(&gate, "").unwrap();
    wait_state(&d, t, State::Landed, 600);
}

#[test]
fn a_test_step_red_to_the_last_round_keeps_its_output_for_the_diagnosis() {
    let rec = tempfile::tempdir().unwrap();
    let r = rec.path();
    let (_s, d) = start(
        TESTED,
        json!({
            "develop": [develop(r, "dev1", "RED-ONE"), develop(r, "dev2", "RED-TWO")],
            "diagnose": [diagnosis(r)],
        }),
        "",
    );
    let t = file_task(&d, "Never green");
    wait_state(&d, t, State::Failed, 600);
    let h = history(&d, t);
    assert_eq!(entries(&h), 2, "{h}");
    assert!(h.contains("RED-ONE") && h.contains("RED-TWO"), "{h}");
    let last = read(&d, t, "feedback.txt").expect("the last output is kept");
    assert!(
        last.contains("RED-TWO") && !last.contains("RED-ONE"),
        "{last}"
    );
    let p = diagnosis_prompt(&d, t, r);
    // The history section, not only the log tails below it.
    let told = p
        .split("# What each round was told")
        .nth(1)
        .and_then(|s| s.split("# The end of the last test logs").next())
        .unwrap_or_default();
    assert!(told.contains("RED-TWO"), "{p}");
}

#[test]
fn a_retry_from_review_that_approves_removes_the_old_findings() {
    let rec = tempfile::tempdir().unwrap();
    let r = rec.path();
    // No way back: the first send-back fails the task.
    let def = REVIEWED.replace("max_rounds = 2\non_red = \"develop\"\n", "");
    let (_s, d) = start(
        &def,
        json!({
            "develop": [develop(r, "dev1", "one")],
            "review": [changes("FINDING-ONLY"), [approve()]],
        }),
        "diagnose = false",
    );
    let t = file_task(&d, "Reviewed twice");
    wait_state(&d, t, State::Failed, 600);
    let f = read(&d, t, "feedback.txt").expect("kept with no way back");
    assert!(f.contains("FINDING-ONLY"), "{f}");
    retry(&d, t, "review", None);
    wait_state(&d, t, State::Landed, 600);
    assert_eq!(read(&d, t, "feedback.txt"), None);
    let h = history(&d, t);
    assert_eq!(entries(&h), 1, "{h}");
}

fn wait_sign_off(d: &Arc<mason_daemon::Daemon>, t: TaskId, round: u32) {
    wait_for("the approval", 600, || {
        let k = task_of(d, t);
        k.step_attempts.get("develop").copied() == Some(round)
            && k.state == State::Review
            && matches!(k.activity, Activity::Waiting(WaitReason::On(_)))
    });
}

fn reject(d: &Arc<mason_daemon::Daemon>, t: TaskId, why: &str) {
    mason_daemon::jobs::conclude_approval(d, t, Outcome::Red, why).unwrap();
}

#[test]
fn an_owner_rejection_with_a_way_back_is_in_the_history_and_so_is_the_one_that_fails() {
    let rec = tempfile::tempdir().unwrap();
    let r = rec.path();
    let (_s, d) = start(
        SIGNED,
        json!({"develop": [develop(r, "dev1", "one"), develop(r, "dev2", "two")]}),
        "diagnose = false",
    );
    let t = file_task(&d, "Rejected twice");
    wait_sign_off(&d, t, 1);
    reject(&d, t, "REJECT-ONE");
    wait_sign_off(&d, t, 2);
    let dev = recorded(r, "dev2");
    assert!(dev.contains("REJECT-ONE"), "{dev}");
    reject(&d, t, "REJECT-TWO");
    wait_state(&d, t, State::Failed, 60);
    let h = history(&d, t);
    assert_eq!(entries(&h), 2, "{h}");
    assert!(
        h.contains("(sign-off): rejected by the owner: REJECT-ONE"),
        "{h}"
    );
    assert!(
        h.contains("(sign-off): rejected by the owner: REJECT-TWO"),
        "{h}"
    );
    let f = read(&d, t, "feedback.txt").expect("the last rejection is kept");
    assert!(f.contains("REJECT-TWO"), "{f}");
    assert_eq!(read(&d, t, "round-reason.txt").as_deref(), Some("fix"));
}

#[test]
fn an_owner_rejection_with_no_way_back_is_in_the_history() {
    let rec = tempfile::tempdir().unwrap();
    let r = rec.path();
    let def = SIGNED.replace("max_rounds = 2\non_red = \"develop\"\n", "");
    let (_s, d) = start(
        &def,
        json!({"develop": [develop(r, "dev1", "one")]}),
        "diagnose = false",
    );
    let t = file_task(&d, "Rejected outright");
    wait_sign_off(&d, t, 1);
    reject(&d, t, "REJECT-FINAL");
    wait_state(&d, t, State::Failed, 60);
    let h = history(&d, t);
    assert_eq!(entries(&h), 1, "{h}");
    assert!(h.contains("REJECT-FINAL"), "{h}");
    let f = read(&d, t, "feedback.txt").expect("the rejection is kept");
    assert!(f.contains("REJECT-FINAL"), "{f}");
}

#[test]
fn a_failure_with_no_feedback_leaves_the_earlier_files_alone() {
    let rec = tempfile::tempdir().unwrap();
    let r = rec.path();
    // The second develop round does nothing at all: it fails with an error, not a red.
    let (_s, d) = start(
        REVIEWED,
        json!({
            "develop": [develop(r, "dev1", "one")],
            "review": [changes("FINDING-KEPT")],
        }),
        "diagnose = false",
    );
    let t = file_task(&d, "Then an error");
    wait_state(&d, t, State::Failed, 600);
    assert_eq!(task_of(&d, t).step_attempts.get("develop"), Some(&2));
    let h = history(&d, t);
    assert_eq!(entries(&h), 1, "{h}");
    assert!(h.contains("FINDING-KEPT"), "{h}");
    let f = read(&d, t, "feedback.txt").expect("the earlier feedback stays");
    assert!(f.contains("FINDING-KEPT"), "{f}");
    assert_eq!(read(&d, t, "round-reason.txt").as_deref(), Some("fix"));
}
