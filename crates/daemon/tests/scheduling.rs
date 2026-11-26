// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 8 end to end: one ordering rule for every contended resource, across
//! projects. Steps wait for what they use, every wait says why, interactive
//! callers go first, a dead holder frees its resource, and waiting is outside
//! every timeout.

mod support;
use mason_core::*;
use mason_daemon::sched::LeaseState;
use mason_daemon::Daemon;
use mason_resources::{Held, Locks, Take};
use std::sync::Arc;
use std::time::Duration;
use support::*;

/// Agent develops, then a test step that needs the one `compile` slot.
const DEF_COMPILE: &str = r#"schema = 1
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
uses = { compile = 1 }
max_rounds = 2
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

const DEF_LONG: &str = r#"schema = 1
start = "explore"

[step.explore]
type = "agent"
category = "exploring"
next = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

const DEF_SHORT: &str = r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

fn compile_scratch() -> Scratch {
    let s = Scratch::new("test -f hello.txt", unique_script());
    s.push_files(&[(".mason/mason.toml", DEF_COMPILE)], "definition");
    s
}

/// What `mason run --needs` does, in process: queue, wait for the grant, then
/// take the kernel locks ourselves.
fn hold(d: &Arc<Daemon>, label: &str, uses: &[(&str, u32)]) -> (u64, Held) {
    let map = uses.iter().map(|(k, v)| (k.to_string(), *v)).collect();
    let id = d.broker.lease_open(label, &map);
    take_when_granted(d, id, label)
}

fn take_when_granted(d: &Arc<Daemon>, id: u64, label: &str) -> (u64, Held) {
    let end = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        match d.broker.lease_poll(id) {
            LeaseState::Granted {
                classes,
                exclusive,
                capacity,
                locks_dir,
            } => {
                // Like the real client: a slot just released may still be busy for a
                // moment, so retry rather than fail.
                let locks = Locks::new(locks_dir).unwrap();
                let want = Take { classes, exclusive };
                let held = loop {
                    match locks.take(id, label, 0, &want, &capacity) {
                        Ok(h) => break h,
                        Err(mason_resources::TakeError::Busy)
                            if std::time::Instant::now() < end =>
                        {
                            std::thread::sleep(Duration::from_millis(25));
                        }
                        Err(e) => panic!("could not take the granted resources: {e}"),
                    }
                };
                // Let the daemon see the marker locked, as it will for a real caller.
                d.broker.lease_poll(id);
                return (id, held);
            }
            LeaseState::Waiting { .. } => {}
            LeaseState::Gone => panic!("the request vanished"),
        }
        assert!(std::time::Instant::now() < end, "never granted");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_explained(d: &Arc<Daemon>, t: TaskId) -> String {
    let dump = |d: &Arc<Daemon>| {
        let evs: Vec<String> = d
            .store
            .read_since(0, 1000)
            .unwrap()
            .iter()
            .map(|e| format!("{}: {:?}", e.seq, e.event.kind))
            .collect();
        format!("task {t}: {:?}\nevents:\n{}", task_of(d, t), evs.join("\n"))
    };
    // One deadline, generous for a loaded machine: the scripted develop step can be slow
    // to queue the task, and that is not the daemon misbehaving.
    let end = std::time::Instant::now() + Duration::from_secs(90);
    loop {
        if let Some(s) = d.broker.explain_task(t) {
            return s;
        }
        if std::time::Instant::now() >= end {
            panic!("never waited. {}", dump(d));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_step_waits_for_a_held_class_and_says_why_then_lands() {
    let s = compile_scratch();
    let d = s.start();
    let (id, held) = hold(&d, "interactive `cargo build`", &[("compile", 1)]);
    let t = file_task(&d, "Needs the compiler");
    let why = wait_explained(&d, t);
    assert!(
        why.starts_with(
            "Waiting for compile slot: 1st in line, held by interactive `cargo build` for "
        ),
        "{why}"
    );
    // The same sentence is the task's activity, so every monitor shows it. The broker
    // explains the wait as soon as the request is queued; the step's job records it as
    // the task's activity (a `WaitStarted` event) just after, so wait for that.
    wait_for("the task's activity to say what it waits for", 20, || {
        !matches!(task_of(&d, t).activity, Activity::Running)
    });
    let task = task_of(&d, t);
    assert_eq!(task.state, State::Testing);
    match &task.activity {
        Activity::Waiting(WaitReason::On(w)) => assert!(w.contains("compile slot"), "{w}"),
        other => panic!("not waiting on the class: {other:?}"),
    }
    drop(held);
    d.broker.lease_close(id);
    wait_state(&d, t, State::Landed, 60);
    assert!(d.broker.explain_task(t).is_none());
    assert_eq!(d.broker.used("compile"), 0, "nothing leaked");
}

#[test]
fn a_step_is_told_what_it_holds_so_its_command_need_not_queue_for_it() {
    // MASON_HELD names the step's `uses`: a command that would otherwise queue for
    // `compile` itself (`mason run --needs compile`) can see it already has it.
    let s = Scratch::new(
        "test x$MASON_HELD = xcompile && test -f hello.txt",
        unique_script(),
    );
    s.push_files(&[(".mason/mason.toml", DEF_COMPILE)], "definition");
    let d = s.start();
    let t = file_task(&d, "Holds the compiler");
    wait_state(&d, t, State::Landed, 60);
}

/// A test step listing one suite whose command records `MASON_HELD` and `MASON_TASK`.
fn suite_def(step_extra: &str, suite_extra: &str, log: &std::path::Path) -> String {
    let l = log.display();
    format!(
        r#"schema = 1
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
suites = ["checks"]
{step_extra}
max_rounds = 2
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.checks]
command = "echo \"r:$MASON_HELD:$MASON_TASK\" >> {l}; echo 1..1; echo ok 1 - a"
parser = "tap"
min_tests = 1
level = "unit"
{suite_extra}
"#
    )
}

fn run_suite_def(def: &str, log: &std::path::Path) -> (String, TaskId) {
    let s = Scratch::new("false", unique_script());
    s.push_files(&[(".mason/mason.toml", def)], "definition");
    let d = s.start();
    let t = file_task(&d, "Suites hold");
    wait_state(&d, t, State::Landed, 90);
    (std::fs::read_to_string(log).unwrap(), t)
}

#[test]
fn a_suite_command_is_told_what_its_step_holds() {
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("held");
    let def = suite_def("", "uses = { test-suite = 1 }", &log);
    let (text, t) = run_suite_def(&def, &log);
    let line = text.lines().next().unwrap();
    assert_eq!(line, format!("r:test-suite:{}", t.0), "{text}");
}

#[test]
fn a_suite_build_and_run_are_each_told_what_is_leased_for_them() {
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("held");
    let mut def = suite_def(
        "uses = { compile = 1, test-suite = 1 }",
        "uses = { test-suite = 1 }",
        &log,
    );
    def = def.replace(
        "parser = \"tap\"",
        &format!(
            "build = \"echo b:$MASON_HELD >> {}\"\nparser = \"tap\"",
            log.display()
        ),
    );
    let (text, t) = run_suite_def(&def, &log);
    assert_eq!(
        text.lines().collect::<Vec<_>>(),
        ["b:compile".to_string(), format!("r:test-suite:{}", t.0)],
        "{text}"
    );
}

#[test]
fn an_exclusive_suite_lease_is_named_in_the_held_list() {
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("held");
    let def = suite_def("", "uses = { exclusive = 1 }", &log).replace(
        "[resource.compile]",
        "[resource.exclusive]\ncount = 1\n\n[resource.compile]",
    );
    let (text, _) = run_suite_def(&def, &log);
    assert!(text.contains("exclusive"), "{text}");
}

#[test]
fn a_step_with_an_exclusive_suite_holds_the_machine_only_for_that_suite() {
    // On 2026-11-14 a star test step held `exclusive` for all 17 minutes of its suites, though
    // one suite of two minutes needed it: 26 builds waited and the CPU sat at 8% (s340).
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("held");
    let def = suite_def("", "uses = { exclusive = 1 }", &log)
        .replace(
            "[resource.compile]",
            "[resource.exclusive]\ncount = 1\n\n[resource.compile]",
        )
        .replace("suites = [\"checks\"]", "suites = [\"plain\", \"checks\"]")
        + &format!(
            r#"
[suite.plain]
command = "echo \"p:$MASON_HELD\" >> {}; echo 1..1; echo ok 1 - p"
parser = "tap"
min_tests = 1
level = "unit"
uses = {{ test-suite = 1 }}
"#,
            log.display()
        );
    let (text, _) = run_suite_def(&def, &log);
    let plain = text.lines().find(|l| l.starts_with("p:")).expect(&text);
    assert_eq!(
        plain, "p:test-suite",
        "the plain suite held more than its own: {text}"
    );
    let exclusive = text.lines().find(|l| l.starts_with("r:")).expect(&text);
    assert!(exclusive.contains("exclusive"), "{text}");
}

#[test]
fn a_steps_build_suites_run_side_by_side() {
    // On 2026-11-14 a star test step built five packages one after another for ten minutes,
    // each on one of three free `compile` slots (s345). Each suite here passes only if it sees
    // the other one start while it waits, so run one after the other the first goes red.
    let runs = tempfile::tempdir().unwrap();
    let dir = runs.path().display().to_string();
    let suite = |me: &str, other: &str| {
        format!(
            r#"
[suite.build-{me}]
command = "touch {dir}/{me}-$MASON_ATTEMPT; i=0; while [ ! -e {dir}/{other}-$MASON_ATTEMPT ] && [ $i -lt 300 ]; do sleep 0.1; i=$((i+1)); done; echo 1..1; if [ -e {dir}/{other}-$MASON_ATTEMPT ]; then echo {me}:saw >> {dir}/log; echo ok 1 - {me}; else echo {me}:alone >> {dir}/log; echo not ok 1 - {me}; fi"
parser = "tap"
min_tests = 1
level = "unit"
uses = {{ compile = 1 }}
"#
        )
    };
    let def = format!(
        r#"schema = 1
start = "develop"

[resource.compile]
count = 2

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
suites = ["build-a", "build-b"]
max_rounds = 1
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
{}{}"#,
        suite("a", "b"),
        suite("b", "a")
    );
    let s = Scratch::new("false", unique_script());
    s.push_files(&[(".mason/mason.toml", &def)], "definition");
    let d = s.start();
    let t = file_task(&d, "Builds side by side");
    wait_state(&d, t, State::Landed, 120);
    let log = std::fs::read_to_string(runs.path().join("log")).unwrap();
    assert!(
        !log.contains("alone"),
        "a build ran without the other beside it: {log}"
    );
}

#[test]
fn a_suite_already_green_for_the_tree_takes_no_lease_when_its_step_runs_again() {
    // 2026-11-14: star #216's test step restarted after a daemon update and waited 25 min for
    // compile slots to rebuild two suites already green for its exact tree, which it then
    // reused without running (s380). Here the only compile slot is held elsewhere when the
    // step runs again: it must finish anyway, and run neither build a second time.
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("ran");
    let suite = |me: &str| {
        format!(
            r#"
[suite.build-{me}]
command = "echo {me} >> {log}; echo 1..1; echo ok 1 - {me}"
parser = "tap"
min_tests = 1
level = "unit"
uses = {{ compile = 1 }}
"#,
            log = log.display()
        )
    };
    let def = format!(
        r#"schema = 1
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
suites = ["build-a", "build-b"]
max_rounds = 2
on_red = "develop"
next = "sign-off"

[step.sign-off]
type = "approval"
category = "review"
next = "land"

[step.land]
type = "landing"
category = "landing"
{}{}"#,
        suite("a"),
        suite("b")
    );
    let s = Scratch::new("false", unique_script());
    s.push_files(&[(".mason/mason.toml", &def)], "definition");
    let d = s.start();
    let t = file_task(&d, "Proven builds");
    wait_state(&d, t, State::Review, 120);
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 2);
    let (c, held) = hold(&d, "someone else's build", &[("compile", 1)]);
    d.emit(EventKind::Retried {
        task: t,
        to: State::Testing,
        step: String::new(),
        auto: false,
    })
    .unwrap();
    wait_for("the task to leave testing", 60, || {
        d.world().unwrap().tasks[&t].state == State::Review
    });
    assert_eq!(
        std::fs::read_to_string(&log).unwrap().lines().count(),
        2,
        "a proven build ran again"
    );
    drop(held);
    d.broker.lease_close(c);
}

#[test]
fn a_suites_run_time_is_recorded_and_gives_an_estimate_after_two_runs() {
    // The scheduler lets short work run while the machine drains for an exclusive request
    // only with an estimate of how long it takes, from the times recorded here (s385).
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("held");
    let def = suite_def("", "uses = { test-suite = 1 }", &log);
    let s = Scratch::new("false", unique_script());
    s.push_files(&[(".mason/mason.toml", &def)], "definition");
    let d = s.start();
    let first = file_task(&d, "First run");
    wait_state(&d, first, State::Landed, 90);
    assert_eq!(
        d.store.suite_estimate("scratch", "checks", "run").unwrap(),
        None
    );
    let second = file_task(&d, "Second run");
    wait_state(&d, second, State::Landed, 90);
    assert!(
        d.store
            .suite_estimate("scratch", "checks", "run")
            .unwrap()
            .is_some(),
        "two finished runs of `checks` give an estimate"
    );
}

#[test]
fn run_check_queues_like_the_test_step_and_frees_the_agents_slot() {
    // An agent waiting for its tests holds no LLM session slot, and the check waits
    // for the test step's `compile` like the test step does.
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("held");
    let cmd = format!("echo $MASON_HELD >> {}; test -f hello.txt", log.display());
    let develop = serde_json::json!([
        {"sh": TASK_SCRIPT_COMMIT},
        {"call": {"tool": "run_check", "args": {"name": "tests"}}},
        {"call": {"tool": "submit_change", "args": {"summary": "ok", "head": "$HEAD"}}}
    ]);
    let s = Scratch::new(
        &cmd,
        serde_json::json!({"steps": {"explore": [explore_turn()], "develop": [develop]}}),
    );
    s.push_files(&[(".mason/mason.toml", DEF_COMPILE)], "definition");
    let d = s.start();
    let (id, held) = hold(&d, "interactive", &[("compile", 1)]);
    let t = file_task(&d, "Checks its work");
    wait_for("the check to queue for compile", 60, || {
        d.broker
            .explain_task(t)
            .is_some_and(|w| w.contains("compile"))
    });
    assert_eq!(state_of(&d, t), State::Developing);
    assert_eq!(
        d.broker.used("llm:claude"),
        0,
        "the agent's slot was given back"
    );
    assert!(!log.exists(), "the check waits for its slot");
    drop(held);
    d.broker.lease_close(id);
    wait_state(&d, t, State::Landed, 90);
    let text = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        text.lines().collect::<Vec<_>>(),
        // The check ran it; the test step reused that green for the same tree (s179).
        ["compile"],
        "{text}"
    );
    assert_eq!(d.broker.used("compile"), 0, "nothing leaked");
}

#[test]
fn skipping_the_test_step_ends_a_waiting_run_check_and_runs_nothing() {
    // On 2026-11-13 the owner skipped testing for every active task, and their agents
    // still waited in `run_check` for compile slots to run the tests anyway.
    skip_ends_a_waiting_run_check("step:test");
}

#[test]
fn immunity_to_testing_ends_a_waiting_run_check_and_runs_nothing_through_landing() {
    // The monitor's Skip all testing, set while the task is still developing.
    skip_ends_a_waiting_run_check(mason_daemon::suites::ALL_TESTS);
}

fn skip_ends_a_waiting_run_check(scope: &str) {
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("held");
    let cmd = format!("echo $MASON_HELD >> {}; test -f hello.txt", log.display());
    let develop = serde_json::json!([
        {"sh": TASK_SCRIPT_COMMIT},
        {"call": {"tool": "run_check", "args": {"name": "tests"}}},
        {"call": {"tool": "submit_change", "args": {"summary": "ok", "head": "$HEAD"}}}
    ]);
    let s = Scratch::new(
        &cmd,
        serde_json::json!({"steps": {"explore": [explore_turn()], "develop": [develop]}}),
    );
    // Shaped like mason's own: a test step whose suite builds first, phase by phase.
    let def = DEF_COMPILE
        .replace(
            "uses = { compile = 1 }\nmax_rounds = 2",
            "uses = { compile = 1 }\nsuites = [\"unit\"]\nmax_rounds = 2",
        )
        + "\n[suite.unit]\nbuild = \"true\"\ncommand = \"sh -c 'echo 1..1; echo ok 1 - a'\"\nparser = \"tap\"\nmin_tests = 1\nlevel = \"unit\"\n";
    s.push_files(&[(".mason/mason.toml", &def)], "definition");
    let d = s.start();
    // The only compile slot stays held outside for the whole test.
    let (id, held) = hold(&d, "interactive", &[("compile", 1)]);
    let t = file_task(&d, "Checks its work");
    wait_for("the check to queue for compile", 60, || {
        d.broker
            .explain_task(t)
            .is_some_and(|w| w.contains("compile"))
    });
    let actions = |d: &std::sync::Arc<mason_daemon::daemon::Daemon>| {
        let w = d.world().unwrap();
        let ctx = mason_daemon::views::Ctx::new(d, &w);
        let row = ctx.row(&w.tasks[&t]);
        (row.allowed_actions, row.test_immune)
    };
    let (before, immune) = actions(&d);
    assert!(before.iter().any(|a| a == "skip_all_tests"), "{before:?}");
    assert!(!immune);
    mason_daemon::suites::skip(&d, t, scope, "drain the queue", "brian", false).unwrap();
    if scope == mason_daemon::suites::ALL_TESTS {
        let (after, immune) = actions(&d);
        assert!(!after.iter().any(|a| a == "skip_all_tests"), "{after:?}");
        assert!(immune, "the row says the task is immune");
        assert!(
            mason_daemon::suites::skip(&d, t, scope, "again", "brian", false).is_err(),
            "immunity is recorded once"
        );
    }
    // The check stops waiting and says it did not run; the test step is skipped too, and the
    // task is on to landing with the slot still held outside.
    wait_state(&d, t, State::Landing, 90);
    assert!(!log.exists(), "nothing ran the tests");
    assert_eq!(
        d.broker.used("compile"),
        1,
        "only the outside holder has it"
    );
    // Landing builds the batch before it promotes, whatever was skipped (the build floor), and
    // that build takes a compile slot of its own: the task lands once the outside holder lets go.
    drop(held);
    d.broker.lease_close(id);
    wait_state(&d, t, State::Landed, 90);
    assert!(!log.exists(), "the landing build ran no tests either");
    wait_for("the landing build to give its slot back", 60, || {
        d.broker.used("compile") == 0
    });
}

#[test]
fn waiting_is_outside_every_timeout() {
    let s = compile_scratch();
    // A 2 second test timeout, and a wait for the class that lasts longer.
    let p = s.config_dir.join("projects/scratch.toml");
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(text.contains("test_timeout_s = 60"));
    std::fs::write(
        &p,
        text.replace("test_timeout_s = 60", "test_timeout_s = 2"),
    )
    .unwrap();
    let d = s.start();
    let (id, held) = hold(&d, "interactive", &[("compile", 1)]);
    let t = file_task(&d, "Patient");
    wait_explained(&d, t);
    std::thread::sleep(Duration::from_secs(4));
    assert_eq!(state_of(&d, t), State::Testing, "still waiting, not failed");
    drop(held);
    d.broker.lease_close(id);
    wait_state(&d, t, State::Landed, 60);
}

#[test]
fn an_interactive_caller_goes_ahead_of_pipeline_work() {
    let s = compile_scratch();
    let d = s.start();
    let (a, held) = hold(&d, "first", &[("compile", 1)]);
    let t = file_task(&d, "Pipeline work");
    wait_explained(&d, t);
    // An interactive request arrives after the task started waiting.
    let map = [("compile".to_string(), 1)].into();
    let i = d.broker.lease_open("interactive `make`", &map);
    // The task now sees it is second in line, without naming the caller.
    let why = wait_explained(&d, t);
    assert!(
        why.contains("2nd in line") && !why.contains("behind"),
        "{why}"
    );
    drop(held);
    d.broker.lease_close(a);
    let (_, held_i) = take_when_granted(&d, i, "interactive `make`");
    assert_eq!(
        state_of(&d, t),
        State::Testing,
        "the task still waits its turn"
    );
    assert!(d.broker.explain_task(t).is_some());
    drop(held_i);
    d.broker.lease_close(i);
    wait_state(&d, t, State::Landed, 60);
}

/// A `mason run` made on behalf of a task from `develop` (3 steps from landing
/// in this definition's builtin shape is more than a task in `test`).
fn linked(d: &Arc<Daemon>, label: &str, steps: u32) -> u64 {
    let map = [("compile".to_string(), 1)].into();
    d.broker.lease_open_as(
        label,
        &map,
        Some(mason_daemon::sched::Standing {
            steps_remaining: steps,
            span: 1,
            priority: 1,
        }),
    )
}

fn queue_test_step_behind_a_holder() -> (Scratch, Arc<Daemon>, u64, Held, TaskId) {
    let s = compile_scratch();
    let d = s.start();
    let (a, held) = hold(&d, "first", &[("compile", 1)]);
    let t = file_task(&d, "Pipeline work");
    wait_explained(&d, t);
    (s, d, a, held, t)
}

fn wait_until_task_holds(d: &Arc<Daemon>, t: TaskId) {
    wait_for("the test step to be granted", 30, || {
        d.broker.has_lease(t) && d.broker.explain_task(t).is_none()
    });
}

#[test]
fn a_test_step_outranks_a_task_linked_run_from_a_task_with_more_steps_left() {
    let (_s, d, a, held, t) = queue_test_step_behind_a_holder();
    let l = linked(&d, "task 99 `cargo test`", 9);
    let why = wait_explained(&d, t);
    assert!(why.contains("1st in line"), "{why}");
    assert!(!why.contains("behind"), "{why}");
    match d.broker.lease_poll(l) {
        LeaseState::Waiting { explain } => {
            assert!(!explain.contains("(interactive)"), "{explain}");
        }
        _ => panic!("should be waiting"),
    }
    drop(held);
    d.broker.lease_close(a);
    wait_until_task_holds(&d, t);
    assert!(matches!(d.broker.lease_poll(l), LeaseState::Waiting { .. }));
    d.broker.lease_close(l);
    wait_state(&d, t, State::Landed, 60);
}

#[test]
fn a_run_with_no_task_still_outranks_all_pipeline_work_and_task_linked_runs() {
    let (_s, d, a, held, t) = queue_test_step_behind_a_holder();
    let l = linked(&d, "task 99 `cargo test`", 9);
    let map = [("compile".to_string(), 1)].into();
    let person = d.broker.lease_open("interactive `make`", &map);
    drop(held);
    d.broker.lease_close(a);
    let (_, held_p) = take_when_granted(&d, person, "interactive `make`");
    assert!(
        d.broker.explain_task(t).is_some(),
        "the test step still waits"
    );
    assert!(matches!(d.broker.lease_poll(l), LeaseState::Waiting { .. }));
    drop(held_p);
    d.broker.lease_close(person);
    d.broker.lease_close(l);
    wait_state(&d, t, State::Landed, 60);
}

#[test]
fn a_stream_of_task_linked_requests_cannot_hold_a_waiting_test_step_back() {
    let (_s, d, a, held, t) = queue_test_step_behind_a_holder();
    let stream: Vec<u64> = (0..6)
        .map(|i| linked(&d, &format!("task 99 `cargo test` #{i}"), 9))
        .collect();
    drop(held);
    d.broker.lease_close(a);
    wait_until_task_holds(&d, t);
    for id in &stream {
        assert!(matches!(
            d.broker.lease_poll(*id),
            LeaseState::Waiting { .. }
        ));
    }
    // New runs keep arriving while the step runs; it still finishes first.
    let more = linked(&d, "task 99 `cargo test` late", 9);
    wait_state(&d, t, State::Landed, 60);
    for id in stream.into_iter().chain([more]) {
        d.broker.lease_close(id);
    }
}

#[test]
fn a_dead_holder_frees_its_resource_and_the_next_waiter_starts() {
    let s = compile_scratch();
    let d = s.start();
    let (_id, held) = hold(&d, "will die", &[("compile", 1)]);
    let t = file_task(&d, "Next in line");
    wait_explained(&d, t);
    // The holder dies without telling anyone: its locks simply vanish.
    drop(held);
    wait_state(&d, t, State::Landed, 60);
    assert_eq!(d.broker.used("compile"), 0);
}

#[test]
fn a_new_grant_is_given_time_to_take_its_locks_before_it_is_reaped() {
    let s = compile_scratch();
    let d = s.start();
    let map = [("compile".to_string(), 1)].into();
    let id = d.broker.lease_open("slow starter", &map);
    assert!(matches!(
        d.broker.lease_poll(id),
        LeaseState::Granted { .. }
    ));
    std::thread::sleep(Duration::from_millis(1500));
    // Not locked yet, but well inside the grace period: still ours.
    assert!(matches!(
        d.broker.lease_poll(id),
        LeaseState::Granted { .. }
    ));
    d.broker.lease_close(id);
}

#[test]
fn a_request_nobody_polls_for_goes_away() {
    // (The 10 second silence rule is exercised by the unit-level reaper; here we
    // check the cheap half: closing frees the queue position.)
    let s = compile_scratch();
    let d = s.start();
    let (a, held) = hold(&d, "holder", &[("compile", 1)]);
    let map = [("compile".to_string(), 1)].into();
    let waiting = d.broker.lease_open("gives up", &map);
    assert!(matches!(
        d.broker.lease_poll(waiting),
        LeaseState::Waiting { .. }
    ));
    assert!(d.broker.lease_close(waiting));
    assert!(matches!(d.broker.lease_poll(waiting), LeaseState::Gone));
    drop(held);
    d.broker.lease_close(a);
}

#[test]
fn llm_sessions_are_adjustable_live_and_lowering_kills_nothing() {
    let s = Scratch::new("test -f hello.txt", unique_script());
    let d = s.start_with(|c| c.max_concurrent = 2);
    assert_eq!(d.broker.limit("llm:claude"), Some(2));
    d.broker.set_limit("llm:claude", 0).unwrap();
    let t = file_task(&d, "Held back");
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(
        state_of(&d, t),
        State::Queued,
        "no claim without a free session"
    );
    // No worktree for it either.
    let p = d.project(task_of(&d, t).project).unwrap().clone();
    assert!(!p.worktree(&d.cfg, t.0).exists());
    d.broker.set_limit("llm:claude", 1).unwrap();
    d.admit();
    wait_state(&d, t, State::Landed, 60);
    // The limit survives a restart.
    drop(d);
    let d2 = s.start_with(|c| c.max_concurrent = 2);
    assert_eq!(d2.broker.limit("llm:claude"), Some(1));
}

fn home_active(d: &Arc<Daemon>) -> (Vec<u64>, u32) {
    let world = d.world().unwrap();
    let h = mason_daemon::views::Ctx::new(d, &world).home("");
    (h.active.iter().map(|a| a.task).collect(), h.sessions_used)
}

#[test]
fn active_models_lists_only_tasks_that_hold_a_session() {
    let s = Scratch::new("test -f hello.txt", unique_script());
    let d = s.start_with(|c| c.max_concurrent = 1);
    // Nothing running: empty, and it matches the counter.
    assert_eq!(home_active(&d), (vec![], 0));
    // No session is free, so nothing is admitted; admit one task by hand, as
    // if its session had been taken from under it. Its agent step now waits.
    d.broker.set_limit("llm:claude", 0).unwrap();
    let t = file_task(&d, "Waits for a slot");
    let task = task_of(&d, t);
    let entry = d.entry_for(&task);
    d.emit(EventKind::Admitted {
        task: t,
        entry: Some(entry),
    })
    .unwrap();
    wait_explained(&d, t);
    assert!(state_of(&d, t).is_step());
    assert_eq!(home_active(&d), (vec![], 0));
    // A second task in a step state changes nothing either.
    let t2 = file_task(&d, "Also waits");
    let entry = d.entry_for(&task_of(&d, t2));
    d.emit(EventKind::Admitted {
        task: t2,
        entry: Some(entry),
    })
    .unwrap();
    wait_explained(&d, t2);
    assert_eq!(home_active(&d), (vec![], 0));
    // A session held by someone else counts, but is not a task's. Under 0747 the
    // list also holds started steps waiting for a check or a session, so the
    // hold is opened first: a person's request ranks ahead of pipeline work and
    // gets the one session, so no task starts during the assertions.
    let map = std::iter::once(("llm:claude".to_string(), 1)).collect();
    let id = d.broker.lease_open("other session", &map);
    d.broker.set_limit("llm:claude", 1).unwrap();
    let (id, held) = take_when_granted(&d, id, "other session");
    assert_eq!(home_active(&d), (vec![], 1));
    // Lowering the limit below what is held changes nothing about who is listed.
    d.broker.set_limit("llm:claude", 0).unwrap();
    assert_eq!(home_active(&d), (vec![], 1));
    drop(held);
    d.broker.lease_close(id);
}

#[test]
fn an_agent_whose_check_is_on_github_is_waiting_and_the_run_is_counted() {
    let s = Scratch::new("test -f hello.txt", unique_script());
    let d = s.start_with(|c| c.max_concurrent = 1);
    d.broker.set_limit("llm:claude", 0).unwrap();
    let t = file_task(&d, "Checking on GitHub");
    let entry = d.entry_for(&task_of(&d, t));
    d.emit(EventKind::Admitted {
        task: t,
        entry: Some(entry),
    })
    .unwrap();
    wait_explained(&d, t);
    // Its `run_check` is out on GitHub: a hold and a run record that has not ended.
    let holds = mason_daemon::remote::holds_dir(&d.cfg.hosts_dir());
    let record = holds.with_file_name("c1.remote");
    std::fs::create_dir_all(&holds).unwrap();
    mason_step_host::registry::write_kv(
        &record,
        &[
            ("sha", "abc".to_string()),
            ("collected", "0".to_string()),
            ("url", "https://github.com/o/r/actions/runs/9".to_string()),
            ("runs_on", "ubuntu-latest".to_string()),
        ],
    )
    .unwrap();
    mason_daemon::remote::write_hold(
        &holds,
        "c1",
        &mason_daemon::remote::RemoteHold {
            key: "c1".into(),
            label: "c1".into(),
            units: mason_daemon::remote::units_for("ubuntu-latest", 1),
            record,
            kind: "check".into(),
            task: t.0,
            ..Default::default()
        },
    );
    let world = d.world().unwrap();
    let h = mason_daemon::views::Ctx::new(&d, &world).home("");
    let a = h.active.iter().find(|a| a.task == t.0).expect("listed");
    assert_eq!(
        (a.state.as_str(), a.place.as_str()),
        ("waiting_check", "github")
    );
    assert_eq!(a.run_url, "https://github.com/o/r/actions/runs/9");
    let r = h.remote.expect("a run is out");
    assert_eq!((r.checks, r.test_steps, r.batches, r.debts), (1, 0, 0, 0));
    assert_eq!(r.runs.len(), 1);
    assert_eq!(r.runs[0].task, t.0);
}

/// #1155: a check running on a node names the node, from the list of runs.
#[test]
fn an_agent_whose_check_is_on_a_node_names_the_node() {
    use mason_daemon::running;
    let s = Scratch::new("test -f hello.txt", unique_script());
    let d = s.start_with(|c| c.max_concurrent = 1);
    d.broker.set_limit("llm:claude", 0).unwrap();
    let t = file_task(&d, "Checking on a node");
    let entry = d.entry_for(&task_of(&d, t));
    d.emit(EventKind::Admitted {
        task: t,
        entry: Some(entry),
    })
    .unwrap();
    wait_explained(&d, t);
    let on_node = running::start(
        &d.cfg.hosts_dir(),
        running::Run {
            id: format!("t{}r1-check0", t.0),
            project: "scratch".into(),
            task: Some(t.0),
            kind: running::Kind::Check,
            name: "unit".into(),
            host: running::Host::Node("marmot".into()),
            since_ms: 1,
            url: String::new(),
        },
    );
    let world = d.world().unwrap();
    let h = mason_daemon::views::Ctx::new(&d, &world).home("");
    let a = h.active.iter().find(|a| a.task == t.0).expect("listed");
    assert_eq!(
        (a.state.as_str(), a.place.as_str()),
        ("waiting_check", "node:marmot")
    );
    let node = h
        .hosts
        .iter()
        .find(|x| x.kind == "node" && x.name == "marmot")
        .expect("the node of a run is a host");
    assert_eq!(
        node.runs[0].task.as_ref().map(|r| r.id),
        Some(t.0),
        "{node:?}"
    );
    // Another project's view keeps the hosts and drops the run.
    let other = mason_daemon::views::Ctx::new(&d, &world).home("elsewhere");
    assert!(other.hosts.iter().any(|x| x.kind == "local"));
    assert!(other.hosts.iter().all(|x| x.runs.is_empty()));
    drop(on_node);
    let h = mason_daemon::views::Ctx::new(&d, &world).home("");
    assert!(h
        .hosts
        .iter()
        .all(|x| x.runs.iter().all(|r| !r.host.starts_with("node:"))));
}

fn file_in(d: &Arc<Daemon>, project: &str, title: &str) -> TaskId {
    let pid = d.project_id_by_name(project).unwrap();
    let a = d
        .emit(EventKind::TaskFiled {
            project: pid,
            title: title.into(),
            body: format!("Please do: {title}"),
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            explored: false,
            proposed: false,
            blockers: vec![],
            not_before: None,
        })
        .unwrap();
    a.effects
        .iter()
        .find_map(|e| match e {
            Effect::TaskCreated { task } => Some(*task),
            _ => None,
        })
        .unwrap()
}

#[test]
fn two_projects_share_the_pool_and_admission_follows_progress_through_each_route() {
    let s = Scratch::new("test -f hello.txt", unique_script());
    s.push_files(&[(".mason/mason.toml", DEF_LONG)], "long definition");
    // A second project on its own base branch, with a shorter graph.
    sh(&s.repo, &["push", "-q", "origin", "main:short"]);
    let dir = s.repo.parent().unwrap().join("pusher-short");
    sh(
        s.repo.parent().unwrap(),
        &[
            "clone",
            "-q",
            "-b",
            "short",
            s.origin.to_str().unwrap(),
            dir.to_str().unwrap(),
        ],
    );
    std::fs::create_dir_all(dir.join(".mason")).unwrap();
    std::fs::write(dir.join(".mason/mason.toml"), DEF_SHORT).unwrap();
    sh(&dir, &["add", "-A"]);
    sh(&dir, &["commit", "-q", "-m", "short definition"]);
    sh(&dir, &["push", "-q", "origin", "short"]);
    let p = s.config_dir.join("projects/short.toml");
    let text = std::fs::read_to_string(s.config_dir.join("projects/scratch.toml")).unwrap();
    std::fs::write(&p, text.replace("base = \"main\"", "base = \"short\"")).unwrap();

    let d = s.start();
    d.broker.set_limit("llm:claude", 0).unwrap();
    // The long-graph task is filed first, so it has the earlier enqueue time.
    let long = file_in(&d, "scratch", "long graph, filed first");
    std::thread::sleep(Duration::from_millis(20));
    let short = file_in(&d, "short", "short graph, filed second");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(state_of(&d, long), State::Queued);
    assert_eq!(state_of(&d, short), State::Queued);
    d.broker.set_limit("llm:claude", 1).unwrap();
    d.admit();
    wait_state(&d, short, State::Landed, 90);
    wait_state(&d, long, State::Landed, 90);
    // The first step run is the first admission. Both tasks are at the start of
    // their own route, so neither has come further (4 of 4 steps left against
    // 2 of 2): the route's length no longer decides across projects (#287), and
    // the one filed first goes first.
    let w = d.world().unwrap();
    let first = |t: TaskId| {
        w.step_runs
            .values()
            .filter(|r| r.task == t)
            .map(|r| r.id)
            .min()
            .unwrap()
    };
    assert!(
        first(long) < first(short),
        "equally far along ({} of {} steps left against {} of {}): the one filed first goes first",
        4,
        4,
        2,
        2
    );
    // Isolation: each landed on its own base.
    let on = |branch: &str| sh(&s.origin, &["log", "--format=%s", branch]);
    assert!(on("short").contains("add hello"));
    assert!(on("main").contains("add hello"));
}

#[test]
fn releasing_several_held_tasks_at_once_never_runs_more_than_max_concurrent() {
    let s = Scratch::new("test -f hello.txt", unique_script());
    s.push_files(&[(".mason/mason.toml", DEF_LONG)], "long definition");
    let d = s.start_with(|c| c.max_concurrent = 2);
    // The blocker is queued but cannot start (no session), so the others are held behind it.
    d.broker.set_limit("llm:claude", 0).unwrap();
    let blocker = file_in(&d, "scratch", "blocker");
    let pid = d.project_id_by_name("scratch").unwrap();
    let held: Vec<TaskId> = (0..5)
        .map(|i| {
            let a = d
                .emit(EventKind::TaskFiled {
                    project: pid,
                    title: format!("held {i}"),
                    body: "x".into(),
                    priority: Priority::Normal,
                    trust: TrustSource::Owner,
                    explored: false,
                    proposed: false,
                    blockers: vec![blocker],
                    not_before: None,
                })
                .unwrap();
            a.effects
                .iter()
                .find_map(|e| match e {
                    Effect::TaskCreated { task } => Some(*task),
                    _ => None,
                })
                .unwrap()
        })
        .collect();
    std::thread::sleep(Duration::from_millis(300));
    for t in &held {
        assert!(matches!(task_of(&d, *t).activity, Activity::Held(_)));
    }
    // Land the blocker: all five are released in the same instant.
    d.broker.set_limit("llm:claude", 2).unwrap();
    d.admit();
    let started = std::time::Instant::now();
    let mut peak = 0;
    loop {
        let w = d.world().unwrap();
        let running = w
            .step_runs
            .values()
            .filter(|r| {
                r.outcome.is_none()
                    && matches!(r.step, State::Exploring | State::Developing | State::Review)
            })
            .count();
        peak = peak.max(running);
        assert!(
            running <= 2,
            "{running} agent steps run at once; max_concurrent is 2"
        );
        if held.iter().all(|t| w.tasks[t].state == State::Landed) {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(150), "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(peak >= 1);
}

/// s56: two accounts with a session each. A full (here: zero-session) account
/// holds back only its own project; the other account's task goes through.
#[test]
fn a_full_account_does_not_block_another_accounts_task() {
    let s = Scratch::new("test -f hello.txt", unique_script());
    std::fs::write(
        s.config_dir.join("providers.toml"),
        "[provider.a]\nadapter = \"claude\"\nbilling = \"subscription\"\nsessions = 1\n\
         [provider.b]\nadapter = \"claude\"\nbilling = \"api\"\nsessions = 1\n",
    )
    .unwrap();
    s.write_project("scratch", "test -f hello.txt", "account = \"a\"");
    s.write_project("other", "test -f hello.txt", "account = \"b\"");
    let d = s.start();
    assert_eq!(d.broker.limit("llm:a"), Some(1));
    assert_eq!(d.broker.limit("llm:b"), Some(1));
    assert_eq!(d.broker.limit("llm:claude"), None);
    d.broker.set_limit("llm:a", 0).unwrap();
    let held = file_in(&d, "scratch", "on the full account");
    let free = file_in(&d, "other", "on the free account");
    wait_state(&d, free, State::Landed, 90);
    assert_eq!(state_of(&d, held), State::Queued);
    let w = d.world().unwrap();
    assert_eq!(d.llm_free(&w, "a"), 0);
    assert_eq!(d.llm_free(&w, "b"), 1);
    assert_eq!(d.account_of(task_of(&d, held).project), "a");
    d.broker.set_limit("llm:a", 1).unwrap();
    d.admit();
    wait_state(&d, held, State::Landed, 90);
}

/// A test step that builds first: the build holds the one `compile` slot, and
/// the tests hold `test-suite` without it.
const DEF_BUILD: &str = r#"schema = 1
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
build = "echo b:$MASON_HELD >> \"$HELD_LOG\""
uses = { compile = 1, test-suite = 1 }
max_rounds = 2
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

#[test]
fn a_test_step_with_a_build_holds_compile_only_while_it_builds() {
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("held");
    let def = DEF_BUILD.replace("$HELD_LOG", &log.display().to_string());
    let cmd = format!("echo t:$MASON_HELD >> {}; test -f hello.txt", log.display());
    let s = Scratch::new(&cmd, unique_script());
    s.push_files(&[(".mason/mason.toml", &def)], "definition");
    let d = s.start();
    d.broker.declare("test-suite", 1);
    let (id, held) = hold(&d, "interactive test run", &[("test-suite", 1)]);
    let t = file_task(&d, "Builds, then tests");
    // The build ran, and the tests wait for `test-suite` holding no build slot.
    wait_for(
        "the tests to queue for test-suite after the build",
        90,
        || {
            d.broker
                .explain_task(t)
                .is_some_and(|w| w.contains("test-suite"))
        },
    );
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "b:compile\n");
    assert_eq!(d.broker.used("compile"), 0, "the build slot was given back");
    let (c, held_c) = hold(&d, "interactive build", &[("compile", 1)]);
    drop(held_c);
    d.broker.lease_close(c);
    drop(held);
    d.broker.lease_close(id);
    wait_state(&d, t, State::Landed, 60);
    let text = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        text.lines().collect::<Vec<_>>(),
        ["b:compile", "t:test-suite"],
        "{text}"
    );
    assert_eq!(d.broker.used("compile"), 0, "nothing leaked");
    assert_eq!(d.broker.used("test-suite"), 0, "nothing leaked");
}

#[test]
fn a_failed_build_is_red_and_the_tests_do_not_run() {
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("held");
    let def = DEF_BUILD
        .replace("$HELD_LOG", &log.display().to_string())
        .replace(
            "echo b:$MASON_HELD",
            "echo cannot compile; exit 1; echo b:$MASON_HELD",
        );
    let cmd = format!("echo t:$MASON_HELD >> {}; test -f hello.txt", log.display());
    let s = Scratch::new(&cmd, unique_script());
    s.push_files(&[(".mason/mason.toml", &def)], "definition");
    let d = s.start();
    let t = file_task(&d, "Does not build");
    // Red sends the task back to develop with the build's output as feedback.
    let fb = d.cfg.task_dir(t.0).join("feedback.txt");
    wait_for("the build's failure to come back as feedback", 90, || {
        std::fs::read_to_string(&fb).is_ok_and(|f| f.contains("`test`'s build failed"))
    });
    let f = std::fs::read_to_string(&fb).unwrap();
    assert!(f.contains("cannot compile"), "{f}");
    assert!(!log.exists(), "the tests never ran");
    assert_eq!(d.broker.used("compile"), 0, "nothing leaked");
}

#[test]
fn run_check_builds_then_tests_like_the_test_step() {
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("held");
    let def = DEF_BUILD.replace("$HELD_LOG", &log.display().to_string());
    let cmd = format!("echo t:$MASON_HELD >> {}; test -f hello.txt", log.display());
    let develop = serde_json::json!([
        {"sh": TASK_SCRIPT_COMMIT},
        {"call": {"tool": "run_check", "args": {"name": "tests"}}},
        {"call": {"tool": "submit_change", "args": {"summary": "ok", "head": "$HEAD"}}}
    ]);
    let s = Scratch::new(
        &cmd,
        serde_json::json!({"steps": {"explore": [explore_turn()], "develop": [develop]}}),
    );
    s.push_files(&[(".mason/mason.toml", &def)], "definition");
    let d = s.start();
    let t = file_task(&d, "Checks its work in two phases");
    wait_state(&d, t, State::Landed, 120);
    let text = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        text.lines().collect::<Vec<_>>(),
        // The check built and tested; the test step reused that green (s179).
        ["b:compile", "t:test-suite"],
        "{text}"
    );
    assert_eq!(d.broker.used("compile"), 0, "nothing leaked");
}

#[test]
fn home_compile_holders_carry_the_tasks_project_and_full_title() {
    let s = Scratch::new("test -f hello.txt", unique_script());
    let d = s.start_with(|c| c.max_concurrent = 1);
    d.broker.set_limit("llm:claude", 0).unwrap();
    let title = "A very long title that is certainly longer than the monitor will show in a row";
    // Task 0 would read as "no task" in a holder, so file one first.
    let _first = file_in(&d, "scratch", "first");
    let t = file_in(&d, "scratch", title);
    assert!(t.0 > 0);
    d.broker.set_limit("compile", 3).unwrap();
    let Ok(held_task) = d.broker.acquire(
        t,
        "#task",
        mason_scheduler::Rank::new(false, 3, 2, 1, 1001, false),
        mason_scheduler::Needs::of(&[("compile", 1)]),
        &|| true,
        &mut |_| {},
    ) else {
        panic!("the task was not granted a compile slot")
    };
    let (pid, held_person) = hold(&d, "interactive `cargo build`", &[("compile", 1)]);
    let world = d.world().unwrap();
    let name = world.projects[&world.tasks[&t].project].name.clone();
    assert!(!name.is_empty());
    // Home filtered to a project the holder does not belong to still names it.
    for filter in ["", "elsewhere"] {
        let h = mason_daemon::views::Ctx::new(&d, &world).home(filter);
        let running = h.compilation.unwrap().running;
        let task_row = running.iter().find(|r| r.task == t.0).unwrap();
        assert_eq!(task_row.project, name);
        assert_eq!(task_row.title, title);
        let person = running.iter().find(|r| r.task == 0).unwrap();
        assert_eq!((person.project.as_str(), person.title.as_str()), ("", ""));
    }
    drop(held_task);
    drop(held_person);
    d.broker.lease_close(pid);
}

#[test]
fn a_long_interactive_script_is_one_short_line_with_the_whole_command_beside_it() {
    let s = Scratch::new("test -f hello.txt", unique_script());
    let d = s.start_with(|c| c.max_concurrent = 1);
    d.broker.set_limit("compile", 1).unwrap();
    let script = format!(
        "bash -c python3 - <<'EOF'\np=\"crates/adapters/src/hosted.rs\"\n{}\nEOF",
        "x = 1  # ".repeat(60)
    );
    let label = format!("interactive `{script}`");
    let (first, held) = hold(&d, &label, &[("compile", 1)]);
    let map = [("compile".to_string(), 1u32)].into_iter().collect();
    let second = d.broker.lease_open(&label, &map);
    let v = d.broker.snapshot();
    let mut seen = 0;
    for list in ["holders", "waiting"] {
        for e in v[list].as_array().unwrap() {
            let l = e["label"].as_str().unwrap();
            assert!(!l.contains('\n') && l.chars().count() <= 50, "{l}");
            assert_eq!(e["command"].as_str().unwrap(), script);
            if let Some(x) = e["explain"].as_str() {
                assert!(!x.contains('\n'), "{x}");
            }
            seen += 1;
        }
    }
    assert_eq!(seen, 2);
    let c = d.broker.compilation();
    let w = &c.pending[0];
    assert_eq!(w.command, script);
    assert!(w.label.chars().count() <= 50 && !w.label.contains('\n'));
    assert!(
        !w.reason.contains('\n') && w.reason.chars().count() <= 50,
        "{}",
        w.reason
    );
    assert_eq!(c.running[0].command, script);
    drop(held);
    d.broker.lease_close(first);
    d.broker.lease_close(second);
}
