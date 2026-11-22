// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg(unix)]
// Unix-only: process groups and signals.

//! A `mason run` whose caller has gone, or whose task's step has ended, is dropped (s688).

use crate::support;
use mason_core::TaskId;
use mason_daemon::sched::LeaseState;
use serde_json::{json, Value};
use std::process::{Command, Stdio};
use support::*;

fn compile() -> std::collections::BTreeMap<String, u32> {
    [("compile".to_string(), 1)].into()
}

fn lease_of_task(d: &std::sync::Arc<mason_daemon::Daemon>, label: &str, task: u64) -> u64 {
    let id = d.broker.lease_open_from(
        label,
        &compile(),
        Some(mason_daemon::sched::Standing {
            steps_remaining: 3,
            span: 1,
            priority: 1,
        }),
        None,
        Some("task"),
    );
    d.broker.lease_set_task(id.id, TaskId(task));
    id.id
}

#[test]
fn a_step_that_ends_drops_its_waiting_run_and_the_next_waiter_is_first() {
    let s = Scratch::new("true", json!({"steps": {}}));
    s.write_project("scratch", "true", "");
    let d = s.start();
    let holder = d.broker.lease_open("holder", &compile());
    assert!(matches!(
        d.broker.lease_poll(holder),
        LeaseState::Granted { .. }
    ));
    let gone = lease_of_task(&d, "task 669 `gradlew`", 669);
    let other_task = lease_of_task(&d, "task 5 `cargo test`", 5);
    // A person's request (no task) is never touched by a step ending.
    let person = d.broker.lease_open("interactive `make`", &compile());
    mason_daemon::jobs::end_run_leases(&d, TaskId(669));
    assert!(matches!(d.broker.lease_poll(gone), LeaseState::Gone));
    assert!(matches!(
        d.broker.lease_poll(other_task),
        LeaseState::Waiting { .. }
    ));
    assert!(matches!(
        d.broker.lease_poll(person),
        LeaseState::Waiting { .. }
    ));
    assert!(d.broker.lease_holds(holder), "the holder did not finish");
    // Closing the holder grants the person first (interactive), proving the line moved.
    d.broker.lease_close(holder);
    assert!(matches!(
        d.broker.lease_poll(person),
        LeaseState::Granted { .. }
    ));
    d.broker.lease_close(person);
    d.broker.lease_close(other_task);
}

#[test]
fn a_granted_run_of_an_ended_step_has_its_command_stopped_and_its_units_freed() {
    let s = Scratch::new("true", json!({"steps": {}}));
    s.write_project("scratch", "true", "");
    let d = s.start();
    // `sh` stands for the `mason run` client and its `sleep` for the command it runs.
    let mut client = Command::new("sh")
        .args(["-c", "sleep 300 & wait"])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let pid = client.id();
    let id = lease_of_task(&d, "task 669 `gradlew createDistributable`", 669);
    d.broker.lease_set_pid(id, pid);
    assert!(matches!(
        d.broker.lease_poll(id),
        LeaseState::Granted { .. }
    ));
    let next = d.broker.lease_open("next", &compile());
    assert!(matches!(
        d.broker.lease_poll(next),
        LeaseState::Waiting { .. }
    ));
    mason_daemon::jobs::end_run_leases(&d, TaskId(669));
    // Keep `next` polled through every wait: the broker reaps a request nobody polls. A grant
    // seen here counts: `next` never takes its kernel lock, so the broker reaps the grant 5 s
    // later, and on a busy machine the `ps` wait below outlasts that. It then read as "dropped".
    let granted = std::cell::Cell::new(false);
    let poll_next = || {
        if matches!(d.broker.lease_poll(next), LeaseState::Granted { .. }) {
            granted.set(true);
        }
    };
    wait_for("the units to be freed", 30, || {
        poll_next();
        !d.broker.lease_holds(id)
    });
    wait_for("the command to end", 30, || {
        poll_next();
        !mason_daemon::hung::read_ps().iter().any(|p| p.ppid == pid)
    });
    // The grant may wait on the CPU gate or the settle time; wait on the state, not on time.
    let mut last = String::new();
    wait_poll("the next waiter to be granted", 60, || {
        if granted.get() {
            return Poll::Done;
        }
        match d.broker.lease_poll(next) {
            LeaseState::Granted { .. } => Poll::Done,
            LeaseState::Gone => Poll::Never(format!(
                "the request was dropped, not granted (last explain: {last})"
            )),
            LeaseState::Waiting { explain } => {
                last = explain;
                Poll::Pending
            }
        }
    });
    d.broker.lease_close(next);
    let _ = client.kill();
    let _ = client.wait();
}

fn resources(s: &Scratch) -> Value {
    serde_json::from_str(&s.mason_ok(&["resources", "--json"])).unwrap()
}

fn waiting(s: &Scratch) -> usize {
    resources(s)["waiting"].as_array().map_or(0, Vec::len)
}

/// The command as one shell word list.
fn shell_words(c: &Command) -> String {
    std::iter::once(c.get_program())
        .chain(c.get_args())
        .map(|a| format!("'{}'", a.to_string_lossy().replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn a_mason_run_whose_parent_exits_drops_its_waiting_request() {
    let s = Scratch::new("true", json!({"steps": {}}));
    s.write_project("scratch", "true", "");
    let mason = bin("mason-pipeline", "mason");
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    s.mason_ok(&["resources", "limit", "compile", "1"]);
    let run = |args: &[&str]| {
        let mut c = Command::new(&mason);
        c.args(["--data-dir", &s.data_dir.display().to_string()])
            .args(["--config-dir", &s.config_dir.display().to_string()])
            .args(["run", "--needs", "compile"])
            .args(args);
        c
    };
    // The one holder of the only unit, which never finishes during the test.
    let mut holder = run(&["--", "sleep", "300"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for("the holder to hold compile", 60, || {
        resources(&s)["holders"]
            .as_array()
            .is_some_and(|h| !h.is_empty())
    });
    // The abandoned request: its parent shell is killed while it waits.
    let dir = tempfile::tempdir().unwrap();
    let (pidfile, errfile) = (dir.path().join("pid"), dir.path().join("err"));
    let script = format!(
        "{} >/dev/null 2>{} & echo $! > {}; wait",
        shell_words(&run(&["--", "true"])),
        errfile.display(),
        pidfile.display()
    );
    let mut shell = Command::new("sh")
        .args(["-c", &script])
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    wait_for("the request to wait", 60, || waiting(&s) == 1);
    let waiter: u32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // A second, live waiter.
    let mut second = run(&["--", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for("two requests to wait", 60, || waiting(&s) == 2);
    shell.kill().unwrap();
    let _ = shell.wait();
    wait_for("the abandoned request to be dropped", 60, || {
        waiting(&s) == 1 && !mason_platform::process::exists(waiter)
    });
    let said = std::fs::read_to_string(&errfile).unwrap();
    assert!(
        said.contains("the command that asked for this has gone"),
        "{said}"
    );
    // The second waiter is first in line, and the holder never finished.
    let v = resources(&s);
    let why = v["waiting"][0]["explain"].as_str().unwrap();
    assert!(why.contains("1st in line"), "{v}");
    assert!(holder.try_wait().unwrap().is_none());
    let _ = holder.kill();
    let _ = holder.wait();
    let _ = second.kill();
    let _ = second.wait();
}
