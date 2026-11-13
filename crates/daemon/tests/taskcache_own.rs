// SPDX-License-Identifier: GPL-3.0-or-later

//! `MASON_TASK_CACHE` (s172): a directory MASON owns for each task, seen by its steps, removed
//! when the task lands or is canceled, and swept when a leftover remains.

mod support;
use mason_core::*;
use mason_daemon::sweep::sweep_once;
use serde_json::json;
use std::path::PathBuf;
use support::*;

fn own(d: &mason_daemon::daemon::Daemon, t: TaskId) -> PathBuf {
    d.cfg.task_cache_dir(t.0)
}

fn long_script() -> serde_json::Value {
    let develop = json!([{"sh": TASK_SCRIPT_COMMIT}, {"sleep_ms": 60000}]);
    json!({"steps": {"explore": [explore_turn()], "develop": [develop]}})
}

#[test]
fn a_step_sees_the_directory_and_it_is_gone_after_landing() {
    let s = Scratch::new("true", happy_script());
    let seen = s.root.path().join("seen");
    s.write_project(
        "scratch",
        &format!(
            "test -d $MASON_TASK_CACHE && echo x > $MASON_TASK_CACHE/artifact && echo $MASON_TASK_CACHE > {}",
            seen.display()
        ),
        "",
    );
    let d = s.start();
    let t = file_task(&d, "Lands");
    wait_state(&d, t, State::Landed, 60);
    wait_for("the job", 15, || d.job_count() == 0);
    let said = std::fs::read_to_string(&seen).expect("the test step saw MASON_TASK_CACHE");
    assert_eq!(said.trim(), own(&d, t).to_string_lossy());
    wait_for("the directory to go", 15, || !own(&d, t).exists());
}

#[test]
fn the_directory_is_gone_after_cancel() {
    let s = Scratch::new("true", long_script());
    let d = s.start();
    let t = file_task(&d, "Long running");
    wait_state(&d, t, State::Developing, 60);
    // The agent step's own environment has it, so the directory exists while it runs.
    wait_for("the directory to exist", 15, || own(&d, t).is_dir());
    d.emit(EventKind::Canceled {
        task: t,
        delete_branch: true,
    })
    .unwrap();
    wait_for("the directory to go", 60, || !own(&d, t).exists());
}

#[test]
fn the_sweep_removes_a_finished_tasks_leftover() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let t = file_task(&d, "Lands");
    wait_state(&d, t, State::Landed, 60);
    wait_for("the job", 15, || d.job_count() == 0);
    wait_for("removal at landing", 15, || !own(&d, t).exists());
    std::fs::create_dir_all(own(&d, t).join("deep")).unwrap();
    std::fs::write(own(&d, t).join("deep/f"), "x").unwrap();
    sweep_once(&d);
    wait_for("the sweep to remove it", 15, || !own(&d, t).exists());
}

#[test]
fn the_sweep_leaves_a_running_tasks_directory() {
    let s = Scratch::new("true", long_script());
    let d = s.start();
    let t = file_task(&d, "Live");
    wait_state(&d, t, State::Developing, 60);
    wait_for("the directory to exist", 15, || own(&d, t).is_dir());
    std::fs::write(own(&d, t).join("f"), "x").unwrap();
    sweep_once(&d);
    sweep_once(&d);
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(
        own(&d, t).join("f").exists(),
        "a running task's directory is not touched"
    );
}
