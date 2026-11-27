// SPDX-License-Identifier: GPL-3.0-or-later

//! Task 1132: only a blocking review finding sends a task back. Minor ones are kept in the
//! feedback history, shown with the task and given to the next develop round. A real daemon,
//! with the fake agent recording the prompts it is given. Nothing here asserts a duration.

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

fn minor(issue: &str) -> Value {
    json!({"issue": issue, "file": "a.txt", "severity": "minor", "quote": REAL_LINE})
}

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

fn run(rounds: u32, develops: &[u32], reviews: Vec<Value>, rec: &Path) -> (Scratch, Value) {
    let develops: Vec<Value> = develops.iter().map(|n| develop(rec, *n)).collect();
    let s = Scratch::new(
        "false",
        json!({"steps": {"develop": develops, "review": reviews}}),
    );
    s.write_project("scratch", "false", "auto_retry = false\ntest_reruns = 0");
    s.push_files(&[(".mason/mason.toml", &def(rounds))], "project setup");
    let d = s.start();
    let t = file_task(&d, "Reviewed");
    wait_state(&d, t, State::Landed, 600);
    let mut v = shown(&d, t);
    v["history"] = json!(
        std::fs::read_to_string(d.task_dir(t).join("feedback-history.txt")).unwrap_or_default()
    );
    (s, v)
}

#[test]
fn an_approval_with_minor_findings_lands_and_keeps_them() {
    let rec = tempfile::tempdir().unwrap();
    let approve = json!([review(
        "approve",
        "fine",
        json!([minor("FIRST-NIT"), minor("SECOND-NIT")])
    )]);
    let (_s, v) = run(1, &[1], vec![approve], rec.path());
    assert_eq!(
        v["review_findings"],
        json!([
            format!("minor: FIRST-NIT (a.txt)\n    > {REAL_LINE}"),
            format!("minor: SECOND-NIT (a.txt)\n    > {REAL_LINE}")
        ]),
        "{v}"
    );
    let history = v["history"].as_str().unwrap();
    assert!(
        history.contains("approved with minor findings"),
        "{history}"
    );
    assert!(history.contains("- [minor] FIRST-NIT (a.txt)"), "{history}");
    assert!(
        history.contains("- [minor] SECOND-NIT (a.txt)"),
        "{history}"
    );
}

#[test]
fn an_approvals_advice_reaches_a_develop_round_that_follows() {
    let rec = tempfile::tempdir().unwrap();
    let reviews = vec![
        json!([review("approve", "fine", json!([minor("LATER-NIT")]))]),
        json!([review("approve", "fine", json!([]))]),
    ];
    // The check after the review is red after the first round, so there is a second one.
    let (_s, _v) = run(2, &[1, 2], reviews, rec.path());
    let get = |n: &str| std::fs::read_to_string(rec.path().join(n)).unwrap_or_default();
    assert!(!get("dev1.txt").contains("LATER-NIT"));
    assert!(
        get("dev2.txt").contains("[minor] LATER-NIT"),
        "{}",
        get("dev2.txt")
    );
}

#[test]
fn changes_with_only_minor_findings_is_refused_and_a_blocking_one_sends_back_marked() {
    let rec = tempfile::tempdir().unwrap();
    let reviews = vec![
        json!([
            review("changes", "REFUSED-SUMMARY", json!([minor("ONLY-A-NIT")])),
            // A quote that is not in the diff, the file or the task is refused too.
            review(
                "changes",
                "INVENTED-SUMMARY",
                json!([{"issue": "INVENTED", "file": "a.txt", "severity": "blocking",
                        "quote": "this line is nowhere at all"}])
            ),
            review(
                "changes",
                "ACCEPTED-SUMMARY",
                json!([
                    {"issue": "REAL-BUG", "file": "a.txt", "severity": "blocking", "quote": REAL_LINE},
                    minor("SMALL-NIT")
                ])
            )
        ]),
        // Round 2 answers the earlier blocking finding (task 1135).
        json!([{"call": {"tool": "submit_review", "args": {
            "verdict": "approve", "summary": "fine", "findings": [],
            "earlier": [{"round": 1, "index": 0, "status": "fixed", "note": "done"}]
        }}}]),
    ];
    let (_s, v) = run(1, &[1, 2], reviews, rec.path());
    let again = std::fs::read_to_string(rec.path().join("dev2.txt")).unwrap();
    assert!(again.contains("ACCEPTED-SUMMARY"), "{again}");
    assert!(!again.contains("REFUSED-SUMMARY"), "{again}");
    assert!(!again.contains("INVENTED"), "{again}");
    assert!(again.contains(&format!("> {REAL_LINE}")), "{again}");
    assert!(again.contains("- [blocking] REAL-BUG (a.txt)"), "{again}");
    assert_eq!(again.matches("[minor] SMALL-NIT").count(), 1, "{again}");
    let history = v["history"].as_str().unwrap();
    assert!(
        history.contains("- [blocking] REAL-BUG (a.txt)"),
        "{history}"
    );
    assert!(history.contains("- [minor] SMALL-NIT (a.txt)"), "{history}");
    assert_eq!(
        v["review_findings"],
        json!([format!("minor: SMALL-NIT (a.txt)\n    > {REAL_LINE}")]),
        "{v}"
    );
}
