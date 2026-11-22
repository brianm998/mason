// SPDX-License-Identifier: GPL-3.0-or-later

//! #783: a `kind = "suites"` schedule runs every listed suite whole on the base tip, as work
//! below every task, here or on GitHub; it records greens (which settle debts), files reds with
//! the range since the last green full test, and runs nothing on a tip that is fully tested.
//! The clock is a parameter and nothing here waits for time to pass.

use crate::support;
use mason_core::full_test as full;
use mason_core::*;
use mason_daemon::ci_schedule::{suites_as, Go, Repo};
use mason_daemon::schedule::CiSpec;
use mason_daemon::Daemon;
use serde_json::json;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::*;

/// A TAP suite that logs each run. It is red while `legacy.broken` exists.
fn checks(log: &Path) -> String {
    format!(
        "#!/bin/sh\necho run >> {}\necho 1..3\necho 'ok 1 - alpha'\nif [ -f hello.txt ]; then echo 'ok 2 - hello'; else echo 'not ok 2 - hello'; fi\nif [ -f legacy.broken ]; then echo 'not ok 3 - legacy'; else echo 'ok 3 - legacy'; fi\n[ -f legacy.broken ] && exit 1\n[ -f hello.txt ] || exit 1\nexit 0\n",
        log.display()
    )
}

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
suites = ["checks"]
uses = { compile = 1 }
on_red = "develop"
max_rounds = 4
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.checks]
command = "sh checks.sh"
parser = "tap"
min_tests = 3
level = "unit"
"#;

const HELLO: &str = "echo hello > hello.txt && echo x > u-$(basename \"$PWD\").txt";

fn commit(cmd: &str, msg: &str) -> serde_json::Value {
    json!([
        {"sh": format!("{cmd} && git add -A && git -c user.name=t -c user.email=t@t commit -qm '{msg}'")},
        {"call": {"tool": "submit_change", "args": {"summary": msg, "head": "$HEAD"}}}
    ])
}

/// A project whose first task landed `hello.txt`; the suite's runs are counted in `log`.
fn world(extra: &str, log: &Path) -> (Scratch, Arc<Daemon>, TaskId) {
    let s = Scratch::new(
        "true",
        json!({"steps": {"develop": [commit(HELLO, "add hello")]}}),
    );
    s.write_project("scratch", "true", extra);
    s.push_files(
        &[(".mason/mason.toml", DEF), ("checks.sh", &checks(log))],
        "setup",
    );
    let d = s.start_with(|c| {
        c.env_retry = Duration::from_millis(200);
        // The hourly debt pass is off: the test pays by running the schedule.
        c.debt_interval = Duration::ZERO;
    });
    let t = file_task_as(
        &d,
        "Add hello",
        "Please do it.",
        true,
        TrustSource::Owner,
        false,
    );
    wait_state(&d, t, State::Landed, 120);
    (s, d, t)
}

fn runs_of(log: &Path) -> usize {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .count()
}

/// The repository as the schedule reads it: the real one, fetched from its origin.
struct Real {
    d: Arc<Daemon>,
    pid: ProjectId,
}

impl Repo for Real {
    fn fetch(&self) -> Result<(), String> {
        self.d.fetch_remote(self.pid)
    }
    fn tip(&self) -> Result<String, String> {
        let p = self.d.project(self.pid).ok_or("no project")?.clone();
        mason_landing::git::run(&p.path, &["rev-parse", &p.base_ref()])
            .map(|s| s.trim().to_string())
            .map_err(|e| e.to_string())
    }
    fn file_at_tip(&self, _path: &str) -> Result<String, String> {
        Err("no such file".into())
    }
    fn changed_since(&self, sha: &str) -> Option<Vec<String>> {
        let p = self.d.project(self.pid)?.clone();
        let out =
            mason_landing::git::run(&p.path, &["diff", "--name-only", sha, &p.base_ref()]).ok()?;
        Some(out.lines().map(String::from).collect())
    }
}

fn spec(suites: &[&str]) -> CiSpec {
    CiSpec {
        suites: suites.iter().map(|s| s.to_string()).collect(),
        when_quiet: Some(30 * 60_000),
        ..Default::default()
    }
}

fn run(d: &Arc<Daemon>, ci: &CiSpec, trigger: &str, force: bool) -> String {
    let pid = d.project_id_by_name("scratch").unwrap();
    let repo = Real { d: d.clone(), pid };
    let go = Go {
        schedule: "full",
        trigger,
        force,
    };
    suites_as(d, pid, ci, &repo, 1_000, &go)
}

fn tip(d: &Arc<Daemon>) -> String {
    let pid = d.project_id_by_name("scratch").unwrap();
    Real { d: d.clone(), pid }.tip().unwrap()
}

#[test]
fn a_due_schedule_with_remote_off_runs_each_listed_suite_once_on_the_tip_and_a_green_settles_a_debt(
) {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("ran");
    let (_s, d, t) = world("", &log);
    // The task's tests were skipped: a debt, with no green on record at the tip.
    d.store
        .add_debt(
            t.0,
            "scratch",
            "suite:checks",
            "owner skipped",
            "brian",
            false,
            0,
        )
        .unwrap();
    // The landing records the commit a debt's task landed as; this debt comes after the
    // landing, whose task branch is already deleted, so record it here (the tip holds it).
    let debt = d.store.debts(Some(t.0), true).unwrap()[0].id;
    d.store.set_debt_landed(debt, &tip(&d)).unwrap();
    d.store
        .write(|c| {
            c.execute("DELETE FROM suite_green", [])?;
            // Skipped tests leave no evidence for the tree either; the world's own test step
            // did, and the schedule would reuse it instead of running the suite.
            c.execute("DELETE FROM evidence", [])?;
            c.execute("DELETE FROM suite_skip_green", []).map(|_| ())
        })
        .unwrap();
    let before = runs_of(&log);
    let said = run(&d, &spec(&["checks"]), full::NIGHTLY, false);
    assert!(said.contains(": green"), "{said}");
    assert_eq!(runs_of(&log) - before, 1, "the suite ran once: {said}");
    // The green is recorded at the tip and the debt it covers is closed.
    let tip = tip(&d);
    let greens = d.store.suite_green_bases("scratch", "checks").unwrap();
    assert!(greens.iter().any(|(b, _)| *b == tip), "{greens:?}");
    let debts = d.store.debts(Some(t.0), false).unwrap();
    assert_eq!(debts[0].status, "paid", "{}", debts[0].how);
    // The verdict is in the record, with the commit.
    let rows = d.store.full_tests("scratch", "full").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].commit.as_str(), rows[0].verdict.as_str()),
        (tip.as_str(), full::GREEN)
    );
    assert!(rows[0].finished_ms > 0);
}

#[test]
fn a_quiet_trigger_on_a_fully_tested_tip_runs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("ran");
    let (_s, d, _t) = world("", &log);
    let first = run(&d, &spec(&["checks"]), full::NIGHTLY, true);
    assert!(first.contains(": green"), "{first}");
    let after_first = runs_of(&log);
    let said = run(&d, &spec(&["checks"]), full::QUIET, false);
    assert!(said.starts_with("not run:"), "{said}");
    assert!(said.contains("fully tested"), "{said}");
    assert_eq!(runs_of(&log), after_first, "nothing ran");
    assert_eq!(d.store.full_tests("scratch", "full").unwrap().len(), 1);
}

#[test]
fn a_red_suite_is_recorded_red_and_files_a_ticket_naming_the_range_since_the_last_green() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("ran");
    let (s, d, _t) = world("", &log);
    let pid = d.project_id_by_name("scratch").unwrap();
    run(&d, &spec(&["checks"]), full::NIGHTLY, true);
    let green_at = tip(&d);
    // A later landing breaks a test the suite runs.
    s.push_files(&[("legacy.broken", "x\n")], "break legacy");
    d.fetch_remote(pid).unwrap();
    let said = run(&d, &spec(&["checks"]), full::NIGHTLY, false);
    assert!(said.contains(": red"), "{said}");
    let rows = d.store.full_tests("scratch", "full").unwrap();
    assert_eq!(rows[0].verdict, full::RED);
    // No green is recorded for the broken tip.
    let red_at = tip(&d);
    let greens = d.store.suite_green_bases("scratch", "checks").unwrap();
    assert!(!greens.iter().any(|(b, _)| *b == red_at), "{greens:?}");
    // A ticket names the suite, the failing test and the range.
    let w = d.world().unwrap();
    let ticket = w
        .tasks
        .values()
        .find(|t| t.title.contains("suite `checks` red"))
        .expect("a ticket for the red suite");
    assert!(ticket.body.contains("legacy"), "{}", ticket.body);
    assert!(
        ticket.body.contains(&format!("{green_at}..{red_at}")),
        "{}",
        ticket.body
    );
    // A second run on the same tip files nothing new: the tip was fully tested (red).
    let again = run(&d, &spec(&["checks"]), full::QUIET, false);
    assert!(again.starts_with("not run:"), "{again}");
}

#[test]
fn an_unknown_suite_name_is_reported_and_the_record_is_not_left_running() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("ran");
    let (_s, d, _t) = world("", &log);
    let before = runs_of(&log);
    let said = run(&d, &spec(&["no-such-suite"]), full::MANUAL, false);
    assert!(said.contains("no suite `no-such-suite`"), "{said}");
    let rows = d.store.full_tests("scratch", "full").unwrap();
    assert!(rows.iter().all(|r| r.finished_ms > 0), "{rows:?}");
    assert_eq!(runs_of(&log), before, "no suite ran");
}

#[test]
fn the_default_runs_the_test_steps_suites_and_the_run_waits_behind_every_task() {
    use mason_scheduler::{Needs, Rank};
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("ran");
    let (_s, d, _t) = world("", &log);
    // The world's test step left green evidence for the tip's tree, which the schedule would
    // reuse without running anything: this test is about a run that does run.
    d.store
        .write(|c| c.execute("DELETE FROM evidence", []).map(|_| ()))
        .unwrap();
    d.broker.declare("compile", 1);
    let before = runs_of(&log);
    // Something holds the only compile slot.
    let held = d
        .broker
        .acquire(
            TaskId(900),
            "holder",
            Rank::new(false, 1, 2, 1, 900, false),
            Needs::of(&[("compile", 1)]),
            &|| true,
            &mut |_| {},
        )
        .unwrap();
    // The schedule asks first (no suites named: the Test step's suites).
    let d2 = d.clone();
    let order: Arc<Mutex<Vec<&'static str>>> = Arc::default();
    let o1 = order.clone();
    let sched = std::thread::spawn(move || {
        let said = run(&d2, &spec(&[]), full::MANUAL, true);
        o1.lock().unwrap().push("schedule");
        said
    });
    wait_for("the schedule to queue for the slot", 60, || {
        !d.broker.compilation().pending.is_empty()
    });
    // A task asks after it and has a better standing, however late it comes.
    let (release_task, wait_release) = std::sync::mpsc::channel::<()>();
    let (d3, o2) = (d.clone(), order.clone());
    let task = std::thread::spawn(move || {
        let lease = d3
            .broker
            .acquire(
                TaskId(901),
                "a task",
                Rank::new(false, 3, 2, 2, 901, false),
                Needs::of(&[("compile", 1)]),
                &|| true,
                &mut |_| {},
            )
            .unwrap();
        o2.lock().unwrap().push("task");
        let _ = wait_release.recv();
        drop(lease);
    });
    wait_for("both to be waiting", 60, || {
        d.broker.compilation().pending.len() >= 2
    });
    drop(held);
    wait_for("the task to get the slot", 60, || {
        order.lock().unwrap().contains(&"task")
    });
    assert_eq!(
        runs_of(&log),
        before,
        "the schedule ran before the task had its turn"
    );
    assert!(!order.lock().unwrap().contains(&"schedule"));
    release_task.send(()).unwrap();
    task.join().unwrap();
    let said = sched.join().unwrap();
    assert_eq!(order.lock().unwrap().as_slice(), ["task", "schedule"]);
    assert_eq!(
        runs_of(&log) - before,
        1,
        "the Test step's suite ran once: {said}"
    );
}

#[test]
fn with_remote_on_the_run_goes_to_github_and_holds_no_local_compile_slot() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("ran");
    let (url, requests) = crate::landing::silent_github();
    std::env::set_var("MASON_T783_TOKEN", "x");
    let extra = format!(
        "[remote_runs]\nkind = \"github-actions\"\napi = \"{url}\"\nrepo = \"o/r\"\ntoken = \"env:MASON_T783_TOKEN\"\nruns_on = \"macos-14\"\n"
    );
    let (_s, d, _t) = world(&extra, &log);
    mason_daemon::place::set_global(&d, mason_daemon::place::Mode::Remote, "test").unwrap();
    let before = runs_of(&log);
    let d2 = d.clone();
    // The fake GitHub never assigns a runner, so the run stays queued there.
    std::thread::spawn(move || run(&d2, &spec(&["checks"]), full::MANUAL, true));
    wait_for("the run to reach GitHub", 90, || {
        requests.load(std::sync::atomic::Ordering::SeqCst) > 0
    });
    assert_eq!(
        d.broker.used("compile"),
        0,
        "a queued remote run holds a compile slot here"
    );
    assert_eq!(runs_of(&log), before, "the suite ran on this Mac");
}
