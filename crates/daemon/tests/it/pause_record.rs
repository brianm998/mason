// SPDX-License-Identifier: GPL-3.0-or-later

//! A pause and a resume leave a record of the client that made them and of the pause rows
//! they made or lifted, and a lift is shown for a while after it.

use crate::support;
use mason_daemon::views::Ctx;
use mason_daemon::Daemon;
use mason_protocol::command::Body as Cmd;
use serde_json::{json, Value};
use std::sync::Arc;
use support::*;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn http(d: &Arc<Daemon>, method: &str, path: &str, client: &str, body: Value) -> Value {
    let d = d.clone();
    runtime().block_on(async move {
        use tower::ServiceExt;
        let mut req = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        if !client.is_empty() {
            req = req.header("x-mason-client", client);
        }
        let req = req.body(axum::body::Body::from(body.to_string())).unwrap();
        let resp = mason_daemon::http::api_router(d)
            .oneshot(req)
            .await
            .unwrap();
        assert!(resp.status().is_success(), "{path}: {}", resp.status());
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    })
}

fn ws(d: &Arc<Daemon>, actor: &str, body: Cmd) {
    let cmd = mason_protocol::Command {
        id: "1".into(),
        actor: actor.into(),
        body: Some(body),
        ..Default::default()
    };
    let r = runtime().block_on(mason_daemon::ws::run_command(d, cmd));
    assert!(r.ok, "{}", r.error);
}

fn pause_ws(d: &Arc<Daemon>, actor: &str, mode: &str) -> u64 {
    ws(
        d,
        actor,
        Cmd::Pause(mason_protocol::PauseWork {
            project: String::new(),
            mode: mode.into(),
            for_seconds: 7200,
        }),
    );
    let rows = d.store.pauses(0).unwrap();
    rows.iter().rev().find(|p| p.mode == mode).unwrap().id
}

fn rows(d: &Daemon, kind: &str) -> Vec<mason_store::InterventionRow> {
    let all = d.store.interventions(0).unwrap();
    all.into_iter().filter(|r| r.kind == kind).collect()
}

#[test]
fn a_pause_through_ws_records_the_actor_and_the_monitor() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let id = pause_ws(&d, "brian", "claims");
    // Recorded before the command's result, so there is nothing to wait for.
    let r = rows(&d, "pause");
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0].who, "brian");
    assert!(
        r[0].detail.starts_with(&format!(
            "/pause via monitor: paused #{id} claims for everything, expires "
        )),
        "{}",
        r[0].detail
    );
    assert!(r[0].detail.ends_with(" UTC"), "{}", r[0].detail);
}

#[test]
fn each_resume_names_its_client_and_the_pause_it_lifted() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();

    let first = pause_ws(&d, "brian", "claims");
    let out = http(
        &d,
        "POST",
        "/resume",
        "cli",
        json!({"project": "", "mode": "claims"}),
    );
    assert_eq!(out["lifted"], 1);

    let second = pause_ws(&d, "brian", "landing");
    ws(
        &d,
        "brian",
        Cmd::Resume(mason_protocol::ResumeWork {
            project: String::new(),
            mode: "landing".into(),
        }),
    );

    let r = rows(&d, "resume");
    assert_eq!(r.len(), 2, "{r:?}");
    assert!(
        r[0].detail.starts_with(&format!(
            "/resume via cli: lifted #{first} claims for everything, expires "
        )),
        "{}",
        r[0].detail
    );
    assert!(
        r[1].detail.starts_with(&format!(
            "/resume via monitor: lifted #{second} landing for everything, expires "
        )),
        "{}",
        r[1].detail
    );
    assert_eq!(r[1].who, "brian");

    // The newest lift is what the list and the header say.
    let list = http(&d, "GET", "/pauses", "", json!({}));
    assert_eq!(list["last_lift"]["via"], "monitor", "{list}");
    assert_eq!(list["last_lift"]["who"], "brian");
    let hv = Ctx::new(&d, &d.world().unwrap()).header("");
    let b = hv.banners.iter().find(|b| b.kind == "pause_lifted");
    let b = b.expect("a banner for the lift");
    assert!(b.text.starts_with("Pause lifted "), "{}", b.text);
    assert!(b.text.ends_with(" by brian via monitor"), "{}", b.text);
    assert!(b.detail.contains(&format!("#{second} landing")));

    // A resume that lifts nothing is recorded, and does not replace the last lift.
    http(&d, "POST", "/resume", "", json!({}));
    let r = rows(&d, "resume");
    assert_eq!(r[2].detail, "/resume via unknown: lifted nothing");
    let list = http(&d, "GET", "/pauses", "", json!({}));
    assert_eq!(list["last_lift"]["via"], "monitor", "{list}");
}

#[test]
fn an_expired_row_is_cleared_but_not_called_lifted() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let old = d.store.add_pause("", "all", "brian", 1, 2).unwrap();
    let out = http(&d, "POST", "/resume", "agent:7", json!({}));
    assert_eq!(out["lifted"], 1);
    assert_eq!(
        rows(&d, "resume")[0].detail,
        format!("/resume via agent:7: lifted nothing; cleared expired #{old}")
    );
    let list = http(&d, "GET", "/pauses", "", json!({}));
    assert!(list["last_lift"].is_null(), "{list}");
}
