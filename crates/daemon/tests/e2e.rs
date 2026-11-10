// SPDX-License-Identifier: GPL-3.0-or-later

//! End to end, in one process: the real daemon, real git, real step hosts, and
//! a scripted stand-in for the agent CLI. Only the model is fake.

mod support;
use mason_core::*;
use mason_daemon::http::render_logs;
use mason_landing::git;
use serde_json::json;
use support::*;

#[test]
fn a_task_goes_from_queued_to_landed() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    let d = s.start();
    let t = file_task(&d, "Add a hello file");
    wait_state(&d, t, State::Landed, 60);
    wait_for("the job to finish cleaning up", 15, || d.job_count() == 0);
    maybe_record(&d, "landed_task");

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
            State::Landing
        ],
        "the built-in graph, in order, once each"
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
    wait_for("cleanup", 15, || d.job_count() == 0);
    maybe_record(&d, "red_then_green");
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
fn a_branch_that_conflicts_with_the_base_goes_to_the_agent_to_merge_and_lands() {
    // The first develop turn commits conflict.txt, and meanwhile the base gets its own
    // conflict.txt. Its tests are red, so it goes back to develop. Bringing the branch up
    // to date with the base now conflicts. That used to fail every step before any agent
    // ran; now the develop agent is told about it, merges, and the task lands.
    let base_moves = "tmp=$(mktemp -d) && git worktree add -q --detach \"$tmp\" origin/main && \
        (cd \"$tmp\" && echo theirs > conflict.txt && git add -A && \
         git -c user.name=t -c user.email=t@t commit -qm 'base changes conflict.txt' && \
         git push -q origin HEAD:main) && git worktree remove --force \"$tmp\"";
    let develop1 = json!([
        {"sh": base_moves},
        {"sh": format!("echo mine > conflict.txt && {TASK_SCRIPT_COMMIT}")},
        {"call": {"tool": "submit_change", "args": {"summary": "first try", "head": "$HEAD"}}}
    ]);
    let develop2 = json!([
        {"sh": "printf '%s' \"$FAKE_PROMPT\" > seen-prompt.txt; \
                grep -q 'no longer merges cleanly' seen-prompt.txt && grep -q 'conflict.txt' seen-prompt.txt || exit 1; \
                rm seen-prompt.txt; git -c user.name=t -c user.email=t@t merge -q origin/main; echo both > conflict.txt && echo ok > fixed.txt && \
                git add -A && git -c user.name=t -c user.email=t@t commit -qm 'merge the base and fix'"},
        {"call": {"tool": "submit_change", "args": {"summary": "merged", "head": "$HEAD"}}}
    ]);
    let script = json!({"steps": {"explore": [explore_turn()], "develop": [develop1, develop2]}});
    let s = Scratch::new("test -f hello.txt && test -f fixed.txt", script);
    let d = s.start();
    let t = file_task(&d, "Conflicts with the base");
    wait_state(&d, t, State::Landed, 120);
    let w = d.world().unwrap();
    assert_eq!(w.tasks[&t].failures, 0, "a conflict is not a failure");
    assert_eq!(w.tasks[&t].attempts[&State::Developing], 2);
    assert_eq!(sh(&s.origin, &["show", "main:conflict.txt"]), "both");
    assert_eq!(sh(&s.origin, &["show", "main:fixed.txt"]), "ok");
}

#[test]
fn run_check_runs_the_test_command_the_owner_overrode_at_runtime() {
    // The owner's `mason config set step.test.command ...` used to reach only the test
    // step: the develop agent's run_check (and landing's legacy suite) kept running the
    // project's own command, outside any queue the override added.
    let develop = json!([
        {"sh": TASK_SCRIPT_COMMIT},
        {"call": {"tool": "run_check", "args": {"name": "tests"}}},
        {"call": {"tool": "submit_change", "args": {"summary": "ok", "head": "$HEAD"}}}
    ]);
    let s = Scratch::new(
        "echo FROM-THE-PROJECT; exit 1",
        json!({"steps": {"explore": [explore_turn()], "develop": [develop]}}),
    );
    let d = s.start();
    let now = mason_daemon::daemon::now_ms();
    d.store
        .add_override(
            "scratch",
            "step.test.command",
            "echo FROM-THE-OVERRIDE && test -f hello.txt",
            "owner",
            now,
            now + 3_600_000,
        )
        .unwrap();
    let t = file_task(&d, "Overridden tests");
    wait_state(&d, t, State::Landed, 90);
    let logs = render_logs(&d, t.0);
    assert!(logs.contains("FROM-THE-OVERRIDE"), "{logs}");
    assert!(!logs.contains("FROM-THE-PROJECT"), "{logs}");
}

#[test]
fn a_red_test_that_is_green_when_run_again_is_a_flake_not_a_red_round() {
    // The test command fails the first time it runs and passes the second: a flaky
    // test. With `test_reruns = 1` that is green, recorded as a flake, and the task
    // is not sent back to develop for a change that had nothing wrong with it.
    let s = Scratch::new("true", happy_script());
    let count = s.root.path().join("runs");
    let cmd = format!(
        "n=$(cat {c} 2>/dev/null || echo 0); echo $((n+1)) > {c}; test -f hello.txt && [ $n -ge 1 ]",
        c = count.display()
    );
    s.write_project("scratch", &cmd, "test_reruns = 1");
    let d = s.start();
    let t = file_task(&d, "Flaky once");
    wait_state(&d, t, State::Landed, 90);
    let w = d.world().unwrap();
    assert_eq!(w.tasks[&t].attempts[&State::Developing], 1, "not sent back");
    assert_eq!(w.tasks[&t].attempts[&State::Testing], 1, "one round");
    assert_eq!(std::fs::read_to_string(&count).unwrap().trim(), "2");
    let run = w
        .step_runs
        .values()
        .find(|r| r.task == t && r.step == State::Testing)
        .unwrap()
        .id;
    let flaky = std::fs::read_to_string(d.task_dir(t).join(format!("test-r{}.flaky", run.0)))
        .expect("the flake is recorded");
    assert!(flaky.contains("red, then green"), "{flaky}");
    let logs = render_logs(&d, t.0);
    assert!(logs.contains("run again (1)"), "{logs}");
}

#[test]
fn a_test_that_is_red_every_time_is_still_red_with_reruns() {
    let s = Scratch::new(
        "true",
        json!({"steps": {"explore": [explore_turn()], "develop": [develop_turn(), develop_turn()]}}),
    );
    let count = s.root.path().join("runs");
    let cmd = format!(
        "n=$(cat {c} 2>/dev/null || echo 0); echo $((n+1)) > {c}; echo nope; exit 1",
        c = count.display()
    );
    s.write_project("scratch", &cmd, "max_test_rounds = 2\ntest_reruns = 1");
    let d = s.start();
    let t = file_task(&d, "Really broken");
    wait_state(&d, t, State::Failed, 120);
    let w = d.world().unwrap();
    assert_eq!(w.tasks[&t].attempts[&State::Testing], 2);
    assert_eq!(
        std::fs::read_to_string(&count).unwrap().trim(),
        "4",
        "each of the two rounds ran it twice"
    );
    assert!(
        w.tasks[&t]
            .failure
            .as_deref()
            .unwrap_or("")
            .contains("red every time"),
        "{:?}",
        w.tasks[&t].failure
    );
    assert!(
        !s.origin_log().contains(&"add hello".to_string()),
        "nothing red may land"
    );
}

#[test]
fn the_test_step_folds_in_the_base_so_a_fix_that_landed_meanwhile_counts() {
    // The tests need base-fix.txt, which reaches the base while the task is in
    // develop (as a fix for a broken test does). The test step folds the base in
    // before it runs, so the task is green without another develop round.
    let base_moves = "tmp=$(mktemp -d) && git worktree add -q --detach \"$tmp\" origin/main && \
        (cd \"$tmp\" && echo fixed > base-fix.txt && git add -A && \
         git -c user.name=t -c user.email=t@t commit -qm 'fix on the base' && \
         git push -q origin HEAD:main) && git worktree remove --force \"$tmp\"";
    let develop = json!([
        {"sh": base_moves},
        {"sh": TASK_SCRIPT_COMMIT},
        {"call": {"tool": "submit_change", "args": {"summary": "ok", "head": "$HEAD"}}}
    ]);
    let s = Scratch::new(
        "test -f hello.txt && test -f base-fix.txt",
        json!({"steps": {"explore": [explore_turn()], "develop": [develop]}}),
    );
    let d = s.start();
    let t = file_task(&d, "Picks up the base");
    wait_state(&d, t, State::Landed, 90);
    let w = d.world().unwrap();
    assert_eq!(w.tasks[&t].attempts[&State::Developing], 1);
    assert_eq!(w.tasks[&t].attempts[&State::Testing], 1);
    assert_eq!(sh(&s.origin, &["show", "main:hello.txt"]), "hello");
}

/// A shell step that moves the base: `conflict.txt` there becomes `theirs-<pid>`.
const BASE_TAKES_CONFLICT_TXT: &str =
    "tmp=$(mktemp -d) && git worktree add -q --detach \"$tmp\" origin/main && \
    (cd \"$tmp\" && echo theirs-$$ > conflict.txt && git add -A && \
     git -c user.name=t -c user.email=t@t commit -qm 'base changes conflict.txt' && \
     git push -q origin HEAD:main) && git worktree remove --force \"$tmp\"";

#[test]
fn a_conflict_with_the_base_at_the_test_step_is_not_a_red_round() {
    // The base moves while the task is in develop, and the test step's merge of it
    // conflicts. With one test round allowed that used to fail the task, though its
    // tests were never red: tasks #17, #23 and #40 failed this way on 2026-11-07/08.
    let develop1 = json!([
        {"sh": format!("echo mine > conflict.txt && {TASK_SCRIPT_COMMIT}")},
        {"sh": BASE_TAKES_CONFLICT_TXT},
        {"call": {"tool": "submit_change", "args": {"summary": "first try", "head": "$HEAD"}}}
    ]);
    let develop2 = json!([
        {"sh": "git -c user.name=t -c user.email=t@t merge -q origin/main; echo both > conflict.txt && git add -A && \
                git -c user.name=t -c user.email=t@t commit -qm 'merge the base'"},
        {"call": {"tool": "submit_change", "args": {"summary": "merged", "head": "$HEAD"}}}
    ]);
    let s = Scratch::new(
        "true",
        json!({"steps": {"explore": [explore_turn()], "develop": [develop1, develop2]}}),
    );
    s.write_project("scratch", "test -f hello.txt", "max_test_rounds = 1");
    let d = s.start();
    let t = file_task(&d, "The base moved");
    wait_state(&d, t, State::Landed, 120);
    let w = d.world().unwrap();
    assert_eq!(w.tasks[&t].failures, 0);
    assert_eq!(w.tasks[&t].attempts[&State::Testing], 2);
    assert_eq!(w.tasks[&t].attempts[&State::Developing], 2);
    let conflicts = std::fs::read_to_string(d.task_dir(t).join("conflicts.txt")).unwrap();
    assert_eq!(conflicts.lines().count(), 1, "{conflicts}");
    assert!(conflicts.contains("conflict.txt"), "{conflicts}");
    assert_eq!(sh(&s.origin, &["show", "main:conflict.txt"]), "both");
}

#[test]
fn a_base_the_agent_never_catches_up_with_still_fails_the_task() {
    // Every develop round merges the base and the base moves again before the
    // test step: conflicts do not count as red rounds, but they are not endless.
    let turn = |first: bool| {
        let commit = if first {
            format!("echo mine-$$ > conflict.txt && {TASK_SCRIPT_COMMIT}")
        } else {
            "git -c user.name=t -c user.email=t@t merge -q origin/main; echo mine-$$ > conflict.txt && git add -A && \
             git -c user.name=t -c user.email=t@t commit -qm 'merge the base'"
                .to_string()
        };
        json!([
            {"sh": commit},
            {"sh": BASE_TAKES_CONFLICT_TXT},
            {"call": {"tool": "submit_change", "args": {"summary": "again", "head": "$HEAD"}}}
        ])
    };
    let develop: Vec<serde_json::Value> = (0..6).map(|i| turn(i == 0)).collect();
    let s = Scratch::new(
        "test -f hello.txt",
        json!({"steps": {"explore": [explore_turn()], "develop": develop}}),
    );
    let d = s.start();
    let t = file_task(&d, "The base keeps moving");
    wait_state(&d, t, State::Failed, 240);
    let w = d.world().unwrap();
    let failure = w.tasks[&t].failure.clone().unwrap_or_default();
    assert!(
        failure.contains("still conflicting with the base after 5 merges"),
        "{failure}"
    );
    assert_eq!(w.tasks[&t].attempts[&State::Testing], 6);
    let conflicts = std::fs::read_to_string(d.task_dir(t).join("conflicts.txt")).unwrap();
    assert_eq!(conflicts.lines().count(), 5, "{conflicts}");
    assert!(!s.origin_log().contains(&"add hello".to_string()));
}

#[test]
fn a_retried_task_gets_its_test_rounds_again() {
    // Red, red: the task fails after its two rounds. The owner retries it from the
    // test step. That run used to be its third round, failing at once on one more
    // red; now it is the first round of the retry.
    let s = Scratch::new(
        "true",
        json!({"steps": {"explore": [explore_turn()],
                         "develop": [develop_turn(), develop_turn(), develop_turn()]}}),
    );
    let count = s.root.path().join("runs");
    let cmd = format!(
        "n=$(cat {c} 2>/dev/null || echo 0); echo $((n+1)) > {c}; test -f hello.txt && [ $n -ge 3 ]",
        c = count.display()
    );
    s.write_project("scratch", &cmd, "max_test_rounds = 2");
    let d = s.start();
    let t = file_task(&d, "Retried");
    wait_state(&d, t, State::Failed, 120);
    assert_eq!(std::fs::read_to_string(&count).unwrap().trim(), "2");
    d.emit(EventKind::Retried {
        task: t,
        to: State::Testing,
        step: String::new(),
    })
    .unwrap();
    wait_state(&d, t, State::Landed, 120);
    let w = d.world().unwrap();
    assert_eq!(w.tasks[&t].attempts[&State::Testing], 4);
    assert_eq!(std::fs::read_to_string(&count).unwrap().trim(), "4");
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
    wait_for("the step host to stop", 60, || {
        mason_step_host::registry::Registry::new(d.cfg.hosts_dir())
            .scan()
            .unwrap()
            .iter()
            .all(|h| h.state != mason_step_host::registry::HostState::Running)
    });
    wait_for("the job to end", 60, || d.job_count() == 0);
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
    // The limit is on LLM sessions (Part 7): a task not yet claimed stays queued
    // and gets no worktree, and never more sessions are held than the limit.
    let (mut max_llm, mut max_unclaimed_step) = (0, 0);
    let mut queued_while_busy = false;
    wait_for("both to finish", 150, || {
        let w = d.world().unwrap();
        let llm = d.broker.used("llm:claude");
        max_llm = max_llm.max(llm);
        // Only agent steps hold a session (as `Daemon::llm_free` counts): a task in
        // its test or landing step holds none, by design, so counting those made
        // this read 2 whenever the second task was claimed while the first was
        // testing, a window that is wide only on a busy machine.
        max_unclaimed_step = max_unclaimed_step.max(
            w.tasks
                .values()
                .filter(|t| {
                    matches!(
                        t.state,
                        State::Exploring | State::Developing | State::Review
                    ) && !d.broker.has_lease(t.id)
                })
                .count(),
        );
        queued_while_busy |= llm == 1 && w.tasks.values().any(|t| t.state == State::Queued);
        ts.iter()
            .all(|t| w.tasks[t].state.is_terminal() || w.tasks[t].state == State::Failed)
    });
    assert_eq!(
        max_llm, 1,
        "exactly the limit, never more, was held at once"
    );
    assert!(
        max_unclaimed_step <= 1,
        "a claim is never handed out without a free session"
    );
    assert!(
        queued_while_busy,
        "the second task waited in the queue, unclaimed"
    );
    let w = d.world().unwrap();
    assert!(w.tasks[&ts[0]].landed.is_some(), "the first task lands");
}

#[test]
fn a_step_entered_again_with_uncommitted_work_does_not_merge_the_base_into_it() {
    // An agent edits README and has not committed when its step is entered again
    // (here a retry; in life the daemon restarting for a self-update), and meanwhile
    // the base changed README too. Merging the base into the uncommitted edit fails
    // ("your local changes would be overwritten"), which used to fail the task. Now the
    // step leaves the branch as it is, the agent commits, and the test step folds in
    // the base (the two edits touch different lines).
    let base_moves = "tmp=$(mktemp -d) && git worktree add -q --detach \"$tmp\" origin/main && \
        (cd \"$tmp\" && printf 'scratch\\nfrom the base\\n' > README && git add -A && \
         git -c user.name=t -c user.email=t@t commit -qm 'base edits README' && \
         git push -q origin HEAD:main) && git worktree remove --force \"$tmp\"";
    let develop = json!([
        [{"sh": base_moves}, {"sh": "printf 'from the agent\\nscratch\\n' > README"}, {"say": "working"}],
        [{"say": "still working"}],
        [{"say": "nearly"}],
        [{"sh": format!("git add README && {TASK_SCRIPT_COMMIT}")},
         {"call": {"tool": "submit_change", "args": {"summary": "ok", "head": "$HEAD"}}}]
    ]);
    let s = Scratch::new(
        "test -f hello.txt",
        json!({"steps": {"explore": [explore_turn()], "develop": develop}}),
    );
    let d = s.start();
    let t = file_task(&d, "Uncommitted work");
    wait_state(&d, t, State::Failed, 60);
    d.emit(EventKind::Retried {
        task: t,
        to: State::Developing,
        step: String::new(),
    })
    .unwrap();
    wait_state(&d, t, State::Landed, 90);
    assert_eq!(
        sh(&s.origin, &["show", "main:README"]),
        "from the agent\nscratch\nfrom the base"
    );
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
        step: String::new(),
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
