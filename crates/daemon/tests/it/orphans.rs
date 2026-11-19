// SPDX-License-Identifier: GPL-3.0-or-later

//! s675: a killed daemon test leaves nothing behind. The scratch root's `owner.lock` is held by
//! its test for life; a sweep stops the hosts of, and removes, roots whose owner is gone, and
//! leaves held roots alone. Waits are on process and file state, never on time.

#![cfg(unix)]

use crate::support::orphans::{blocking_loop, claim, scratch_root_of, sweep_now};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(120);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn fresh_root() -> PathBuf {
    tempfile::Builder::new()
        .prefix("ms")
        .tempdir_in("/tmp")
        .unwrap()
        .keep()
}

/// A stand-in step host: its command line names `root` as `--dir`.
fn fake_host(root: &Path) -> Child {
    Command::new("/bin/sh")
        .args(["-c", "sleep 600; :", "sh", "--dir"])
        .arg(root)
        .stdout(Stdio::null())
        .spawn()
        .unwrap()
}

/// Run by `owner_killed_leaves_nothing` in a child process: own a root until killed.
#[test]
fn child_owner() {
    let Ok(root) = std::env::var("MS_TEST_OWNER_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let _owner = claim(&root);
    std::fs::write(root.join("ready"), "").unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[test]
fn owner_killed_leaves_nothing() {
    let root = fresh_root();
    let mut owner = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "orphans::child_owner", "--nocapture"])
        .env("MS_TEST_OWNER_ROOT", &root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_until("the owner to hold its root", || root.join("ready").exists());
    let mut host = fake_host(&root);
    sweep_now();
    assert!(root.exists(), "a held root was removed");
    assert!(
        host.try_wait().unwrap().is_none(),
        "a held root's host was stopped"
    );
    owner.kill().unwrap();
    owner.wait().unwrap();
    sweep_now();
    let status = host.wait().unwrap();
    assert!(!status.success());
    assert!(!root.exists(), "the dead root was not removed");
}

#[test]
fn root_held_by_this_process_is_left_alone() {
    let root = fresh_root();
    let owner = claim(&root);
    let mut host = fake_host(&root);
    sweep_now();
    assert!(root.exists());
    assert!(host.try_wait().unwrap().is_none());
    drop(owner);
    host.kill().unwrap();
    host.wait().unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn young_root_without_lock_is_left_alone() {
    let root = fresh_root();
    let mut host = fake_host(&root);
    sweep_now();
    assert!(root.exists());
    assert!(host.try_wait().unwrap().is_none());
    host.kill().unwrap();
    host.wait().unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn blocking_loop_ends_when_its_test_is_gone() {
    let dir = fresh_root();
    let mut owner = Command::new("/bin/sh")
        .args(["-c", "sleep 600; :"])
        .spawn()
        .unwrap();
    let script = blocking_loop(&dir, &dir.join("release"), owner.id(), "0.05");
    let mut looper = Command::new("/bin/sh")
        .args(["-c", &script])
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(looper.try_wait().unwrap().is_none(), "the loop ended early");
    owner.kill().unwrap();
    owner.wait().unwrap();
    wait_until("the loop to end", || looper.try_wait().unwrap().is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_tmp_ms_roots_match() {
    let m = |c: &str| scratch_root_of(c);
    assert_eq!(
        m("step-host --dir /tmp/msAb12/d/h/x"),
        Some(PathBuf::from("/tmp/msAb12"))
    );
    assert_eq!(
        m("masond --data-dir /private/tmp/msZ9/d"),
        Some(PathBuf::from("/tmp/msZ9"))
    );
    assert_eq!(m("step-host --dir /Users/x/.local/share/mason/h/1"), None);
    assert_eq!(m("step-host --dir /tmp/ms"), None);
    assert_eq!(m("step-host --dir /tmp/other/ms1"), None);
    assert_eq!(m("step-host --dir /var/tmp/ms1"), None);
}
