// SPDX-License-Identifier: GPL-3.0-or-later

//! s197: a retry is idempotent. Two retries that saw the same failure emit one
//! `Retried`; a retry of a running step needs `restart`; a retry of a failed task
//! that names no run is accepted as before.

mod support;
use mason_core::*;
use mason_daemon::Daemon;
use serde_json::{json, Value};
use std::sync::Arc;
use support::*;

fn world() -> (Scratch, Arc<Daemon>) {
    let s = Scratch::new("true", json!({"steps": {}}));
    s.write_project("scratch", "true", "");
    let d = s.start();
    mason_daemon::attention::pause(&d, "", "claims", "test", 3600).unwrap();
    (s, d)
}

fn post(d: &Arc<Daemon>, who: &str, path: String, body: Value) -> (u16, Value) {
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
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            )
        })
}

fn failed_task(d: &Arc<Daemon>) -> TaskId {
    let x = file_task(d, "x");
    d.emit(EventKind::Admitted {
        task: x,
        entry: None,
    })
    .unwrap();
    d.emit(EventKind::StepFailed {
        task: x,
        reason: "r".into(),
        class: None,
    })
    .unwrap();
    x
}

fn retried(d: &Arc<Daemon>, t: TaskId) -> usize {
    let w = d.world().unwrap();
    // Each Retried opens a step run after the failed one.
    w.step_runs.values().filter(|r| r.task == t).count()
}

fn get_run(d: &Arc<Daemon>, owner: &str, t: TaskId) -> u64 {
    let _ = owner;
    mason_daemon::http::latest_run(&d.world().unwrap(), t)
}

#[test]
fn two_retries_that_saw_the_same_failure_retry_once() {
    let (_s, d) = world();
    let owner = d.cfg.owner_names[0].clone();
    let x = failed_task(&d);
    let seen = get_run(&d, &owner, x);
    let runs = retried(&d, x);
    let p = format!("/tasks/{}/retry", x.0);
    let (c1, _) = post(&d, &owner, p.clone(), json!({"seen": seen}));
    let (c2, v2) = post(&d, &owner, p, json!({"seen": seen}));
    assert_eq!(c1, 200);
    assert_eq!(c2, 409, "{v2}");
    assert!(v2["error"].to_string().contains("already retried"), "{v2}");
    assert_eq!(retried(&d, x), runs + 1);
}

#[test]
fn a_retry_on_a_running_task_needs_restart() {
    let (_s, d) = world();
    let owner = d.cfg.owner_names[0].clone();
    let x = failed_task(&d);
    let p = format!("/tasks/{}/retry", x.0);
    assert_eq!(post(&d, &owner, p.clone(), json!({})).0, 200);
    let runs = retried(&d, x);
    assert_eq!(post(&d, &owner, p.clone(), json!({})).0, 409);
    assert_eq!(retried(&d, x), runs);
    assert_eq!(post(&d, &owner, p, json!({"restart": true})).0, 200);
    assert_eq!(retried(&d, x), runs + 1);
}

#[test]
fn a_retry_on_a_failed_task_without_seen_is_accepted() {
    let (_s, d) = world();
    let owner = d.cfg.owner_names[0].clone();
    let x = failed_task(&d);
    let (c, v) = post(&d, &owner, format!("/tasks/{}/retry", x.0), json!({}));
    assert_eq!(c, 200, "{v}");
}
