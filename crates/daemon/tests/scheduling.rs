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
    let mut text = String::new();
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
    let start = std::time::Instant::now();
    wait_for("the task to be waiting with a reason", 40, || {
        if start.elapsed() > Duration::from_secs(15) {
            panic!("never waited. {}", dump(d));
        }
        match d.broker.explain_task(t) {
            Some(s) => {
                text = s;
                true
            }
            None => false,
        }
    });
    text
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
        ["compile", "compile"],
        "{text}"
    );
    assert_eq!(d.broker.used("compile"), 0, "nothing leaked");
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
    // The task now sees it is behind the interactive caller.
    let why = wait_explained(&d, t);
    assert!(
        why.contains("2nd in line, behind interactive `make` (interactive)"),
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
fn two_projects_share_the_pool_and_admission_follows_the_rank() {
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
    // The first step run is the first admission: fewer steps remaining won,
    // across projects, even though it was filed later.
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
        first(short) < first(long),
        "short ({} steps) goes before long ({} steps)",
        2,
        4
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
