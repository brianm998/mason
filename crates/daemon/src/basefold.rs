// SPDX-License-Identifier: GPL-3.0-or-later

//! Fold the base into a develop worktree before a `run_check`, when that cannot disturb the agent.
//!
//! A step brings its branch up to date with the base when it starts (`jobs::prepare`). A develop
//! step can then run for hours on the base as it was, so its checks test against problems the base
//! has since fixed, and a conflict with the base is only found at the test step, which sends the
//! task back. On 2026-11-13 four develop worktrees were 45 to 73 commits behind and would conflict,
//! and five more were behind with work not yet committed.
//!
//! A check is a natural pause: the agent is waiting for the result. Before it runs, the base is
//! merged, but only when the worktree has nothing uncommitted and the merge applies cleanly. That
//! is tested first with `git merge-tree`, which touches neither the worktree nor the index. In
//! every other case the branch is left exactly as it is, the check runs on it, and the agent is
//! told what is waiting and what to do.

use mason_landing::git::{self, CatchUp};
use std::path::Path;
use std::process::Command;

/// What folding the base in before a check did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseFold {
    /// The branch already has every base commit.
    Current,
    /// The base was merged in: this many base commits were new to the branch.
    Merged(u64),
    /// Not merged: the worktree has uncommitted changes. This many base commits are waiting.
    Uncommitted(u64),
    /// Not merged: the merge would conflict in these files (one per line). This many commits wait.
    Conflicts(u64, String),
    /// Not merged: git failed (a lock held, say). The branch is as it was.
    Failed(String),
}

/// Bring `base` into the branch checked out in `wt` if that is safe (see the module notes).
/// The caller holds the repository lock.
pub fn fold(wt: &Path, base: &str) -> BaseFold {
    if git::is_ancestor(wt, base, "HEAD") {
        return BaseFold::Current;
    }
    let behind = git::run(wt, &["rev-list", "--count", &format!("HEAD..{base}")])
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    // `--no-optional-locks`: a plain `git status` takes `index.lock` to refresh the index, and
    // the agent's own `git commit` would then fail on it. Untracked files count: a base that adds
    // the same path would refuse to merge over them.
    match git::run(wt, &["--no-optional-locks", "status", "--porcelain"]) {
        Ok(s) if !s.trim().is_empty() => return BaseFold::Uncommitted(behind),
        Ok(_) => {}
        Err(e) => return BaseFold::Failed(e.to_string()),
    }
    match would_conflict(wt, base) {
        Ok(Some(files)) => return BaseFold::Conflicts(behind, files),
        Ok(None) => {}
        Err(e) => return BaseFold::Failed(e),
    }
    match git::catch_up(wt, base) {
        Ok(CatchUp::UpToDate) => BaseFold::Current,
        Ok(CatchUp::Merged) => BaseFold::Merged(behind),
        // Something changed between the test and the merge; `catch_up` aborted it cleanly.
        Ok(CatchUp::Conflict(files)) => BaseFold::Conflicts(behind, files.trim().to_string()),
        Err(e) => BaseFold::Failed(e.to_string()),
    }
}

/// The files a merge of `base` into `HEAD` would conflict in, or `None` if it merges cleanly.
/// `git merge-tree --write-tree` writes only objects: the worktree and the index are untouched.
fn would_conflict(wt: &Path, base: &str) -> Result<Option<String>, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(wt)
        .args([
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            "HEAD",
            base,
        ])
        .output()
        .map_err(|e| format!("git merge-tree could not start: {e}"))?;
    match out.status.code() {
        Some(0) => Ok(None),
        // Exit 1 is "conflicts": the first line is the tree, then one conflicted file per line.
        Some(1) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let files: Vec<&str> = text
                .lines()
                .skip(1)
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect();
            Ok(Some(files.join("\n")))
        }
        _ => Err(format!(
            "git merge-tree failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
}

/// What the agent is told about it, put before the check's own result. `None` when there is
/// nothing to say.
pub fn note(base: &str, f: &BaseFold) -> Option<String> {
    let commits = |n: u64| {
        if n == 1 {
            "1 commit".to_string()
        } else {
            format!("{n} commits")
        }
    };
    match f {
        BaseFold::Current => None,
        BaseFold::Merged(n) => Some(format!(
            "Before this check, `{base}` was merged into your branch ({} new on the base), so it \
             tests your change on the current base. See what came in with \
             `git log --oneline HEAD^1..HEAD^2`.",
            commits(*n)
        )),
        BaseFold::Uncommitted(n) => Some(format!(
            "`{base}` has {} your branch does not have. They were not merged because your \
             worktree has uncommitted changes; this check ran on your branch as it is. Commit your \
             work, and the next run_check merges them first.",
            commits(*n)
        )),
        BaseFold::Conflicts(n, files) => Some(format!(
            "`{base}` has {} your branch does not have, and merging them conflicts with your \
             branch; this check ran on your branch as it is. {}",
            commits(*n),
            crate::jobs::conflict_note(base, files)
        )),
        BaseFold::Failed(why) => Some(format!(
            "Merging `{base}` before this check failed ({why}); the check ran on your branch as it is."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn write(dir: &Path, f: &str, text: &str) {
        std::fs::write(dir.join(f), text).unwrap();
    }

    fn commit(dir: &Path, f: &str, text: &str) {
        write(dir, f, text);
        git(dir, &["add", f]);
        git(dir, &["commit", "-q", "-m", f]);
    }

    /// A repository on `base` and a worktree on branch `task` cut from it.
    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "base"]);
        commit(&repo, "a.txt", "one\ntwo\nthree\n");
        let wt = tmp.path().join("wt");
        git(
            &repo,
            &["worktree", "add", "-q", "-b", "task", wt.to_str().unwrap()],
        );
        (tmp, repo, wt)
    }

    #[test]
    fn a_branch_that_has_the_base_is_current() {
        let (_t, _repo, wt) = setup();
        commit(&wt, "mine.txt", "x\n");
        assert_eq!(fold(&wt, "base"), BaseFold::Current);
        assert_eq!(note("base", &BaseFold::Current), None);
    }

    #[test]
    fn a_clean_worktree_and_a_clean_merge_get_the_base() {
        let (_t, repo, wt) = setup();
        commit(&wt, "mine.txt", "x\n");
        commit(&repo, "theirs.txt", "y\n");
        commit(&repo, "more.txt", "z\n");
        assert_eq!(fold(&wt, "base"), BaseFold::Merged(2));
        assert!(wt.join("theirs.txt").exists() && wt.join("mine.txt").exists());
        assert_eq!(git(&wt, &["rev-list", "--count", "HEAD..base"]), "0");
        assert!(note("base", &BaseFold::Merged(2))
            .unwrap()
            .contains("2 commits"));
    }

    #[test]
    fn uncommitted_work_is_never_touched() {
        let (_t, repo, wt) = setup();
        commit(&repo, "theirs.txt", "y\n");
        write(&wt, "a.txt", "one\nedited\nthree\n");
        let head = git(&wt, &["rev-parse", "HEAD"]);
        assert_eq!(fold(&wt, "base"), BaseFold::Uncommitted(1));
        assert_eq!(git(&wt, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            std::fs::read_to_string(wt.join("a.txt")).unwrap(),
            "one\nedited\nthree\n"
        );
        assert!(!wt.join("theirs.txt").exists());
    }

    #[test]
    fn an_untracked_file_counts_as_uncommitted() {
        let (_t, repo, wt) = setup();
        commit(&repo, "theirs.txt", "y\n");
        write(&wt, "scratch.txt", "notes\n");
        assert_eq!(fold(&wt, "base"), BaseFold::Uncommitted(1));
    }

    #[test]
    fn a_conflict_is_reported_and_the_worktree_is_left_as_it_was() {
        let (_t, repo, wt) = setup();
        commit(&wt, "a.txt", "one\nmine\nthree\n");
        commit(&repo, "a.txt", "one\ntheirs\nthree\n");
        let head = git(&wt, &["rev-parse", "HEAD"]);
        assert_eq!(fold(&wt, "base"), BaseFold::Conflicts(1, "a.txt".into()));
        assert_eq!(git(&wt, &["rev-parse", "HEAD"]), head);
        assert_eq!(git(&wt, &["status", "--porcelain"]), "");
        let git_dir = PathBuf::from(git(&wt, &["rev-parse", "--absolute-git-dir"]));
        assert!(!git_dir.join("MERGE_HEAD").exists());
        let n = note("base", &BaseFold::Conflicts(1, "a.txt".into())).unwrap();
        assert!(n.contains("1 commit your branch does not have"));
        assert!(n.contains("git merge base") && n.contains("- a.txt"));
    }
}
