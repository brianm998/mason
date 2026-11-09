// SPDX-License-Identifier: GPL-3.0-or-later

//! `task_cache` (s78): a landed task's build directory is removed, a canceled task's is kept,
//! and the sweep removes the leftovers of landed tasks but not those of live ones.

mod support;
use mason_core::*;
use mason_daemon::sweep::sweep_once;
use serde_json::json;
use std::path::PathBuf;
use support::*;

fn cache_root(s: &Scratch) -> PathBuf {
    s.root.path().join("cache")
}

fn extra(s: &Scratch) -> String {
    format!("task_cache = \"{}/{{task}}\"", cache_root(s).display())
}

#[test]
fn a_landed_tasks_directory_is_removed_and_a_canceled_tasks_stays() {
    let develop = json!([{"sh": TASK_SCRIPT_COMMIT}, {"sleep_ms": 60000}]);
    let s = Scratch::new(
        "true",
        json!({"steps": {"explore": [explore_turn()], "develop": [develop]}}),
    );
    // The test command writes into the task's directory, as cargo would.
    let cache = cache_root(&s);
    s.write_project(
        "scratch",
        &format!(
            "mkdir -p {c}/$(basename $PWD) && echo x > {c}/$(basename $PWD)/artifact",
            c = cache.display()
        ),
        &extra(&s),
    );
    let d = s.start();
    // A canceled task keeps its directory.
    let t = file_task(&d, "Long running");
    wait_state(&d, t, State::Developing, 60);
    let p = d.project(d.world().unwrap().tasks[&t].project).unwrap();
    wait_for("the agent to commit", 30, || {
        mason_landing::git::commits_ahead(&s.repo, "origin/main", &p.branch(t.0)).unwrap_or(0) >= 1
    });
    std::fs::create_dir_all(cache.join(t.0.to_string())).unwrap();
    d.emit(EventKind::Canceled {
        task: t,
        delete_branch: false,
    })
    .unwrap();
    wait_for("the step host to stop", 15, || {
        mason_step_host::registry::Registry::new(d.cfg.hosts_dir())
            .scan()
            .unwrap()
            .iter()
            .all(|h| h.state != mason_step_host::registry::HostState::Running)
    });
    wait_for("the job to end", 30, || d.job_count() == 0);
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(cache.join(t.0.to_string()).exists(), "canceled: kept");
}

#[test]
fn the_directory_is_gone_after_landing() {
    let s = Scratch::new("true", happy_script());
    let cache = cache_root(&s);
    s.write_project(
        "scratch",
        &format!(
            "mkdir -p {c}/$(basename $PWD) && echo x > {c}/$(basename $PWD)/artifact",
            c = cache.display()
        ),
        &extra(&s),
    );
    let d = s.start();
    let t = file_task(&d, "Lands");
    wait_state(&d, t, State::Landed, 60);
    wait_for("the job", 15, || d.job_count() == 0);
    wait_for("the directory to go", 15, || {
        !cache.join(t.0.to_string()).exists()
    });
}

#[test]
fn without_task_cache_nothing_is_removed() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let t = file_task(&d, "Lands");
    let other = cache_root(&s).join(t.0.to_string());
    std::fs::create_dir_all(&other).unwrap();
    wait_state(&d, t, State::Landed, 60);
    wait_for("the job", 15, || d.job_count() == 0);
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(other.exists());
}

#[test]
fn the_sweep_removes_a_landed_tasks_leftover() {
    let s = Scratch::new("true", happy_script());
    s.write_project("scratch", "true", &extra(&s));
    let cache = cache_root(&s);
    let d = s.start();
    let t = file_task(&d, "Lands");
    wait_state(&d, t, State::Landed, 60);
    wait_for("the job", 15, || d.job_count() == 0);
    // Left behind, as by a restart between landing and removal.
    let dir = cache.join(t.0.to_string());
    std::fs::create_dir_all(dir.join("deep")).unwrap();
    std::fs::write(dir.join("deep/f"), "x").unwrap();
    sweep_once(&d);
    wait_for("the sweep to remove it", 15, || !dir.exists());
}

#[test]
fn the_sweep_leaves_a_live_tasks_directory() {
    let develop = json!([{"sh": TASK_SCRIPT_COMMIT}, {"sleep_ms": 60000}]);
    let s = Scratch::new(
        "true",
        json!({"steps": {"explore": [explore_turn()], "develop": [develop]}}),
    );
    s.write_project("scratch", "true", &extra(&s));
    let cache = cache_root(&s);
    let d = s.start();
    let live = file_task(&d, "Live");
    wait_state(&d, live, State::Developing, 60);
    let dir = cache.join(live.0.to_string());
    std::fs::create_dir_all(&dir).unwrap();
    sweep_once(&d);
    sweep_once(&d);
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(dir.exists(), "a running task's directory is not touched");
}

/// A task left running a long develop step, then canceled with its branch kept, its directory
/// planted, and its job gone.
fn canceled_with_a_directory(
    s: &Scratch,
    d: &std::sync::Arc<mason_daemon::daemon::Daemon>,
) -> (TaskId, PathBuf) {
    let t = file_task(d, "Long running");
    wait_state(d, t, State::Developing, 60);
    let p = d.project(d.world().unwrap().tasks[&t].project).unwrap();
    wait_for("the agent to commit", 30, || {
        mason_landing::git::commits_ahead(&s.repo, "origin/main", &p.branch(t.0)).unwrap_or(0) >= 1
    });
    let dir = cache_root(s).join(t.0.to_string());
    std::fs::create_dir_all(dir.join("deep")).unwrap();
    std::fs::write(dir.join("deep/f"), "x").unwrap();
    d.emit(EventKind::Canceled {
        task: t,
        delete_branch: false,
    })
    .unwrap();
    wait_for("the job to end", 30, || d.job_count() == 0);
    (t, dir)
}

fn long_develop() -> Scratch {
    let develop = json!([{"sh": TASK_SCRIPT_COMMIT}, {"sleep_ms": 60000}]);
    Scratch::new(
        "true",
        json!({"steps": {"explore": [explore_turn(), explore_turn()], "develop": [develop.clone(), develop]}}),
    )
}

#[test]
fn a_canceled_tasks_directory_older_than_the_limit_goes() {
    let s = long_develop();
    s.write_project(
        "scratch",
        "true",
        &format!("{}\ntask_cache_keep_days = 0", extra(&s)),
    );
    let d = s.start();
    let (_, dir) = canceled_with_a_directory(&s, &d);
    std::thread::sleep(std::time::Duration::from_millis(50));
    sweep_once(&d);
    wait_for("the sweep to remove it", 15, || !dir.exists());
}

#[test]
fn a_canceled_tasks_directory_younger_than_the_limit_stays() {
    let s = long_develop();
    s.write_project("scratch", "true", &extra(&s)); // the default: 7 days
    let d = s.start();
    let (_, dir) = canceled_with_a_directory(&s, &d);
    sweep_once(&d);
    sweep_once(&d);
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(dir.exists());
}

#[test]
fn a_revived_tasks_directory_is_not_touched_while_it_runs() {
    let s = long_develop();
    s.write_project(
        "scratch",
        "true",
        &format!("{}\ntask_cache_keep_days = 0", extra(&s)),
    );
    let d = s.start();
    let (t, dir) = canceled_with_a_directory(&s, &d);
    d.emit(EventKind::Revived { task: t }).unwrap();
    wait_state(&d, t, State::Developing, 60);
    std::thread::sleep(std::time::Duration::from_millis(50));
    sweep_once(&d);
    sweep_once(&d);
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(dir.exists(), "a running task's directory is not touched");
}
