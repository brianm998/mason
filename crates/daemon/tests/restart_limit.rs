// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg(unix)]
// Unix-only: signals, permission bits or unix sockets. Windows coverage comes with the later port tasks.

//! A step's time limit counts from when its host started, not from when the daemon
//! last attached (s84). The daemon is killed part-way through a 15-second limit and
//! restarted; the step is stopped by the timer, and not before its original limit ran out.
//! There is no upper bound on the wall clock: how late the host is stopped depends on how
//! long the daemon takes to start, which a loaded machine stretches. A clock begun at the
//! restart is covered by the unit tests on `remaining_at`.

mod support;
use mason_step_host::registry::{ExitRecord, HostInfo};
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;
use support::*;

/// The limit both tests set.
const LIMIT_MS: u64 = 15_000;
/// How far into the host's life the daemon is killed.
const KILL_AT_MS: u64 = 10_000;

/// Develop with a short budget; the budget, not `step_timeout_s`, is the limit.
const DEF: &str = r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
budget = "15s"
next = "test"

[step.test]
type = "test"
category = "testing"
max_rounds = 3
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

fn kill9(m: &mut Masond) {
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(m.child.id() as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    let _ = m.child.wait();
}

fn find_host(s: &Scratch, test: bool) -> PathBuf {
    let hosts = s.data_dir.join("h");
    let mut found = None;
    wait_for("the step host", 60, || {
        for e in std::fs::read_dir(&hosts).into_iter().flatten().flatten() {
            let id = e.file_name().to_string_lossy().to_string();
            if id.ends_with("-test") == test
                && HostInfo::read(&e.path()).is_ok()
                && ExitRecord::read(&e.path()).is_err()
            {
                found = Some(e.path());
                return true;
            }
        }
        false
    });
    found.unwrap()
}

/// Kill the daemon part-way through the host's life, restart it, and check when the host
/// ended. The host must have been stopped by the timer (its command never ends by itself) and
/// not before `started + LIMIT_MS`. No upper bound is checked, since that would measure how
/// long the daemon took to start.
fn run_on_the_original_clock(s: &Scratch, test: bool) {
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    s.mason_ok(&["task", "add", "Hangs"]);
    s.wait_task(0, if test { "testing" } else { "developing" }, 60);
    let dir = find_host(s, test);
    let started = HostInfo::read(&dir).unwrap().started_ms;
    loop {
        let now = mason_step_host::registry::now_ms();
        if now >= started + KILL_AT_MS {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    kill9(&mut m);
    assert!(
        ExitRecord::read(&dir).is_err(),
        "the host must still be running"
    );
    let mut m2 = s.masond(&[], &[]);
    s.wait_daemon(&mut m2);
    wait_for("the host to be stopped", 60, || {
        ExitRecord::read(&dir).is_ok()
    });
    let rec = ExitRecord::read(&dir).unwrap();
    let ended = rec.ended_ms;
    drop(m2);
    assert!(
        ended >= started + LIMIT_MS - 100,
        "ended {} ms after the host started, before its limit ran out",
        ended - started
    );
    assert!(
        rec.signal != 0,
        "the host ended by itself, not by the timer: {rec:?}"
    );
}

#[test]
fn an_agent_step_times_out_on_the_original_clock_after_a_restart() {
    let script = json!({"steps": {"develop": [[{"sleep_ms": 600000}]]}});
    let s = Scratch::new("true", script);
    s.push_files(&[(".mason/mason.toml", DEF)], "definition");
    run_on_the_original_clock(&s, false);
}

#[test]
fn a_test_step_times_out_on_the_original_clock_after_a_restart() {
    let s = Scratch::new("sleep 60", happy_script());
    let p = s.config_dir.join("projects/scratch.toml");
    let t = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, t.replace("test_timeout_s = 60", "test_timeout_s = 15")).unwrap();
    run_on_the_original_clock(&s, true);
}
