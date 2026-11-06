// SPDX-License-Identifier: GPL-3.0-or-later

//! The real `masond` and `mason` binaries, talking over loopback HTTP.

mod support;
use serde_json::Value;
use std::io::{Read, Write};
use support::*;

#[test]
fn the_cli_drives_a_task_from_filing_to_landing() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);

    let out = s.mason_ok(&[
        "task",
        "add",
        "Add a hello file",
        "--body",
        "Create hello.txt containing hello.",
    ]);
    assert!(out.contains("Filed task #0 in scratch"), "{out}");
    s.wait_task(0, "landed", 90);

    let list = s.mason_ok(&["task", "list"]);
    assert!(
        list.contains("landed") && list.contains("Add a hello file"),
        "{list}"
    );
    assert!(s
        .mason_ok(&["task", "list", "--state", "queued"])
        .contains("no tasks"));

    let show = s.mason_ok(&["task", "show", "0"]);
    assert!(
        show.contains("landed    merged") && show.contains("Create hello.txt containing hello."),
        "{show}"
    );
    for step in ["exploring", "developing", "testing", "landing"] {
        assert!(show.contains(step), "{step} in {show}");
    }
    let logs = s.mason_ok(&["logs", "0"]);
    assert!(
        logs.contains("mcp__mason__submit_plan") && logs.contains("result: accepted"),
        "{logs}"
    );
    assert!(
        logs.contains("(test)"),
        "the test run's output is in the logs too: {logs}"
    );

    let status = s.mason_ok(&["status"]);
    assert!(status.contains("1 landed"), "{status}");
    let j: Value = serde_json::from_str(&s.mason_ok(&["status", "--json"])).unwrap();
    assert_eq!(j["tasks"]["landed"], 1);
    assert_eq!(j["bugs"], 0);
    assert_eq!(j["event_loop_stalls"], 0);
    assert_eq!(sh(&s.origin, &["show", "main:hello.txt"]), "hello");
}

#[test]
fn cancel_retry_and_mistakes_are_reported_plainly() {
    let develop = serde_json::json!([{"sh": TASK_SCRIPT_COMMIT}, {"sleep_ms": 60000}]);
    let s = Scratch::new(
        "true",
        serde_json::json!({"steps": {"explore": [explore_turn()], "develop": [develop]}}),
    );
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    s.mason_ok(&["task", "add", "Long one"]); // one project: no --project needed
    s.wait_task(0, "developing", 60);

    // Retrying a task that is running is a legal "start this step again"; cancelling is final.
    let out = s.mason_ok(&["task", "cancel", "0"]);
    assert!(out.contains("its branch is kept"), "{out}");
    s.wait_task(0, "canceled", 30);

    for (args, want) in [
        (vec!["task", "show", "99"], "no task #99"),
        (vec!["task", "cancel", "0"], "not allowed"),
        (
            vec!["task", "add", "x", "--project", "nope"],
            "no project \"nope\"",
        ),
        (
            vec!["task", "add", "x", "--priority", "whenever"],
            "unknown priority",
        ),
        (
            vec!["task", "retry", "0", "--from", "sideways"],
            "unknown step",
        ),
        (vec!["task", "list", "--state", "nonsense"], "unknown state"),
        (vec!["task", "show", "abc"], "not a task number"),
        (vec!["frobnicate"], "unknown command"),
    ] {
        let o = s.mason(&args);
        assert!(!o.status.success(), "{args:?} should fail");
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains(want), "{args:?}: wanted {want:?} in {err:?}");
    }
}

#[test]
fn without_a_daemon_the_cli_says_so() {
    let s = Scratch::new("true", happy_script());
    let o = s.mason(&["status"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("masond is not running"));
}

#[test]
fn doctor_checks_the_setup_and_names_what_is_wrong() {
    let s = Scratch::new("true", happy_script());
    let out = s.mason_ok(&["doctor"]);
    assert!(
        out.contains("is a git repository") && out.contains("base ref \"origin/main\" exists"),
        "{out}"
    );
    assert!(out.contains("remote \"origin\" is configured"), "{out}");
    // Break the project: a base branch that does not exist, and a missing agent binary.
    let toml = std::fs::read_to_string(s.config_dir.join("projects/scratch.toml")).unwrap();
    std::fs::write(
        s.config_dir.join("projects/scratch.toml"),
        toml.replace("base = \"main\"", "base = \"no-such-branch\"")
            .replace("fake-claude", "missing-claude"),
    )
    .unwrap();
    let o = s.mason(&["doctor"]);
    assert!(!o.status.success(), "doctor fails when something is broken");
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("FAIL") && out.contains("no-such-branch") && out.contains("missing-claude"),
        "{out}"
    );
    std::fs::write(
        s.config_dir.join("projects/scratch.toml"),
        "path = \"/nope\"\n",
    )
    .unwrap();
    let out = String::from_utf8_lossy(&s.mason(&["doctor"]).stdout).to_string();
    assert!(out.contains("`base` is required"), "{out}");
}

fn http_post(port: u16, path: &str) -> String {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        s,
        "POST {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

#[test]
fn the_watchdog_notices_when_the_event_loop_is_blocked() {
    let s = Scratch::new("true", happy_script());
    let mut m = s.masond(&["--debug"], &[]);
    s.wait_daemon(&mut m);
    let before: Value = serde_json::from_str(&s.mason_ok(&["status", "--json"])).unwrap();
    assert_eq!(before["event_loop_stalls"], 0);

    let info: Value =
        serde_json::from_str(&std::fs::read_to_string(s.data_dir.join("daemon.json")).unwrap())
            .unwrap();
    let reply = http_post(
        info["port"].as_u64().unwrap() as u16,
        "/debug/stall?ms=1200",
    ); // blocks the loop on purpose
    assert!(reply.contains("blocked_ms"), "{reply}");
    wait_for("the watchdog to report", 10, || {
        let v: Value = serde_json::from_str(&s.mason_ok(&["status", "--json"])).unwrap();
        v["event_loop_stalls"].as_u64().unwrap_or(0) >= 1
            && v["worst_stall_ms"].as_u64().unwrap_or(0) >= 500
    });
    // Without --debug the endpoint does not exist.
    let s2 = Scratch::new("true", happy_script());
    let mut m2 = s2.masond(&[], &[]);
    s2.wait_daemon(&mut m2);
    let info: Value =
        serde_json::from_str(&std::fs::read_to_string(s2.data_dir.join("daemon.json")).unwrap())
            .unwrap();
    assert!(http_post(info["port"].as_u64().unwrap() as u16, "/debug/stall?ms=10").contains("404"));
}

#[test]
fn a_second_daemon_refuses_to_start_on_the_same_data() {
    let s = Scratch::new("true", happy_script());
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    let mut second = s.masond(&[], &[]);
    let st = {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if let Some(st) = second.child.try_wait().unwrap() {
                break st;
            }
            assert!(
                std::time::Instant::now() < end,
                "the second daemon should have exited"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    };
    assert!(!st.success());
    // The first is unharmed.
    assert!(s.mason(&["status"]).status.success());
}
