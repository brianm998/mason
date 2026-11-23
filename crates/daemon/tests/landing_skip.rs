// SPDX-License-Identifier: GPL-3.0-or-later

//! 2026-11-15: the owner pressed Skip test and Land now on a task that sat in a running
//! landing batch for two hours, and nothing happened: the debts were recorded and never acted
//! on. A batch's verification now stops when every member has skipped (and, as for a task's own
//! test, lands with the debts), and when any member is forced.
//!
//! Real git, real suites, the real coordinator. A suite that blocks (`hang.alpha`, or
//! `hang.build.alpha` for the build floor) writes its pid to the path the file holds, which is
//! what the tests wait on; nothing here asserts a duration.

mod support;
use mason_core::*;
use mason_daemon::suites::{landing_scope, skip, Members};
use std::path::PathBuf;
use support::lw::*;
use support::*;

const BATCH_TIER: &str =
    "window = \"2s\"\nmax_size = 5\n[[tier]]\nname = \"batch\"\nsuites = [\"alpha\"]\n";

fn marker(w: &W, name: &str) -> PathBuf {
    w.s.root.path().join(name)
}

fn pid_of(m: &PathBuf) -> Option<String> {
    let t = std::fs::read_to_string(m).ok()?;
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn debt_scopes(w: &W, t: TaskId) -> Vec<String> {
    w.d.store
        .debts(Some(t.0), true)
        .unwrap()
        .into_iter()
        .map(|x| x.scope)
        .collect()
}

#[test]
fn skipping_the_landing_step_of_a_one_task_batch_stops_its_suite_and_it_lands_with_the_debt() {
    let w = world_built(BATCH_TIER, &[]);
    let m = marker(&w, "alpha.pid");
    let t = w.task("blocks", &[("a/hang.alpha", &m.to_string_lossy())]);
    wait_for("the batch's suite to start", 120, || pid_of(&m).is_some());
    let pid = pid_of(&m).unwrap();
    assert!(!w.landed(t));
    // What the clients send for a task in landing: its own step.
    skip(&w.d, t, "step:land", "too slow", "owner", false).unwrap();
    wait_for("it to land", 120, || w.landed(t));
    wait_for("the blocked suite to be gone", 30, || !alive(&pid));
    assert!(w.on_main("a/hang.alpha"));
    assert_eq!(debt_scopes(&w, t), vec!["step:test".to_string()]);
}

#[test]
fn a_batch_keeps_running_while_only_some_of_its_members_have_skipped() {
    let w = world_built(
        "window = \"4s\"\nmax_size = 5\n[[tier]]\nname = \"batch\"\nsuites = [\"alpha\"]\n",
        &[],
    );
    let m = marker(&w, "alpha.pid");
    let t1 = w.task("blocks", &[("a/hang.alpha", &m.to_string_lossy())]);
    let t2 = w.task("bystander", &[("b/ok.txt", "1")]);
    wait_for("the batch's suite to start", 120, || pid_of(&m).is_some());
    let pid = pid_of(&m).unwrap();
    let batch = w.d.store.batches(Some("scratch"), 10).unwrap();
    assert!(
        batch.iter().any(|b| {
            let ids: Vec<&str> = b.tasks.split(',').collect();
            ids.contains(&t1.0.to_string().as_str()) && ids.contains(&t2.0.to_string().as_str())
        }),
        "the two tasks must share a batch: {batch:?}"
    );
    let pid_def = w.d.current_def(w.d.project_id_by_name("scratch").unwrap());
    let members = Members::new(&[t1, t2], &pid_def);
    skip(&w.d, t1, "step:land", "slow", "owner", false).unwrap();
    // Decided from the debts alone, so nothing here waits for a poll: one member is not all.
    assert!(members.stop(&w.d, "alpha").is_none());
    assert!(alive(&pid), "the suite must keep running");
    assert!(!w.landed(t1) && !w.landed(t2));
    skip(&w.d, t2, "step:land", "slow", "owner", false).unwrap();
    assert!(members
        .stop(&w.d, "alpha")
        .unwrap()
        .contains("skipped by the owner"));
    wait_for("both to land", 120, || w.landed(t1) && w.landed(t2));
    wait_for("the blocked suite to be gone", 30, || !alive(&pid));
}

#[test]
fn forcing_a_task_in_a_running_batch_ends_its_verification_and_lands_it() {
    let w = world_built(BATCH_TIER, &[]);
    let m = marker(&w, "alpha.pid");
    let t = w.task("blocks", &[("a/hang.alpha", &m.to_string_lossy())]);
    wait_for("the batch's suite to start", 120, || pid_of(&m).is_some());
    let pid = pid_of(&m).unwrap();
    w.d.store
        .set_landing_flag(t.0, "force", "owner", mason_daemon::daemon::now_ms())
        .unwrap();
    w.d.landing().wake();
    wait_for("it to land", 120, || w.landed(t));
    wait_for("the blocked suite to be gone", 30, || !alive(&pid));
    assert!(w.on_main("a/hang.alpha"));
}

#[test]
fn a_skip_from_a_landing_row_is_recorded_against_the_test_step() {
    let w = world_built("window = \"60s\"\n", &[]);
    let t = w.task("one", &[("x/one.txt", "1")]);
    let def = w.d.current_def(w.d.project_id_by_name("scratch").unwrap());
    // In landing, the landing step is translated to the Test step.
    assert_eq!(
        landing_scope(&def, State::Landing, "step:land"),
        "step:test"
    );
    // Everything else is as sent.
    assert_eq!(
        landing_scope(&def, State::Landing, "step:test"),
        "step:test"
    );
    assert_eq!(
        landing_scope(&def, State::Landing, "suite:alpha"),
        "suite:alpha"
    );
    assert_eq!(
        landing_scope(&def, State::Testing, "step:land"),
        "step:land"
    );
    // And end to end.
    let debt = skip(&w.d, t, "step:land", "slow", "owner", false).unwrap();
    let row = w.d.store.debts(Some(t.0), true).unwrap();
    assert_eq!(
        row.iter().find(|x| x.id == debt).unwrap().scope,
        "step:test"
    );
}

#[test]
fn skipping_stops_the_build_floor_of_a_batch_with_no_batch_tier() {
    let w = world_built("window = \"2s\"\nmax_size = 5\n", &[]);
    let m = marker(&w, "build.pid");
    let t = w.task("blocks", &[("a/hang.build.alpha", &m.to_string_lossy())]);
    wait_for("the floor build to start", 120, || pid_of(&m).is_some());
    let pid = pid_of(&m).unwrap();
    assert!(!w.landed(t));
    skip(&w.d, t, "step:land", "too slow", "owner", false).unwrap();
    wait_for("it to land", 120, || w.landed(t));
    wait_for("the blocked build to be gone", 30, || !alive(&pid));
    assert!(w.on_main("a/hang.build.alpha"));
}

#[test]
fn a_skip_of_a_step_that_is_not_the_test_step_is_refused() {
    let w = world_built("window = \"60s\"\n", &[]);
    let t = w.task("one", &[("x/one.txt", "1")]);
    let def = w.d.current_def(w.d.project_id_by_name("scratch").unwrap());
    let non_test = def
        .steps
        .iter()
        .find(|s| s.kind != mason_definition::StepType::Test)
        .expect("the definition has a step that is not the test step")
        .name
        .clone();
    let before = debt_scopes(&w, t).len();
    let err = skip(&w.d, t, &format!("step:{non_test}"), "r", "owner", false).unwrap_err();
    assert!(err.contains("only the test step"), "{err}");
    assert_eq!(debt_scopes(&w, t).len(), before, "no debt is recorded");
    // The Test step still can be.
    skip(&w.d, t, "step:test", "r", "owner", false).unwrap();
}
