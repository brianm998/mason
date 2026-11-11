// SPDX-License-Identifier: GPL-3.0-or-later

//! An agent that goes quiet (s111): past `agent_idle_alert_s` an alert names the task, the
//! step and how long; past `agent_idle_stop_s` the session is stopped and continued like a lost
//! host, counting toward the same two restarts. A running `run_check` is not silence.

mod support;
use mason_core::State;
use mason_daemon::attention::alerts_now;
use mason_daemon::daemon::Daemon;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use support::*;

fn silent_alerts(d: &Arc<Daemon>) -> Vec<String> {
    alerts_now(d)
        .into_iter()
        .filter(|(k, _, _)| k.starts_with("silent:"))
        .map(|(_, _, text)| text)
        .collect()
}

fn world(develop: Vec<Value>, extra: &str, test_cmd: &str) -> Scratch {
    let script = json!({"steps": {"explore": [explore_turn()], "develop": develop}});
    let s = Scratch::new(test_cmd, script);
    s.write_project("scratch", test_cmd, extra);
    s
}

/// Samples the silence alerts until told to stop; counts the samples that had one.
fn sampler(d: &Arc<Daemon>, stop: Arc<AtomicBool>, seen: Arc<AtomicUsize>) {
    let d = d.clone();
    std::thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            // Only the develop step: the explore agent may be slow to print its first line.
            if silent_alerts(&d).iter().any(|t| t.contains("`develop`")) {
                seen.fetch_add(1, Ordering::SeqCst);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    });
}

fn quiet(secs: u32) -> Value {
    json!([{"say": "working"}, {"sh": format!("sleep {secs}")}])
}

#[test]
fn the_alert_names_the_silence_and_clears_when_output_resumes() {
    // Alert at 2 s, no stop: the agent is quiet for 5 s, then speaks and finishes.
    let develop = json!([
        {"say": "working"},
        {"sh": "sleep 5"},
        {"say": "back"},
        {"sh": TASK_SCRIPT_COMMIT},
        {"call": {"tool": "submit_change", "args": {"summary": "ok", "head": "$HEAD"}}}
    ]);
    let s = world(
        vec![develop],
        "agent_idle_alert_s = 2\nagent_idle_stop_s = 0",
        "true",
    );
    let d = s.start();
    let t = file_task(&d, "Goes quiet");
    let mut text = String::new();
    wait_for("the silence alert", 60, || {
        // The explore agent may be slow to print its first line; only develop is quiet on purpose.
        text = silent_alerts(&d)
            .into_iter()
            .find(|t| t.contains("`develop`"))
            .unwrap_or_default();
        !text.is_empty()
    });
    assert!(
        text.contains(&format!("Task #{}", t.0))
            && text.contains("`develop`")
            && text.contains("silent for"),
        "{text}"
    );
    wait_for("the alert to clear", 60, || {
        !silent_alerts(&d).iter().any(|t| t.contains("`develop`"))
    });
    wait_state(&d, t, State::Landed, 90);
    let hosts: Vec<_> = std::fs::read_dir(s.data_dir.join("h"))
        .unwrap()
        .flatten()
        .collect();
    assert!(
        hosts.iter().all(|h| !h.path().join("silent").exists()),
        "nothing was stopped"
    );
}

#[test]
fn a_silent_session_is_stopped_and_resumed_and_the_task_lands() {
    let s = world(
        vec![quiet(120), develop_turn()],
        "agent_idle_alert_s = 1\nagent_idle_stop_s = 5",
        "true",
    );
    let d = s.start();
    let t = file_task(&d, "Hangs once");
    wait_state(&d, t, State::Landed, 120);
    let w = d.world().unwrap();
    assert!(w.tasks[&t].failure.is_none());
    assert_eq!(
        w.tasks[&t]
            .attempts
            .get(&State::Developing)
            .copied()
            .unwrap_or(0),
        1
    );
    // Only the develop step is quiet on purpose; a slow explore agent may be stopped too.
    let losses: Vec<_> = d
        .store
        .host_losses_since(0)
        .unwrap()
        .into_iter()
        .filter(|l| l.step == "develop")
        .collect();
    assert_eq!(losses.len(), 1, "{losses:?}");
    assert!(losses[0].detail.contains("silence"), "{}", losses[0].detail);
    let argv = std::fs::read_to_string(format!("{}.argv", s.script.display())).unwrap();
    assert!(
        argv.lines()
            .any(|l| l.contains("--resume fake-claude-session")),
        "{argv}"
    );
    assert!(
        silent_alerts(&d).is_empty(),
        "the alert is gone with the session"
    );
}

#[test]
fn a_third_stall_fails_the_step_and_says_so() {
    let s = world(
        vec![quiet(120), quiet(120), quiet(120), develop_turn()],
        "agent_idle_alert_s = 0\nagent_idle_stop_s = 4",
        "true",
    );
    let d = s.start();
    let t = file_task(&d, "Always hangs");
    wait_state(&d, t, State::Failed, 120);
    let w = d.world().unwrap();
    let why = w.tasks[&t].failure.clone().unwrap_or_default();
    assert!(why.contains("3 times") && why.contains("silence"), "{why}");
}

#[test]
fn a_slow_run_check_is_not_silence() {
    // The check takes 8 s; both limits are shorter. Neither fires.
    let develop = json!([
        {"sh": TASK_SCRIPT_COMMIT},
        {"call": {"tool": "run_check", "args": {"name": "tests"}}},
        {"call": {"tool": "submit_change", "args": {"summary": "ok", "head": "$HEAD"}}}
    ]);
    let s = world(
        vec![develop],
        "agent_idle_alert_s = 2\nagent_idle_stop_s = 4",
        "sleep 8; test -f hello.txt",
    );
    let d = s.start();
    let (stop, seen) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicUsize::new(0)),
    );
    sampler(&d, stop.clone(), seen.clone());
    let t = file_task(&d, "Slow check");
    wait_state(&d, t, State::Landed, 120);
    stop.store(true, Ordering::SeqCst);
    assert_eq!(seen.load(Ordering::SeqCst), 0, "an alert was raised");
    assert!(
        d.store
            .host_losses_since(0)
            .unwrap()
            .iter()
            .all(|l| l.step != "develop"),
        "the develop session was stopped"
    );
}
