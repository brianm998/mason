// SPDX-License-Identifier: GPL-3.0-or-later

//! Real `masond` against a remote that accepts connections and never answers:
//! it opens its port at once, a step's fetch stops at its deadline and says why,
//! and cancelling another task meanwhile returns at once and removes its worktree.

mod support;
use serde_json::json;
use std::net::TcpListener;
use std::time::{Duration, Instant};
use support::*;

/// A listener that holds every connection open and says nothing.
fn silent_remote() -> (String, std::thread::JoinHandle<()>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/x.git", l.local_addr().unwrap().port());
    let h = std::thread::spawn(move || {
        let mut held = vec![];
        for c in l.incoming().take(16) {
            held.push(c);
        }
    });
    (url, h)
}

const ENV: [(&str, &str); 2] = [
    ("MASON_GIT_TIMEOUT_SECS", "8"),
    // a sandbox may allow only `file`
    ("GIT_ALLOW_PROTOCOL", "file:http"),
];

#[test]
fn masond_opens_its_port_before_any_network_and_a_stalled_fetch_blocks_nothing_else() {
    let develop = json!([{"sh": TASK_SCRIPT_COMMIT}, {"sleep_ms": 60000}]);
    let s = Scratch::new(
        "true",
        json!({"steps": {"explore": [explore_turn()], "develop": [develop]}}),
    );
    bin("mason-pipeline", "mason");
    let mut m = s.masond(&[], &ENV);
    s.wait_daemon(&mut m);
    s.mason_ok(&["task", "add", "Other task"]);
    s.wait_task(0, "developing", 60);
    let wt = s.data_dir.join("w/scratch/0");
    assert!(wt.join(".git").exists(), "{}", wt.display());

    // From now on the remote never answers.
    let (url, _l) = silent_remote();
    sh(&s.repo, &["remote", "set-url", "origin", &url]);
    s.mason_ok(&["task", "add", "Stuck task"]);
    std::thread::sleep(Duration::from_secs(2)); // its first step is now fetching

    let t = Instant::now();
    assert!(s.mason(&["status"]).status.success());
    s.mason_ok(&["task", "cancel", "0", "--delete-branch"]);
    wait_for("the worktree to be removed", 5, || {
        !wt.join(".git").exists()
    });
    assert!(
        t.elapsed() < Duration::from_secs(5),
        "cancelling waited for the stalled fetch: {:?}",
        t.elapsed()
    );

    // The stalled fetch ends at its deadline and the task says why.
    wait_for("the stuck task to fail", 40, || {
        s.task_json(1)["state"] == "failed"
    });
    let shown = s.task_json(1).to_string();
    assert!(shown.contains("timed out"), "{shown}");
}

#[test]
fn startup_does_not_wait_for_the_network() {
    let s = Scratch::new("true", happy_script());
    let (url, _l) = silent_remote();
    sh(&s.repo, &["remote", "set-url", "origin", &url]);
    bin("mason-pipeline", "mason"); // builds it, if need be, before the clock starts
    let t = Instant::now();
    let mut m = s.masond(
        &[],
        &[
            ("MASON_GIT_TIMEOUT_SECS", "8"),
            ("GIT_ALLOW_PROTOCOL", "file:http"),
        ],
    );
    s.wait_daemon(&mut m);
    assert!(
        t.elapsed() < Duration::from_secs(20),
        "masond waited for the remote: {:?}",
        t.elapsed()
    );
}
