// SPDX-License-Identifier: GPL-3.0-or-later

//! Crash injection (Part 23). The real `masond` is killed, uncleanly, at every
//! effect boundary on the way from queued to landed, then restarted. In every
//! case the task must still land, exactly once, with nothing lost and nothing
//! done twice. Plus real `kill -9` while an agent session and a test run are
//! in flight.

mod support;
use mason_core::*;
use mason_store::Store;
use serde_json::{json, Value};
use std::os::unix::process::ExitStatusExt;
use std::time::{Duration, Instant};
use support::*;

/// Every named boundary the happy path crosses (`@n` is the n-th time).
const POINTS: &[&str] = &[
    "after_append:TaskFiled",
    "after_append:Admitted",
    "job_started:Exploring",
    "accepted_persisted@1", // explore submission durable, the agent not yet told
    "before_event:Exploring",
    "after_append:StepSucceeded@1", // explore accepted, nothing after it has happened
    "job_started:Developing",
    "accepted_persisted@2", // develop submission durable, the agent not yet told
    "before_event:Developing",
    "after_append:StepSucceeded@2", // develop accepted
    "job_started:Testing",
    "before_event:Testing",
    "after_append:StepSucceeded@3", // tests green
    "job_started:Landing",
    "land_after_fetch",
    "land_after_ff",
    "land_after_push",              // pushed to the remote, not yet recorded
    "land_before_event",            // same, after the landing call returned
    "after_append:StepSucceeded@4", // recorded as landed, cleanup not done
    "land_after_event",
];

fn died_of_sigkill(m: &mut Masond, secs: u64) -> Result<(), String> {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(st) = m.child.try_wait().map_err(|e| e.to_string())? {
            return if st.signal() == Some(9) {
                Ok(())
            } else {
                Err(format!("exited with {st} instead of being killed"))
            };
        }
        if Instant::now() > end {
            return Err("the injected crash never happened".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What must be true once a task has landed after a crash.
fn check_landed_exactly_once(s: &Scratch, task: u64) -> Result<(), String> {
    let log = s.origin_log();
    let n = log.iter().filter(|m| *m == "add hello").count();
    if n != 1 {
        return Err(format!("the commit is on the remote {n} times: {log:?}"));
    }
    // Stop the daemon, then read the event log it left.
    let store = Store::open(s.data_dir.join("store.db")).map_err(|e| e.to_string())?;
    store
        .verify()
        .map_err(|e| format!("log and projections disagree: {e}"))?;
    let kinds: Vec<String> = store
        .read_since(0, 10_000)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter_map(|e| {
            let k = format!("{:?}", e.event.kind);
            let ours = k.contains(&format!("TaskId({task})")) || k.starts_with("TaskFiled");
            ours.then(|| k.split([' ', '{']).next().unwrap_or("").to_string())
        })
        .collect();
    let want = [
        "TaskFiled",
        "Admitted",
        "StepSucceeded",
        "StepSucceeded",
        "StepSucceeded",
        "StepSucceeded",
    ];
    if kinds != want {
        return Err(format!(
            "the task's events are not exactly one pass through the graph: {kinds:?}"
        ));
    }
    let w = store.world().map_err(|e| e.to_string())?;
    let t = &w.tasks[&TaskId(task)];
    if t.landed != Some(LandedOutcome::Merged) || t.failures != 0 {
        return Err(format!(
            "wrong final state: {:?}, failures {}",
            t.landed, t.failures
        ));
    }
    if w.step_runs.values().any(|r| r.outcome.is_none()) {
        return Err("a step run was left open".into());
    }
    let bugs = store.bugs().map_err(|e| e.to_string())?;
    if !bugs.is_empty() {
        return Err(format!(
            "recovery should be a normal path, but bugs were filed: {bugs:?}"
        ));
    }
    Ok(())
}

fn crash_at(point: &str) -> Result<(), String> {
    let s = Scratch::new("test -f hello.txt", happy_script());
    let mut first = s.masond(&[], &[("MASON_CRASH_AT", point)]);
    // Some points are hit before the daemon answers anything; that is fine.
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(60)
        && first.child.try_wait().ok().flatten().is_none()
        && !s.mason(&["status", "--json"]).status.success()
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    // File the task. If the daemon dies mid-request the answer is lost, not the task.
    let _ = s.mason(&[
        "task",
        "add",
        "Add a hello file",
        "--body",
        "Create hello.txt.",
    ]);
    died_of_sigkill(&mut first, 60)?;
    // Not dropped yet: dropping it would kill the step hosts that must survive.
    let _first = first;

    let mut second = s.masond(&[], &[]);
    let started = Instant::now();
    while !s.mason(&["status", "--json"]).status.success() {
        if Instant::now() - started > Duration::from_secs(60) {
            return Err("the restarted daemon never answered".into());
        }
        if let Some(st) = second.child.try_wait().map_err(|e| e.to_string())? {
            return Err(format!("the restarted daemon exited: {st}"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // If the crash came before the task was recorded, the client simply files it again.
    let listed: Value = serde_json::from_str(&s.mason_ok(&["task", "list", "--json"]))
        .map_err(|e| e.to_string())?;
    if listed["tasks"].as_array().is_none_or(|t| t.is_empty()) {
        s.mason_ok(&[
            "task",
            "add",
            "Add a hello file",
            "--body",
            "Create hello.txt.",
        ]);
    }
    let end = Instant::now() + Duration::from_secs(120);
    loop {
        let o = s.mason(&["task", "show", "0", "--json"]);
        let v: Value = serde_json::from_slice(&o.stdout).unwrap_or(Value::Null);
        match v["state"].as_str() {
            Some("landed") => break,
            Some(st @ ("failed" | "canceled")) => {
                let mut logs = String::from_utf8_lossy(&s.mason(&["logs", "0"]).stdout).to_string();
                for h in mason_step_host::registry::Registry::new(s.data_dir.join("h"))
                    .scan()
                    .unwrap_or_default()
                {
                    logs.push_str(&format!(
                        "HOST {} state {:?} journal {} hb {:?}\n",
                        h.info.id, h.state, h.journal_len, h.last_heartbeat_ms
                    ));
                }
                for e in std::fs::read_dir(s.data_dir.join("h"))
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    logs.push_str(&format!("ENTRY {:?}\n", e.file_name()));
                }
                return Err(format!(
                    "the task ended {st} after the restart: {}\nLOGS:\n{logs}",
                    v["failure"]
                ));
            }
            _ => {}
        }
        if Instant::now() > end {
            return Err(format!("the task never landed after the restart: {v:#}"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Cleanup after landing may be done by recovery: give it a moment, then look.
    let p = "mason/scratch/t0".to_string();
    let end = Instant::now() + Duration::from_secs(30);
    while mason_landing::git::branch_exists(&s.repo, &p) || s.data_dir.join("w/scratch/0").exists()
    {
        if Instant::now() > end {
            return Err("the landed task's branch or worktree was never cleaned up".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Stop the daemon so the store can be opened, and check everything.
    drop(second);
    kill_hosts(&s.data_dir);
    check_landed_exactly_once(&s, 0)
}

#[test]
fn killing_the_daemon_at_every_effect_boundary_loses_nothing_and_repeats_nothing() {
    let mut failures: Vec<String> = vec![];
    // Seven at a time: each scenario has its own repository, data directory and daemon.
    for chunk in POINTS.chunks(7) {
        let results: Vec<(String, Result<(), String>)> = std::thread::scope(|sc| {
            let hs: Vec<_> = chunk
                .iter()
                .map(|p| (p.to_string(), sc.spawn(move || crash_at(p))))
                .collect();
            hs.into_iter()
                .map(|(p, h)| {
                    (
                        p,
                        h.join()
                            .unwrap_or_else(|_| Err("the scenario panicked".into())),
                    )
                })
                .collect()
        });
        for (p, r) in results {
            match r {
                Ok(()) => eprintln!("ok   {p}"),
                Err(e) => failures.push(format!("{p}: {e}")),
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// A develop step that takes a few seconds, so there is time to kill the daemon in the middle of it.
fn slow_develop_script() -> Value {
    json!({"steps": {"explore": [explore_turn()], "develop": [[
        {"sh": TASK_SCRIPT_COMMIT},
        {"sleep_ms": 4000},
        {"call": {"tool": "submit_change", "args": {"summary": "done after a wait", "head": "$HEAD"}}}
    ]]}})
}

fn running_hosts(s: &Scratch) -> Vec<mason_step_host::registry::HostStatus> {
    mason_step_host::registry::Registry::new(s.data_dir.join("h"))
        .scan()
        .unwrap()
        .into_iter()
        .filter(|h| h.state == mason_step_host::registry::HostState::Running)
        .collect()
}

#[test]
fn kill_9_in_the_middle_of_an_agent_session_loses_nothing() {
    let s = Scratch::new("test -f hello.txt", slow_develop_script());
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    s.mason_ok(&["task", "add", "Slow develop"]);
    s.wait_task(0, "developing", 60);
    wait_for("the agent's commit", 30, || {
        mason_landing::git::commits_ahead(&s.repo, "origin/main", "mason/scratch/t0").unwrap_or(0)
            >= 1
    });

    // kill -9 the daemon while the agent is mid-session.
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(m.child.id() as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    let _ = m.child.wait();
    let hosts = running_hosts(&s);
    assert!(
        !hosts.is_empty(),
        "the agent's step host must outlive the daemon"
    );
    assert!(hosts
        .iter()
        .all(|h| mason_step_host::registry::pid_alive(h.info.host_pid)));

    // A second death straight after a restart, before it has finished recovering.
    let mut m2 = s.masond(&[], &[]);
    s.wait_daemon(&mut m2);
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(m2.child.id() as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    let _ = m2.child.wait();

    let mut m3 = s.masond(&[], &[]);
    s.wait_daemon(&mut m3);
    s.wait_task(0, "landed", 120);
    let log = s.origin_log();
    assert_eq!(
        log.iter().filter(|m| *m == "add hello").count(),
        1,
        "{log:?}"
    );
    let t = s.task_json(0);
    // One develop step run, however many times the daemon died: the same session carried on.
    let develops = t["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["step"] == "developing")
        .count();
    assert_eq!(develops, 1, "{t:#}");
    drop(m3);
    kill_hosts(&s.data_dir);
    let store = Store::open(s.data_dir.join("store.db")).unwrap();
    store.verify().unwrap();
    assert!(
        store.bugs().unwrap().is_empty(),
        "{:?}",
        store.bugs().unwrap()
    );
}

#[test]
fn kill_9_in_the_middle_of_a_test_run_loses_nothing() {
    // The test command takes a few seconds; the daemon dies while it runs.
    let s = Scratch::new("sleep 4; test -f hello.txt", happy_script());
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    s.mason_ok(&["task", "add", "Slow tests"]);
    s.wait_task(0, "testing", 60);
    wait_for("the test host", 20, || {
        running_hosts(&s)
            .iter()
            .any(|h| h.info.id.ends_with("-test"))
    });
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(m.child.id() as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    let _ = m.child.wait();
    assert!(
        running_hosts(&s)
            .iter()
            .any(|h| h.info.id.ends_with("-test")),
        "the test run survives the daemon"
    );

    let mut m2 = s.masond(&[], &[]);
    s.wait_daemon(&mut m2);
    s.wait_task(0, "landed", 120);
    let t = s.task_json(0);
    let tests = t["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["step"] == "testing")
        .count();
    assert_eq!(
        tests, 1,
        "the same test run was reattached, not repeated: {t:#}"
    );
    assert_eq!(
        s.origin_log().iter().filter(|m| *m == "add hello").count(),
        1
    );
}

#[test]
fn a_task_that_finished_while_the_daemon_was_down_is_picked_up_on_restart() {
    // The agent finishes its work and submits while nobody is listening; it waits (retrying)
    // for the daemon, and the session resumes the moment a new daemon is up.
    let s = Scratch::new("test -f hello.txt", slow_develop_script());
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    s.mason_ok(&["task", "add", "Finishes while down"]);
    s.wait_task(0, "developing", 60);
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(m.child.id() as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    let _ = m.child.wait();
    std::thread::sleep(Duration::from_secs(6)); // longer than the agent's remaining work
    let mut m2 = s.masond(&[], &[]);
    s.wait_daemon(&mut m2);
    s.wait_task(0, "landed", 120);
    assert_eq!(
        s.origin_log().iter().filter(|m| *m == "add hello").count(),
        1
    );
}
