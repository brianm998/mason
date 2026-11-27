// SPDX-License-Identifier: GPL-3.0-or-later

//! s1152: a developing agent's writes are checked against the rules on the base commit.

use crate::support;
use mason_core::*;
use mason_daemon::Daemon;
use serde_json::{json, Value};
use std::sync::Arc;
use support::*;

const RULE: &str = include_str!("../../../../.mason/rules/timing-assertion.toml");
const PATH: &str = "crates/a/tests/a.rs";

#[test]
fn a_timing_assertion_is_refused_and_its_rewrite_is_written() {
    // The hook runs the `mason` beside the MCP bridge.
    bin("mason-pipeline", "mason");
    let timed = "assert!(start.elapsed() < Duration::from_secs(2));\n";
    let guard = "assert!(rx.recv_timeout(Duration::from_secs(60)).is_ok());\n";
    // A rule that exists only in the worktree would refuse the second write too.
    let own = "mkdir -p .mason/rules && printf '%s\\n' 'id = \"own\"' 'paths = [\"**\"]' \
        'pattern = \"recv_timeout\"' 'action = \"block\"' 'message = \"own\"' > .mason/rules/own.toml";
    let s = Scratch::new(
        "true",
        json!({"steps": {"develop": [[
            {"sh": own},
            {"write": {"file_path": PATH, "content": timed}},
            {"sh": format!("test ! -e {PATH} && rm -r .mason/rules/own.toml && echo refused > refused.txt")},
            {"sh": own},
            {"write": {"file_path": PATH, "content": guard}},
            {"sh": "rm .mason/rules/own.toml && git add -A && \
                    git -c user.name=t -c user.email=t@t commit -qm test"},
            {"call": {"tool": "submit_change", "args": {"summary": "a test", "head": "$HEAD"}}},
        ]]}}),
    );
    s.push_files(&[(".mason/rules/timing-assertion.toml", RULE)], "rules");
    let d = s.start();
    let t = file_task_as(
        &d,
        "Add a test",
        "Please add a test.",
        true,
        TrustSource::Owner,
        false,
    );
    wait_state(&d, t, State::Landed, 600);

    let show = |path: &str| sh(&s.origin, &["show", &format!("main:{path}")]);
    assert_eq!(show(PATH).trim(), guard.trim());
    assert_eq!(show("refused.txt").trim(), "refused");
    let logs = mason_daemon::http::render_logs(&d, t.0);
    assert!(
        logs.contains("MASON rule `timing-assertion`: No timing assertions."),
        "{logs}"
    );
    let hits: Vec<Value> = mason_daemon::rules::hits(&d.task_dir(t));
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0]["rule"], "timing-assertion");
    assert_eq!(hits[0]["action"], "block");
    assert_eq!(hits[0]["path"], PATH);
    // What `mason task show` prints comes from the task's detail.
    let detail = get(&d, format!("/tasks/{}", t.0));
    assert_eq!(detail["rule_hits"], json!(hits), "{detail}");
}

fn get(d: &Arc<Daemon>, path: String) -> Value {
    let (d, who) = (d.clone(), d.cfg.owner_names[0].clone());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            use tower::ServiceExt;
            let req = axum::http::Request::builder()
                .uri(path)
                .header("x-mason-as", who)
                .body(axum::body::Body::empty())
                .unwrap();
            let resp = mason_daemon::http::api_router(d)
                .oneshot(req)
                .await
                .unwrap();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
                .await
                .unwrap();
            serde_json::from_slice(&bytes).unwrap()
        })
}
