// SPDX-License-Identifier: GPL-3.0-or-later

//! End to end, in one process: the real daemon, real git, real step hosts, and
//! a scripted stand-in for the agent CLI. Only the model is fake.

mod support;
use mason_core::*;
use mason_daemon::http::render_logs;
use mason_landing::git;
use serde_json::json;
use std::time::Duration;
use support::*;

#[test]
fn a_task_goes_from_queued_to_landed() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    let d = s.start();
    let t = file_task(&d, "Add a hello file");
    wait_state(&d, t, State::Landed, 60);
    wait_for("the job to finish cleaning up", 15, || d.job_count() == 0);

    let w = d.world().unwrap();
    let task = &w.tasks[&t];
    assert_eq!(task.landed, Some(LandedOutcome::Merged));
    let steps: Vec<State> = w
        .step_runs
        .values()
        .filter(|r| r.task == t)
        .map(|r| r.step)
        .collect();
    assert_eq!(
        steps,
        vec![
            State::Exploring,
            State::Developing,
            State::Testing,
            State::Review,
            State::Landing
        ],
        "the fixed graph, in order, once each"
    );
    // It really reached the remote, exactly once, with the agent's work.
    let log = s.origin_log();
    assert_eq!(
        log.iter().filter(|m| *m == "add hello").count(),
        1,
        "{log:?}"
    );
    assert_eq!(sh(&s.origin, &["show", "main:hello.txt"]), "hello");
    // Cleaned up: no worktree, no branch; the owner's checkout was never touched.
    let p = d.project(task.project).unwrap();
    assert!(!p.worktree(&d.cfg, t.0).exists());
    assert!(!git::branch_exists(&s.repo, &p.branch(t.0)));
    assert_eq!(
        sh(&s.repo, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "owner-work"
    );
    // Consistent, and nothing needed repairing.
    d.store.verify().unwrap();
    assert!(
        d.store.bugs().unwrap().is_empty(),
        "{:?}",
        d.store.bugs().unwrap()
    );
    // The plan and the logs are there to read.
    assert!(d.task_dir(t).join("plan.json").exists());
    let logs = render_logs(&d, t.0);
    assert!(
        logs.contains("mcp__mason__submit_plan") && logs.contains("mcp__mason__submit_change"),
        "{logs}"
    );
    assert!(logs.contains("accepted"));
}

#[test]
fn red_tests_send_the_task_back_to_development_and_then_it_lands() {
    // The first attempt makes hello.txt only; the test also wants fixed.txt.
    let develop2 = json!([
        {"sh": "echo ok > fixed.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm 'add fixed'"},
        {"call": {"tool": "submit_change", "args": {"summary": "fixed", "head": "$HEAD"}}}
    ]);
    let script =
        json!({"steps": {"explore": [explore_turn()], "develop": [develop_turn(), develop2]}});
    let s = Scratch::new("test -f hello.txt && test -f fixed.txt", script);
    let d = s.start();
    let t = file_task(&d, "Needs a second try");
    wait_state(&d, t, State::Landed, 90);
    let w = d.world().unwrap();
    assert_eq!(w.tasks[&t].attempts[&State::Developing], 2);
    assert_eq!(w.tasks[&t].attempts[&State::Testing], 2);
    assert_eq!(w.tasks[&t].failures, 0, "red tests are not a failure");
    let log = s.origin_log();
    assert!(
        log.contains(&"add hello".to_string()) && log.contains(&"add fixed".to_string()),
        "{log:?}"
    );
}

#[test]
fn tests_that_never_pass_fail_the_task_after_the_round_limit() {
    let s = Scratch::new(
        "echo nope; exit 1",
        json!({"steps": {"explore": [explore_turn()], "develop": [develop_turn(), develop_turn()]}}),
    );
    s.write_project("scratch", "echo nope; exit 1", "max_test_rounds = 2");
    let d = s.start();
    let t = file_task(&d, "Never green");
    wait_state(&d, t, State::Failed, 90);
    let w = d.world().unwrap();
    assert_eq!(w.tasks[&t].failures, 1);
    assert_eq!(w.tasks[&t].attempts[&State::Testing], 2);
    assert!(
        !s.origin_log().contains(&"add hello".to_string()),
        "nothing red may land"
    );
}

#[test]
fn an_agent_that_never_submits_fails_its_step() {
    let script = json!({"steps": {"explore": [explore_turn()], "develop": [[{"say": "all done!"}], [{"say": "still done"}], [{"say": "done"}]]}});
    let s = Scratch::new("true", script);
    let d = s.start();
    let t = file_task(&d, "Silent agent");
    wait_state(&d, t, State::Failed, 60);
    let w = d.world().unwrap();
    let dev: Vec<_> = w
        .step_runs
        .values()
        .filter(|r| r.step == State::Developing)
        .collect();
    assert_eq!(dev.len(), 1);
    // The nudges were resumes of the same session, not new steps.
    let hosts = std::fs::read_dir(d.cfg.hosts_dir())
        .unwrap()
        .flatten()
        .filter(|e| {
            e.path().is_dir()
                && e.file_name()
                    .to_string_lossy()
                    .starts_with(&format!("t{}r{}-", t.0, dev[0].id.0))
        })
        .count();
    assert_eq!(hosts, 3, "the first turn plus two nudges");
}

#[test]
fn a_submission_is_rejected_until_the_work_is_really_committed() {
    // First submit with nothing committed (no commits beyond the base), then do the work and resubmit.
    let develop = json!([
        {"call": {"tool": "submit_change", "args": {"summary": "I think I am done", "head": "$HEAD"}}},
        {"sh": "echo hello > hello.txt"},
        {"call": {"tool": "submit_change", "args": {"summary": "forgot to commit", "head": "$HEAD"}}},
        {"sh": "git add -A && git -c user.name=t -c user.email=t@t commit -qm 'add hello'"},
        {"call": {"tool": "submit_change", "args": {"summary": "now committed", "head": "$HEAD"}}}
    ]);
    let s = Scratch::new(
        "test -f hello.txt",
        json!({"steps": {"explore": [explore_turn()], "develop": [develop]}}),
    );
    let d = s.start();
    let t = file_task(&d, "Sloppy agent");
    wait_state(&d, t, State::Landed, 60);
    let logs = render_logs(&d, t.0);
    assert!(logs.contains("no commits beyond"), "{logs}");
    assert!(logs.contains("uncommitted changes"), "{logs}");
    assert_eq!(
        s.origin_log().iter().filter(|m| *m == "add hello").count(),
        1
    );
}

#[test]
fn cancel_stops_the_running_agent_and_keeps_the_branch() {
    let develop = json!([{"sh": TASK_SCRIPT_COMMIT}, {"sleep_ms": 60000}]);
    let s = Scratch::new(
        "true",
        json!({"steps": {"explore": [explore_turn()], "develop": [develop]}}),
    );
    let d = s.start();
    let t = file_task(&d, "Long running");
    wait_state(&d, t, State::Developing, 60);
    let p = d
        .project(d.world().unwrap().tasks[&t].project)
        .unwrap()
        .clone();
    wait_for("the agent to commit", 30, || {
        git::branch_exists(&s.repo, &p.branch(t.0))
            && git::commits_ahead(&s.repo, "origin/main", &p.branch(t.0)).unwrap_or(0) >= 1
    });

    d.emit(EventKind::Canceled {
        task: t,
        delete_branch: false,
    })
    .unwrap();
    assert_eq!(state_of(&d, t), State::Canceled);
    wait_for("the step host to stop", 15, || {
        mason_step_host::registry::Registry::new(d.cfg.hosts_dir())
            .scan()
            .unwrap()
            .iter()
            .all(|h| h.state != mason_step_host::registry::HostState::Running)
    });
    wait_for("the job to end", 15, || d.job_count() == 0);
    assert!(
        git::branch_exists(&s.repo, &p.branch(t.0)),
        "the branch is kept unless asked otherwise"
    );
    assert!(!s.origin_log().contains(&"add hello".to_string()));
    assert_eq!(
        state_of(&d, t),
        State::Canceled,
        "the dead job did not resurrect the task"
    );
}

#[test]
fn cancel_can_delete_the_branch() {
    let develop = json!([{"sh": TASK_SCRIPT_COMMIT}, {"sleep_ms": 60000}]);
    let s = Scratch::new(
        "true",
        json!({"steps": {"explore": [explore_turn()], "develop": [develop]}}),
    );
    let d = s.start();
    let t = file_task(&d, "Throwaway");
    wait_state(&d, t, State::Developing, 60);
    let p = d
        .project(d.world().unwrap().tasks[&t].project)
        .unwrap()
        .clone();
    wait_for("the branch", 30, || {
        git::branch_exists(&s.repo, &p.branch(t.0))
    });
    d.emit(EventKind::Canceled {
        task: t,
        delete_branch: true,
    })
    .unwrap();
    wait_for("the branch to go", 20, || {
        !git::branch_exists(&s.repo, &p.branch(t.0))
    });
}

#[test]
fn admission_respects_the_concurrency_limit() {
    let s = Scratch::new(
        "test -f hello.txt",
        json!({"steps": {"explore": [explore_turn()], "develop": [develop_turn()]}}),
    );
    // Each task has its own worktree, so the same scripted commit lands three different ways:
    // give each task a distinct file by letting the later ones fold in the earlier ones' work.
    let mut cfg = s.config();
    cfg.max_concurrent = 1;
    let d = mason_daemon::Daemon::start(cfg).unwrap();
    let ts: Vec<TaskId> = (0..2)
        .map(|i| file_task(&d, &format!("task {i}")))
        .collect();
    let mut max_active = 0;
    wait_for("both to finish", 150, || {
        let w = d.world().unwrap();
        max_active = max_active.max(w.tasks.values().filter(|t| t.state.is_step()).count());
        ts.iter()
            .all(|t| w.tasks[t].state.is_terminal() || w.tasks[t].state == State::Failed)
    });
    assert_eq!(
        max_active, 1,
        "never more than max_concurrent in a step at once"
    );
    let w = d.world().unwrap();
    assert!(w.tasks[&ts[0]].landed.is_some(), "the first task lands");
}

#[test]
fn a_failed_task_can_be_retried_and_lands() {
    // Three silent turns fail the step (the turn and two nudges); the fourth invocation does the work.
    let develop = json!([[{"say": "done"}], [{"say": "done"}], [{"say": "done"}], [{"sh": TASK_SCRIPT_COMMIT}, {"call": {"tool": "submit_change", "args": {"summary": "ok", "head": "$HEAD"}}}]]);
    let s = Scratch::new(
        "test -f hello.txt",
        json!({"steps": {"explore": [explore_turn()], "develop": develop}}),
    );
    let d = s.start();
    let t = file_task(&d, "Flaky agent");
    wait_state(&d, t, State::Failed, 60);
    d.emit(EventKind::Retried {
        task: t,
        to: State::Developing,
    })
    .unwrap();
    wait_state(&d, t, State::Landed, 60);
    assert_eq!(d.world().unwrap().tasks[&t].failures, 1);
    assert_eq!(
        s.origin_log().iter().filter(|m| *m == "add hello").count(),
        1
    );
}

#[test]
fn a_provider_hold_waits_and_resumes_without_failing_the_task() {
    // The usage-limit result comes back on the first developing invocation; the daemon
    // waits (hold_retry is 300 ms in tests) and resumes the session, which then works.
    let limit = json!([{"replay": format!("{}/../adapters/fixtures/claude/synthetic_usage_limit.jsonl", env!("CARGO_MANIFEST_DIR"))}]);
    let script =
        json!({"steps": {"explore": [explore_turn()], "develop": [limit, develop_turn()]}});
    let s = Scratch::new("test -f hello.txt", script);
    let d = s.start();
    let t = file_task(&d, "Hit the limit");
    let mut saw_wait = false;
    wait_for("the task to land", 90, || {
        let w = d.world().unwrap();
        if matches!(w.tasks[&t].activity, Activity::Waiting(WaitReason::On(ref on)) if on.contains("usage limit"))
        {
            saw_wait = true;
        }
        w.tasks[&t].state == State::Landed
    });
    assert!(saw_wait, "the hold was visible as a wait, with its reason");
    assert_eq!(
        d.world().unwrap().tasks[&t].failures,
        0,
        "a hold is never a failure"
    );
}
