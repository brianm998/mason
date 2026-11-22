// SPDX-License-Identifier: GPL-3.0-or-later

//! Decision 0962: a red landing result is reused when the daemon re-submits a task by itself,
//! and run again when the owner retries the task or presses Land now on it.
//!
//! Each run of the red suite is recorded in `reds`; a run is one line, or two when the failing
//! tests are rerun at once (as in `landing_yield`). Nothing here asserts a duration.

use crate::support;
use mason_core::*;
use support::lw::*;
use support::*;

const TOML: &str =
    "window = \"2s\"\nmax_size = 5\n[[tier]]\nname = \"batch\"\nsuites = [\"integration\"]\n";
const NOW_TOML: &str =
    "window = \"600s\"\nmax_size = 5\n[[tier]]\nname = \"batch\"\nsuites = [\"integration\"]\n";

fn checks(gate: &std::path::Path) -> String {
    let g = gate.display();
    format!(
        r#"#!/bin/sh
n="$1"
echo "1..1"
if [ -n "$(find . -name "red.$n" -not -path "./.git/*")" ]; then
  echo "$PWD $$" >> {g}/reds
  echo "not ok 1 - $n"; exit 1
fi
echo "ok 1 - $n"
"#
    )
}

fn reds(gate: &std::path::Path) -> usize {
    std::fs::read_to_string(gate.join("reds")).map_or(0, |t| t.lines().count())
}

fn post(
    d: &std::sync::Arc<mason_daemon::Daemon>,
    path: String,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
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

/// How many finished batches say they reused an earlier run.
fn reusing_batches(w: &W) -> usize {
    w.d.store
        .batches(Some("scratch"), 100)
        .unwrap()
        .iter()
        .filter(|b| b.finished_at.is_some() && b.note.contains("reused"))
        .count()
}

fn finished_batches(w: &W) -> usize {
    w.d.store
        .batches(Some("scratch"), 100)
        .unwrap()
        .iter()
        .filter(|b| b.finished_at.is_some())
        .count()
}

/// One red task, sent back once by its first batch. Returns the world, the task, the gate dir and
/// the number of red lines the first batch made.
fn sent_back_once(toml: &str, now: bool) -> (W, TaskId, tempfile::TempDir, usize) {
    let gate = tempfile::tempdir().unwrap();
    let script = checks(gate.path());
    let w = world(toml, &[("checks.sh", &script)]);
    let t = w.task("red", &[("x/red.integration", "boom")]);
    if now {
        w.d.store.set_landing_flag(t.0, "now", "owner", 1).unwrap();
        w.d.landing().wake();
    }
    wait_for("the red task to be sent back", 180, || {
        w.sent_back(t).is_some()
    });
    wait_for("the batch to finish", 60, || finished_batches(&w) >= 1);
    // Land now's mark is for that batch only here: the next round waits for the window.
    let _ = w.d.store.clear_landing_flag(t.0);
    let n = reds(gate.path());
    assert!(n >= 1, "the first batch ran the suite");
    (w, t, gate, n)
}

fn retry_from_land(w: &W, t: TaskId, auto: bool) {
    if auto {
        let ask = mason_daemon::http::RetryAsk {
            to: Some("land".into()),
            restart: true,
            auto: true,
            ..Default::default()
        };
        mason_daemon::http::retry_task(&w.d, t.0, ask).unwrap();
    } else {
        let (code, body) = post(
            &w.d,
            format!("/tasks/{}/retry", t.0),
            serde_json::json!({"to": "land", "restart": true}),
        );
        assert_eq!(code, 200, "{body}");
    }
}

#[test]
fn an_automatic_re_submit_reuses_the_red_result() {
    let (w, t, gate, n) = sent_back_once(TOML, false);
    let before = finished_batches(&w);
    retry_from_land(&w, t, true);
    wait_for("the next batch to finish", 180, || {
        finished_batches(&w) > before
    });
    assert_eq!(reds(gate.path()), n, "the suite did not run again");
    assert!(reusing_batches(&w) >= 1);
}

#[test]
fn an_owner_retry_from_land_runs_the_suite_again() {
    let (w, t, gate, n) = sent_back_once(TOML, false);
    let before = finished_batches(&w);
    retry_from_land(&w, t, false);
    wait_for("the next batch to finish", 180, || {
        finished_batches(&w) > before
    });
    assert!(reds(gate.path()) > n, "the suite ran again");
    assert_eq!(reusing_batches(&w), 0);
}

#[test]
fn land_now_on_a_task_with_a_cached_red_runs_the_suite_again() {
    let (w, t, gate, n) = sent_back_once(NOW_TOML, true);
    let before = finished_batches(&w);
    // The daemon puts it back by itself; the 10 minute window keeps it queued.
    retry_from_land(&w, t, true);
    wait_for("the task to be queued again", 60, || {
        state_of(&w.d, t) == State::Landing
    });
    let (code, body) = post(
        &w.d,
        format!("/tasks/{}/land-now", t.0),
        serde_json::json!({}),
    );
    assert_eq!(code, 200, "{body}");
    wait_for("the land-now batch to finish", 180, || {
        finished_batches(&w) > before
    });
    assert!(reds(gate.path()) > n, "the suite ran again");
    assert_eq!(reusing_batches(&w), 0);
}
