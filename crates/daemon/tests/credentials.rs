// SPDX-License-Identifier: GPL-3.0-or-later

//! Task #95 end to end: an expired agent login is a provider hold and a
//! `credentials` alert, not a task failure. The real `masond` and `mason`
//! binaries run two tasks with a fake agent that answers "Not logged in"; both
//! stay unfailed, `mason alerts` shows credentials, no time is charged, and both
//! land once the fake agent is switched back to working. The same again with the
//! owner releasing the account (`mason account release`) instead of waiting for
//! a probe.

mod support;
use serde_json::{json, Value};
use support::*;

const COPIES: usize = 60;

fn login_turn() -> Value {
    json!([{"result": {"is_error": true, "text": "Not logged in \u{b7} Please run /login",
        "api_error_status": null}}])
}

/// Every turn the same, however many times each step is started.
fn script_of(explore: Value, develop: Value) -> Value {
    json!({"steps": {"explore": vec![explore; COPIES], "develop": vec![develop; COPIES]}})
}

fn working() -> Value {
    let u = unique_script();
    script_of(explore_turn(), u["steps"]["develop"][0].clone())
}

fn refusing() -> Value {
    script_of(login_turn(), login_turn())
}

fn alerts(s: &Scratch) -> String {
    s.mason_ok(&["alerts"])
}

fn run(release: bool) {
    let s = Scratch::new("true", refusing());
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    s.mason_ok(&["task", "add", "First", "--body", "Create a file."]);
    s.mason_ok(&["task", "add", "Second", "--body", "Create another file."]);

    wait_for("the credentials alert", 60, || {
        alerts(&s).contains("credentials")
    });
    let text = alerts(&s);
    assert!(text.contains("log in again"), "{text}");
    // Give the tasks time to fail if they were going to.
    std::thread::sleep(std::time::Duration::from_secs(3));
    for id in [0u64, 1] {
        let v = s.task_json(id);
        let st = v["state"].as_str().unwrap_or("");
        assert!(
            !matches!(st, "failed" | "canceled" | "landed"),
            "task {id}: {v:#}"
        );
        assert_eq!(v["failures"].as_u64().unwrap_or(0), 0, "task {id}: {v:#}");
    }

    std::fs::write(&s.script, working().to_string()).unwrap();
    if release {
        s.mason_ok(&["account", "release", "claude"]);
    }
    s.wait_task(0, "landed", 120);
    s.wait_task(1, "landed", 120);
    wait_for("the alert to clear", 30, || {
        !alerts(&s).contains("credentials")
    });
    for id in [0u64, 1] {
        assert_eq!(s.task_json(id)["failures"].as_u64().unwrap_or(0), 0);
    }
}

#[test]
fn an_expired_login_holds_the_tasks_and_they_land_when_it_is_renewed() {
    run(false);
}

#[test]
fn releasing_the_account_does_not_wait_for_the_probe() {
    run(true);
}
