// SPDX-License-Identifier: GPL-3.0-or-later

//! s246: a red the base has too is not the task's. The task waits in its test step (wait class
//! `base-red`), one urgent task is filed for the base, and when the base turns green the new base
//! is folded in and the step runs again. A red the base does not have still sends the task back.

mod support;
use mason_core::*;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use support::*;

const GOOD_CHECKS: &str = "#!/bin/sh\necho \"1..4\"\necho \"ok 1 - a\"\necho \"ok 2 - b\"\necho \"ok 3 - c\"\necho \"ok 4 - d\"\n";
/// What proves nothing: one test where three are needed.
const THIN_CHECKS: &str = "#!/bin/sh\necho \"1..1\"\necho \"ok 1 - a\"\n";

const DEFINITION: &str = r#"schema = 1
start = "develop"

[resource.compile]
count = 1

[resource.test-suite]
count = 1

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
uses = { compile = 1, test-suite = 1 }
suites = ["checks"]
on_red = "develop"
max_rounds = 3
next = "land"

[suite.checks]
build = "grep -q fixed broken.txt || { echo \"compiling\"; echo \"error[E0001]: the base does not build at $PWD/src.rs:1:2\"; exit 1; }"
command = "sh checks.sh"
parser = "tap"
min_tests = 3
level = "unit"

[step.land]
type = "landing"
category = "landing"
"#;

fn commit(cmd: &str, msg: &str) -> Value {
    json!([
        {"sh": format!("{cmd} && git add -A && git -c user.name=t -c user.email=t@t commit -qm '{msg}'")},
        {"call": {"tool": "submit_change", "args": {"summary": msg, "head": "$HEAD"}}}
    ])
}

fn start(change: &str, base_files: &[(&str, &str)]) -> (Scratch, Arc<mason_daemon::Daemon>) {
    let s = Scratch::new(
        "false",
        json!({"steps": {"develop": [commit(change, "change")]}}),
    );
    let mut files: Vec<(&str, &str)> = vec![(".mason/mason.toml", DEFINITION)];
    files.extend_from_slice(base_files);
    s.push_files(&files, "project setup");
    let d = s.start_with(|c| {
        c.env_retry = Duration::from_millis(200);
        c.base_red_poll = Duration::from_millis(200);
    });
    (s, d)
}

fn waits(d: &Arc<mason_daemon::Daemon>, class: &str) -> Vec<String> {
    d.store
        .read_since(0, 100_000)
        .unwrap()
        .iter()
        .filter_map(|e| match &e.event.kind {
            EventKind::WaitStarted {
                on, class: Some(c), ..
            } if c == class => Some(on.clone()),
            _ => None,
        })
        .collect()
}

fn filed(d: &Arc<mason_daemon::Daemon>) -> Vec<Task> {
    d.world()
        .unwrap()
        .tasks
        .values()
        .filter(|t| t.title.contains("` is red on "))
        .cloned()
        .collect()
}

fn develop_runs(d: &Arc<mason_daemon::Daemon>, t: TaskId) -> usize {
    steps_run(d, t).iter().filter(|s| *s == "develop").count()
}

#[test]
fn a_vacuous_run_the_base_also_has_waits_and_runs_again_once_the_base_is_green() {
    let (s, d) = start(
        "echo hi > hello.txt && echo x > u-$(basename \"$PWD\").txt",
        &[("broken.txt", "fixed"), ("checks.sh", THIN_CHECKS)],
    );
    let t = file_task_as(
        &d,
        "Vacuous",
        "Please do it.",
        true,
        TrustSource::Owner,
        false,
    );
    wait_for("the base-red wait", 120, || {
        !waits(&d, "base-red").is_empty()
    });
    let on = waits(&d, "base-red").remove(0);
    assert!(
        on.contains("`checks` is red on ") && on.contains(" too: "),
        "{on}"
    );
    // One task for the base, urgent; the waiting task was neither sent back nor failed.
    let f = filed(&d);
    assert_eq!(
        f.len(),
        1,
        "{:?}",
        f.iter().map(|t| &t.title).collect::<Vec<_>>()
    );
    assert!(f[0].title.ends_with("it runs no tests"), "{}", f[0].title);
    assert_eq!(f[0].priority, Priority::Urgent);
    assert_eq!(state_of(&d, t), State::Testing);
    assert_eq!(develop_runs(&d, t), 1, "no send-back, no round spent");
    // The base is mended: the new base is folded in and the step runs again.
    s.push_files(&[("checks.sh", GOOD_CHECKS)], "mend the base");
    wait_state(&d, t, State::Landed, 120);
    assert_eq!(develop_runs(&d, t), 1, "still no send-back");
    assert_eq!(filed(&d).len(), 1, "one task, not one per look");
    let note = mason_daemon::basecheck::recorded(&d.task_dir(t)).join("\n");
    assert!(
        note.contains("was red on") && note.contains("is green"),
        "{note}"
    );
    assert!(
        d.store.base_red(t.0).unwrap().is_none(),
        "the wait is cleared"
    );
}

#[test]
fn a_build_the_base_also_fails_the_same_way_waits_and_a_second_task_files_nothing_more() {
    let (s, d) = start(
        "echo hi > hello.txt && echo x > u-$(basename \"$PWD\").txt",
        &[("broken.txt", "broken"), ("checks.sh", GOOD_CHECKS)],
    );
    let a = file_task_as(
        &d,
        "First",
        "Please do it.",
        true,
        TrustSource::Owner,
        false,
    );
    let b = file_task_as(
        &d,
        "Second",
        "Please do it.",
        true,
        TrustSource::Owner,
        false,
    );
    wait_for("both to wait", 180, || waits(&d, "base-red").len() >= 2);
    let f = filed(&d);
    assert_eq!(
        f.len(),
        1,
        "one task per normalised first error: {:?}",
        f.iter().map(|t| &t.title).collect::<Vec<_>>()
    );
    assert!(
        f[0].title.contains("error[E0001]: the base does not build"),
        "{}",
        f[0].title
    );
    assert_eq!(develop_runs(&d, a), 1);
    assert_eq!(develop_runs(&d, b), 1);
    s.push_files(&[("broken.txt", "fixed")], "mend the base");
    wait_state(&d, a, State::Landed, 180);
    wait_state(&d, b, State::Landed, 180);
}

#[test]
fn a_build_red_only_on_the_branch_is_still_sent_back() {
    // The base builds; the change breaks it.
    let (_s, d) = start(
        "echo broken > broken.txt && echo x > u-$(basename \"$PWD\").txt",
        &[("broken.txt", "fixed"), ("checks.sh", GOOD_CHECKS)],
    );
    let t = file_task_as(
        &d,
        "Breaks",
        "Please do it.",
        true,
        TrustSource::Owner,
        false,
    );
    wait_for("the send-back to develop", 180, || develop_runs(&d, t) >= 2);
    assert!(waits(&d, "base-red").is_empty());
    assert!(filed(&d).is_empty());
}
