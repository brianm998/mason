// SPDX-License-Identifier: GPL-3.0-or-later

//! Real git, real worktrees, a bare repository standing in for the remote.

use mason_landing::direct::{DirectLanding, LandError, Landed, MAX_ATTEMPTS};
use mason_landing::git::{self, CatchUp};
use std::cell::Cell;
use std::path::{Path, PathBuf};

struct Fx {
    _d: tempfile::TempDir,
    origin: PathBuf,
    repo: PathBuf,
    wt: PathBuf,
    landing: PathBuf,
}

fn sh(cwd: &Path, args: &[&str]) -> String {
    git::run(cwd, args).unwrap_or_else(|e| panic!("{e}"))
}

fn commit_file(cwd: &Path, name: &str, body: &str, msg: &str) -> String {
    std::fs::write(cwd.join(name), body).unwrap();
    sh(cwd, &["add", "-A"]);
    sh(cwd, &["commit", "-q", "-m", msg]);
    sh(cwd, &["rev-parse", "HEAD"])
}

impl Fx {
    fn new() -> Fx {
        let d = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(d.path()).unwrap();
        let origin = root.join("origin.git");
        let repo = root.join("repo");
        std::fs::create_dir_all(&origin).unwrap();
        sh(&origin, &["init", "-q", "--bare", "-b", "main"]);
        sh(&root, &["clone", "-q", origin.to_str().unwrap(), "repo"]);
        sh(&repo, &["checkout", "-q", "-b", "main"]);
        commit_file(&repo, "README", "hi\n", "init");
        sh(&repo, &["push", "-q", "origin", "main"]);
        Fx {
            origin,
            repo,
            wt: root.join("wt/t1"),
            landing: root.join("land"),
            _d: d,
        }
    }

    /// A task branch with one commit, in its own worktree.
    fn task(&self, file: &str) -> String {
        git::ensure_worktree(&self.repo, &self.wt, "mason/t1", "origin/main").unwrap();
        commit_file(&self.wt, file, "work\n", &format!("add {file}"))
    }

    fn landing(&self, remote: bool) -> DirectLanding<'_> {
        DirectLanding {
            repo: &self.repo,
            task_wt: &self.wt,
            task_branch: "mason/t1",
            landing_wt: &self.landing,
            base: "main",
            remote: remote.then_some("origin"),
        }
    }

    /// Another person pushes to the remote's main.
    fn someone_else_pushes(&self, file: &str, body: &str) -> String {
        let other = self.repo.parent().unwrap().join(format!("other-{file}"));
        sh(
            self.repo.parent().unwrap(),
            &[
                "clone",
                "-q",
                self.origin.to_str().unwrap(),
                other.to_str().unwrap(),
            ],
        );
        let c = commit_file(&other, file, body, &format!("other {file}"));
        sh(&other, &["push", "-q", "origin", "HEAD:main"]);
        c
    }

    fn origin_main_log(&self) -> Vec<String> {
        sh(&self.origin, &["log", "--format=%s", "main"])
            .lines()
            .map(String::from)
            .collect()
    }
}

fn no_point(_: &str) {}

#[test]
fn ensure_worktree_is_idempotent_and_finds_an_existing_branch_again() {
    let fx = Fx::new();
    git::ensure_worktree(&fx.repo, &fx.wt, "mason/t1", "origin/main").unwrap();
    git::ensure_worktree(&fx.repo, &fx.wt, "mason/t1", "origin/main").unwrap(); // second call: no-op
    assert!(git::branch_exists(&fx.repo, "mason/t1"));
    // Lose the worktree directory but keep the branch (a crash, or cleanup): it comes back.
    let tip = commit_file(&fx.wt, "x", "1", "x");
    git::remove_worktree(&fx.repo, &fx.wt);
    assert!(!fx.wt.exists());
    git::ensure_worktree(&fx.repo, &fx.wt, "mason/t1", "origin/main").unwrap();
    assert_eq!(git::rev_parse(&fx.wt, "HEAD").unwrap(), tip);
}

#[test]
fn catch_up_reports_up_to_date_merged_and_conflict() {
    let fx = Fx::new();
    fx.task("a.txt");
    sh(&fx.repo, &["fetch", "-q", "origin"]);
    assert_eq!(
        git::catch_up(&fx.wt, "origin/main").unwrap(),
        CatchUp::UpToDate
    );

    fx.someone_else_pushes("b.txt", "b\n");
    sh(&fx.repo, &["fetch", "-q", "origin"]);
    assert_eq!(
        git::catch_up(&fx.wt, "origin/main").unwrap(),
        CatchUp::Merged
    );
    assert!(fx.wt.join("b.txt").exists() && fx.wt.join("a.txt").exists());

    // A real conflict: both sides change the same line of the same file.
    commit_file(&fx.wt, "c.txt", "mine\n", "mine");
    fx.someone_else_pushes("c.txt", "theirs\n");
    sh(&fx.repo, &["fetch", "-q", "origin"]);
    match git::catch_up(&fx.wt, "origin/main").unwrap() {
        CatchUp::Conflict(files) => assert!(files.contains("c.txt")),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        sh(&fx.wt, &["status", "--porcelain"]),
        "",
        "the aborted merge leaves a clean tree"
    );
}

#[test]
fn direct_landing_fast_forwards_and_pushes() {
    let fx = Fx::new();
    let tip = fx.task("a.txt");
    let ran = Cell::new(0);
    let r = fx
        .landing(true)
        .land(
            &mut || {
                ran.set(ran.get() + 1);
                true
            },
            &no_point,
        )
        .unwrap();
    assert_eq!(
        r,
        Landed::Promoted {
            commit: tip.clone()
        }
    );
    assert_eq!(ran.get(), 0, "no tests re-run when the base did not move");
    assert_eq!(sh(&fx.origin, &["rev-parse", "main"]), tip);
    assert_eq!(fx.origin_main_log(), vec!["add a.txt", "init"]);
}

#[test]
fn landing_the_same_branch_again_does_nothing() {
    let fx = Fx::new();
    fx.task("a.txt");
    assert!(matches!(
        fx.landing(true).land(&mut || true, &no_point).unwrap(),
        Landed::Promoted { .. }
    ));
    // A crash after the push, before the daemon recorded it: the job runs again.
    let again = fx.landing(true).land(&mut || true, &no_point).unwrap();
    assert_eq!(again, Landed::AlreadyLanded);
    assert_eq!(
        fx.origin_main_log(),
        vec!["add a.txt", "init"],
        "landed exactly once"
    );
}

#[test]
fn a_moved_base_is_folded_in_and_verified_again() {
    let fx = Fx::new();
    fx.task("a.txt");
    fx.someone_else_pushes("b.txt", "b\n");
    let ran = Cell::new(0);
    let r = fx
        .landing(true)
        .land(
            &mut || {
                ran.set(ran.get() + 1);
                true
            },
            &no_point,
        )
        .unwrap();
    assert!(matches!(r, Landed::Promoted { .. }));
    assert_eq!(
        ran.get(),
        1,
        "re-verified once after folding the new base in"
    );
    let log = fx.origin_main_log();
    assert!(
        log.contains(&"add a.txt".to_string()) && log.contains(&"other b.txt".to_string()),
        "{log:?}"
    );
    assert!(log[0].starts_with("Merge"), "{log:?}");
}

#[test]
fn red_after_folding_the_base_in_stops_the_landing_and_pushes_nothing() {
    let fx = Fx::new();
    fx.task("a.txt");
    let other = fx.someone_else_pushes("b.txt", "b\n");
    let r = fx.landing(true).land(&mut || false, &no_point);
    assert!(matches!(r, Err(LandError::RedAfterFold)), "{r:?}");
    assert_eq!(
        sh(&fx.origin, &["rev-parse", "main"]),
        other,
        "the remote is untouched"
    );
}

#[test]
fn a_conflict_with_the_new_base_is_reported_with_the_files() {
    let fx = Fx::new();
    git::ensure_worktree(&fx.repo, &fx.wt, "mason/t1", "origin/main").unwrap();
    commit_file(&fx.wt, "c.txt", "mine\n", "mine");
    fx.someone_else_pushes("c.txt", "theirs\n");
    match fx.landing(true).land(&mut || true, &no_point) {
        Err(LandError::Conflict(f)) => assert!(f.contains("c.txt")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn losing_a_push_race_folds_and_retries() {
    let fx = Fx::new();
    fx.task("a.txt");
    // Someone pushes between our fast-forward and our push.
    let raced = Cell::new(false);
    let point = |name: &str| {
        if name == "land_after_ff" && !raced.get() {
            raced.set(true);
            fx.someone_else_pushes("late.txt", "late\n");
        }
    };
    let ran = Cell::new(0);
    let r = fx
        .landing(true)
        .land(
            &mut || {
                ran.set(ran.get() + 1);
                true
            },
            &point,
        )
        .unwrap();
    assert!(matches!(r, Landed::Promoted { .. }));
    assert_eq!(ran.get(), 1);
    let log = fx.origin_main_log();
    assert!(
        log.contains(&"add a.txt".to_string()) && log.contains(&"other late.txt".to_string()),
        "{log:?}"
    );
}

#[test]
fn a_base_that_keeps_moving_gives_up() {
    let fx = Fx::new();
    fx.task("a.txt");
    let n = Cell::new(0);
    let point = |name: &str| {
        if name == "land_after_ff" {
            n.set(n.get() + 1);
            fx.someone_else_pushes(&format!("race{}.txt", n.get()), "r\n");
        }
    };
    let r = fx.landing(true).land(&mut || true, &point);
    assert!(matches!(r, Err(LandError::BaseKeptMoving)), "{r:?}");
    assert_eq!(n.get(), MAX_ATTEMPTS);
}

#[test]
fn a_branch_with_no_commits_is_not_landed() {
    let fx = Fx::new();
    git::ensure_worktree(&fx.repo, &fx.wt, "mason/t1", "origin/main").unwrap();
    // Even though it was never ahead, it is "already in the base", which is a no-op, not a failure.
    assert_eq!(
        fx.landing(true).land(&mut || true, &no_point).unwrap(),
        Landed::AlreadyLanded
    );
}

#[test]
fn without_a_remote_the_local_base_moves_only_if_nobody_has_it_checked_out() {
    let fx = Fx::new();
    let tip = fx.task("a.txt");
    // `main` is checked out in the owner's checkout: refuse rather than change files under it.
    match fx.landing(false).land(&mut || true, &no_point) {
        Err(LandError::LocalBaseCheckedOut(p)) => assert_eq!(p, fx.repo),
        other => panic!("{other:?}"),
    }
    assert_ne!(sh(&fx.repo, &["rev-parse", "main"]), tip);
    // Move the owner's checkout off `main`: now it is safe.
    sh(&fx.repo, &["checkout", "-q", "-b", "elsewhere"]);
    assert!(matches!(
        fx.landing(false).land(&mut || true, &no_point).unwrap(),
        Landed::Promoted { .. }
    ));
    assert_eq!(sh(&fx.repo, &["rev-parse", "main"]), tip);
}

#[test]
fn branch_names_are_validated() {
    for ok in ["mason/p/t1-fix-bug", "main", "release/1.2"] {
        assert!(git::valid_branch(ok), "{ok}");
    }
    for bad in [
        "", "-rf", "/abs", ".hidden", "a..b", "a b", "a;b", "x/", "x.", "$(x)",
    ] {
        assert!(!git::valid_branch(bad), "{bad:?}");
    }
}

#[cfg(unix)] // Unix-only; Windows coverage comes with the later port tasks.
#[test]
fn a_push_refused_for_another_reason_is_an_error_not_a_race() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fx::new();
    fx.task("a.txt");
    // The remote refuses every push (a policy hook), though its base never moved.
    let hook = fx.origin.join("hooks/pre-receive");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, "#!/bin/sh\necho 'no pushes today' >&2\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    match fx.landing(true).land(&mut || true, &no_point) {
        Err(LandError::Git(e)) => assert!(
            e.stderr.contains("no pushes today")
                || e.stderr.contains("rejected")
                || e.stderr.contains("failed"),
            "{e}"
        ),
        other => panic!("expected a git error, got {other:?}"),
    }
}
