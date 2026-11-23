// SPDX-License-Identifier: GPL-3.0-or-later

//! Task 988: a note the owner gives a retry reaches the reviewer and later develop rounds, not
//! only the next step, and goes when the task lands. An automatic retry's note is not kept.
//! The fake agent records the prompts it is given. Nothing here asserts a duration.

use crate::support;
use mason_core::*;
use serde_json::{json, Value};
use support::*;

fn def(gate: &std::path::Path) -> String {
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
command = "test -f {}/ok"
on_red = "develop"
max_rounds = 1
next = "review"

[step.review]
type = "agent"
category = "review"
next = "land"

[step.land]
type = "landing"
category = "landing"
"#,
        gate.display()
    )
}

fn post(d: &std::sync::Arc<mason_daemon::Daemon>, path: String, body: Value) -> (u16, Value) {
    let d = d.clone();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            use tower::ServiceExt;
            let req = axum::http::Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .header("x-mason-as", "owner")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap();
            let resp = mason_daemon::http::api_router(d)
                .oneshot(req)
                .await
                .unwrap();
            let status = resp.status().as_u16();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap();
            (status, serde_json::from_slice(&bytes).unwrap_or_default())
        })
}

fn turn(rec: &std::path::Path, name: &str) -> Value {
    json!([
        {"sh": format!("printf '%s\\n=====\\n' \"$FAKE_PROMPT\" >> {}/{name}.txt", rec.display())},
        {"sh": "echo $(date +%s%N) >> a.txt && git add a.txt && git -c user.name=t -c user.email=t@t commit -qm round"},
        {"call": {"tool": "submit_change", "args": {"summary": "round", "head": "$HEAD"}}}
    ])
}

const NOTE: &str = "OWNER-NOTE judge against the reduced scope";

#[test]
fn an_owner_retry_note_reaches_the_review_and_goes_when_the_task_lands() {
    let rec = tempfile::tempdir().unwrap();
    let gate = tempfile::tempdir().unwrap();
    let r = rec.path();
    let s = Scratch::new(
        "false",
        json!({"steps": {
            "explore": [explore_turn()],
            "develop": [turn(r, "dev"), turn(r, "dev"), turn(r, "dev")],
            "review": [[
                {"sh": format!("printf '%s' \"$FAKE_PROMPT\" > {}/review.txt", r.display())},
                {"call": {"tool": "submit_review", "args": {"verdict": "approve", "summary": "fine"}}}
            ]],
        }}),
    );
    s.write_project("scratch", "false", "auto_retry = false");
    s.push_files(&[(".mason/mason.toml", &def(gate.path()))], "project setup");
    let d = s.start();
    let t = file_task(&d, "Needs a retry");
    wait_state(&d, t, State::Failed, 600);

    // An automatic retry's note is not kept.
    let latest = {
        let w = d.world().unwrap();
        mason_daemon::http::latest_run(&w, t)
    };
    let _ = latest;
    assert!(mason_daemon::ownernotes::list(&d.task_dir(t)).is_empty());

    std::fs::write(gate.path().join("ok"), "").unwrap();
    let (code, body) = post(
        &d,
        format!("/tasks/{}/retry", t.0),
        json!({"to": "develop", "message": NOTE}),
    );
    assert_eq!(code, 200, "{body}");
    // The notes are kept the moment the retry is made.
    let notes = mason_daemon::ownernotes::list(&d.task_dir(t));
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["step"], "develop");
    wait_state(&d, t, State::Landed, 600);

    let get = |n: &str| std::fs::read_to_string(r.join(format!("{n}.txt"))).unwrap_or_default();
    let review = get("review");
    assert!(review.contains("Owner decisions on this task"), "{review}");
    assert!(review.contains(NOTE), "{review}");
    // The develop round the note was given to has it once, in its feedback.
    assert_eq!(get("dev").matches(NOTE).count(), 1, "{}", get("dev"));
    // The notes go in the landed task's cleanup (`jobs::cleanup`), which runs after the state
    // reads Landed: checked at that moment it was red on busy runners, green alone. Wait for the
    // cleanup, not for a moment; a note that never goes still fails.
    let _ = support::try_wait_for_exact("landing to remove the notes", 120, 200, || {
        mason_daemon::ownernotes::list(&d.task_dir(t)).is_empty()
    });
    assert!(
        mason_daemon::ownernotes::list(&d.task_dir(t)).is_empty(),
        "landing removed the notes"
    );
}

#[test]
fn an_automatic_retry_note_is_not_kept() {
    let rec = tempfile::tempdir().unwrap();
    let gate = tempfile::tempdir().unwrap();
    let s = Scratch::new(
        "false",
        json!({"steps": {
            "explore": [explore_turn()],
            "develop": [turn(rec.path(), "dev"), turn(rec.path(), "dev")],
        }}),
    );
    s.write_project("scratch", "false", "auto_retry = false");
    s.push_files(&[(".mason/mason.toml", &def(gate.path()))], "project setup");
    let d = s.start();
    let t = file_task(&d, "Automatic");
    wait_state(&d, t, State::Failed, 600);
    let latest = {
        let w = d.world().unwrap();
        mason_daemon::http::latest_run(&w, t)
    };
    let ask = mason_daemon::http::RetryAsk {
        to: Some("develop".into()),
        seen: Some(latest),
        auto: true,
        message: Some("by the daemon".into()),
        ..Default::default()
    };
    mason_daemon::http::retry_task(&d, t.0, ask).unwrap();
    assert!(mason_daemon::ownernotes::list(&d.task_dir(t)).is_empty());
}
