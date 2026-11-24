// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg(unix)]
// Unix-only: signals, permission bits or unix sockets. Windows coverage comes with the later port tasks.

//! `run_check` and landing's re-test run under a step host: a daemon that dies
//! and comes back finds the run again, a second call attaches to it, and the
//! limit is enforced from the host's start.

use crate::support;
use mason_daemon::hostedcheck::{answered, hold_results, pending, run, CheckSpec};
use serde_json::json;
use std::path::Path;
use std::time::{Duration, Instant};
use support::*;

fn spec(
    d: &std::sync::Arc<mason_daemon::daemon::Daemon>,
    dir: &Path,
    cmd: &str,
    tree: &str,
    secs: u64,
) -> CheckSpec {
    let _ = d;
    CheckSpec {
        id_prefix: "t0r1-check".into(),
        log_dir: dir.join("logs"),
        cmd: cmd.into(),
        build: None,
        cwd: dir.to_path_buf(),
        env: vec![],
        limit: Duration::from_secs(secs),
        tree: tree.into(),
        stop: None,
        holds: Default::default(),
        label: String::new(),
        place: None,
        global: false,
    }
}

fn lines(p: &Path) -> usize {
    std::fs::read_to_string(p).map_or(0, |t| t.lines().count())
}

#[test]
fn a_second_call_attaches_and_a_different_tree_runs_afresh() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let work = tempfile::tempdir().unwrap();
    let marker = work.path().join("ran");
    let cmd = format!("echo x >> {}; sleep 2; echo done", marker.display());
    let sp = spec(&d, work.path(), &cmd, "tree-a", 60);
    let (r1, r2) = std::thread::scope(|sc| {
        let a = sc.spawn(|| run(&d, &sp));
        std::thread::sleep(Duration::from_millis(700));
        let b = sc.spawn(|| run(&d, &sp));
        (a.join().unwrap().unwrap(), b.join().unwrap().unwrap())
    });
    assert!(r1.passed && r2.passed);
    assert_eq!(r1.id, r2.id, "the second call attached to the first");
    assert_eq!(lines(&marker), 1, "the command ran once");
    assert!(r1.output.contains("done"));

    // Another tree does not reuse the old result.
    let other = spec(&d, work.path(), &cmd, "tree-b", 60);
    let r3 = run(&d, &other).unwrap();
    assert_ne!(r3.id, r1.id);
    assert_eq!(lines(&marker), 2);
}

#[test]
fn a_run_in_flight_is_pending_until_given_and_then_a_call_runs_afresh() {
    // After a restart the agent's run_check arrives again. While the run it
    // started is still going, or finished with nobody given its result, the
    // call joins it and must not queue for a slot first (#72, 2026-11-10: its
    // check finished while the re-sent call waited in the queue).
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let work = tempfile::tempdir().unwrap();
    let sp = spec(&d, work.path(), "sleep 2; echo done", "tree-a", 60);
    assert!(!pending(&d, &sp), "nothing has run yet");
    std::thread::scope(|sc| {
        let a = sc.spawn(|| run(&d, &sp));
        let end = Instant::now() + Duration::from_secs(30);
        while !pending(&d, &sp) {
            assert!(Instant::now() < end, "the run never showed as pending");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !pending(
                &d,
                &spec(&d, work.path(), "sleep 2; echo done", "tree-b", 60)
            ),
            "another tree has nothing to join"
        );
        assert!(a.join().unwrap().unwrap().passed);
    });
    assert!(
        !pending(&d, &sp),
        "its result was given: a later call runs afresh"
    );
}

#[test]
fn a_hung_check_ends_at_the_limit() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let work = tempfile::tempdir().unwrap();
    let sp = spec(&d, work.path(), "sleep 120", "t", 2);
    let t = Instant::now();
    let r = run(&d, &sp).unwrap();
    assert!(!r.passed && r.timed_out, "{r:?}");
    assert!(t.elapsed() < Duration::from_secs(30), "{:?}", t.elapsed());
}

#[test]
fn a_check_that_exits_red_is_red() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let work = tempfile::tempdir().unwrap();
    let r = run(&d, &spec(&d, work.path(), "echo bad; exit 3", "t", 30)).unwrap();
    assert!(!r.passed && !r.timed_out);
    assert!(r.output.contains("bad"));
}

#[test]
fn kill_9_while_run_check_runs_the_command_once_and_the_task_lands() {
    let work = tempfile::tempdir().unwrap();
    let marker = work.path().join("checks");
    let tmp = work.path().join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    // Only a run_check lacks MASON_STEP; the test step is not counted.
    let cmd = format!(
        "case ${{MASON_STEP+set}} in set) ;; *) echo c >> {}; sleep 5;; esac; test -f hello.txt",
        marker.display()
    );
    let script = json!({"steps": {"explore": [explore_turn()], "develop": [[
        {"sh": TASK_SCRIPT_COMMIT},
        {"call": {"tool": "run_check", "args": {"name": "tests"}}},
        {"call": {"tool": "submit_change", "args": {"summary": "ok", "head": "$HEAD"}}}
    ]]}});
    let s = Scratch::new(&cmd, script);
    let envs = [("TMPDIR", tmp.to_str().unwrap())];
    let mut m = s.masond(&[], &envs);
    s.wait_daemon(&mut m);
    s.mason_ok(&["task", "add", "Checks its work"]);
    let end = Instant::now() + Duration::from_secs(240);
    while lines(&marker) == 0 {
        if Instant::now() > end {
            let st = s.mason(&["task", "show", "0", "--json"]);
            panic!(
                "the check did not start in 240s\n{}",
                String::from_utf8_lossy(&st.stdout)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(m.child.id() as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    let _ = m.child.wait();
    let mut m2 = s.masond(&[], &envs);
    s.wait_daemon(&mut m2);
    s.wait_task(0, "landed", 240);
    assert_eq!(lines(&marker), 1, "the command ran once");
    let left: Vec<_> = std::fs::read_dir(&tmp)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("mason-sh-"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn a_running_check_records_the_slots_it_holds_beside_its_host_until_it_ends() {
    // s589: a daemon that restarts counts these as held. Asserted on the host list, not on time.
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let work = tempfile::tempdir().unwrap();
    let gate = work.path().join("release");
    let cmd = format!("while [ ! -e {} ]; do sleep 0.1; done", gate.display());
    let mut sp = spec(&d, work.path(), &cmd, "tree-h", 120);
    sp.holds = [("test-suite".to_string(), 1)].into();
    sp.label = "#9 check".into();
    let hosts = d.cfg.hosts_dir();
    std::thread::scope(|sc| {
        let a = sc.spawn(|| run(&d, &sp));
        let end = Instant::now() + Duration::from_secs(60);
        let held = loop {
            let h = mason_daemon::hostedcheck::running_holds(&hosts);
            if !h.is_empty() {
                break h;
            }
            assert!(Instant::now() < end, "the host never showed its holds");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].label, "#9 check");
        assert_eq!(held[0].uses["test-suite"], 1);
        std::fs::write(&gate, "").unwrap();
        assert!(a.join().unwrap().unwrap().passed);
    });
    // The command ended: the host holds nothing, though its directory is still there.
    assert!(mason_daemon::hostedcheck::running_holds(&hosts).is_empty());
}

#[test]
fn a_check_feeds_the_develop_runs_counts_and_phases() {
    use mason_core::checks::CheckPhase;
    use mason_core::progress::Progress;
    use mason_core::{ProjectId, StepRunId, TaskId};
    use mason_daemon::hostedcheck::{executable_of, run_with, CheckProgress};
    use mason_testing::{ParserKind, Suite};

    assert_eq!(
        executable_of(
            "     Running unittests src/lib.rs (target/debug/deps/mason_core-0123456789abcdef)"
        )
        .as_deref(),
        Some("mason_core")
    );
    assert_eq!(executable_of("ok 1 - a"), None);

    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let work = tempfile::tempdir().unwrap();
    let run_id = StepRunId(9001);
    let mut p = Progress::new(run_id.0, "develop", 0);
    p.begin_agent(None, 0);
    d.progress_seed(TaskId(9001), ProjectId(1), p);
    let i = d.check_begin(run_id, "full");
    d.check_edit(run_id, i, |c, now| c.enter(CheckPhase::Running, now));
    let cmd = "echo '   Running unittests src/lib.rs (target/debug/deps/foo-0123456789abcdef)'; \
               printf 'TAP version 13\\n1..3\\nok 1 - a\\nok 2 - b\\nnot ok 3 - c\\n'; exit 1";
    let sp = spec(&d, work.path(), cmd, "tree-p", 60);
    let pr = CheckProgress {
        run: run_id,
        check: i,
        name: "check s".into(),
        suite: Some(Suite::new("s", "true", ParserKind::Tap)),
    };
    let r = run_with(&d, &sp, Some(&pr)).unwrap();
    assert!(!r.passed);
    let p = d.progress_of(run_id.0).unwrap();
    let sub = p.subs.iter().find(|s| s.name == "check s").expect("a sub");
    assert_eq!((sub.unit.as_str(), sub.done.value), ("tests", 2));
    assert_eq!(p.checks[0].current, "foo");
    // The agent's own figure is not replaced by the check's count.
    assert_ne!(p.unit, "tests");
    d.check_edit(run_id, i, |c, now| c.finish("red", &["a::c".into()], now));
    let p = d.progress_of(run_id.0).unwrap();
    assert_eq!(p.checks.len(), 1);
    assert_eq!(p.checks[0].phase, CheckPhase::Done);
    assert_eq!(p.checks[0].result, "red");
    assert_eq!(p.checks[0].failed, 1);
}

/// Start `cmd` under a step host named `id`, as a daemon that has since gone would have,
/// with a run record keyed `key` that nobody has been given. Returns once it runs.
fn start_left_behind(
    d: &mason_daemon::daemon::Daemon,
    log_dir: Option<&Path>,
    id: &str,
    key: &str,
) {
    use mason_step_host::launcher::{spawn_host, SpawnOptions};
    if let Some(dir) = log_dir {
        std::fs::create_dir_all(dir).unwrap();
        mason_step_host::registry::write_kv(
            &dir.join(format!("{id}.check")),
            &[("key", key.to_string()), ("delivered", "0".into())],
        )
        .unwrap();
    }
    spawn_host(&SpawnOptions {
        host_bin: d.cfg.host_bin.clone(),
        root: d.cfg.hosts_dir(),
        id: id.to_string(),
        cmd: vec!["sh".into(), "-c".into(), "sleep 300".into()],
        cwd: None,
        env: vec![],
        clear_env: false,
        stdin: None,
        linger: Duration::from_secs(5),
        heartbeat: d.cfg.heartbeat,
    })
    .unwrap();
    wait_for("the left-behind host to run", 60, || host_running(d, id));
}

fn host_running(d: &mason_daemon::daemon::Daemon, id: &str) -> bool {
    matches!(
        mason_step_host::registry::status_of(&d.cfg.hosts_dir().join(id)),
        Ok(s) if s.state == mason_step_host::registry::HostState::Running
    )
}

#[test]
fn a_call_on_a_new_tree_stops_an_uncollected_run_on_an_older_tree() {
    // 2026-11-18: after a daemon restart, develop agents asked for checks on new trees while
    // their old runs kept going under surviving step hosts, results nobody would read, each a
    // whole suite holding a slot.
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let work = tempfile::tempdir().unwrap();
    let sp = spec(&d, work.path(), "echo done", "tree-b", 60);
    start_left_behind(&d, Some(&sp.log_dir), "t0r1-check0", "an older tree");
    let r = run(&d, &sp).unwrap();
    assert!(r.passed, "{r:?}");
    assert_ne!(r.id, "t0r1-check0", "the new tree ran afresh");
    wait_for("the older run to stop", 60, || {
        !host_running(&d, "t0r1-check0")
    });
    let rec = mason_step_host::registry::read_kv(&sp.log_dir.join("t0r1-check0.check")).unwrap();
    assert_eq!(rec.get("delivered").map(String::as_str), Some("1"));
}

#[test]
fn the_sweep_stops_a_check_whose_run_is_over_and_nothing_else() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    start_left_behind(&d, None, "t999r7-check-x0", "k");
    start_left_behind(&d, None, "b3-retest0", "k");
    // The daemon sweeps on its own too, so the count here may be 0: the outcome is what counts.
    mason_daemon::hostedcheck::stop_unwanted(&d);
    wait_for("the dead run's check to stop", 60, || {
        !host_running(&d, "t999r7-check-x0")
    });
    assert!(
        host_running(&d, "b3-retest0"),
        "a landing re-test is not a develop check"
    );
    let sock = d.cfg.hosts_dir().join("b3-retest0").join("sock");
    let mut h = mason_step_host::client::HostClient::connect(
        &sock,
        mason_step_host::protocol::PREVIOUS,
        mason_step_host::protocol::CURRENT,
        "test",
    )
    .unwrap();
    h.stop(1000).unwrap();
}

#[test]
fn a_check_id_names_its_task_and_run() {
    use mason_daemon::hostedcheck::check_owner;
    assert_eq!(
        check_owner("t575r3670-check-cargo-workspace2"),
        Some((575, 3670))
    );
    assert_eq!(check_owner("t0r1-check0"), Some((0, 1)));
    assert_eq!(check_owner("b3-retest0"), None);
    assert_eq!(check_owner("t5r7-1"), None);
}

#[test]
fn a_held_result_is_reused_by_a_replay_until_the_agent_has_the_answer() {
    // A takeover cuts a `run_check` call off after one suite finished. The replay of the call
    // takes that suite's result instead of running it again, and the other suite likewise;
    // once the agent has the answer, a new call on the same tree runs them again (#1051).
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let work = tempfile::tempdir().unwrap();
    let (ma, mb) = (work.path().join("a"), work.path().join("b"));
    let suite = |prefix: &str, marker: &Path| {
        let mut sp = spec(
            &d,
            work.path(),
            &format!("echo x >> {}; echo done", marker.display()),
            "tree-a",
            60,
        );
        sp.id_prefix = prefix.into();
        sp
    };
    let (a, b) = (suite("t3r4-check-a", &ma), suite("t3r4-check-b", &mb));
    let call = || {
        let _hold = hold_results(3, 4);
        let ra = run(&d, &a).unwrap();
        let rb = run(&d, &b).unwrap();
        (ra.id, rb.id)
    };
    let first = call();
    assert!(
        pending(&d, &a),
        "finished, not yet answered: a replay takes it"
    );
    let replay = call();
    assert_eq!(first, replay, "the replay took both results");
    assert_eq!((lines(&ma), lines(&mb)), (1, 1), "each ran once");
    answered(3, 4);
    assert!(!pending(&d, &a), "answered: nothing left to take");
    let again = call();
    assert_ne!(first, again, "a new call on the same tree runs again");
    assert_eq!((lines(&ma), lines(&mb)), (2, 2));
}

#[test]
fn a_call_without_a_hold_delivers_at_once_as_before() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let work = tempfile::tempdir().unwrap();
    let marker = work.path().join("ran");
    let sp = spec(
        &d,
        work.path(),
        &format!("echo x >> {}", marker.display()),
        "tree-a",
        60,
    );
    let r1 = run(&d, &sp).unwrap();
    let r2 = run(&d, &sp).unwrap();
    assert_ne!(r1.id, r2.id);
    assert_eq!(lines(&marker), 2);
}
