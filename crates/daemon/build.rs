// SPDX-License-Identifier: GPL-3.0-or-later

//! Records the commit this binary is built from as `MASON_COMMIT`, for `--version`:
//! `MASON_BUILD_SHA` when given (a build from a source archive), else `git rev-parse`,
//! else `unknown`.
//!
//! The script reruns when the checked-out commit changes: it watches `HEAD` and the branch file
//! `HEAD` names, at the paths git reports. It must not name a path that does not exist. Cargo
//! treats a missing `rerun-if-changed` file as always changed, so it reruns this script and
//! rebuilds the crate and everything that depends on it on every build. That happened in every
//! task worktree until 2026-11-13: there `.git` is a file, so the old `../../.git/HEAD` was never
//! there, and each `cargo build` or `cargo test` recompiled mason-daemon and mason-pipeline.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=MASON_BUILD_SHA");
    println!("cargo:rerun-if-changed=build.rs");
    for p in head_files() {
        println!("cargo:rerun-if-changed={}", p.display());
    }
    let given = std::env::var("MASON_BUILD_SHA")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let sha = given.or_else(|| git(&["rev-parse", "--short=12", "HEAD"]));
    println!(
        "cargo:rustc-env=MASON_COMMIT={}",
        sha.unwrap_or_else(|| "unknown".into())
    );
}

/// `HEAD`, the branch file it points at and `packed-refs`, those that exist. Empty outside a git
/// checkout.
fn head_files() -> Vec<PathBuf> {
    let Some(head) = git_path("HEAD") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(r) = std::fs::read_to_string(&head)
        .ok()
        .and_then(|t| t.strip_prefix("ref:").map(|r| r.trim().to_string()))
    {
        out.extend(git_path(&r));
        out.extend(git_path("packed-refs"));
    }
    out.push(head);
    out.retain(|p| p.exists());
    out
}

/// Where git keeps `name` for this checkout, absolute. In a worktree that is under the main
/// repository's `.git/worktrees/<name>/` (or its common directory, for refs).
fn git_path(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(git(&["rev-parse", "--git-path", name])?);
    Some(if p.is_absolute() {
        p
    } else {
        Path::new(&std::env::var("CARGO_MANIFEST_DIR").ok()?).join(p)
    })
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}
