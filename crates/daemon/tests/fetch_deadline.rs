// SPDX-License-Identifier: GPL-3.0-or-later

//! A remote that accepts connections and never answers stalls a fetch, but only up to
//! its deadline, and not the cleanup of another task in the same repository.

mod support;
use mason_core::*;
use mason_landing::git;
use serde_json::json;
use std::time::{Duration, Instant};
use support::*;

#[test]
fn a_stalled_fetch_neither_blocks_cleanup_nor_outlives_its_deadline() {
    // This binary holds only this test, so the process-wide default is ours to set.
    git::set_default_deadline(Duration::from_secs(5));
    let s = Scratch::new("true", json!({"steps": {}}));
    let d = s.start();
    mason_daemon::attention::pause(&d, "", "claims", "test", 3600).unwrap();
    let pid = d.project_id_by_name("scratch").unwrap();
    let p = d.project(pid).unwrap();

    let b = file_task(&d, "other task");
    let wt = p.worktree(&d.cfg, b.0);
    git::ensure_worktree(&p.path, &wt, &p.branch(b.0), "main").unwrap();
    assert!(wt.join(".git").exists());

    // The remote stops answering: its upload-pack takes a minute to say anything. (The
    // exit script does the same with a real listener; a sandbox may allow only `file`.)
    git::run(
        &p.path,
        &["config", "remote.origin.uploadpack", "sleep 60 #"],
    )
    .unwrap();

    let started = Instant::now();
    let d2 = d.clone();
    let fetching = std::thread::spawn(move || d2.fetch_remote(pid));
    std::thread::sleep(Duration::from_millis(800));

    let t = Instant::now();
    d.emit(EventKind::Canceled {
        task: b,
        delete_branch: true,
    })
    .unwrap();
    wait_for("the worktree to go", 3, || !wt.join(".git").exists());
    assert!(
        t.elapsed() < Duration::from_secs(3) && started.elapsed() < Duration::from_secs(5),
        "cleanup waited for the fetch"
    );

    let err = fetching.join().unwrap().unwrap_err();
    assert!(err.contains("timed out"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(15));
}
