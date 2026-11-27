// SPDX-License-Identifier: GPL-3.0-or-later

//! Task 1134: a review that asks for changes is checked by a read-only validation session before
//! it sends the task back. A real daemon; the fake agent plays the reviewer and the validator.
//! Nothing here asserts a duration.

use crate::support;
use mason_core::*;
use serde_json::{json, Value};
use std::path::Path;
use support::*;

/// Test, review, then a check after the review that is red until develop has run `rounds` times.
fn def(rounds: u32) -> String {
    format!(
        r#"schema = 1
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
next = "check"

[step.check]
type = "command"
category = "review"
command = "test $(wc -l < a.txt) -ge {rounds}"
on_red = "develop"
max_rounds = 3
next = "land"

[step.land]
type = "landing"
category = "landing"
"#
    )
}

fn develop(rec: &Path, n: u32) -> Value {
    json!([
        {"sh": format!("printf '%s' \"$FAKE_PROMPT\" > {}/dev{n}.txt", rec.display())},
        {"sh": format!("echo {n} >> a.txt && git add a.txt && git -c user.name=t -c user.email=t@t commit -qm round{n}")},
        {"call": {"tool": "submit_change", "args": {"summary": "round", "head": "$HEAD"}}}
    ])
}

fn review(verdict: &str, summary: &str, findings: Value) -> Value {
    json!({"call": {"tool": "submit_review",
        "args": {"verdict": verdict, "summary": summary, "findings": findings}}})
}

/// A line of every diff these tests make.
const REAL_LINE: &str = "diff --git a/a.txt b/a.txt";

fn shown(d: &std::sync::Arc<mason_daemon::Daemon>, t: TaskId) -> Value {
    let d = d.clone();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            use tower::ServiceExt;
            let req = axum::http::Request::builder()
                .uri(format!("/tasks/{}", t.0))
                .header("x-mason-as", "owner")
                .body(axum::body::Body::empty())
                .unwrap();
            let resp = mason_daemon::http::api_router(d)
                .oneshot(req)
                .await
                .unwrap();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap();
            serde_json::from_slice(&bytes).unwrap()
        })
}

fn validation(verdicts: Value) -> Value {
    json!([{"call": {"tool": "submit_validation", "args": {"verdicts": verdicts}}}])
}

fn blocking(issue: &str) -> Value {
    json!({"issue": issue, "file": "a.txt", "severity": "blocking", "quote": REAL_LINE})
}

fn run(
    extra: &str,
    develops: &[u32],
    reviews: Vec<Value>,
    validations: Vec<Value>,
    rec: &Path,
) -> (Scratch, std::sync::Arc<mason_daemon::Daemon>, Value) {
    let develops: Vec<Value> = develops.iter().map(|n| develop(rec, *n)).collect();
    let mut steps = json!({"develop": develops, "review": reviews});
    if !validations.is_empty() {
        steps["validate"] = json!(validations);
    }
    let s = Scratch::new("false", json!({ "steps": steps }));
    s.write_project(
        "scratch",
        "false",
        &format!("auto_retry = false\ntest_reruns = 0\n{extra}"),
    );
    s.push_files(&[(".mason/mason.toml", &def(1))], "project setup");
    let d = s.start();
    let t = file_task(&d, "Reviewed");
    wait_state(&d, t, State::Landed, 600);
    let v = shown(&d, t);
    (s, d, v)
}

fn overruled(d: &std::sync::Arc<mason_daemon::Daemon>) -> usize {
    d.store
        .health_since(&["review_overruled"], 0)
        .unwrap()
        .len()
}

#[test]
fn one_confirmed_finding_sends_the_task_back_with_the_other_as_a_minor_note() {
    let rec = tempfile::tempdir().unwrap();
    let reviews = vec![
        json!([review(
            "changes",
            "S",
            json!([blocking("REAL-BUG"), blocking("DOUBTFUL")])
        )]),
        json!([review("approve", "fine", json!([]))]),
    ];
    let validations = vec![validation(json!([
        {"index": 0, "verdict": "confirmed", "reason": "seen it"},
        {"index": 1, "verdict": "not_confirmed", "reason": "the code is fine"}]))];
    let (_s, d, _v) = run("", &[1, 2], reviews, validations, rec.path());
    let again = std::fs::read_to_string(rec.path().join("dev2.txt")).unwrap();
    assert!(again.contains("- [blocking] REAL-BUG (a.txt)"), "{again}");
    assert!(again.contains("[minor] DOUBTFUL"), "{again}");
    assert!(again.contains("not confirmed by validation"), "{again}");
    assert!(!again.contains("[blocking] DOUBTFUL"), "{again}");
    assert_eq!(overruled(&d), 0);
}

#[test]
fn no_confirmed_finding_approves_with_minor_notes_and_logs_the_overrule() {
    let rec = tempfile::tempdir().unwrap();
    let reviews = vec![json!([review(
        "changes",
        "S",
        json!([blocking("INVENTED")])
    )])];
    let validations = vec![validation(json!([
        {"index": 0, "verdict": "not_confirmed", "reason": "no such problem"}]))];
    let (_s, d, v) = run("", &[1], reviews, validations, rec.path());
    let findings = v["review_findings"].to_string();
    assert!(findings.contains("minor: INVENTED"), "{findings}");
    assert!(findings.contains("not confirmed"), "{findings}");
    assert_eq!(overruled(&d), 1);
}

#[test]
fn a_validation_that_gives_no_verdict_leaves_the_reviewers_changes() {
    let rec = tempfile::tempdir().unwrap();
    let reviews = vec![
        json!([review("changes", "S", json!([blocking("REAL-BUG")]))]),
        json!([review("approve", "fine", json!([]))]),
    ];
    // The fake has no "validate" turns, so the session ends without submitting.
    let (_s, d, _v) = run("", &[1, 2], reviews, vec![], rec.path());
    let again = std::fs::read_to_string(rec.path().join("dev2.txt")).unwrap();
    assert!(again.contains("- [blocking] REAL-BUG (a.txt)"), "{again}");
    assert_eq!(overruled(&d), 0);
}

#[test]
fn validate_false_starts_no_validation() {
    let rec = tempfile::tempdir().unwrap();
    let reviews = vec![
        json!([review("changes", "S", json!([blocking("REAL-BUG")]))]),
        json!([review("approve", "fine", json!([]))]),
    ];
    // Would reject the finding, if it ran.
    let validations = vec![validation(json!([
        {"index": 0, "verdict": "not_confirmed", "reason": "r"}]))];
    let (_s, d, _v) = run(
        "[review]\nvalidate = false\n",
        &[1, 2],
        reviews,
        validations,
        rec.path(),
    );
    let again = std::fs::read_to_string(rec.path().join("dev2.txt")).unwrap();
    assert!(again.contains("- [blocking] REAL-BUG (a.txt)"), "{again}");
    assert_eq!(overruled(&d), 0);
}
