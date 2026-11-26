// SPDX-License-Identifier: GPL-3.0-or-later

//! The worktree sweep (s1145) against a scratch daemon and repository. The sweep's clock is a
//! parameter, so a test passes a later time instead of waiting.

use crate::support;
use mason_core::*;
use mason_daemon::daemon::{now_ms, Daemon};
use mason_daemon::disk::FakeDisk;
use mason_daemon::worktrees;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use support::*;

const HOUR_MS: u64 = 3_600_000;

fn later(hours: u64) -> u64 {
    now_ms() + hours * HOUR_MS
}

/// Plenty of room, so only age decides; the real disk is never read.
fn start(s: &Scratch) -> Arc<Daemon> {
    s.start_with(|c| c.disk = Some(FakeDisk::new_gb(500)))
}

/// `target/` is build output git ignores, in every worktree of the scratch repository.
fn ignore_target(s: &Scratch) {
    let exclude = s.repo.join(".git/info/exclude");
    std::fs::create_dir_all(exclude.parent().unwrap()).unwrap();
    let mut text = std::fs::read_to_string(&exclude).unwrap_or_default();
    text.push_str("\ntarget/\n");
    std::fs::write(exclude, text).unwrap();
}

fn plant_target(wt: &std::path::Path) -> PathBuf {
    let dir = wt.join("target");
    std::fs::create_dir_all(dir.join("debug")).unwrap();
    std::fs::write(dir.join("debug/artifact"), vec![0u8; 1 << 20]).unwrap();
    dir
}

/// Develop commits and then stays running.
fn long_develop() -> Scratch {
    let develop = json!([
        {"sh": format!("git log --oneline | grep -q 'add hello' || {{ {TASK_SCRIPT_COMMIT}; }}")},
        {"sleep_ms": 60000}
    ]);
    let s = Scratch::new(
        "true",
        json!({"steps": {"explore": [explore_turn(), explore_turn()],
                         "develop": [develop.clone(), develop]}}),
    );
    ignore_target(&s);
    s
}

fn worktree(d: &Daemon, t: TaskId) -> (PathBuf, String) {
    let p = d.project(d.world().unwrap().tasks[&t].project).unwrap();
    (p.worktree(&d.cfg, t.0), p.branch(t.0))
}

fn running(s: &Scratch, d: &Arc<Daemon>) -> TaskId {
    let t = file_task(d, "Long running");
    wait_state(d, t, State::Developing, 60);
    let (_, branch) = worktree(d, t);
    wait_for("the agent to commit", 30, || {
        mason_landing::git::commits_ahead(&s.repo, "origin/main", &branch).unwrap_or(0) >= 1
    });
    t
}

fn hosts_stopped(d: &Daemon) -> bool {
    mason_step_host::registry::Registry::new(d.cfg.hosts_dir())
        .scan()
        .unwrap()
        .iter()
        .all(|h| h.state != mason_step_host::registry::HostState::Running)
}

fn cancel_keeping_the_branch(d: &Arc<Daemon>, t: TaskId) {
    d.emit(EventKind::Canceled {
        task: t,
        delete_branch: false,
    })
    .unwrap();
    wait_for("the step host to stop", 30, || hosts_stopped(d));
    wait_for("the job to end", 30, || d.job_count() == 0);
}

fn item(report: &Value, t: TaskId) -> &Value {
    report["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["task"] == t.0)
        .unwrap_or_else(|| panic!("task {t} is not in {report}"))
}

fn request(d: &Arc<Daemon>, method: &str, uri: &str, who: &str, body: Value) -> (u16, Value) {
    let d = d.clone();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            use tower::ServiceExt;
            let req = axum::http::Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .header("x-mason-as", who)
                .body(axum::body::Body::from(body.to_string()))
                .unwrap();
            let resp = mason_daemon::http::api_router(d)
                .oneshot(req)
                .await
                .unwrap();
            let status = resp.status().as_u16();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            )
        })
}

#[test]
fn a_canceled_tasks_worktree_goes_after_a_day_and_revive_brings_it_back() {
    let s = long_develop();
    let d = start(&s);
    let t = running(&s, &d);
    let (wt, branch) = worktree(&d, t);
    cancel_keeping_the_branch(&d, t);

    assert!(worktrees::pass(&d, later(23)).is_empty());
    assert!(wt.join("hello.txt").exists(), "younger than a day: kept");

    // Reported before the sweep, with what it would free.
    let r = worktrees::report(&d, later(25));
    let i = item(&r, t);
    assert_eq!(i["action"], "remove", "{r}");
    assert_eq!(i["state"], "canceled");
    assert!(i["frees_bytes"].as_u64().unwrap() > 0, "{r}");
    assert!(r["free_next_bytes"].as_u64().unwrap() > 0, "{r}");
    assert!(wt.exists(), "a report removes nothing");

    worktrees::pass(&d, later(25));
    assert!(!wt.exists(), "the worktree is gone");
    assert!(
        mason_landing::git::branch_exists(&s.repo, &branch),
        "the branch stays"
    );

    d.emit(EventKind::Revived { task: t }).unwrap();
    wait_state(&d, t, State::Developing, 60);
    wait_for("the worktree to come back", 30, || {
        wt.join("hello.txt").exists()
    });
}

#[test]
fn a_dirty_canceled_worktree_is_left_and_reported() {
    let s = long_develop();
    let d = start(&s);
    let t = running(&s, &d);
    let (wt, _) = worktree(&d, t);
    cancel_keeping_the_branch(&d, t);
    std::fs::write(wt.join("unsaved.txt"), "work\n").unwrap();
    worktrees::pass(&d, later(1000));
    assert!(wt.join("unsaved.txt").exists());
    let r = worktrees::report(&d, later(1000));
    let why = item(&r, t)["skipped"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(why.contains("uncommitted changes"), "{r}");
}

#[test]
fn a_canceled_branch_already_in_the_base_goes_at_once() {
    let s = long_develop();
    let d = start(&s);
    let t = running(&s, &d);
    let (wt, branch) = worktree(&d, t);
    cancel_keeping_the_branch(&d, t);
    // The work reaches the base some other way.
    sh(
        &s.repo,
        &["push", "-q", "origin", &format!("{branch}:main")],
    );
    sh(&s.repo, &["fetch", "-q", "origin"]);
    worktrees::pass(&d, now_ms());
    assert!(!wt.exists());
    assert!(mason_landing::git::branch_exists(&s.repo, &branch));
}

#[test]
fn a_running_steps_worktree_is_never_touched() {
    let s = long_develop();
    let d = start(&s);
    let t = running(&s, &d);
    let (wt, _) = worktree(&d, t);
    let target = plant_target(&wt);
    worktrees::pass(&d, later(1000));
    assert!(target.join("debug/artifact").exists());
    assert!(wt.join("hello.txt").exists());
}

#[test]
fn a_waiting_developing_task_loses_idle_build_output_and_keeps_its_sources() {
    let gate = tempfile::tempdir().unwrap();
    let open = gate.path().join("open");
    let explore = json!([
        {"sh": format!("while [ ! -f {} ]; do sleep 0.05; done", open.display())},
        explore_turn()[0].clone()
    ]);
    let s = Scratch::new(
        "true",
        json!({"steps": {"explore": [explore], "develop": [develop_turn()]}}),
    );
    ignore_target(&s);
    let d = start(&s);
    let t = file_task(&d, "Waits for its account");
    wait_state(&d, t, State::Exploring, 60);
    // A held account: develop is entered and waits, with no agent running.
    let (status, body) = request(
        &d,
        "POST",
        "/account/hold",
        &d.cfg.owner_names[0].clone(),
        json!({"account": "claude", "for_s": 3600}),
    );
    assert_eq!(status, 200, "{body}");
    std::fs::write(&open, "").unwrap();
    wait_state(&d, t, State::Developing, 60);
    wait_for("the explore host to stop", 30, || hosts_stopped(&d));
    let (wt, _) = worktree(&d, t);
    let target = plant_target(&wt);
    std::fs::write(wt.join("source.txt"), "kept, though not committed\n").unwrap();

    worktrees::pass(&d, later(11));
    assert!(target.exists(), "not idle long enough");

    let r = worktrees::report(&d, later(13));
    let i = item(&r, t);
    assert_eq!(i["action"], "trim", "{r}");
    assert_eq!(i["outputs"], json!(["target"]));
    assert!(i["frees_bytes"].as_u64().unwrap() >= 1 << 20, "{r}");

    // A dry run lists it and removes nothing; removing is the owner's.
    let (status, listed) = request(
        &d,
        "POST",
        "/disk/clean",
        "somebody",
        json!({"dry_run": true}),
    );
    assert_eq!(status, 200, "{listed}");
    assert!(target.exists());
    let (status, refused) = request(&d, "POST", "/disk/clean", "somebody", json!({}));
    assert_eq!(status, 403, "{refused}");

    worktrees::pass(&d, later(13));
    assert!(!target.exists(), "the idle build output is gone");
    assert!(wt.join("source.txt").exists());
    assert!(wt.join(".git").exists(), "the checkout stays");
}

#[test]
fn low_disk_space_trims_without_waiting_and_a_dry_run_only_lists() {
    let s = long_develop();
    let disk = FakeDisk::new_gb(500);
    let d = s.start_with(|c| c.disk = Some(disk.clone()));
    let t = running(&s, &d);
    let (wt, _) = worktree(&d, t);
    cancel_keeping_the_branch(&d, t);
    let target = plant_target(&wt);
    assert!(worktrees::pass(&d, now_ms()).is_empty());

    disk.set_gb(30); // above the floor of 20, under twice it
    let listed = worktrees::clean(&d, now_ms(), true);
    assert_eq!(listed["items"][0]["action"], "trim", "{listed}");
    assert!(target.exists(), "a dry run removes nothing");
    let done = worktrees::clean(&d, now_ms(), false);
    assert_eq!(done["items"][0]["action"], "trim", "{done}");
    assert!(!target.exists());
    assert!(wt.join("hello.txt").exists());
}

#[test]
fn an_unused_landing_scratch_worktree_goes() {
    let s = Scratch::new("true", happy_script());
    let d = start(&s);
    let t = file_task(&d, "Lands");
    wait_state(&d, t, State::Landed, 60);
    wait_for("the job", 15, || d.job_count() == 0);
    wait_for("the batch to end", 30, || {
        d.project_id_by_name("scratch")
            .is_some_and(|p| d.landing().running_batches(p) == 0)
    });
    let scratch = d.cfg.landing_dir().join("scratch-b0");
    assert!(
        scratch.exists(),
        "landing keeps its worktree for the next batch"
    );
    worktrees::pass(&d, later(47));
    // The sweep at 47 h found it idle, not running.
    assert!(scratch.exists());
    worktrees::pass(&d, later(49));
    assert!(!scratch.exists());
}
