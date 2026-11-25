// SPDX-License-Identifier: GPL-3.0-or-later

//! s397: a `retry` diagnosis of a failure outside the code is applied by itself for a project
//! that is not MASON's own; any other verdict stays a card. The project has `auto_retry` off,
//! so the retry loop is not the one retrying.

#![cfg(unix)]

use crate::support;
use mason_core::*;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use support::*;

fn auto_retries(d: &std::sync::Arc<mason_daemon::Daemon>, t: TaskId) -> usize {
    d.store
        .read_since(0, 100_000)
        .unwrap()
        .iter()
        .filter(
            |e| matches!(&e.event.kind, EventKind::Retried { task, auto: true, .. } if *task == t),
        )
        .count()
}

#[test]
fn a_retry_diagnosis_of_an_outside_failure_is_applied_by_itself_in_any_project() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    s.write_project(
        "scratch",
        "test -f hello.txt",
        "auto_retry = false\ndiagnose = false",
    );
    let hook = s.origin.join("hooks/pre-receive");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(
        &hook,
        "#!/bin/sh\necho 'rejected by the hook' >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let d = s.start();
    let t = file_task(&d, "Push is rejected");
    wait_for("the outside failure", 180, || {
        let k = task_of(&d, t);
        k.state == State::Failed && k.failure_class == Some(FailClass::Outside)
    });
    let ask = json!({"verdict": "ask", "summary": "s", "question": "q?", "recommendation": "r"});
    mason_daemon::diagnose::record_verdict(&d, t, ask);
    assert_eq!(auto_retries(&d, t), 0, "an ask is a card");
    assert_eq!(d.store.diagnoses(Some(t.0)).unwrap()[0].status, "pending");

    let retry = json!({"verdict": "retry", "summary": "the push was refused", "note": "again"});
    mason_daemon::diagnose::record_verdict(&d, t, retry);
    assert_eq!(auto_retries(&d, t), 1, "the retry was applied by itself");
    let rows = d.store.diagnoses(Some(t.0)).unwrap();
    assert_eq!(rows.last().unwrap().status, "applied");
}

fn post(d: &Arc<mason_daemon::Daemon>, who: &str, path: String) -> (u16, Value) {
    let d = d.clone();
    let who = who.to_string();
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
                .header("x-mason-as", who)
                .body(axum::body::Body::from("{}"))
                .unwrap();
            let resp = mason_daemon::http::api_router(d)
                .oneshot(req)
                .await
                .unwrap();
            let status = resp.status().as_u16();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            )
        })
}

fn card_turn() -> Value {
    json!([{"call": {"tool": "submit_diagnosis", "args": {
        "summary": "the push was refused by a hook; retrying will not help until it is lifted",
        "evidence_lines": ["rejected by the hook"],
        "options": [
            {"label": "Cancel the task", "recommended": true, "action": "cancel"},
            {"label": "Retry from the failed step", "recommended": false,
             "action": "retry", "note": "the hook may have been lifted"}]}}}])
}

#[test]
fn the_owner_can_ask_for_a_diagnosis_and_it_ends_in_a_card_once_per_failure() {
    let mut script = happy_script();
    script["steps"]["diagnose"] = json!([card_turn(), card_turn()]);
    let s = Scratch::new("test -f hello.txt", script);
    s.write_project(
        "scratch",
        "test -f hello.txt",
        "auto_retry = false\ndiagnose = false",
    );
    let hook = s.origin.join("hooks/pre-receive");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(
        &hook,
        "#!/bin/sh\necho 'rejected by the hook' >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let d = s.start();
    let owner = d.cfg.owner_names[0].clone();
    let t = file_task(&d, "Push is rejected");
    wait_for("the failure", 180, || task_of(&d, t).state == State::Failed);
    let path = format!("/tasks/{}/diagnose", t.0);
    assert_eq!(post(&d, "stranger", path.clone()).0, 403, "not the owner");
    assert_eq!(post(&d, &owner, path.clone()).0, 200);
    wait_for("the first diagnosis", 120, || {
        d.store.diagnoses(Some(t.0)).unwrap().len() == 1
    });
    let rows = d.store.diagnoses(Some(t.0)).unwrap();
    let card: Value = serde_json::from_str(&rows[0].card).unwrap();
    let opts = card["options"].as_array().unwrap();
    assert!((2..=4).contains(&opts.len()));
    assert_eq!(
        opts.iter().filter(|o| o["recommended"] == true).count(),
        1,
        "one recommended"
    );
    assert_eq!(card["key"], format!("diagnosis:{}", rows[0].id));
    assert_eq!(rows[0].status, "pending", "a recommended cancel waits");
    // The same failure is not diagnosed twice.
    assert_eq!(post(&d, &owner, path.clone()).0, 409);
    // Not failed: refused.
    let first_entered = task_of(&d, t).state_entered_at;
    let (code, _) = post(&d, &owner, format!("/tasks/{}/retry", t.0));
    assert_eq!(code, 200);
    wait_for("the second failure", 180, || {
        let k = task_of(&d, t);
        k.state == State::Failed && k.state_entered_at != first_entered
    });
    assert_eq!(post(&d, &owner, path).0, 200);
    wait_for("the second diagnosis", 120, || {
        d.store.diagnoses(Some(t.0)).unwrap().len() == 2
    });
}

#[test]
fn a_task_that_is_not_failed_is_not_diagnosed() {
    let s = Scratch::new("true", json!({"steps": {}}));
    s.write_project("scratch", "true", "");
    let d = s.start();
    mason_daemon::attention::pause(&d, "", "claims", "test", 3600).unwrap();
    let owner = d.cfg.owner_names[0].clone();
    let t = file_task(&d, "Waits");
    assert_eq!(post(&d, &owner, format!("/tasks/{}/diagnose", t.0)).0, 409);
    assert_eq!(post(&d, &owner, "/tasks/9999/diagnose".into()).0, 404);
}

fn reject_pushes(s: &Scratch) {
    let hook = s.origin.join("hooks/pre-receive");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(
        &hook,
        "#!/bin/sh\necho 'rejected by the hook' >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn an_outside_failure_with_an_automatic_retry_coming_is_not_diagnosed() {
    let mut script = happy_script();
    script["steps"]["diagnose"] = json!([card_turn()]);
    let s = Scratch::new("test -f hello.txt", script);
    s.write_project("scratch", "test -f hello.txt", "auto_retry = true");
    reject_pushes(&s);
    let d = s.start_with(|c| {
        c.auto_retry_delays = vec![std::time::Duration::ZERO];
        c.auto_retry_tick = std::time::Duration::from_secs(3600);
    });
    let t = file_task(&d, "Push is rejected");
    wait_for("the failure", 180, || task_of(&d, t).state == State::Failed);
    let w = d.world().unwrap();
    assert!(mason_daemon::autoretry::pending(
        &d,
        w.tasks.get(&t).unwrap()
    ));
    drop(w);
    let why = mason_daemon::diagnose::start(&d, t, StepRunId(0), false).unwrap_err();
    assert!(why.contains("automatic retry"), "{why}");
    assert!(d.store.diagnoses(Some(t.0)).unwrap().is_empty());
}

#[test]
fn an_outside_failure_with_no_retry_coming_is_diagnosed_by_itself() {
    let mut script = happy_script();
    script["steps"]["diagnose"] = json!([card_turn()]);
    let s = Scratch::new("test -f hello.txt", script);
    s.write_project("scratch", "test -f hello.txt", "auto_retry = false");
    reject_pushes(&s);
    let d = s.start();
    let t = file_task(&d, "Push is rejected");
    wait_for("the diagnosis", 180, || {
        !d.store.diagnoses(Some(t.0)).unwrap().is_empty()
    });
}
