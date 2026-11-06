// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitError {
    pub args: Vec<String>,
    pub code: Option<i32>,
    pub stderr: String,
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "git {} failed ({:?}): {}",
            self.args.join(" "),
            self.code,
            self.stderr.trim()
        )
    }
}

impl std::error::Error for GitError {}

/// Run `git` in `cwd`. Never prompts; commits and merges carry a fixed identity.
pub fn run(cwd: &Path, args: &[&str]) -> Result<String, GitError> {
    let out = Command::new("git")
        .args(["-c", "user.name=MASON", "-c", "user.email=mason@localhost"])
        .args(["-c", "commit.gpgsign=false", "-c", "core.editor=true"])
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .output()
        .map_err(|e| GitError {
            args: args.iter().map(|s| s.to_string()).collect(),
            code: None,
            stderr: e.to_string(),
        })?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(GitError {
            args: args.iter().map(|s| s.to_string()).collect(),
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        })
    }
}

pub fn ok(cwd: &Path, args: &[&str]) -> bool {
    run(cwd, args).is_ok()
}

pub fn rev_parse(cwd: &Path, rev: &str) -> Result<String, GitError> {
    run(
        cwd,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ],
    )
}

pub fn is_ancestor(cwd: &Path, ancestor: &str, of: &str) -> bool {
    ok(cwd, &["merge-base", "--is-ancestor", ancestor, of])
}

pub fn branch_exists(cwd: &Path, branch: &str) -> bool {
    ok(
        cwd,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
}

pub fn commits_ahead(cwd: &Path, base: &str, head: &str) -> Result<u32, GitError> {
    Ok(
        run(cwd, &["rev-list", "--count", &format!("{base}..{head}")])?
            .parse()
            .unwrap_or(0),
    )
}

/// Every worktree path of the repository at `cwd`, with its checked-out branch
/// (if any).
pub fn worktrees(cwd: &Path) -> Result<Vec<(PathBuf, Option<String>)>, GitError> {
    let out = run(cwd, &["worktree", "list", "--porcelain"])?;
    let mut v = vec![];
    let (mut path, mut branch): (Option<PathBuf>, Option<String>) = (None, None);
    for l in out.lines().chain(std::iter::once("")) {
        if let Some(p) = l.strip_prefix("worktree ") {
            path = Some(PathBuf::from(p));
        } else if let Some(b) = l.strip_prefix("branch refs/heads/") {
            branch = Some(b.to_string());
        } else if l.is_empty() {
            if let Some(p) = path.take() {
                v.push((p, branch.take()));
            }
            branch = None;
        }
    }
    Ok(v)
}

/// Create (or find again, after a restart) a worktree on `branch`, starting
/// from `start` when the branch doesn't exist yet. Idempotent.
pub fn ensure_worktree(repo: &Path, wt: &Path, branch: &str, start: &str) -> Result<(), GitError> {
    if wt.join(".git").exists() {
        return Ok(());
    }
    let _ = run(repo, &["worktree", "prune"]);
    if let Some(parent) = wt.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let w = wt.display().to_string();
    if branch_exists(repo, branch) {
        run(repo, &["worktree", "add", &w, branch])?;
    } else {
        run(repo, &["worktree", "add", "-b", branch, &w, start])?;
    }
    Ok(())
}

pub fn remove_worktree(repo: &Path, wt: &Path) {
    let w = wt.display().to_string();
    let _ = run(repo, &["worktree", "remove", "--force", &w]);
    let _ = std::fs::remove_dir_all(wt);
    let _ = run(repo, &["worktree", "prune"]);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatchUp {
    /// The branch already contained the base.
    UpToDate,
    /// The base was folded in; the branch changed.
    Merged,
    /// Folding in the base conflicts. The merge was aborted; the tree is clean.
    Conflict(String),
}

/// Bring the branch checked out in `wt` up to date with `base_ref`.
pub fn catch_up(wt: &Path, base_ref: &str) -> Result<CatchUp, GitError> {
    if is_ancestor(wt, base_ref, "HEAD") {
        return Ok(CatchUp::UpToDate);
    }
    match run(wt, &["merge", "--no-edit", base_ref]) {
        Ok(_) => Ok(CatchUp::Merged),
        Err(e) => {
            let conflicts =
                run(wt, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
            let _ = run(wt, &["merge", "--abort"]);
            if conflicts.trim().is_empty() {
                Err(e)
            } else {
                Ok(CatchUp::Conflict(conflicts))
            }
        }
    }
}

/// Refuse anything but plain, safe branch names.
pub fn valid_branch(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['-', '/', '.'])
        && !name.ends_with(['/', '.'])
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/-_.".contains(c))
}
