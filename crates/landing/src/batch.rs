// SPDX-License-Identifier: GPL-3.0-or-later

//! A batch on an integration branch (Part 9). The task branches are merged,
//! one by one, into a disposable branch in a persistent worktree; the suites
//! run once over the union; the result is promoted by fast-forward.
//!
//! Every step can be repeated: assembling resets the integration branch to
//! the base and merges again, a merge of something already merged does
//! nothing, and a task already in the base is reported as landed. So a crash
//! at any point resumes by running the same steps over.

use crate::git::{self, CatchUp, GitError};
use crate::promote::{promote, PromoteError};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug)]
pub struct BatchTask {
    pub id: u64,
    pub branch: String,
}

#[derive(Clone, Debug)]
pub struct Batch<'a> {
    /// Any checkout of the repository.
    pub repo: &'a Path,
    /// The persistent integration worktree.
    pub int_wt: &'a Path,
    /// `mason/int/<project>/b<id>`: disposable.
    pub int_branch: &'a str,
    pub base: &'a str,
    pub remote: Option<&'a str>,
    /// A command that resolves a merge conflict in the worktree.
    pub resolver: Option<&'a str>,
    pub max_fold: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ejected {
    pub id: u64,
    pub files: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Assembled {
    /// The base tip the batch was built on.
    pub base_tip: String,
    /// The integration head.
    pub head: String,
    /// Merged cleanly, or resolved by the resolver, in order.
    pub merged: Vec<u64>,
    /// Of `merged`, those the resolver had to resolve.
    pub resolved: Vec<u64>,
    /// Already in the base: nothing to merge, nothing to land.
    pub already: Vec<u64>,
    /// Could not be merged (the resolver failed or there is none): sent back.
    pub ejected: Vec<Ejected>,
}

#[derive(Debug)]
pub enum Finish {
    /// The base now points at `commit` (or the batch was already in it).
    Promoted { commit: String, folds: u32 },
    /// Folding in a moved base conflicts.
    Conflict(String),
    /// The base kept moving.
    BaseKeptMoving,
    /// Nobody may move a checked-out local base.
    LocalBaseCheckedOut(PathBuf),
}

impl Batch<'_> {
    pub fn base_ref(&self) -> String {
        match self.remote {
            Some(r) => format!("{r}/{}", self.base),
            None => self.base.to_string(),
        }
    }

    fn fetch(&self) -> Result<(), GitError> {
        if let Some(r) = self.remote {
            git::run(self.repo, &["fetch", "--prune", r])?;
        }
        Ok(())
    }

    fn ensure_wt(&self, start: &str) -> Result<(), GitError> {
        if self.int_wt.join(".git").exists() {
            return Ok(());
        }
        let _ = git::run(self.repo, &["worktree", "prune"]);
        if let Some(p) = self.int_wt.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        // Detached first: the branch is (re)created below by `checkout -B`.
        git::run(
            self.repo,
            &[
                "worktree",
                "add",
                "--detach",
                &self.int_wt.display().to_string(),
                start,
            ],
        )
        .map(|_| ())
    }

    /// Reset the integration branch to the base and merge each task in order.
    pub fn assemble(
        &self,
        tasks: &[BatchTask],
        point: &dyn Fn(&str),
    ) -> Result<Assembled, GitError> {
        self.fetch()?;
        let base_ref = self.base_ref();
        let base_tip = git::rev_parse(self.repo, &base_ref)?;
        self.ensure_wt(&base_tip)?;
        let wt = self.int_wt;
        // A crash may have left a merge half done.
        let _ = git::run(wt, &["merge", "--abort"]);
        git::run(wt, &["checkout", "--force", "-B", self.int_branch, &base_tip])?;
        git::run(wt, &["reset", "--hard", "--quiet", &base_tip])?;
        // Untracked files go; ignored ones (build caches) stay so builds are incremental.
        git::run(wt, &["clean", "-fd", "--quiet"])?;

        let mut out = Assembled {
            base_tip: base_tip.clone(),
            ..Default::default()
        };
        for t in tasks {
            if git::is_ancestor(wt, &t.branch, &base_tip) {
                out.already.push(t.id);
                continue;
            }
            match git::run(wt, &["merge", "--no-edit", "--no-ff", &t.branch]) {
                Ok(_) => out.merged.push(t.id),
                Err(e) => {
                    let files = git::run(wt, &["diff", "--name-only", "--diff-filter=U"])
                        .unwrap_or_default();
                    if files.trim().is_empty() {
                        let _ = git::run(wt, &["merge", "--abort"]);
                        return Err(e);
                    }
                    if self.resolve(t.id, &files) {
                        out.merged.push(t.id);
                        out.resolved.push(t.id);
                    } else {
                        let _ = git::run(wt, &["merge", "--abort"]);
                        let _ = git::run(wt, &["reset", "--hard", "--quiet"]);
                        out.ejected.push(Ejected {
                            id: t.id,
                            files: files.trim().replace('\n', ", "),
                        });
                    }
                }
            }
        }
        out.head = git::rev_parse(wt, "HEAD")?;
        point("batch_merged");
        Ok(out)
    }

    /// Run the resolver on a conflicted merge. True if it left the merge
    /// complete: no conflict markers left unmerged, committed.
    fn resolve(&self, task: u64, files: &str) -> bool {
        let Some(cmd) = self.resolver else {
            return false;
        };
        let ran = Command::new("sh")
            .args(["-c", cmd])
            .current_dir(self.int_wt)
            .env("MASON_TASK", task.to_string())
            .env("MASON_CONFLICT_FILES", files.trim().replace('\n', " "))
            .env("GIT_TERMINAL_PROMPT", "0")
            .output();
        if !matches!(ran, Ok(o) if o.status.success()) {
            return false;
        }
        let wt = self.int_wt;
        // Staging marks the files resolved, so look for leftover markers after.
        if git::run(wt, &["add", "-A"]).is_err() {
            return false;
        }
        if git::run(wt, &["diff", "--cached", "--check"]).is_err_and(|e| e.stderr.contains("conflict marker")) {
            return false;
        }
        // The resolver may have committed itself (MERGE_HEAD gone) or left it to us.
        let merging = git::ok(wt, &["rev-parse", "--verify", "--quiet", "MERGE_HEAD"]);
        !merging || git::run(wt, &["commit", "--no-edit"]).is_ok()
    }

    /// Paths changed between two commits.
    pub fn changed(&self, from: &str, to: &str) -> Vec<String> {
        git::run(
            self.int_wt,
            &["diff", "--name-only", &format!("{from}..{to}")],
        )
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
    }

    /// The union diff of the batch: everything it changes against its base.
    pub fn union_diff(&self, a: &Assembled) -> Vec<String> {
        self.changed(&a.base_tip, &a.head)
    }

    /// Promote the verified batch under the caller's lock. If the base moved
    /// since assembly, fold it in and call `reverify(moved_paths)`; if that
    /// says the suites must run again and they fail, return `Err(false)`.
    /// `Ok(true)` verdict means landed. `point` marks the crash boundaries.
    pub fn finish(
        &self,
        a: &Assembled,
        reverify: &mut dyn FnMut(&[String]) -> bool,
        point: &dyn Fn(&str),
    ) -> Result<Result<Finish, RedAfterFold>, GitError> {
        let base_ref = self.base_ref();
        let wt = self.int_wt;
        let mut seen = a.base_tip.clone();
        let mut folds = 0;
        loop {
            self.fetch()?;
            let tip = git::rev_parse(self.repo, &base_ref)?;
            if tip != seen {
                // The base moved while the batch was verified: fold it in.
                match git::catch_up(wt, &base_ref)? {
                    CatchUp::Conflict(files) => return Ok(Ok(Finish::Conflict(files))),
                    CatchUp::UpToDate | CatchUp::Merged => {}
                }
                folds += 1;
                if folds > self.max_fold {
                    return Ok(Ok(Finish::BaseKeptMoving));
                }
                let moved = self.changed(&seen, &tip);
                seen = tip;
                if !reverify(&moved) {
                    return Ok(Err(RedAfterFold));
                }
            }
            let commit = git::rev_parse(wt, "HEAD")?;
            if git::is_ancestor(wt, &commit, &base_ref) {
                // Already promoted (a repeat after a crash).
                return Ok(Ok(Finish::Promoted { commit, folds }));
            }
            point("batch_before_promote");
            match promote(self.repo, wt, self.remote, self.base, &base_ref, &commit) {
                Ok(()) => {
                    point("batch_after_promote");
                    return Ok(Ok(Finish::Promoted { commit, folds }));
                }
                Err(PromoteError::Moved) => {
                    // Someone else landed first: look again, fold, retry.
                    folds += 1;
                    if folds > self.max_fold {
                        return Ok(Ok(Finish::BaseKeptMoving));
                    }
                    let _ = self.fetch();
                    continue;
                }
                Err(PromoteError::Git(e)) => return Err(e),
                Err(PromoteError::LocalBaseCheckedOut(p)) => {
                    return Ok(Ok(Finish::LocalBaseCheckedOut(p)))
                }
            }
        }
    }
}

/// Re-verification after folding in a moved base came out red.
#[derive(Debug, PartialEq, Eq)]
pub struct RedAfterFold;
