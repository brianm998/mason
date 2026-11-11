// SPDX-License-Identifier: GPL-3.0-or-later

//! A batch on an integration branch (Part 9). The task branches are merged,
//! one by one, into a disposable branch in a persistent worktree; the suites
//! run once over the union; the result is promoted by fast-forward.
//!
//! Every step can be repeated: assembling resets the integration branch to
//! the base and merges again, a merge of something already merged does
//! nothing, and a task already in the base is reported as landed. So a crash
//! at any point resumes by running the same steps over.

use crate::audit::{self, Dropped, Removal};
use crate::git::{self, CatchUp, GitError};
use crate::promote::{promote, PromoteError};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug)]
pub struct BatchTask {
    pub id: u64,
    pub branch: String,
    /// Base lines the task removes on purpose (its submission's `removes`).
    pub removes: Vec<Removal>,
}

/// The subject of the commit that regenerates the README status block.
pub const STATUS_SUBJECT: &str = "Regenerate the README status block";

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

/// A task whose conflict resolution dropped lines the base had landed: its merge was undone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unkept {
    pub id: u64,
    pub lines: Vec<Dropped>,
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
    /// Resolved, but the resolution dropped base lines: the merge was undone, so the
    /// task is in none of the lists above. Sent back to restore them.
    pub unkept: Vec<Unkept>,
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

/// The status script reads stage notes and rewrites one block: well under a second.
pub const STATUS_DEADLINE: std::time::Duration = std::time::Duration::from_secs(300);

/// The resolver may be an agent working through a conflict. `MASON_RESOLVER_TIMEOUT_SECS`
/// changes it.
pub fn resolver_deadline() -> std::time::Duration {
    std::env::var("MASON_RESOLVER_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|s| *s > 0.0)
        .map_or(
            std::time::Duration::from_secs(3600),
            std::time::Duration::from_secs_f64,
        )
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
        git::run(
            wt,
            &["checkout", "--force", "-B", self.int_branch, &base_tip],
        )?;
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
            let before = git::rev_parse(wt, "HEAD")?;
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
                        // Whatever the resolver did, the base's landed lines must still be there.
                        let mb = git::run(wt, &["merge-base", &t.branch, &base_tip])?;
                        let lines = audit::audit(
                            wt,
                            &mb,
                            &base_tip,
                            &t.branch,
                            None,
                            "HEAD",
                            &t.removes,
                        )?;
                        if lines.is_empty() {
                            out.merged.push(t.id);
                            out.resolved.push(t.id);
                        } else {
                            git::run(wt, &["reset", "--hard", "--quiet", &before])?;
                            git::run(wt, &["clean", "-fd", "--quiet"])?;
                            out.unkept.push(Unkept { id: t.id, lines });
                        }
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

    /// Regenerate the README status block from the stage notes with the repository's
    /// own `scripts/gen-status.sh`, and commit it on the integration branch. Tasks never
    /// edit the block, so two tasks never touch the same line. Does nothing when the
    /// repository has no such script. Returns true if a commit was made.
    pub fn regenerate_status(&self) -> bool {
        let wt = self.int_wt;
        if !wt.join("scripts/gen-status.sh").is_file() {
            return false;
        }
        let ran = crate::proc::output(
            Command::new("bash")
                .args(["scripts/gen-status.sh"])
                .current_dir(wt),
            STATUS_DEADLINE,
        );
        if !matches!(ran, Ok(o) if o.status.success()) {
            return false;
        }
        let dirty = git::run(wt, &["status", "--porcelain", "--", "README.md"]).unwrap_or_default();
        if dirty.trim().is_empty() {
            return false;
        }
        git::run(wt, &["add", "README.md"]).is_ok()
            && git::run(wt, &["commit", "-q", "-s", "-m", STATUS_SUBJECT]).is_ok()
    }

    /// Take off the regeneration commit, if the integration head is one, so a moved base
    /// folds in without a conflict in the generated block; it is made again after.
    pub fn drop_status_commit(&self) {
        let wt = self.int_wt;
        if git::run(wt, &["log", "-1", "--format=%s"]).is_ok_and(|s| s == STATUS_SUBJECT) {
            let _ = git::run(wt, &["reset", "--hard", "--quiet", "HEAD~1"]);
        }
    }

    /// Run the resolver on a conflicted merge. True if it left the merge
    /// complete: no conflict markers left unmerged, committed.
    fn resolve(&self, task: u64, files: &str) -> bool {
        let Some(cmd) = self.resolver else {
            return false;
        };
        let ran = crate::proc::output(
            Command::new("sh")
                .args(["-c", cmd])
                .current_dir(self.int_wt)
                .env("MASON_TASK", task.to_string())
                .env("MASON_CONFLICT_FILES", files.trim().replace('\n', " "))
                .env("GIT_TERMINAL_PROMPT", "0"),
            resolver_deadline(),
        );
        if !matches!(ran, Ok(o) if o.status.success()) {
            return false;
        }
        let wt = self.int_wt;
        // Staging marks the files resolved, so look for leftover markers after.
        if git::run(wt, &["add", "-A"]).is_err() {
            return false;
        }
        if git::run(wt, &["diff", "--cached", "--check"])
            .is_err_and(|e| e.stderr.contains("conflict marker"))
        {
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

    /// Promote the verified batch with no lock to take (a single caller owns the
    /// repository). See [`Batch::finish_with`].
    pub fn finish(
        &self,
        a: &Assembled,
        reverify: &mut dyn FnMut(&[String]) -> bool,
        point: &dyn Fn(&str),
    ) -> Result<Result<Finish, RedAfterFold>, GitError> {
        self.finish_with(a, &|| Box::new(()), reverify, point)
    }

    /// Promote the verified batch. The caller's lock is taken only around git: once
    /// for the fold (fetch, compare, catch up, regenerate the status block) and once
    /// for the promote (fetch, compare, push). If the base moved since assembly, it
    /// is folded in and `reverify(moved_paths)` runs between the two with the lock
    /// released; if it says the tree is red, return `Err(RedAfterFold)`. A tree is
    /// promoted only when the base, fetched under the lock, is still the one the
    /// tree was last verified on; if it moved during the re-verification the loop
    /// folds and re-verifies again, up to `max_fold`. `Ok(Ok(..))` means landed.
    /// `point` marks the crash boundaries.
    pub fn finish_with<'l>(
        &self,
        a: &Assembled,
        lock: &dyn Fn() -> Guard<'l>,
        reverify: &mut dyn FnMut(&[String]) -> bool,
        point: &dyn Fn(&str),
    ) -> Result<Result<Finish, RedAfterFold>, GitError> {
        let base_ref = self.base_ref();
        let wt = self.int_wt;
        // The base tip the tree in the worktree has been verified on (or built on).
        let mut seen = a.base_tip.clone();
        let mut folds = 0;
        loop {
            // -- fold, under the lock
            let moved = {
                let _g = lock();
                self.fetch()?;
                let tip = git::rev_parse(self.repo, &base_ref)?;
                if tip != seen {
                    // The base moved while the batch was verified: fold it in.
                    self.drop_status_commit();
                    match git::catch_up(wt, &base_ref)? {
                        CatchUp::Conflict(files) => return Ok(Ok(Finish::Conflict(files))),
                        CatchUp::UpToDate | CatchUp::Merged => {
                            self.regenerate_status();
                        }
                    }
                    folds += 1;
                    if folds > self.max_fold {
                        return Ok(Ok(Finish::BaseKeptMoving));
                    }
                    let moved = self.changed(&seen, &tip);
                    seen = tip;
                    Some(moved)
                } else {
                    None
                }
            };
            // -- re-verify, lock released: the worktree belongs to this batch's slot
            if let Some(moved) = moved {
                if !reverify(&moved) {
                    return Ok(Err(RedAfterFold));
                }
            }
            // -- promote, under the lock
            let _g = lock();
            self.fetch()?;
            let tip = git::rev_parse(self.repo, &base_ref)?;
            let commit = git::rev_parse(wt, "HEAD")?;
            if git::is_ancestor(wt, &commit, &base_ref) {
                // Already promoted (a repeat after a crash).
                return Ok(Ok(Finish::Promoted { commit, folds }));
            }
            if tip != seen {
                // The base moved during the re-verification: this tree was not
                // verified on it. Fold again.
                continue;
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

/// What a caller's lock hands back: dropping it releases the lock.
pub trait Held {}
impl<T> Held for T {}
pub type Guard<'a> = Box<dyn Held + 'a>;

/// Re-verification after folding in a moved base came out red.
#[derive(Debug, PartialEq, Eq)]
pub struct RedAfterFold;
