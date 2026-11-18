// SPDX-License-Identifier: GPL-3.0-or-later

//! Bring a fix that landed on the base into a develop worktree whose agent is waiting (s525).
//!
//! On 2026-11-16 develop's matrix red was fixed at 16:01, yet ten of fifteen develop worktrees
//! still had the bug: an agent picks the base up only when it merges by itself, and an agent whose
//! `run_check` sits in the queue for a slot is not going to. Its check would run, fail on the
//! known bug, and send the agent after a test that is not its own.
//!
//! The known-red ledger says which tests are red on the base. A check that failed some of them
//! remembers which ([`Reds::saw`]). While a later check is queued, [`Reds::sync`] looks at the
//! ledger again. When one of those tests has left it, the base has been proven green for it, and
//! the base is merged into the worktree, provided the agent is idle in the way a queued check
//! makes it: no uncommitted tracked change, no merge in progress, and a merge that applies
//! cleanly. Otherwise the worktree is left exactly as it was. The agent is told on the check's
//! result what was merged and why.

use crate::basefold::BaseFold;
use mason_landing::git;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// The known-red tests the last checks of one worktree failed, and what was merged for them.
#[derive(Default)]
pub struct Reds {
    /// `(scope, test)`: the suite or test step the ledger keeps the test under, and its name.
    red: Mutex<Vec<(String, String)>>,
    notes: Mutex<Vec<String>>,
    merges: AtomicU64,
}

impl Reds {
    /// A check of `scope` failed `failing`; `known` are the tests the ledger holds for the scope.
    /// What was remembered for the scope before is replaced.
    pub fn saw(&self, scope: &str, failing: &[String], known: &[String]) {
        let mut red = self.red.lock().unwrap_or_else(|e| e.into_inner());
        red.retain(|(s, _)| s != scope);
        for t in failing.iter().filter(|t| known.contains(t)) {
            red.push((scope.to_string(), t.clone()));
        }
    }

    /// A check of `scope` passed: nothing is waiting on a fix for it.
    pub fn passed(&self, scope: &str) {
        self.red
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(s, _)| s != scope);
    }

    /// How many merges this worktree has had from here. A check that sees it change while it
    /// waited has a different tree from the one it started with.
    pub fn merges(&self) -> u64 {
        self.merges.load(Ordering::SeqCst)
    }

    /// What was done since the last call, to put before a check's result.
    pub fn take_notes(&self) -> Vec<String> {
        std::mem::take(&mut *self.notes.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Is any test remembered? (False means [`Reds::sync`] has nothing to look at.)
    pub fn any(&self) -> bool {
        !self
            .red
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }

    /// If a remembered test is no longer known red on the base (`still_known(scope)` lists the
    /// ledger's tests for a scope), merge `base` into `wt` when that is safe. The caller holds the
    /// repository lock and has the agent waiting in a queued check. Returns what was done.
    pub fn sync(
        &self,
        wt: &Path,
        base: &str,
        still_known: &dyn Fn(&str) -> Vec<String>,
    ) -> Option<BaseFold> {
        let fixed: Vec<String> = {
            let mut red = self.red.lock().unwrap_or_else(|e| e.into_inner());
            let mut fixed = vec![];
            red.retain(|(scope, test)| {
                if still_known(scope).contains(test) {
                    return true;
                }
                fixed.push(test.clone());
                false
            });
            fixed
        };
        if fixed.is_empty() {
            return None;
        }
        let fold = merge_clean(wt, base);
        let why = format!(
            "`{base}` fixed {} (red there until now, and failing in your last check)",
            fixed.join(", ")
        );
        let note = match &fold {
            BaseFold::Current => return Some(fold),
            BaseFold::Merged(n) => {
                self.merges.fetch_add(1, Ordering::SeqCst);
                Some(format!(
                    "While your check was queued, {why}. `{base}` was merged into your branch \
                     ({} new on the base; a merge commit, signed off), so the check runs with the \
                     fix. See what came in with `git log --oneline HEAD^1..HEAD^2`.",
                    if *n == 1 {
                        "1 commit".to_string()
                    } else {
                        format!("{n} commits")
                    }
                ))
            }
            other => crate::basefold::note(base, other).map(|n| {
                format!("While your check was queued, {why}. It was not merged for you. {n}")
            }),
        };
        if let Some(n) = note {
            self.notes.lock().unwrap_or_else(|e| e.into_inner()).push(n);
        }
        Some(fold)
    }
}

/// Merge `base` into the branch in `wt` if nothing of the agent's can be disturbed: no merge in
/// progress, no uncommitted change to a tracked file, and a merge that `git merge-tree` finds
/// clean. Any other case leaves the worktree untouched.
pub fn merge_clean(wt: &Path, base: &str) -> BaseFold {
    if git::is_ancestor(wt, base, "HEAD") {
        return BaseFold::Current;
    }
    let behind = git::run(wt, &["rev-list", "--count", &format!("HEAD..{base}")])
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    if git::run(wt, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok() {
        return BaseFold::Failed("a merge is in progress in your worktree".into());
    }
    // `--no-optional-locks`: a plain `git status` would take `index.lock` and fail the agent's
    // own commit. Untracked files do not matter: a merge that would overwrite one refuses.
    match git::run(
        wt,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain",
            "--untracked-files=no",
        ],
    ) {
        Ok(s) if !s.trim().is_empty() => return BaseFold::Uncommitted(behind),
        Ok(_) => {}
        Err(e) => return BaseFold::Failed(e.to_string()),
    }
    match crate::basefold::would_conflict(wt, base) {
        Ok(Some(files)) => return BaseFold::Conflicts(behind, files),
        Ok(None) => {}
        Err(e) => return BaseFold::Failed(e),
    }
    match git::run(wt, &["merge", "--no-edit", "--signoff", base]) {
        Ok(_) => BaseFold::Merged(behind),
        Err(e) => {
            let _ = git::run(wt, &["merge", "--abort"]);
            BaseFold::Failed(e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

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

    fn commit(dir: &Path, f: &str, text: &str) {
        std::fs::write(dir.join(f), text).unwrap();
        git(dir, &["add", f]);
        git(dir, &["commit", "-q", "-m", f]);
    }

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
        commit(&wt, "mine.txt", "x\n");
        (tmp, repo, wt)
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// The ledger no longer holds anything: every remembered test has been fixed.
    fn nothing(_: &str) -> Vec<String> {
        vec![]
    }

    fn reds() -> Reds {
        let r = Reds::default();
        r.saw("suite", &s(&["m::t", "mine::t"]), &s(&["m::t"]));
        r
    }

    #[test]
    fn an_idle_clean_worktree_gets_the_fix_and_is_told() {
        let (_t, repo, wt) = setup();
        commit(&repo, "fix.txt", "fixed\n");
        let r = reds();
        assert!(r.any(), "only the known-red test is remembered");
        let before = r.merges();
        assert_eq!(r.sync(&wt, "base", &nothing), Some(BaseFold::Merged(1)));
        assert!(wt.join("fix.txt").exists() && wt.join("mine.txt").exists());
        assert_eq!(git(&wt, &["rev-list", "--count", "HEAD..base"]), "0");
        assert_eq!(r.merges(), before + 1);
        // A merge commit with a sign-off, and the agent's own commit is still under it.
        let msg = git(&wt, &["log", "-1", "--format=%B"]);
        assert!(msg.contains("Signed-off-by:"), "{msg}");
        assert_eq!(
            git(&wt, &["rev-list", "--count", "--merges", "HEAD^1..HEAD"]),
            "1"
        );
        let notes = r.take_notes();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("m::t") && notes[0].contains("merged into your branch"));
        assert!(r.take_notes().is_empty(), "a note is told once");
        assert!(!r.any(), "and the test is not watched again");
    }

    #[test]
    fn a_test_still_known_red_merges_nothing() {
        let (_t, repo, wt) = setup();
        commit(&repo, "fix.txt", "fixed\n");
        let head = git(&wt, &["rev-parse", "HEAD"]);
        let r = reds();
        assert_eq!(r.sync(&wt, "base", &|_| s(&["m::t"])), None);
        assert_eq!(git(&wt, &["rev-parse", "HEAD"]), head);
        assert!(r.any());
    }

    #[test]
    fn a_dirty_worktree_is_left_exactly_as_it_was() {
        let (_t, repo, wt) = setup();
        commit(&repo, "fix.txt", "fixed\n");
        std::fs::write(wt.join("mine.txt"), "edited\n").unwrap();
        let head = git(&wt, &["rev-parse", "HEAD"]);
        let r = reds();
        assert_eq!(
            r.sync(&wt, "base", &nothing),
            Some(BaseFold::Uncommitted(1))
        );
        assert_eq!(git(&wt, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            std::fs::read_to_string(wt.join("mine.txt")).unwrap(),
            "edited\n"
        );
        assert!(!wt.join("fix.txt").exists());
        assert_eq!(r.merges(), 0);
        let n = r.take_notes();
        assert!(
            n[0].contains("not merged") && n[0].contains("uncommitted"),
            "{n:?}"
        );
    }

    #[test]
    fn a_merge_in_progress_is_left_alone() {
        let (_t, repo, wt) = setup();
        commit(&repo, "a.txt", "one\ntheirs\nthree\n");
        commit(&wt, "a.txt", "one\nmine\nthree\n");
        // The agent is resolving a merge of the base itself.
        let out = mason_landing::git::command()
            .arg("-C")
            .arg(&wt)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
                "merge",
                "--no-edit",
                "base",
            ])
            .env("GIT_EDITOR", "true")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .unwrap();
        let said = format!(
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!out.status.success(), "the merge should conflict: {said}");
        git(&wt, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]);
        assert!(
            git(&wt, &["status", "--porcelain"]).contains("UU a.txt"),
            "no unmerged entry after the merge: {said}"
        );
        let status = git(&wt, &["status", "--porcelain"]);
        let r = reds();
        assert!(matches!(
            r.sync(&wt, "base", &nothing),
            Some(BaseFold::Failed(_)) | Some(BaseFold::Uncommitted(_))
        ));
        assert_eq!(git(&wt, &["status", "--porcelain"]), status);
        assert!(std::fs::read_to_string(wt.join("a.txt"))
            .unwrap()
            .contains("<<<<<<<"));
        assert_eq!(r.merges(), 0);
    }

    #[test]
    fn a_merge_that_conflicts_is_left_exactly_as_it_was() {
        let (_t, repo, wt) = setup();
        commit(&wt, "a.txt", "one\nmine\nthree\n");
        commit(&repo, "a.txt", "one\ntheirs\nthree\n");
        let head = git(&wt, &["rev-parse", "HEAD"]);
        let r = reds();
        assert_eq!(
            r.sync(&wt, "base", &nothing),
            Some(BaseFold::Conflicts(1, "a.txt".into()))
        );
        assert_eq!(git(&wt, &["rev-parse", "HEAD"]), head);
        assert_eq!(git(&wt, &["status", "--porcelain"]), "");
        let git_dir = PathBuf::from(git(&wt, &["rev-parse", "--absolute-git-dir"]));
        assert!(!git_dir.join("MERGE_HEAD").exists());
        assert_eq!(r.merges(), 0);
        assert!(r.take_notes()[0].contains("- a.txt"));
    }

    #[test]
    fn a_failure_that_was_not_known_red_is_never_watched() {
        let (_t, repo, wt) = setup();
        commit(&repo, "fix.txt", "fixed\n");
        let r = Reds::default();
        r.saw("suite", &s(&["mine::t"]), &s(&["other::t"]));
        assert!(!r.any());
        assert_eq!(r.sync(&wt, "base", &nothing), None);
        assert!(!wt.join("fix.txt").exists());
        r.saw("suite", &s(&["other::t"]), &s(&["other::t"]));
        r.passed("suite");
        assert!(!r.any(), "a green check clears what it watched");
    }

    #[test]
    fn the_next_check_runs_on_the_fixed_tree() {
        // What a queued check sees after the merge: the fix is in the worktree it runs in.
        let (_t, repo, wt) = setup();
        commit(&repo, "fix.txt", "fixed\n");
        let r = reds();
        let key = |w: &Path| git(w, &["rev-parse", "HEAD^{tree}"]);
        let before = key(&wt);
        r.sync(&wt, "base", &nothing);
        assert_ne!(key(&wt), before);
        assert_eq!(
            std::fs::read_to_string(wt.join("fix.txt")).unwrap(),
            "fixed\n"
        );
    }
}
