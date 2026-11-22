// SPDX-License-Identifier: GPL-3.0-or-later

//! s178: a suite may build first. The build holds `compile` and gives it back before the
//! suite's own `uses` are taken for the run; suites with one build command share one build;
//! `run_check` on a suites step runs only the suites the impact map selects.

use crate::support;
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

/// One suite that can be narrowed: a rerun template for test filters and a development command
/// for packages. Its `command`, which the test step runs, says RAN-FULL.
fn narrowable() -> String {
    format!(
        r#"{HEAD}
[step.test]
type = "test"
category = "testing"
uses = {{ compile = 1, test-suite = 1 }}
suites = ["only"]
on_red = "develop"
max_rounds = 2
next = "land"

[suite.only]
command = "echo RAN-FULL; sh checks.sh"
rerun = "echo RAN-RERUN {{tests}}; sh checks.sh"
dev_command = "echo RAN-DEV {{packages}}; sh checks.sh"
parser = "tap"
min_tests = 3
level = "unit"

[binding.unit]
when = ["development", "test_step"]

[impact]
"hello.txt" = ["only"]
"#
    )
}

#[test]
fn a_scoped_run_check_runs_the_narrowed_command_and_the_test_step_still_runs_it_all() {
    let turns = json!([
        {"sh": "echo hello > hello.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm change"},
        {"call": {"tool": "run_check", "args": {"name": "tests", "tests": ["foo"]}}},
        {"call": {"tool": "run_check", "args": {"name": "tests", "packages": ["pk-one"]}}},
        {"call": {"tool": "submit_change", "args": {"summary": "change", "head": "$HEAD"}}}
    ]);
    let (_s, d) = start(vec![turns], &narrowable());
    let t = file_task_as(
        &d,
        "Scoped",
        "Please do it.",
        true,
        TrustSource::Owner,
        false,
    );
    wait_state(&d, t, State::Landed, 120);
    let logs = render_logs(&d, t.0);
    assert!(logs.contains("RAN-RERUN foo"), "{logs}");
    assert!(logs.contains("RAN-DEV -p pk-one"), "{logs}");
    assert!(
        logs.contains("this green covers only those tests"),
        "{logs}"
    );
    // Neither scoped green stood in for the test step: it ran the whole suite.
    assert!(logs.contains("RAN-FULL"), "{logs}");
}

/// s523: after a red full run, an unscoped `run_check` re-runs only the failing tests and the
/// changed packages; after a green run it runs what the change affects;
/// `full: true` is always full.
#[test]
fn after_a_red_full_run_an_unscoped_check_reruns_only_the_failures_and_the_changed_crates() {
    // The failing test is named by the full command and by the re-run, until `hello.txt` exists.
    let def = narrowable()
        .replace(
            r#"command = "echo RAN-FULL; sh checks.sh""#,
            r#"command = "echo RAN-FULL; echo 'test a::b ... FAILED'; sh checks.sh""#,
        )
        .replace(
            "rerun = \"echo RAN-RERUN {tests};",
            "rerun = \"echo RAN-RERUN {tests}; [ -f crates/pk/hello.txt ] || echo 'test a::b ... FAILED';",
        )
        .replace(r#""hello.txt" = ["only"]"#, r#""crates/**" = ["only"]"#);
    // Every changed path belongs to the one crate, so the changed packages can be named.
    let checks = CHECKS.replace("hello.txt", "crates/pk/hello.txt");
    let run = json!({"call": {"tool": "run_check", "args": {"name": "tests"}}});
    let turns = json!([
        {"sh": "mkdir -p crates/pk/src && printf '[package]\\nname = \"pk\"\\nversion = \"0.1.0\"\\nedition = \"2021\"\\n' > crates/pk/Cargo.toml && : > crates/pk/src/lib.rs && git add -A && git -c user.name=t -c user.email=t@t commit -qm crate"},
        // Full, red: no hello.txt yet.
        {"call": {"tool": "run_check", "args": {"name": "tests", "full": true}}},
        // Narrowed to the one failure and the changed crate; still red.
        run,
        {"sh": "echo hello > crates/pk/hello.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm fix"},
        // Narrowed again (the last run was red); green.
        run,
        // Green last time: what the change affects.
        run,
        {"call": {"tool": "run_check", "args": {"name": "tests", "full": true, "why": "touched build.rs"}}},
        {"call": {"tool": "submit_change", "args": {"summary": "change", "head": "$HEAD"}}}
    ]);
    let s = Scratch::new("false", json!({"steps": {"develop": vec![turns]}}));
    s.push_files(
        &[
            (".mason/mason.toml", def.as_str()),
            ("checks.sh", checks.as_str()),
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"crates/pk\"]\nresolver = \"2\"\n",
            ),
        ],
        "project setup",
    );
    let d = s.start_with(|c| c.env_retry = Duration::from_millis(200));
    let t = file_task_as(
        &d,
        "Rerun",
        "Please do it.",
        true,
        TrustSource::Owner,
        false,
    );
    wait_state(&d, t, State::Landed, 120);
    let logs = render_logs(&d, t.0);
    assert_eq!(
        logs.matches("this re-ran the 1 tests that failed and the crates you changed")
            .count(),
        2,
        "{logs}"
    );
    assert!(logs.contains("RAN-RERUN a::b"), "{logs}");
    assert!(logs.contains("RAN-DEV -p pk"), "{logs}");
    assert!(
        !logs.contains("the full run comes before you submit"),
        "{logs}"
    );
    assert!(logs.contains("Full run (why: touched build.rs)"), "{logs}");
}

/// An unscoped `run_check` runs what the change affects, not the whole suite (2026-11-18: 87 of
/// 111 develop checks ran every test binary), and the test step still runs it all.
#[test]
fn an_unscoped_run_check_runs_what_the_change_affects_and_the_test_step_runs_it_all() {
    // The change is inside one crate, so the packages it touches can be named.
    let def = narrowable().replace(r#""hello.txt" = ["only"]"#, r#""crates/**" = ["only"]"#);
    let checks = CHECKS.replace("hello.txt", "crates/pk/hello.txt");
    let turns = json!([
        {"sh": "mkdir -p crates/pk/src && printf '[package]\\nname = \"pk\"\\nversion = \"0.1.0\"\\nedition = \"2021\"\\n' > crates/pk/Cargo.toml && : > crates/pk/src/lib.rs && echo hello > crates/pk/hello.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm change"},
        {"call": {"tool": "run_check", "args": {"name": "tests"}}},
        {"call": {"tool": "submit_change", "args": {"summary": "change", "head": "$HEAD"}}}
    ]);
    let s = Scratch::new("false", json!({"steps": {"develop": vec![turns]}}));
    s.push_files(
        &[
            (".mason/mason.toml", def.as_str()),
            ("checks.sh", checks.as_str()),
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"crates/pk\"]\nresolver = \"2\"\n",
            ),
        ],
        "project setup",
    );
    let d = s.start();
    let t = file_task_as(
        &d,
        "Default",
        "Please do it.",
        true,
        TrustSource::Owner,
        false,
    );
    wait_state(&d, t, State::Landed, 120);
    let logs = render_logs(&d, t.0);
    assert!(logs.contains("RAN-DEV -p pk"), "{logs}");
    assert!(logs.contains("Ran what your change affects"), "{logs}");
    assert!(
        !logs.contains("The test step runs the whole workspace"),
        "{logs}"
    );
    assert!(!logs.contains("pass full: true only if"), "{logs}");
    // The test step ran the whole suite.
    assert!(logs.contains("RAN-FULL"), "{logs}");
}

/// A change only to paths no test of a `{tests_for_change}` suite reads (docs, markdown, exit
/// scripts) runs none of it in `run_check`; the test step still runs it whole.
#[test]
fn a_docs_only_change_runs_no_cargo_style_suite_in_run_check() {
    let def = narrowable()
        .replace(
            r#"command = "echo RAN-FULL; sh checks.sh""#,
            r#"command = "echo RAN-FULL; printf '1..3\\nok 1\\nok 2\\nok 3\\n'""#,
        )
        .replace(
            r#"dev_command = "echo RAN-DEV {packages}; sh checks.sh""#,
            r#"dev_command = "echo RAN-DEV {tests_for_change}; printf '1..3\\nok 1\\nok 2\\nok 3\\n'""#,
        )
        .replace(r#""hello.txt" = ["only"]"#, r#""docs/**" = ["only"]"#);
    let turns = json!([
        {"sh": "mkdir -p docs && echo note > docs/note.md && git add -A && git -c user.name=t -c user.email=t@t commit -qm docs"},
        {"call": {"tool": "run_check", "args": {"name": "tests"}}},
        {"call": {"tool": "submit_change", "args": {"summary": "docs", "head": "$HEAD"}}}
    ]);
    let (_s, d) = start(vec![turns], &def);
    let t = file_task_as(&d, "Docs", "Please do it.", true, TrustSource::Owner, false);
    wait_state(&d, t, State::Landed, 120);
    let logs = render_logs(&d, t.0);
    assert!(
        logs.contains("only: not run, none of its tests reads what you changed"),
        "{logs}"
    );
    assert!(!logs.contains("RAN-DEV"), "{logs}");
    assert!(logs.contains("RAN-FULL"), "{logs}");
}

/// Only a check filtered to named tests takes the small lane. Narrowed by change or package it
/// can still be a whole integration binary, so it keeps the test step's own lane (2026-11-18).
#[test]
fn only_a_name_filtered_check_takes_the_small_lane() {
    let def = narrowable()
        .replace(
            r#"rerun = "echo RAN-RERUN {tests}; sh checks.sh""#,
            r#"rerun = "echo RAN-RERUN {tests} HELD=$MASON_HELD; sh checks.sh""#,
        )
        .replace(
            r#"dev_command = "echo RAN-DEV {packages}; sh checks.sh""#,
            r#"dev_command = "echo RAN-DEV {packages} HELD=$MASON_HELD; sh checks.sh""#,
        )
        .replace(
            "[step.test]",
            "[resource.test-small]\ncount = 1\n\n[step.test]",
        );
    let turns = json!([
        {"sh": "echo hello > hello.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm change"},
        {"call": {"tool": "run_check", "args": {"name": "tests", "tests": ["foo"]}}},
        {"call": {"tool": "run_check", "args": {"name": "tests", "packages": ["pk-one"]}}},
        {"call": {"tool": "submit_change", "args": {"summary": "change", "head": "$HEAD"}}}
    ]);
    let (_s, d) = start(vec![turns], &def);
    let t = file_task_as(
        &d,
        "Lanes",
        "Please do it.",
        true,
        TrustSource::Owner,
        false,
    );
    wait_state(&d, t, State::Landed, 120);
    let logs = render_logs(&d, t.0);
    let line = |tag: &str| {
        logs.lines()
            .find(|l| l.contains(tag))
            .unwrap_or_else(|| panic!("no {tag} line in {logs}"))
            .to_string()
    };
    let rerun = line("RAN-RERUN foo");
    let dev = line("RAN-DEV -p pk-one");
    assert!(
        rerun.contains("test-small") && !rerun.contains("test-suite"),
        "{rerun}"
    );
    assert!(
        dev.contains("test-suite") && !dev.contains("test-small"),
        "{dev}"
    );
}

/// #812: a fast-tracked task (`tests:all` debt) still has its change built and the tests it adds
/// run, by name; the rest of the suite does not run.
fn fast_track_definition(build: &str, rerun: &str) -> String {
    format!(
        r#"{HEAD}
[step.test]
type = "test"
category = "testing"
uses = {{ compile = 1, test-suite = 1 }}
suites = ["only"]
on_red = "develop"
max_rounds = 2
next = "land"

[suite.only]
build = "{build}"
command = "echo RAN-FULL"
rerun = "{rerun}"
parser = "tap"
min_tests = 1
level = "unit"

[binding.unit]
when = ["development", "test_step"]

[impact]
"src/**" = ["only"]
"#
    )
}

/// Run one fast-tracked task whose agent commits `src/a.rs` with `body`, asks for a check and
/// submits. Returns the task's logs once the check's answer is in them.
fn fast_track_check(build: &str, rerun: &str, body: &str) -> String {
    let gate = tempfile::tempdir().unwrap();
    let go = gate.path().join("go");
    let turns = json!([
        {"sh": format!("until [ -f {} ]; do sleep 0.2; done", go.display())},
        {"sh": format!("mkdir -p src && printf '{body}' > src/a.rs && git add -A && git -c user.name=t -c user.email=t@t commit -qm change")},
        {"call": {"tool": "run_check", "args": {"name": "tests"}}},
        {"call": {"tool": "submit_change", "args": {"summary": "change", "head": "$HEAD"}}}
    ]);
    let (_s, d) = start(vec![turns], &fast_track_definition(build, rerun));
    let t = file_task_as(&d, "Fast", "Do it.", true, TrustSource::Owner, false);
    d.store
        .add_debt(
            t.0,
            "scratch",
            mason_daemon::suites::ALL_TESTS,
            "owner skipped",
            "brian",
            false,
            0,
        )
        .unwrap();
    std::fs::write(&go, "go").unwrap();
    let mut logs = String::new();
    for _ in 0..600 {
        logs = render_logs(&d, t.0);
        if logs.contains("Not run: the owner skipped testing") {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    logs
}

#[test]
fn a_fast_tracked_check_is_red_naming_the_failing_test_the_change_adds() {
    let logs = fast_track_check(
        "true",
        "for n in {tests}; do echo \\\"test m::$n ... FAILED\\\"; done; exit 1",
        "#[test]\\nfn adds_one() {}\\n",
    );
    assert!(logs.contains("RED"), "{logs}");
    assert!(logs.contains("adds_one"), "{logs}");
    assert!(!logs.contains("RAN-FULL"), "{logs}");
}

#[test]
fn a_fast_tracked_check_is_red_when_the_change_does_not_build() {
    let logs = fast_track_check(
        "echo BUILD-BROKEN; exit 1",
        "echo RAN {tests}",
        "fn x() {}\\n",
    );
    assert!(logs.contains("RED"), "{logs}");
    assert!(logs.contains("BUILD-BROKEN"), "{logs}");
}

#[test]
fn a_fast_tracked_green_check_says_it_built_and_ran_only_the_new_test() {
    let logs = fast_track_check(
        "echo BUILD-RAN",
        "echo RAN-ONLY {tests}",
        "#[test]\\nfn adds_one() {}\\n",
    );
    assert!(logs.contains("build of only"), "{logs}");
    assert!(logs.contains("test adds_one"), "{logs}");
    // `sh -c` strips the quotes `shell_word` adds, so the journal has none.
    assert!(logs.contains("RAN-ONLY adds_one"), "{logs}");
    assert!(!logs.contains("RAN-FULL"), "{logs}");
    assert!(!logs.contains("RED"), "{logs}");
}
