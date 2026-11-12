// SPDX-License-Identifier: GPL-3.0-or-later

//! s178: a suite may build first. The build holds `compile` and gives it back before the
//! suite's own `uses` are taken for the run; suites with one build command share one build;
//! `run_check` on a suites step runs only the suites the impact map selects.

mod support;
use mason_core::*;
use mason_daemon::http::render_logs;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use support::*;

/// A TAP suite of four tests that pass once `hello.txt` exists.
const CHECKS: &str = r#"#!/bin/sh
echo "1..4"
echo "ok 1 - alpha"
if [ -f hello.txt ]; then echo "ok 2 - hello"; else echo "not ok 2 - hello"; exit 1; fi
echo "ok 3 - gamma"
echo "ok 4 - delta"
"#;

const HEAD: &str = r#"schema = 1
start = "develop"

[resource.compile]
count = 1

[resource.test-suite]
count = 1

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.land]
type = "landing"
category = "landing"
"#;

fn commit(cmd: &str, msg: &str, check: bool) -> Value {
    let mut turns = vec![
        json!({"sh": format!("{cmd} && git add -A && git -c user.name=t -c user.email=t@t commit -qm '{msg}'")}),
    ];
    if check {
        turns.push(json!({"call": {"tool": "run_check", "args": {"name": "tests"}}}));
    }
    turns.push(
        json!({"call": {"tool": "submit_change", "args": {"summary": msg, "head": "$HEAD"}}}),
    );
    Value::Array(turns)
}

fn start(turns: Vec<Value>, definition: &str) -> (Scratch, Arc<mason_daemon::Daemon>) {
    let s = Scratch::new("false", json!({"steps": {"develop": turns}}));
    s.push_files(
        &[(".mason/mason.toml", definition), ("checks.sh", CHECKS)],
        "project setup",
    );
    let d = s.start_with(|c| c.env_retry = Duration::from_millis(200));
    (s, d)
}

fn exists(p: &Path) -> bool {
    p.exists()
}

#[test]
fn a_build_holds_compile_and_the_run_holds_only_the_suites_own_uses() {
    let gates = tempfile::tempdir().unwrap();
    let g = gates.path().display().to_string();
    let definition = format!(
        r#"{HEAD}
[step.test]
type = "test"
category = "testing"
uses = {{ compile = 1, test-suite = 1 }}
suites = ["checks", "again"]
on_red = "develop"
max_rounds = 2
next = "land"

[suite.checks]
build = "echo b >> {g}/builds; touch {g}/building; while [ ! -f {g}/go-build ]; do sleep 0.1; done"
command = "touch {g}/running; while [ ! -f {g}/go-run ]; do sleep 0.1; done; sh checks.sh"
parser = "tap"
min_tests = 3
level = "unit"
uses = {{ test-suite = 1 }}

[suite.again]
build = "echo b >> {g}/builds; touch {g}/building; while [ ! -f {g}/go-build ]; do sleep 0.1; done"
command = "sh checks.sh"
parser = "tap"
min_tests = 3
level = "unit"
uses = {{ test-suite = 1 }}
"#
    );
    let (_s, d) = start(
        vec![commit(
            "echo hello > hello.txt && echo x > u-$(basename \"$PWD\").txt",
            "add hello",
            false,
        )],
        &definition,
    );
    let t = file_task_as(
        &d,
        "Builds",
        "Please do it.",
        true,
        TrustSource::Owner,
        false,
    );
    let (building, running) = (gates.path().join("building"), gates.path().join("running"));
    wait_for("the build to start", 90, || exists(&building));
    // Holdings, read from the broker: the build holds compile and nothing else.
    assert_eq!(d.broker.used("compile"), 1, "the build holds compile");
    assert_eq!(d.broker.used("test-suite"), 0, "and not the run's slot");
    std::fs::write(gates.path().join("go-build"), "").unwrap();
    wait_for("the run to start", 90, || exists(&running));
    assert_eq!(
        d.broker.used("compile"),
        0,
        "compile was given back before the run"
    );
    assert_eq!(
        d.broker.used("test-suite"),
        1,
        "the run holds the suite's own uses"
    );
    std::fs::write(gates.path().join("go-run"), "").unwrap();
    wait_state(&d, t, State::Landed, 90);
    // The second suite has the same build command, so the tree was built once.
    let builds = std::fs::read_to_string(gates.path().join("builds")).unwrap();
    assert_eq!(builds.lines().count(), 1, "one build per tree: {builds}");
    assert_eq!(d.broker.used("compile"), 0);
    assert_eq!(d.broker.used("test-suite"), 0);
}

fn two_suites(extra_binding: &str) -> String {
    format!(
        r#"{HEAD}
[step.test]
type = "test"
category = "testing"
uses = {{ compile = 1, test-suite = 1 }}
suites = ["first", "second"]
on_red = "develop"
max_rounds = 2
next = "land"

[suite.first]
build = "true"
command = "echo RAN-FIRST; sh checks.sh"
parser = "tap"
min_tests = 3
level = "unit"

[suite.second]
build = "true"
command = "echo RAN-SECOND; sh checks.sh"
parser = "tap"
min_tests = 3
level = "unit"

{extra_binding}

[impact]
"first/**" = ["first"]
"second/**" = ["second"]
"hello.txt" = ["second"]
"#
    )
}

#[test]
fn run_check_runs_only_the_development_suites_the_impact_map_selects() {
    let definition = two_suites("[binding.unit]\nwhen = [\"development\", \"test_step\"]");
    let (_s, d) = start(
        vec![commit(
            "mkdir -p second && echo hello > hello.txt && echo x > second/f-$(basename \"$PWD\").txt",
            "change the second",
            true,
        )],
        &definition,
    );
    let t = file_task_as(
        &d,
        "Selected",
        "Please do it.",
        true,
        TrustSource::Owner,
        false,
    );
    wait_state(&d, t, State::Landed, 120);
    let logs = render_logs(&d, t.0);
    assert!(logs.contains("RAN-SECOND"), "{logs}");
    assert!(!logs.contains("RAN-FIRST"), "{logs}");
    assert!(logs.contains("second: passed"), "{logs}");
    assert!(logs.contains("first: not run"), "{logs}");
}

#[test]
fn run_check_without_a_development_binding_keeps_the_single_command() {
    // No binding: the suites run in the test step only, and run_check runs the project's
    // `test` command as before.
    let (_s, d) = {
        let s = Scratch::new(
            "echo FROM-THE-PROJECT-TEST",
            json!({"steps": {"develop": [commit(
                "echo hello > hello.txt",
                "change",
                true,
            )]}}),
        );
        s.push_files(
            &[
                (".mason/mason.toml", &two_suites("")),
                ("checks.sh", CHECKS),
            ],
            "project setup",
        );
        let d = s.start_with(|c| c.env_retry = Duration::from_millis(200));
        (s, d)
    };
    let t = file_task_as(
        &d,
        "Single",
        "Please do it.",
        true,
        TrustSource::Owner,
        false,
    );
    wait_state(&d, t, State::Landed, 120);
    let logs = render_logs(&d, t.0);
    assert!(logs.contains("FROM-THE-PROJECT-TEST"), "{logs}");
    assert!(!logs.contains("second: passed"), "{logs}");
}
