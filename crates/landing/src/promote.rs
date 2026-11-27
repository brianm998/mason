// SPDX-License-Identifier: GPL-3.0-or-later

//! Moving the base branch to a commit, only ever by fast-forward. Used by the
//! single-task `direct` lander and by the batch engine alike.

use crate::git::{self, GitError};
use std::path::{Path, PathBuf};
use std::time::Duration;

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

/// Whether an error is the credential (or the account's rights) being refused, or the branch
/// being protected: waiting does not change it. The one copy of the rule; the daemon's
/// `net::refused` calls it.
pub fn refused(e: &str) -> bool {
    let l = e.to_ascii_lowercase();
    [
        "authentication failed",
        "permission denied",
        "could not read username",
        "invalid username or password",
        "bad credentials",
        "error: 401",
        "error: 403",
        "protected branch",
    ]
    .iter()
    .any(|m| l.contains(m))
}

/// Whether a failed push is the remote rejecting it for a reason of its own that a later try may
/// not meet ("remote rejected", #1247) and not a refusal.
pub fn transient_rejection(e: &str) -> bool {
    !refused(e) && e.to_ascii_lowercase().contains("remote rejected")
}

/// Make `base` point at `commit`.
///
/// With a remote: a plain push, refused by the remote unless it is a
/// fast-forward. Without: a compare-and-swap `update-ref` against the tip
/// last seen at `base_ref`. `wt` is any worktree of the repository (the push
/// runs there).
///
/// A push the remote rejects transiently while the base has not moved is tried again after each
/// wait of `push_backoff` (empty: once). Each new try follows a fetch: a moved base is `Moved`,
/// a base that already holds `commit` is success.
pub fn promote(
    repo: &Path,
    wt: &Path,
    remote: Option<&str>,
    base: &str,
    base_ref: &str,
    commit: &str,
    push_backoff: &[Duration],
) -> Result<(), PromoteError> {
    if let Some(r) = remote {
        let dest = format!("{commit}:refs/heads/{base}");
        let before = git::rev_parse(wt, base_ref).ok();
        let mut waits = push_backoff.iter();
        loop {
            let e = match git::run(wt, &["push", "--porcelain", r, &dest]) {
                Ok(_) => return Ok(()),
                Err(e) => e,
            };
            // Don't read git's words for a race: ask whether the remote's base moved
            // since we looked. If it did, someone else landed first.
            let _ = git::run(wt, &["fetch", "--prune", r]);
            let after = git::rev_parse(wt, base_ref).ok();
            if before.is_some() && after != before {
                return Err(PromoteError::Moved);
            }
            if !transient_rejection(&e.stderr) {
                return Err(PromoteError::Git(e));
            }
            if git::is_ancestor(wt, commit, base_ref) {
                return Ok(());
            }
            let Some(wait) = waits.next() else {
                return Err(PromoteError::Git(e));
            };
            std::thread::sleep(*wait);
        }
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
