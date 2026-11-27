// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg(unix)]
// Unix-only: the remote's pre-receive hook is a sh script and the file mode is set with PermissionsExt.

//! #1185: a short network outage fails no landing batch and costs no test round. The remote
//! is a real bare repository whose pre-receive hook says what ssh says without DNS.

use crate::support;
use mason_core::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use support::lw::*;
use support::*;

const DNS: &str =
    "ssh: Could not resolve hostname github.com: nodename nor servname provided, or not known";

fn quick(c: &mut mason_daemon::config::DaemonConfig) {
    c.network_backoff = vec![Duration::from_millis(20); 3];
    c.network_rerun = Duration::from_millis(20);
    c.network_probe = Duration::from_millis(200);
    c.network_suite_hold = Duration::from_millis(200);
}

/// The remote refuses pushes, as an outage would, while `gate/down` exists, and for the next
/// `fails` pushes. Every push it hears of is a line of `gate/pushes`.
fn outage_hook(origin: &Path, gate: &Path, fails: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(gate.join("fails"), fails.to_string()).unwrap();
    let hook = origin.join("hooks/pre-receive");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    let script = format!(
        "#!/bin/sh\ng='{g}'\necho push >> \"$g/pushes\"\nn=$(cat \"$g/fails\")\n\
         if [ -f \"$g/down\" ] || [ \"$n\" -gt 0 ]; then\n  echo $((n-1)) > \"$g/fails\"\n  \
         echo '{DNS}' >&2\n  exit 1\nfi\nexit 0\n",
        g = gate.display()
    );
    std::fs::write(&hook, script).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn pushes(gate: &Path) -> usize {
    std::fs::read_to_string(gate.join("pushes")).map_or(0, |t| t.lines().count())
}

fn notes(w: &W) -> Vec<String> {
    w.d.store
        .batches(Some("scratch"), 50)
        .unwrap()
        .into_iter()
        .map(|b| b.note)
        .collect()
}

fn none_failed(w: &W, tasks: &[TaskId]) {
    for t in tasks {
        assert_ne!(state_of(&w.d, *t), State::Failed, "{:?}", notes(w));
        assert!(w.sent_back(*t).is_none(), "{:?}", notes(w));
    }
}

const DIRECT: &str = "window = \"2s\"\nmax_size = 5\n";

#[test]
fn a_push_that_meets_no_network_twice_is_tried_again_and_the_batch_lands() {
    let gate = tempfile::tempdir().unwrap();
    let w = world_cfg(DIRECT, &|_| String::new(), &quick);
    outage_hook(&w.s.origin, gate.path(), 2);
    let a = w.task("one", &[("x/one.txt", "1")]);
    let b = w.task("two", &[("x/two.txt", "2")]);
    wait_for("both to land", 120, || w.landed(a) && w.landed(b));
    assert!(w.on_main("x/one.txt") && w.on_main("x/two.txt"));
    assert!(pushes(gate.path()) >= 3, "{}", pushes(gate.path()));
    assert!(
        !notes(&w)
            .iter()
            .any(|n| n.contains("waiting for the network")),
        "{:?}",
        notes(&w)
    );
}

#[test]
fn a_batch_whose_every_push_fails_is_held_and_lands_once_the_remote_answers() {
    let gate = tempfile::tempdir().unwrap();
    let w = world_cfg(DIRECT, &|_| String::new(), &quick);
    outage_hook(&w.s.origin, gate.path(), 0);
    std::fs::write(gate.path().join("down"), "").unwrap();
    let a = w.task("one", &[("x/one.txt", "1")]);
    let b = w.task("two", &[("x/two.txt", "2")]);
    wait_for("a batch to be held", 120, || {
        w.d.store
            .batches(Some("scratch"), 50)
            .unwrap()
            .iter()
            .any(|b| b.finished_at.is_some() && b.note.contains("waiting for the network"))
    });
    // The first try and one after each wait.
    assert!(pushes(gate.path()) >= 4, "{}", pushes(gate.path()));
    none_failed(&w, &[a, b]);
    // One entry for the whole host.
    let alerts = mason_daemon::attention::alerts_now(&w.d);
    let outages: Vec<_> = alerts
        .iter()
        .filter(|(k, _, _)| k == "network-outage")
        .collect();
    assert_eq!(outages.len(), 1, "{alerts:?}");
    std::fs::remove_file(gate.path().join("down")).unwrap();
    wait_for("both to land", 120, || w.landed(a) && w.landed(b));
    none_failed(&w, &[a, b]);
}

#[test]
fn a_ci_candidate_push_that_meets_no_network_is_tried_again() {
    let gate = tempfile::tempdir().unwrap();
    let log = std::sync::Mutex::new(PathBuf::new());
    let w = world_cfg(
        "mode = \"direct-ci\"\nwindow = \"2s\"\nmax_size = 5\n",
        &|s| {
            let (section, l) = ci_section(s);
            *log.lock().unwrap() = l;
            section
        },
        &quick,
    );
    outage_hook(&w.s.origin, gate.path(), 2);
    let a = w.task("one", &[("x/one.txt", "1")]);
    wait_for("it to land", 120, || w.landed(a));
    none_failed(&w, &[a]);
    let triggers = std::fs::read_to_string(log.lock().unwrap().clone()).unwrap();
    assert_eq!(triggers.lines().count(), 1, "{triggers}");
}

// ---- the test step --------------------------------------------------------------------------

const DEF: &str = r#"schema = 1
start = "develop"

[resource.compile]
count = 1

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
suites = ["fetch"]
on_red = "develop"
max_rounds = 3
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.fetch]
command = "sh fetch.sh"
parser = "tap"
min_tests = 1
level = "unit"
"#;

/// A world with one task about to run a suite whose script is `body`; `$RUNS` in it is the
/// file that gets a line per run.
fn step_world(body: &str) -> (Scratch, Arc<mason_daemon::Daemon>, TaskId, PathBuf) {
    let commit = serde_json::json!([
        {"sh": "echo x > u.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm u"},
        {"call": {"tool": "submit_change", "args": {"summary": "u", "head": "$HEAD"}}}
    ]);
    let s = Scratch::new("false", serde_json::json!({"steps": {"develop": [commit]}}));
    let runs = s.root.path().join("runs");
    let script = format!(
        "#!/bin/sh\necho run >> '{}'\n{}",
        runs.display(),
        body.replace("$RUNS", &format!("'{}'", runs.display()))
    );
    s.push_files(
        &[(".mason/mason.toml", DEF), ("fetch.sh", &script)],
        "project setup",
    );
    let d = s.start_with(|c| {
        quick(c);
        c.env_retry = Duration::from_millis(200);
        c.auto_retry_delays = vec![];
    });
    let t = file_task_as(
        &d,
        "Fetch",
        "Please do it.",
        true,
        TrustSource::Owner,
        false,
    );
    (s, d, t, runs)
}

fn count(runs: &Path) -> usize {
    std::fs::read_to_string(runs).map_or(0, |t| t.lines().count())
}

#[test]
fn a_suite_red_only_for_want_of_the_network_is_run_again_and_then_fails_outside_the_code() {
    let (_s, d, t, runs) =
        step_world("echo 'fatal: Could not resolve host: example.invalid'\nexit 1\n");
    wait_state(&d, t, State::Failed, 120);
    let task = task_of(&d, t);
    assert_eq!(task.failure_class, Some(FailClass::Outside), "{task:?}");
    let why = task.failure.clone().unwrap_or_default();
    assert!(why.contains("could not reach the network"), "{why}");
    assert_eq!(count(&runs), 3, "the run and two more");
    // Not a red round: it never went back to development.
    assert_eq!(steps_run(&d, t), vec!["develop", "test"]);
}

#[test]
fn a_suite_that_reaches_the_network_on_its_second_run_is_green() {
    let (_s, d, t, runs) = step_world(
        "if [ \"$(wc -l < $RUNS)\" -lt 2 ]; then echo 'Could not resolve host: example.invalid'; exit 1; fi\n\
         echo '1..1'\necho 'ok 1 - fetched'\n",
    );
    wait_state(&d, t, State::Landed, 120);
    assert_eq!(count(&runs), 2);
    assert_eq!(steps_run(&d, t), vec!["develop", "test", "land"]);
}

#[test]
fn a_failing_test_beside_a_network_message_is_still_a_red_round() {
    // The test fails only with the task's file in the tree: it passes at the base.
    let (_s, d, t, _runs) = step_world(
        "echo '1..2'\necho 'ok 1 - a'\nif [ ! -f u.txt ]; then echo 'ok 2 - mine'; exit 0; fi\n\
         echo 'not ok 2 - mine'\necho 'Could not resolve host: example.invalid'\nexit 1\n",
    );
    wait_for("the red to go back to development", 120, || {
        d.store
            .read_since(0, 100_000)
            .unwrap()
            .iter()
            .any(|e| matches!(&e.event.kind, EventKind::SentBack { task, .. } if *task == t))
    });
}

// ---- the agent step (#1257) -----------------------------------------------------------------

fn unreachable_turn() -> serde_json::Value {
    serde_json::json!([{"replay": format!(
        "{}/../adapters/fixtures/claude/synthetic_api_unreachable.jsonl",
        env!("CARGO_MANIFEST_DIR")
    )}])
}

fn agent_quick(c: &mut mason_daemon::config::DaemonConfig) {
    c.network_backoff = vec![Duration::from_millis(10); 2];
    c.network_agent_hold = Duration::from_millis(100);
    c.burst_min = 2;
}

#[test]
fn an_agent_that_cannot_reach_its_api_twice_waits_resumes_and_lands_with_one_alert() {
    let script = serde_json::json!({"steps": {
        "explore": [explore_turn()],
        "develop": [unreachable_turn(), unreachable_turn(), develop_turn()]}});
    let s = Scratch::new("test -f hello.txt", script);
    let d = s.start_with(agent_quick);
    let t = file_task(&d, "Meets an outage");
    wait_for("the task to land", 120, || {
        let w = d.world().unwrap();
        assert_ne!(
            w.tasks[&t].state,
            State::Failed,
            "{:?}",
            w.tasks[&t].failure
        );
        w.tasks[&t].state == State::Landed
    });
    let w = d.world().unwrap();
    assert_eq!(w.tasks[&t].failures, 0);
    assert!(w.tasks[&t].failure.is_none());
    let alerts = mason_daemon::attention::alerts_now(&d);
    let outages = alerts.iter().filter(|(k, _, _)| k == "network-outage");
    assert_eq!(outages.count(), 1, "{alerts:?}");
}

#[test]
fn an_agent_that_never_reaches_its_api_fails_as_outside_past_the_bound() {
    let many: Vec<_> = (0..40).map(|_| unreachable_turn()).collect();
    let script = serde_json::json!({"steps": {"explore": [explore_turn()], "develop": many}});
    let s = Scratch::new("test -f hello.txt", script);
    let d = s.start_with(agent_quick);
    let t = file_task(&d, "Never reaches");
    wait_state(&d, t, State::Failed, 120);
    let task = task_of(&d, t);
    let why = task.failure.clone().unwrap_or_default();
    assert!(why.contains(AGENT_UNREACHABLE), "{why}");
    assert_eq!(
        FailClass::classify(State::Developing, &why),
        FailClass::Outside
    );
}
