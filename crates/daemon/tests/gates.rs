// SPDX-License-Identifier: GPL-3.0-or-later

//! `POST /resources/gate`: owner only, bounds held, and `GET /resources` says where each gate's
//! value came from.

mod support;
use serde_json::{json, Value};
use std::sync::Arc;
use support::*;

fn call(
    d: &Arc<mason_daemon::Daemon>,
    method: &str,
    who: &str,
    path: &str,
    body: Value,
) -> (u16, Value) {
    let d = d.clone();
    let (method, who, path) = (method.to_string(), who.to_string(), path.to_string());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            use tower::ServiceExt;
            let req = axum::http::Request::builder()
                .method(method.as_str())
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

fn gate(v: &Value, name: &str) -> Value {
    v["gates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["name"] == name)
        .unwrap()
        .clone()
}

#[test]
fn only_the_owner_changes_a_gate_and_the_source_is_reported() {
    let s = Scratch::new("true", json!({"steps": {}}));
    s.write_project("scratch", "true", "");
    let d = s.start();
    let owner = d.cfg.owner_names[0].clone();
    let body = json!({"name": "compile_cpu_max", "value": 70});

    let (code, _) = call(&d, "POST", "somebody-else", "/resources/gate", body.clone());
    assert_eq!(code, 403, "not the owner");
    let (code, _) = call(&d, "POST", "", "/resources/gate", body.clone());
    assert_eq!(code, 403, "nobody is not the owner");
    let (_, v) = call(&d, "GET", &owner, "/resources", json!(null));
    assert_eq!(gate(&v, "compile_cpu_max")["source"], "default");
    assert_eq!(gate(&v, "compile_cpu_max")["value"], 50);

    let (code, _) = call(
        &d,
        "POST",
        &owner,
        "/resources/gate",
        json!({"name": "compile_cpu_max", "value": 101}),
    );
    assert_eq!(code, 400, "out of range");
    let (code, _) = call(
        &d,
        "POST",
        &owner,
        "/resources/gate",
        json!({"name": "compile_cpu_max", "value": "x"}),
    );
    assert_eq!(code, 400);

    let (code, _) = call(&d, "POST", &owner, "/resources/gate", body);
    assert_eq!(code, 200);
    let (_, v) = call(&d, "GET", &owner, "/resources", json!(null));
    assert_eq!(gate(&v, "compile_cpu_max")["source"], "runtime");
    assert_eq!(gate(&v, "compile_cpu_max")["value"], 70);
    assert_eq!(v["cpu"]["compile_max_pct"], 70);

    let (code, _) = call(
        &d,
        "POST",
        &owner,
        "/resources/gate",
        json!({"name": "compile_cpu_max", "value": null}),
    );
    assert_eq!(code, 200);
    let (_, v) = call(&d, "GET", &owner, "/resources", json!(null));
    assert_eq!(gate(&v, "compile_cpu_max")["source"], "default");
    assert_eq!(gate(&v, "compile_cpu_max")["value"], 50);
}
