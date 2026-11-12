// SPDX-License-Identifier: GPL-3.0-or-later

//! Never test the same tree twice (s179, decision 0174).
//!
//! The single-command path (a test step with no suites, `run_check`, landing's re-test)
//! remembers what a test command did on an exact content tree. The key is
//!
//! - the **content tree**: the tree git would write from the working files, whether or not
//!   they are committed (so a `run_check` on uncommitted work matches the commit of exactly
//!   that content), and
//! - the **command text** after layering: the build command and the test command as the test
//!   step resolves them, hashed whole.
//!
//! Both must be equal. There is no "similar": a changed byte, mode or file, or a changed
//! command (a runtime override), is a different key and runs.

use sha2::{Digest, Sha256};
use std::path::Path;

/// The tree `git write-tree` makes from the working files in `wt` through a temporary index
/// (so the real index and the worktree are not touched). None if git cannot say; then nothing
/// is reused and nothing is recorded.
pub fn content_tree(wt: &Path) -> Option<String> {
    let idx = std::env::temp_dir().join(format!(
        "mason-tree-{}-{}-{}.idx",
        std::process::id(),
        crate::daemon::now_ms(),
        next_seq()
    ));
    let run = |args: &[&str]| -> Option<String> {
        let o = std::process::Command::new("git")
            .arg("-C")
            .arg(wt)
            .args(args)
            .env("GIT_INDEX_FILE", &idx)
            .output()
            .ok()?;
        o.status
            .success()
            .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let out = (|| {
        run(&["read-tree", "HEAD"])?;
        run(&["add", "-A", "--", "."])?;
        run(&["write-tree"]).filter(|t| t.len() >= 40)
    })();
    let _ = std::fs::remove_file(&idx);
    out
}

fn next_seq() -> u64 {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// The key of a command: a hash of the build command (if any) and the test command, each
/// length-prefixed so no two different pairs share a text.
pub fn command_key(build: Option<&str>, test: &str) -> String {
    let mut h = Sha256::new();
    match build {
        Some(b) => {
            h.update(format!("build:{}:", b.len()));
            h.update(b.as_bytes());
        }
        None => h.update("nobuild:"),
    }
    h.update(format!("test:{}:", test.len()));
    h.update(test.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// `Some(source)` if this command already went green on this tree.
pub fn green_for(d: &crate::daemon::Daemon, tree: &str, key: &str) -> Option<String> {
    match d.store.command_run(tree, key) {
        Ok(Some((true, source, _))) => Some(source),
        _ => None,
    }
}

pub fn record_green(d: &crate::daemon::Daemon, tree: &str, key: &str, source: &str) {
    let _ = d
        .store
        .put_command_run(tree, key, true, source, &[], crate::daemon::now_ms());
}

pub fn record_red(
    d: &crate::daemon::Daemon,
    tree: &str,
    key: &str,
    source: &str,
    names: &[String],
) {
    let _ = d
        .store
        .put_command_run(tree, key, false, source, names, crate::daemon::now_ms());
}

/// The refusal text if `tree` was red on the test step for this command and has not changed.
pub fn known_red(d: &crate::daemon::Daemon, tree: &str, key: &str) -> Option<String> {
    match d.store.command_run(tree, key) {
        Ok(Some((false, source, names))) if !names.is_empty() => Some(format!(
            "this exact tree already failed the tests ({source}) and nothing has changed since; \
             failing tests: {}. Fix them and commit a change before submitting again",
            names.join(", ")
        )),
        _ => None,
    }
}
