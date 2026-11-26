// SPDX-License-Identifier: GPL-3.0-or-later

//! Task 1135, the exit test: replay trd #1112's shape. Round 1 sends back with two blocking
//! findings, develop fixes them, and round 2 tries a new blocking finding on a line nobody
//! touched. The contract refuses it as blocking; as minor it is accepted, so round 2 approves
//! with notes and the task lands. A real daemon with scripted agents; no durations asserted.

use crate::support;
use mason_core::*;
use serde_json::{json, Value};
use support::*;

const DEF: &str = r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
command = "true"
on_red = "develop"
max_rounds = 3
next = "review"

[step.review]
type = "agent"
category = "review"
on_red = "develop"
max_rounds = 3
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

fn develop(n: u32) -> Value {
    json!([
        {"sh": format!("echo {n} >> a.txt && git add a.txt && git -c user.name=t -c user.email=t@t commit -qm round{n}")},
        {"call": {"tool": "submit_change", "args": {"summary": "round", "head": "$HEAD"}}}
    ])
}

fn review(args: Value) -> Value {
    json!({"call": {"tool": "submit_review", "args": args}})
}

#[test]
fn a_new_blocking_finding_on_an_untouched_line_is_accepted_only_as_minor() {
    let rec = tempfile::tempdir().unwrap();
    let blocking = |issue: &str, line: u64| {
        json!({"issue": issue, "file": "a.txt", "line": line, "quote": "q", "severity": "blocking"})
    };
    let round1 = json!([review(json!({
        "verdict": "changes", "summary": "FIRST",
        "findings": [blocking("ONE", 1), blocking("TWO", 1)]}))]);
    let round2 = json!([
        // No answers and a new blocking finding on line 1, which develop did not touch.
        review(json!({"verdict": "changes", "summary": "REFUSED",
            "findings": [blocking("NEW-ON-OLD-LINE", 1)]})),
        {"sh": format!("printf '%s' \"$FAKE_PROMPT\" > {}/review2.txt", rec.path().display())},
        review(json!({"verdict": "approve", "summary": "fine",
            "earlier": [
                {"round": 1, "index": 0, "status": "fixed", "note": "done"},
                {"round": 1, "index": 1, "status": "fixed", "note": "done"}],
            "findings": [{"issue": "NEW-ON-OLD-LINE", "file": "a.txt", "line": 1, "severity": "minor"}]}))
    ]);
    let s = Scratch::new(
        "false",
        json!({"steps": {"develop": [develop(1), develop(2)], "review": [round1, round2]}}),
    );
    s.write_project("scratch", "false", "auto_retry = false\ntest_reruns = 0");
    s.push_files(&[(".mason/mason.toml", DEF)], "project setup");
    let d = s.start();
    let t = file_task(&d, "Replayed");
    wait_state(&d, t, State::Landed, 600);
    let rounds: Vec<Value> = serde_json::from_str(
        &std::fs::read_to_string(d.task_dir(t).join("review-rounds.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(rounds.len(), 2, "{rounds:?}");
    assert_eq!(rounds[0]["verdict"], "changes");
    assert_eq!(rounds[1]["verdict"], "approve");
    assert_eq!(rounds[1]["findings"][0]["severity"], "minor");
    let prompt = std::fs::read_to_string(rec.path().join("review2.txt")).unwrap();
    assert!(prompt.contains("EARLIER REVIEW ROUNDS"), "{prompt}");
    assert!(prompt.contains("ONE") && prompt.contains("TWO"), "{prompt}");
}
