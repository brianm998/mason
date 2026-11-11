// SPDX-License-Identifier: GPL-3.0-or-later

//! A full disk is said out loud (task 85). A real `masond` with the free-disk reading below
//! the floor and three filed tasks: `mason status` prints the STUCK line (and `--json` the same
//! fact), `mason resources` names the disk for all three, `mason notify status` lists `disk`,
//! and the alert is delivered on a channel other than the monitor. With room, the line goes and
//! the tasks run.

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

fn json_of(s: &Scratch, args: &[&str]) -> Value {
    serde_json::from_str(&s.mason_ok(args)).unwrap()
}

fn until(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn a_full_disk_is_shown_in_status_resources_and_notify_and_clears_with_room() {
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
    let disk = s.root.path().join("disk-low");
    std::fs::write(&disk, "12\n").unwrap();
    let mut m = s.masond(
        &[],
        &[
            ("MASON_DISK_FILE", disk.to_str().unwrap()),
            ("MASON_DISK_EVERY_MS", "50"),
        ],
    );
    s.wait_daemon(&mut m);
    for t in ["a", "b", "c"] {
        s.mason_ok(&["task", "add", t, "--body", "Do it.", "--explored"]);
    }
    until("the STUCK line with three tasks", 60, || {
        s.mason_ok(&["status"])
            .contains("STUCK: no new compile or agent session can start: disk has 12 GB free, floor 20 GB; 3 tasks waiting")
    });
    let j = json_of(&s, &["status", "--json"]);
    assert_eq!(j["stuck"]["cause"], "disk");
    assert_eq!(j["stuck"]["tasks_waiting"], 3);

    let r = s.mason_ok(&["resources"]);
    assert_eq!(
        r.matches("Waiting: disk has 12 GB free, floor 20 GB")
            .count(),
        3,
        "{r}"
    );
    let n = s.mason_ok(&["notify", "status"]);
    assert!(n.contains("disk"), "{n}");
    until("a delivery on a channel other than the monitor", 60, || {
        let n = json_of(&s, &["notify", "status", "--json"]);
        n["recent"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|a| a.to_string().contains("MASON is stopped"))
    });

    std::fs::write(&disk, "40\n").unwrap();
    until("the STUCK line to go", 60, || {
        !s.mason_ok(&["status"]).contains("STUCK")
    });
    assert!(json_of(&s, &["status", "--json"])["stuck"].is_null());
    s.wait_task(0, "landed", 240);
}
