// SPDX-License-Identifier: GPL-3.0-or-later

//! A panic on a daemon thread is contained (task 63). A real `masond` gets a panic injected
//! at a named point (`MASON_PANIC_AT`) and must go on: the landing coordinator comes back,
//! a batch that panics fails its tasks with the reason and gives its slot back, `admit`
//! admits again, and `mason status` says a thread panicked. The store writer and the
//! reader lock are tested in the store crate.

mod support;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use support::*;

const CHECKS: &str = "#!/bin/sh\necho \"1..1\"\necho \"ok 1 - integration\"\n";

const DEF: &str = r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "land"

[step.land]
type = "landing"
category = "landing"
on_red = "develop"
max_rounds = 2

[suite.integration]
command = "sh checks.sh"
parser = "tap"
min_tests = 1
level = "unit"
"#;

const LANDING: &str =
    "mode = \"direct\"\nwindow = \"1s\"\nmax_size = 1\n[[tier]]\nname = \"batch\"\nsuites = [\"integration\"]\n";

fn script() -> Value {
    json!({"steps": {"develop": [[
        {"sh": "echo ok > file-$(basename \"$PWD\").txt; git add -A && git -c user.name=t -c user.email=t@t commit -qm \"work $(basename \"$PWD\")\""},
        {"call": {"tool": "submit_change", "args": {"summary": "done", "head": "$HEAD"}}}
    ]]}})
}

fn daemon(point: &str) -> (Scratch, Masond) {
    let s = Scratch::new("true", script());
    s.write_project("scratch", "true", "");
    s.push_files(
        &[
            (".mason/mason.toml", DEF),
            (".mason/landing.toml", LANDING),
            ("checks.sh", CHECKS),
        ],
        "setup",
    );
    sh(&s.repo, &["fetch", "-q", "origin"]);
    let mut m = s.masond(&[], &[("MASON_PANIC_AT", point)]);
    s.wait_daemon(&mut m);
    (s, m)
}

fn add(s: &Scratch, title: &str) {
    s.mason_ok(&["task", "add", title, "--body", "Do it.", "--explored"]);
}

fn status(s: &Scratch) -> Value {
    serde_json::from_str(&s.mason_ok(&["status", "--json"])).unwrap()
}

fn panics(s: &Scratch) -> u64 {
    status(s)["thread_panics"]["count"].as_u64().unwrap_or(0)
}

fn assert_shown(s: &Scratch, what: &str) {
    let text = s.mason_ok(&["status"]);
    assert!(
        text.contains("THREAD PANICS") && text.contains(what),
        "`mason status` does not show the panic ({what}):\n{text}"
    );
}

#[test]
fn a_panic_in_the_landing_coordinator_does_not_stop_landing() {
    let (s, _m) = daemon("coordinator");
    add(&s, "first");
    s.wait_task(0, "landed", 240);
    assert!(panics(&s) >= 1);
    assert_shown(&s, "landing-scratch");
    add(&s, "second");
    s.wait_task(1, "landed", 240);
}

#[test]
fn a_panic_in_a_landing_batch_fails_its_tasks_and_the_next_batch_runs() {
    let (s, _m) = daemon("batch");
    add(&s, "first");
    let end = Instant::now() + Duration::from_secs(240);
    loop {
        let v = s.task_json(0);
        if v.to_string().contains("the landing batch panicked") {
            break;
        }
        assert!(Instant::now() < end, "the batch's task never failed: {v:#}");
        assert_ne!(
            v["state"].as_str(),
            Some("landed"),
            "a panicked batch landed"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_shown(&s, "landing batch");
    // No restart: the slot is free and the coordinator is alive.
    add(&s, "second");
    s.wait_task(1, "landed", 240);
}

#[test]
fn a_panic_inside_admit_does_not_stop_admission() {
    // Which caller of `admit` gets the panic (the HTTP handler of the first `task add` or
    // the watcher) is a race, so a 500 from the add is allowed; the task is filed before
    // `admit` runs. Either way the flag must be clear again and the task admitted.
    let (s, _m) = daemon("admit@2");
    let _ = s.mason(&["task", "add", "first", "--body", "Do it.", "--explored"]);
    s.wait_task(0, "landed", 240);
    add(&s, "second");
    s.wait_task(1, "landed", 240);
}

#[test]
fn a_panic_in_a_background_loop_is_filed_and_the_loop_goes_on() {
    let (s, _m) = daemon("loop:sweeper");
    wait_for("the panic to be filed", 60, || panics(&s) >= 1);
    assert_shown(&s, "sweeper");
    let doctor = s.mason(&["doctor"]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&doctor.stdout),
        String::from_utf8_lossy(&doctor.stderr)
    );
    assert!(text.contains("thread panic"), "doctor is silent:\n{text}");
    assert!(
        s.mason(&["status", "--json"]).status.success(),
        "still answering"
    );
    // The owner is told, not just counted.
    wait_for("the daemon_bug alert", 60, || {
        s.mason_ok(&["alerts"]).contains("thread_panic")
    });
}
