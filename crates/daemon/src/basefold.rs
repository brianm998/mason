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
pub(crate) fn would_conflict(wt: &Path, base: &str) -> Result<Option<String>, String> {
    let out = mason_landing::git::command()
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

/// How many checks in a row may be told to merge the base by hand. From this one on, the agent is
/// told the base moves faster than a hand merge can keep up, and the branch goes to the landing
/// queue (decision 1212).
pub const MANUAL_MERGE_ROUNDS: u32 = 3;

/// The task-dir file holding one line per conflicted fold since the last clean one.
pub(crate) const FOLD_CONFLICTS_FILE: &str = "fold-conflicts.txt";

/// Record this check's fold in `dir` and return how many conflicted folds in a row there are,
/// this one included. A clean or merged fold empties the record; other outcomes leave it.
pub fn record_fold(dir: &Path, run: u64, fold: &BaseFold, base_tip: &str) -> u32 {
    let path = dir.join(FOLD_CONFLICTS_FILE);
    let count = |p: &Path| {
        std::fs::read_to_string(p)
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count() as u32)
            .unwrap_or(0)
    };
    match fold {
        BaseFold::Current | BaseFold::Merged(_) => {
            let _ = std::fs::remove_file(&path);
            0
        }
        BaseFold::Conflicts(_, files) => {
            use std::io::Write;
            let _ = std::fs::create_dir_all(dir);
            let line = format!(
                "{run}\t{base_tip}\t{}\n",
                files.lines().collect::<Vec<_>>().join(", ")
            );
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let _ = f.write_all(line.as_bytes());
            }
            count(&path)
        }
        BaseFold::Uncommitted(_) | BaseFold::Failed(_) => count(&path),
    }
}

/// Like [`note`], but after [`MANUAL_MERGE_ROUNDS`] conflicted checks in a row it stops asking for
/// a hand merge.
pub fn note_bounded(base: &str, f: &BaseFold, conflicted_in_a_row: u32) -> Option<String> {
    match f {
        BaseFold::Conflicts(n, _) if conflicted_in_a_row >= MANUAL_MERGE_ROUNDS => Some(format!(
            "`{base}` has {n} commit{} your branch does not have, and `{base}` moved faster than a \
             hand merge can keep up ({conflicted_in_a_row} conflicted checks in a row). Do not merge it \
             by hand again; this check ran on your branch as it is. Finish and submit \
             your change: it goes to the landing queue, which folds `{base}` in when it lands and \
             sends a conflict back to you with a bounded count.",
            if *n == 1 { "" } else { "s" }
        )),
        _ => note(base, f),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = mason_landing::git::command()
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

    #[test]
    fn note_bounded_below_the_bound_is_note() {
        let f = BaseFold::Conflicts(2, "a.txt".into());
        assert_eq!(note_bounded("base", &f, 1), note("base", &f));
        assert_eq!(
            note_bounded("base", &f, MANUAL_MERGE_ROUNDS - 1),
            note("base", &f)
        );
        let m = BaseFold::Merged(2);
        assert_eq!(note_bounded("base", &m, 9), note("base", &m));
    }

    #[test]
    fn note_bounded_at_the_bound_stops_asking_for_a_merge() {
        let f = BaseFold::Conflicts(2, "a.txt".into());
        let n = note_bounded("base", &f, MANUAL_MERGE_ROUNDS).unwrap();
        assert!(!n.contains("git merge"), "{n}");
        assert!(n.contains("landing queue"), "{n}");
    }

    #[test]
    fn record_fold_counts_conflicts_and_resets() {
        let dir = tempfile::tempdir().unwrap();
        let c = BaseFold::Conflicts(1, "a.txt\nb.txt".into());
        assert_eq!(record_fold(dir.path(), 1, &c, "abc"), 1);
        assert_eq!(record_fold(dir.path(), 2, &c, "abd"), 2);
        assert_eq!(
            record_fold(dir.path(), 3, &BaseFold::Uncommitted(1), "abd"),
            2
        );
        assert_eq!(record_fold(dir.path(), 4, &c, "abe"), 3);
        let text = std::fs::read_to_string(dir.path().join(FOLD_CONFLICTS_FILE)).unwrap();
        assert!(text.starts_with("1\tabc\ta.txt, b.txt\n"), "{text}");
        assert_eq!(record_fold(dir.path(), 5, &BaseFold::Merged(1), "abe"), 0);
        assert_eq!(record_fold(dir.path(), 6, &c, "abf"), 1);
        assert_eq!(record_fold(dir.path(), 7, &BaseFold::Current, "abf"), 0);
    }

    #[test]
    fn a_conflicted_fold_then_a_clean_one_counts_zero() {
        let (_t, repo, wt) = setup();
        let dir = tempfile::tempdir().unwrap();
        commit(&wt, "a.txt", "one\nmine\nthree\n");
        commit(&repo, "a.txt", "one\ntheirs\nthree\n");
        let f = fold(&wt, "base");
        assert_eq!(record_fold(dir.path(), 1, &f, "x"), 1);
        git(&wt, &["merge", "-q", "-X", "ours", "base"]);
        let f = fold(&wt, "base");
        assert_eq!(f, BaseFold::Current);
        assert_eq!(record_fold(dir.path(), 2, &f, "x"), 0);
    }
}
