// SPDX-License-Identifier: GPL-3.0-or-later

//! Moving the base branch to a commit, only ever by fast-forward. Used by the
//! single-task `direct` lander and by the batch engine alike.

use crate::git::{self, GitError};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum PromoteError {
    /// The base moved since we looked: someone else landed first.
    Moved,
    Git(GitError),
    /// No remote, and the base branch is checked out somewhere: updating it
    /// would change files under someone.
    LocalBaseCheckedOut(PathBuf),
}

impl From<GitError> for PromoteError {
    fn from(e: GitError) -> Self {
        PromoteError::Git(e)
    }
}

/// Make `base` point at `commit`.
///
/// With a remote: a plain push, refused by the remote unless it is a
/// fast-forward. Without: a compare-and-swap `update-ref` against the tip
/// last seen at `base_ref`. `wt` is any worktree of the repository (the push
/// runs there).
pub fn promote(
    repo: &Path,
    wt: &Path,
    remote: Option<&str>,
    base: &str,
    base_ref: &str,
    commit: &str,
) -> Result<(), PromoteError> {
    if let Some(r) = remote {
        let dest = format!("{commit}:refs/heads/{base}");
        let before = git::rev_parse(wt, base_ref).ok();
        return match git::run(wt, &["push", "--porcelain", r, &dest]) {
            Ok(_) => Ok(()),
            Err(e) => {
                // Don't read git's words: ask whether the remote's base moved
                // since we looked. If it did, someone else landed first.
                let _ = git::run(wt, &["fetch", "--prune", r]);
                let after = git::rev_parse(wt, base_ref).ok();
                if before.is_some() && after != before {
                    Err(PromoteError::Moved)
                } else {
                    Err(PromoteError::Git(e))
                }
            }
        };
    }
    let old = git::rev_parse(repo, base_ref)?;
    if old == commit {
        return Ok(());
    }
    if let Some((p, _)) = git::worktrees(repo)?
        .into_iter()
        .find(|(_, b)| b.as_deref() == Some(base))
    {
        return Err(PromoteError::LocalBaseCheckedOut(p));
    }
    match git::run(
        repo,
        &["update-ref", &format!("refs/heads/{base}"), commit, &old],
    ) {
        Ok(_) => Ok(()),
        Err(_) => Err(PromoteError::Moved),
    }
}
