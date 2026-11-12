// SPDX-License-Identifier: GPL-3.0-or-later

//! The stall watcher (s152): a test run whose process tree goes silent has its stopped
//! processes continued, and one that stays silent is killed and judged red as stalled,
//! unless something in it waits on a lease. Thresholds are 1 s and the watcher reads every
//! 100 ms; every wait is on the run's own state, never on a fixed sleep.

mod support;
use mason_core::*;
use mason_daemon::daemon::Daemon;
use mason_daemon::sched::LeaseState;
use mason_resources::{Locks, Take};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use support::*;

fn script(dir: &Path, body: &str) -> String {
    let p = dir.join("t.sh");
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p.display().to_string()
}

fn world(dir: &Path, body: &str, extra: &str) -> (Scratch, Arc<Daemon>) {
    let s = Scratch::new("true", happy_script());
    s.write_project("scratch", &script(dir, body), extra);
    let d = s.start_with(|c| c.watch_tick = Duration::from_millis(100));
    (s, d)
}

/// Everything the task says about itself: its events (the red's reason is in one), its
/// failure and its logs.
fn dump(d: &Arc<Daemon>, t: TaskId) -> String {
    let evs: Vec<String> = d
        .store
        .read_since(0, 1000)
        .unwrap()
        .iter()
        .map(|e| format!("{:?}", e.event.kind))
        .collect();
    format!(
        "{:?}\n{}\n{}",
        task_of(d, t).failure,
        evs.join("\n"),
        mason_daemon::http::render_logs(d, t.0)
    )
}

#[test]
fn a_stopped_grandchild_is_continued_and_the_run_completes() {
    let work = tempfile::tempdir().unwrap();
    let (_s, d) = world(
        work.path(),
        &format!(
            "[ -e {m} ] && exit 0; touch {m}; sh -c 'kill -STOP $$; echo resumed'",
            m = work.path().join("stopped-once").display()
        ),
        "cont_after_s = 1",
    );
    let t = file_task(&d, "Stops its child");
    wait_state(&d, t, State::Landed, 120);
    assert_eq!(d.stall_conts.load(Ordering::SeqCst), 1);
    assert_eq!(d.stall_kills.load(Ordering::SeqCst), 0);
}

#[test]
fn a_silent_tree_with_no_cpu_is_killed_and_the_result_says_stalled() {
    let work = tempfile::tempdir().unwrap();
    let (_s, d) = world(work.path(), "sleep 600", "kill_after_s = 1");
    let t = file_task(&d, "Goes silent");
    wait_for("the stall kill", 120, || {
        d.stall_kills.load(Ordering::SeqCst) >= 1
    });
    wait_for("the reason to be recorded", 120, || {
        dump(&d, t).contains("stalled: no output or CPU for")
    });
    assert!(!dump(&d, t).contains("ran out of time"), "{}", dump(&d, t));
}

#[test]
fn a_tree_waiting_on_a_lease_is_never_killed_while_it_waits() {
    let work = tempfile::tempdir().unwrap();
    let pid_file = work.path().join("pid");
    let sleep_file = work.path().join("sleeper");
    let (_s, d) = world(
        work.path(),
        &format!(
            "echo $$ > {p}.tmp; mv {p}.tmp {p}; sleep 600 & echo $! > {s}; wait",
            p = pid_file.display(),
            s = sleep_file.display()
        ),
        "kill_after_s = 1",
    );
    // Someone else holds the only `widget`; our request waits behind it.
    let one = [("widget".to_string(), 1)].into();
    let holder = d.broker.lease_open("holder", &one);
    let (locks_dir, take, caps) = loop {
        if let LeaseState::Granted {
            classes,
            exclusive,
            capacity,
            locks_dir,
        } = d.broker.lease_poll(holder)
        {
            break (locks_dir, Take { classes, exclusive }, capacity);
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let held = Locks::new(locks_dir)
        .unwrap()
        .take(holder, "holder", 0, &take, &caps)
        .unwrap();
    d.broker.lease_poll(holder);

    let t = file_task(&d, "Waits its turn");
    wait_for("the script to start", 120, || pid_file.exists());
    let pid: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let waiter = d.broker.lease_open("nested `mason run`", &one);
    d.broker.lease_set_pid(waiter, pid);
    // Keep both requests alive (polling is their heartbeat) until the watcher has looked at
    // the silent tree and spared it.
    wait_for("the watcher to spare the tree", 120, || {
        d.broker.lease_poll(holder);
        assert!(matches!(
            d.broker.lease_poll(waiter),
            LeaseState::Waiting { .. }
        ));
        d.stall_spared.load(Ordering::SeqCst) >= 1
    });
    assert_eq!(d.stall_kills.load(Ordering::SeqCst), 0);
    assert_eq!(state_of(&d, t), State::Testing, "{}", dump(&d, t));
    // The turn comes and the run finishes.
    drop(held);
    d.broker.lease_close(holder);
    d.broker.lease_close(waiter);
    wait_for("the sleeper's pid", 60, || sleep_file.exists());
    let sleeper = std::fs::read_to_string(&sleep_file).unwrap();
    let _ = std::process::Command::new("kill")
        .arg(sleeper.trim())
        .status();
    wait_state(&d, t, State::Landed, 120);
    assert_eq!(d.stall_kills.load(Ordering::SeqCst), 0);
}
