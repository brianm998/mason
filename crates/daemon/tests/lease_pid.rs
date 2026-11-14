// SPDX-License-Identifier: GPL-3.0-or-later

//! A `--pipeline` (outside) request records the pid of the process that asked,
//! so the broker can tell a dead requester from a slow one.

mod support;
use mason_daemon::sched::LeaseState;
use serde_json::{json, Value};
use support::*;

fn post(d: &std::sync::Arc<mason_daemon::Daemon>, path: &str, body: Value) -> (u16, Value) {
    let d = d.clone();
    let path = path.to_string();
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
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            )
        })
}

#[test]
fn an_outside_request_records_the_pid_of_its_requester() {
    let s = Scratch::new("true", json!({"steps": {}}));
    s.write_project("scratch", "true", "");
    let d = s.start();
    let map = [("compile".to_string(), 1)].into();
    let holder = d.broker.lease_open("holder", &map);
    assert!(matches!(
        d.broker.lease_poll(holder),
        LeaseState::Granted { .. }
    ));
    let me = std::process::id();
    let (code, v) = post(
        &d,
        "/leases",
        json!({"uses": {"compile": 1}, "label": "other run", "pipeline": "other", "pid": me}),
    );
    assert_eq!(code, 200, "{v}");
    let id = v["id"].as_u64().unwrap();
    assert!(matches!(
        d.broker.lease_poll(id),
        LeaseState::Waiting { .. }
    ));
    assert!(d.broker.waiting_pids().contains(&me));
    d.broker.lease_close(id);
    d.broker.lease_close(holder);
}
