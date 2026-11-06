// SPDX-License-Identifier: GPL-3.0-or-later

//! `direct` landing (Part 9): verify locally, fast-forward the base, and push.
//! No PR and no CI. Safe to repeat: landing a branch that is already in the
//! base is a no-op, so a crash at any point resumes without double-landing.
//!
//! The base is only ever moved by fast-forward. If it moved while we worked,
//! the new tip is folded into the task branch and the tests run again, up to
//! a limit.

use crate::git::{self, CatchUp, GitError};
use crate::promote::{promote, PromoteError};
use std::path::{Path, PathBuf};

pub const MAX_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone)]
pub struct DirectLanding<'a> {
    /// Any checkout of the repository (worktrees are made from it).
    pub repo: &'a Path,
    /// The task's worktree, on its branch.
    pub task_wt: &'a Path,
    pub task_branch: &'a str,
    /// A persistent worktree used only for landing, so builds stay incremental
    /// and the owner's checkout is never touched.
    pub landing_wt: &'a Path,
    pub base: &'a str,
    pub remote: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Landed {
    /// Moved the base to this commit.
    Promoted { commit: String },
    /// The branch was already part of the base; nothing to do.
    AlreadyLanded,
}

#[derive(Debug)]
pub enum LandError {
    Git(GitError),
    /// Folding in the moved base conflicts.
    Conflict(String),
    /// Tests are red after folding in the moved base.
    RedAfterFold,
    /// The base kept moving: gave up after [`MAX_ATTEMPTS`].
    BaseKeptMoving,
    /// No remote, and the base branch is checked out somewhere: updating it
    /// would change files under someone. Landing without a remote needs the
    /// `local-only` mode.
    LocalBaseCheckedOut(PathBuf),
}

impl std::fmt::Display for LandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LandError::Git(e) => write!(f, "{e}"),
            LandError::Conflict(files) => {
                write!(f, "folding in the new base conflicts in: {}", files.trim())
            }
            LandError::RedAfterFold => write!(f, "tests are red after folding in the new base"),
            LandError::BaseKeptMoving => {
                write!(f, "the base moved {MAX_ATTEMPTS} times while landing")
            }
            LandError::LocalBaseCheckedOut(p) => {
                write!(
                    f,
                    "the base branch is checked out at {}; use a remote, or the local-only mode",
                    p.display()
                )
            }
        }
    }
}

impl std::error::Error for LandError {}

impl From<GitError> for LandError {
    fn from(e: GitError) -> Self {
        LandError::Git(e)
    }
}

impl DirectLanding<'_> {
    /// The ref to land onto: the remote's base when there is a remote.
    pub fn base_ref(&self) -> String {
        match self.remote {
            Some(r) => format!("{r}/{}", self.base),
            None => self.base.to_string(),
        }
    }

    /// `verify` runs the tests in the task worktree and says whether they pass.
    /// `point` is called at the named boundaries, for crash injection.
    pub fn land(
        &self,
        verify: &mut dyn FnMut() -> bool,
        point: &dyn Fn(&str),
    ) -> Result<Landed, LandError> {
        let base_ref = self.base_ref();
        for _ in 0..MAX_ATTEMPTS {
            if let Some(r) = self.remote {
                git::run(self.repo, &["fetch", "--prune", r])?;
            }
            point("land_after_fetch");

            // Already part of the base: a repeat after a crash, or a no-op branch.
            if git::is_ancestor(self.task_wt, self.task_branch, &base_ref) {
                return Ok(Landed::AlreadyLanded);
            }

            match git::catch_up(self.task_wt, &base_ref)? {
                CatchUp::UpToDate => {}
                CatchUp::Merged => {
                    point("land_after_fold");
                    if !verify() {
                        return Err(LandError::RedAfterFold);
                    }
                }
                CatchUp::Conflict(files) => return Err(LandError::Conflict(files)),
            }

            // Fast-forward a detached landing worktree onto the task branch.
            self.ensure_landing_wt(&base_ref)?;
            git::run(
                self.landing_wt,
                &["checkout", "--detach", "--quiet", &base_ref],
            )?;
            git::run(
                self.landing_wt,
                &["merge", "--ff-only", "--quiet", self.task_branch],
            )?;
            let commit = git::rev_parse(self.landing_wt, "HEAD")?;
            point("land_after_ff");

            match promote(
                self.repo,
                self.landing_wt,
                self.remote,
                self.base,
                &base_ref,
                &commit,
            ) {
                Ok(()) => {
                    point("land_after_push");
                    return Ok(Landed::Promoted { commit });
                }
                Err(PromoteError::Moved) => continue, // someone else landed first: fold and retry
                Err(PromoteError::Git(e)) => return Err(e.into()),
                Err(PromoteError::LocalBaseCheckedOut(p)) => {
                    return Err(LandError::LocalBaseCheckedOut(p))
                }
            }
        }
        Err(LandError::BaseKeptMoving)
    }

    fn ensure_landing_wt(&self, base_ref: &str) -> Result<(), GitError> {
        if self.landing_wt.join(".git").exists() {
            return Ok(());
        }
        let _ = git::run(self.repo, &["worktree", "prune"]);
        if let Some(p) = self.landing_wt.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        git::run(
            self.repo,
            &[
                "worktree",
                "add",
                "--detach",
                &self.landing_wt.display().to_string(),
                base_ref,
            ],
        )
        .map(|_| ())
    }
}
