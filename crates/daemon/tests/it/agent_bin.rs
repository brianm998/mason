// SPDX-License-Identifier: GPL-3.0-or-later

//! An agent CLI that cannot be started holds its project with a notice instead of failing its
//! tasks (s1287).

#![cfg(unix)]

use crate::support;
use mason_core::*;
use mason_daemon::{attention, diagnose, Daemon};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use support::*;

/// Point the project's `claude_bin` at `bin` (the file is not reloaded here).
fn point_at(s: &Scratch, bin: &Path) {
    let file = s.config_dir.join("projects/scratch.toml");
    let text = std::fs::read_to_string(&file).unwrap();
    let text: Vec<String> = text
        .lines()
        .map(|l| match l.starts_with("claude_bin = ") {
            true => format!("claude_bin = \"{}\"", bin.display()),
            false => l.to_string(),
        })
        .collect();
    std::fs::write(&file, text.join("\n") + "\n").unwrap();
}

fn fake() -> PathBuf {
    bin("mason-adapters", "fake-claude")
}

/// A file that looks executable and cannot be started: its interpreter, a relative path, is
/// nowhere, so only the launch finds out. (A script: nothing unsigned is run.)
fn unstartable(s: &Scratch) -> PathBuf {
    let p = s.root.path().join("broken-claude");
    std::fs::write(&p, "#!no-such-interpreter\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn notice(d: &Daemon) -> Option<(String, String, String)> {
    attention::alerts_now(d)
        .into_iter()
        .find(|a| a.0 == "agent-bin:scratch")
}

fn step_failures(d: &Daemon, t: TaskId) -> usize {
    d.store
        .read_since(0, 100_000)
        .unwrap()
        .iter()
        .filter(|e| matches!(&e.event.kind, EventKind::StepFailed { task, .. } if *task == t))
        .count()
}

/// How many step hosts were ever asked for on behalf of `t`.
fn launches(d: &Daemon, t: TaskId) -> usize {
    let prefix = format!("t{}r", t.0);
    std::fs::read_dir(d.cfg.hosts_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.starts_with(&prefix) && n.ends_with(".in")
        })
        .count()
}

fn reload(d: &Arc<Daemon>) {
    d.reload_registration("scratch").unwrap();
    d.admit();
}

#[test]
fn a_project_whose_agent_does_not_exist_is_held_until_a_reload_finds_it() {
    let s = Scratch::new("true", happy_script());
    point_at(&s, Path::new("/nonexistent/claude"));
    let d = s.start();
    let t = file_task(&d, "Wait for the agent");
    d.admit();
    assert_eq!(state_of(&d, t), State::Queued);
    let w = d.world().unwrap();
    assert!(w.step_runs.values().all(|r| r.task != t), "no step ran");
    let says = d.queued_wait_text(&w, &w.tasks[&t]).unwrap();
    assert!(says.contains("the agent CLI cannot be started"), "{says}");
    let (_, kind, text) = notice(&d).expect("one notice");
    assert_eq!(kind, "needs_you");
    for want in [
        "scratch",
        "claude_bin",
        "projects/scratch.toml",
        "/nonexistent/claude",
    ] {
        assert!(text.contains(want), "{want} in {text}");
    }
    // No diagnosis can start either, so none of the budget goes.
    assert!(d.store.diagnoses(Some(t.0)).unwrap().is_empty());

    point_at(&s, &fake());
    reload(&d);
    assert!(notice(&d).is_none(), "the notice clears by itself");
    wait_state(&d, t, State::Landed, 180);
    assert_eq!(step_failures(&d, t), 0);
}

#[test]
fn a_launch_that_fails_to_start_is_not_retried_or_counted() {
    let s = Scratch::new("true", happy_script());
    let broken = unstartable(&s);
    point_at(&s, &broken);
    let d = s.start();
    let t = file_task(&d, "The CLI will not start");
    wait_for("the hold", 120, || {
        let k = task_of(&d, t);
        assert_ne!(k.state, State::Failed, "{:?}", k.failure);
        matches!(&k.activity, Activity::Waiting(WaitReason::On(on))
            if on.contains("failed to start"))
    });
    assert_eq!(launches(&d, t), 1, "not tried again, on any model");
    assert_eq!(step_failures(&d, t), 0);
    assert_eq!(task_of(&d, t).failures, 0);
    assert!(notice(&d).unwrap().2.contains("failed to start"));
    // Looking at the same file again does not lift a hold a launch set.
    mason_daemon::agentbin::check_all(&d);
    assert!(notice(&d).is_some());
    // Nor is another task claimed meanwhile.
    let second = file_task(&d, "Behind it");
    d.admit();
    assert_eq!(state_of(&d, second), State::Queued);

    point_at(&s, &fake());
    reload(&d);
    wait_state(&d, t, State::Landed, 180);
    assert_eq!(step_failures(&d, t), 0);
}

#[test]
fn a_diagnosis_session_that_cannot_start_spends_no_budget() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    s.write_project(
        "scratch",
        "test -f hello.txt",
        "auto_retry = false\ndiagnose = false",
    );
    let hook = s.origin.join("hooks/pre-receive");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let d = s.start();
    let t = file_task(&d, "Push is rejected");
    wait_for("the failure", 180, || state_of(&d, t) == State::Failed);
    let run = StepRunId(
        d.world()
            .unwrap()
            .step_runs
            .values()
            .filter(|r| r.task == t)
            .map(|r| r.id.0)
            .max()
            .unwrap(),
    );
    point_at(&s, &unstartable(&s));
    reload(&d);
    diagnose::start(&d, t, run, true).expect("the file looks runnable");
    wait_for("the session to end", 120, || {
        notice(&d).is_some() && diagnose::running(&d, t.0).is_none()
    });
    assert!(d.store.diagnoses(Some(t.0)).unwrap().is_empty());
    let refused = diagnose::start(&d, t, run, true).unwrap_err();
    assert!(refused.contains("cannot be started"), "{refused}");
}

#[test]
fn doctor_reports_what_the_running_daemon_resolves() {
    let s = Scratch::new("true", happy_script());
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    let out = s.mason_ok(&["doctor"]);
    assert!(out.contains("masond runs the agent binary"), "{out}");
    // The file on disk now names a missing binary, but the daemon still runs the old one:
    // doctor asks the daemon.
    point_at(&s, Path::new("/nonexistent/claude"));
    let out = s.mason_ok(&["doctor"]);
    assert!(out.contains("masond runs the agent binary"), "{out}");
    s.mason_ok(&["project", "reload", "--project", "scratch"]);
    let o = s.mason(&["doctor"]);
    assert!(!o.status.success());
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("masond cannot run the agent binary: /nonexistent/claude does not exist"),
        "{out}"
    );
}
