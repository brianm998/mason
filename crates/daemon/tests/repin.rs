// SPDX-License-Identifier: GPL-3.0-or-later

//! s211 (decision 0206): a task that goes back to an earlier step runs the project's
//! current definition, and a red in a suite that changed since the task was pinned is
//! rerun once on the current definition instead of sending the task back.

mod support;
use mason_core::*;
use mason_daemon::defs::Reload;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use support::*;

const HEAD: &str = r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.land]
type = "landing"
category = "landing"
"#;

fn commit(cmd: &str) -> Value {
    json!([
        {"sh": format!("{cmd} && git add -A && git -c user.name=t -c user.email=t@t commit -qm work")},
        {"call": {"tool": "submit_change", "args": {"summary": "work", "head": "$HEAD"}}}
    ])
}

fn start(turns: Vec<Value>, definition: &str) -> (Scratch, Arc<mason_daemon::Daemon>) {
    let s = Scratch::new("false", json!({"steps": {"develop": turns}}));
    s.push_files(&[(".mason/mason.toml", definition)], "project setup");
    // The explicit reload below is the one that applies the change.
    let d = s.start_with(|c| {
        c.reload_interval = Duration::from_secs(3600);
        c.fetch_interval = Duration::from_secs(3600);
    });
    (s, d)
}

fn exists(p: &Path) -> bool {
    p.exists()
}

/// The `Repinned` events of a task: (from, to, changed).
fn repins(d: &Arc<mason_daemon::Daemon>, t: TaskId) -> Vec<(String, String, String)> {
    d.store
        .read_since(0, 100_000)
        .unwrap()
        .into_iter()
        .filter_map(|e| match e.event.kind {
            EventKind::Repinned {
                task,
                from,
                to,
                changed,
            } if task == t => Some((from, to, changed)),
            _ => None,
        })
        .collect()
}

fn apply(s: &Scratch, d: &Arc<mason_daemon::Daemon>, text: &str) -> String {
    s.push_files(&[(".mason/mason.toml", text)], "new definition");
    let pid = d.project_id_by_name("scratch").unwrap();
    assert!(matches!(d.reload_definition(pid), Reload::Applied { .. }));
    d.current_def(pid).hash.clone()
}

fn command_def(g: &str, gated: bool) -> String {
    let wait = if gated {
        format!("touch {g}/started; while [ ! -f {g}/go ]; do sleep 0.1; done; ")
    } else {
        String::new()
    };
    format!(
        r#"{HEAD}
[step.test]
type = "command"
category = "testing"
command = "{wait}test -f hello.txt"
max_rounds = 3
on_red = "develop"
next = "land"
"#
    )
}

#[test]
fn a_task_sent_back_runs_the_definition_on_the_base_now_and_a_running_step_keeps_its_own() {
    let gates = tempfile::tempdir().unwrap();
    let g = gates.path().display().to_string();
    // The first attempt leaves hello.txt out, so the test step is red; the second adds it.
    let (s, d) = start(
        vec![commit("echo one > one.txt"), commit("echo hi > hello.txt")],
        &command_def(&g, true),
    );
    let old = d
        .current_def(d.project_id_by_name("scratch").unwrap())
        .hash
        .clone();
    let t = file_task_as(&d, "Sent back", "Do it.", true, TrustSource::Owner, false);
    wait_for("the test step to start", 90, || {
        exists(&gates.path().join("started"))
    });
    assert_eq!(task_of(&d, t).def, old);

    // The definition changes while the step runs: it ends on the version it started with.
    let new = apply(&s, &d, &command_def(&g, false));
    assert_ne!(old, new);
    assert_eq!(task_of(&d, t).def, old, "(b) a running step keeps its pin");
    assert!(repins(&d, t).is_empty());

    std::fs::write(gates.path().join("go"), "").unwrap();
    wait_state(&d, t, State::Landed, 120);
    assert_eq!(
        steps_run(&d, t),
        vec!["develop", "test", "develop", "test", "land"]
    );
    // (a) The send-back repinned it; the next develop and test steps ran under the new one
    // (the new test step has no wait, and the red one only ended because the gate was opened).
    assert_eq!(task_of(&d, t).def, new);
    let r = repins(&d, t);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!((r[0].0.as_str(), r[0].1.as_str()), (old.as_str(), new.as_str()));
    assert!(r[0].2.contains("steps: test"), "{}", r[0].2);
}

fn suite_def(g: &str, wait: bool, rounds: u32) -> String {
    let w = if wait {
        format!("touch {g}/started; while [ ! -f {g}/go ]; do sleep 0.1; done; ")
    } else {
        String::new()
    };
    format!(
        r#"{HEAD}
[step.test]
type = "test"
category = "testing"
suites = ["s"]
max_rounds = {rounds}
on_red = "develop"
next = "land"

[suite.s]
command = "{w}echo clean"
parser = "regex"
fail_regex = "NEVER-PRINTED"
level = "unit"
"#
    )
}

/// Suite `s` as star's build suites were: a clean run counts no tests, so it is red.
/// The fix gives it a pass marker.
fn fixed_suite_def(g: &str) -> String {
    suite_def(g, false, 3).replace(
        "fail_regex = \"NEVER-PRINTED\"",
        "pass_regex = \"^clean$\"",
    )
}

#[test]
fn a_red_in_a_suite_that_changed_is_rerun_once_on_the_current_definition() {
    let gates = tempfile::tempdir().unwrap();
    let g = gates.path().display().to_string();
    let (s, d) = start(vec![commit("echo hi > hello.txt")], &suite_def(&g, true, 3));
    let old = d
        .current_def(d.project_id_by_name("scratch").unwrap())
        .hash
        .clone();
    let t = file_task_as(&d, "Fixed on base", "Do it.", true, TrustSource::Owner, false);
    wait_for("the suite to start", 90, || {
        exists(&gates.path().join("started"))
    });
    let new = apply(&s, &d, &fixed_suite_def(&g));
    std::fs::write(gates.path().join("go"), "").unwrap();
    // (c) Red under the old definition, green on the rerun, and never sent back.
    wait_state(&d, t, State::Landed, 120);
    assert_eq!(steps_run(&d, t), vec!["develop", "test", "land"]);
    assert_eq!(task_of(&d, t).def, new);
    let r = repins(&d, t);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0].0, old);
    assert!(r[0].2.contains("suites: s"), "{}", r[0].2);
}

#[test]
fn a_red_in_a_suite_that_did_not_change_is_sent_back_as_before() {
    let gates = tempfile::tempdir().unwrap();
    let g = gates.path().display().to_string();
    let (s, d) = start(
        vec![commit("echo hi > hello.txt"), commit("echo two > two.txt")],
        &suite_def(&g, true, 2),
    );
    let t = file_task_as(&d, "Still red", "Do it.", true, TrustSource::Owner, false);
    wait_for("the suite to start", 90, || {
        exists(&gates.path().join("started"))
    });
    // Only the step changes (more rounds); suite `s` is as it was.
    let new = apply(&s, &d, &suite_def(&g, true, 3));
    std::fs::write(gates.path().join("go"), "").unwrap();
    // (d) Sent back, not rerun: a second develop step follows the first test step.
    wait_for("the task to be sent back", 120, || {
        steps_run(&d, t).len() >= 3
    });
    assert_eq!(steps_run(&d, t)[..3], ["develop", "test", "develop"]);
    // The send-back repinned it, as any send-back does.
    assert_eq!(task_of(&d, t).def, new);
    assert_eq!(repins(&d, t).len(), 1);
}

#[test]
fn only_a_project_without_a_self_section_is_told_its_mason_changes_do_nothing() {
    assert!(mason_daemon::jobs::touches_pipeline_definition(
        "src/lib.rs\n.mason/mason.toml\n"
    ));
    assert!(!mason_daemon::jobs::touches_pipeline_definition(
        "src/lib.rs\ndocs/.mason/x\n.masonry/y\n"
    ));
    assert!(mason_daemon::jobs::PIPELINE_DEFINITION_NOTE.contains("task of their own"));
}

#[test]
fn the_suite_a_red_names_is_read_from_its_reason() {
    use mason_daemon::jobs::red_suite_of;
    assert_eq!(red_suite_of("suite `build-x` is red").as_deref(), Some("build-x"));
    assert_eq!(
        red_suite_of("the build of suite `b` is red").as_deref(),
        Some("b")
    );
    assert_eq!(red_suite_of("a gate failed"), None);
    assert_eq!(red_suite_of("the reviewer asked for changes"), None);
}
