// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason task priority`: it re-ranks a task's waiting resource requests in
//! place, keeping their arrival time, leaves other tasks' requests alone, and
//! is the owner's alone. Also `order.max_wait_s` in resources.toml.

mod support;
use mason_core::*;
use mason_daemon::cpu::FakeCpu;
use mason_daemon::sched::{parse_resource_cfg, Broker, LeaseState, Standing};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use support::*;

fn compile() -> BTreeMap<String, u32> {
    [("compile".to_string(), 1)].into()
}

fn broker(name: &str) -> Arc<Broker> {
    let dir = std::env::temp_dir().join(format!("mason-prio-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = parse_resource_cfg("[cpu]\ncompile_settle_s = 0\ncompile_jobs = 1\n").unwrap();
    Broker::new(
        dir.join("locks"),
        dir.join("limits.kv"),
        &cfg,
        10,
        FakeCpu::replacing_compile_count(0),
        &mason_daemon::providers::default_accounts(),
    )
    .unwrap()
}

fn order(b: &Broker) -> Vec<String> {
    b.compilation()
        .pending
        .iter()
        .map(|w| w.label.clone())
        .collect()
}

#[test]
fn a_priority_change_reranks_that_tasks_waiting_requests_and_keeps_their_arrival() {
    let b = broker("rerank");
    let held = b.lease_open("cc", &compile());
    assert!(matches!(b.lease_poll(held), LeaseState::Granted { .. }));
    let standing = Some(Standing {
        steps_remaining: 2,
        span: 4,
        priority: 2,
    });
    let open = |label: &str, task: u64| {
        let id = b.lease_open_as(label, &compile(), standing);
        b.lease_set_task(id, TaskId(task));
        id
    };
    open("one", 1);
    open("two", 2);
    open("three", 3);
    assert_eq!(order(&b), ["one", "two", "three"]);
    // Task 3 becomes urgent: it goes first. The other two keep their order.
    assert_eq!(b.set_task_priority(TaskId(3), 0), 1);
    assert_eq!(order(&b), ["three", "one", "two"]);
    // Back to normal: it returns to its own place by arrival, behind the two earlier ones.
    assert_eq!(b.set_task_priority(TaskId(3), 2), 1);
    assert_eq!(order(&b), ["one", "two", "three"]);
    // Low: behind the others whatever its arrival.
    assert_eq!(b.set_task_priority(TaskId(1), 3), 1);
    assert_eq!(order(&b), ["two", "three", "one"]);
    // A task with nothing waiting changes nothing.
    assert_eq!(b.set_task_priority(TaskId(99), 0), 0);
    assert_eq!(order(&b), ["two", "three", "one"]);
}

#[test]
fn only_the_owner_may_change_a_tasks_priority() {
    let s = Scratch::new("true", json!({"steps": {}}));
    s.write_project("scratch", "true", "");
    let d = s.start();
    mason_daemon::attention::pause(&d, "", "claims", "test", 3600).unwrap();
    let t = file_task(&d, "x");
    let owner = d.cfg.owner_names[0].clone();
    assert_eq!(task_of(&d, t).priority, Priority::Normal);
    for who in ["", "someone-else"] {
        let (code, _) = post(
            &d,
            who,
            format!("/tasks/{}/priority", t.0),
            json!({"priority": "urgent"}),
        );
        assert_eq!(code, 403, "{who:?}");
    }
    assert_eq!(task_of(&d, t).priority, Priority::Normal);
    let (code, _) = post(
        &d,
        &owner,
        format!("/tasks/{}/priority", t.0),
        json!({"priority": "soon"}),
    );
    assert_eq!(code, 400);
    let (code, v) = post(
        &d,
        &owner,
        format!("/tasks/{}/priority", t.0),
        json!({"priority": "urgent"}),
    );
    assert_eq!(code, 200, "{v}");
    assert_eq!(task_of(&d, t).priority, Priority::Urgent);
}

#[test]
fn max_wait_s_is_read_and_a_negative_one_is_refused() {
    assert_eq!(parse_resource_cfg("").unwrap().max_wait_s, 1800);
    assert_eq!(
        parse_resource_cfg("[order]\nmax_wait_s = 90\n")
            .unwrap()
            .max_wait_s,
        90
    );
    assert_eq!(
        parse_resource_cfg("[order]\nmax_wait_s = 0\n")
            .unwrap()
            .max_wait_s,
        0
    );
    assert!(parse_resource_cfg("[order]\nmax_wait_s = -1\n").is_err());
    assert!(parse_resource_cfg("[order]\nmax_wait_s = \"soon\"\n").is_err());
}

fn post(d: &Arc<mason_daemon::Daemon>, who: &str, path: String, body: Value) -> (u16, Value) {
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
