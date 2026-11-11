// SPDX-License-Identifier: GPL-3.0-or-later

//! `run_check` and landing's re-test run under a step host: a daemon that dies
//! and comes back finds the run again, a second call attaches to it, and the
//! limit is enforced from the host's start.

mod support;
use mason_daemon::hostedcheck::{run, CheckSpec};
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
        cwd: dir.to_path_buf(),
        env: vec![],
        limit: Duration::from_secs(secs),
        tree: tree.into(),
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
