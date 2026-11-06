// SPDX-License-Identifier: GPL-3.0-or-later

//! The integration branch against real git: merging a batch, conflicts and the
//! resolver, a moved base, and repeating every step.

use mason_landing::batch::{Batch, BatchTask, Finish};
use mason_landing::git;
use std::path::{Path, PathBuf};

struct Fx {
    _d: tempfile::TempDir,
    root: PathBuf,
    origin: PathBuf,
    repo: PathBuf,
    int_wt: PathBuf,
}

fn sh(cwd: &Path, args: &[&str]) -> String {
    git::run(cwd, args).unwrap_or_else(|e| panic!("{e}"))
}

fn write_commit(cwd: &Path, name: &str, body: &str) {
    std::fs::write(cwd.join(name), body).unwrap();
    sh(cwd, &["add", "-A"]);
    sh(cwd, &["commit", "-q", "-m", &format!("edit {name}")]);
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
        write_commit(&repo, "shared.txt", "one\n");
        sh(&repo, &["push", "-q", "origin", "main"]);
        Fx {
            int_wt: root.join("int"),
            origin,
            repo,
            root,
            _d: d,
        }
    }

    /// A branch off the remote's main with one commit.
    fn branch(&self, name: &str, file: &str, body: &str) -> BatchTask {
        let wt = self.root.join(format!("wt-{name}"));
        git::ensure_worktree(&self.repo, &wt, &format!("mason/{name}"), "origin/main").unwrap();
        write_commit(&wt, file, body);
        BatchTask {
            id: name.trim_start_matches('t').parse().unwrap(),
            branch: format!("mason/{name}"),
        }
    }

    fn batch<'a>(&'a self, resolver: Option<&'a str>, remote: bool) -> Batch<'a> {
        Batch {
            repo: &self.repo,
            int_wt: &self.int_wt,
            int_branch: "mason/int/p/b1",
            base: "main",
            remote: remote.then_some("origin"),
            resolver,
            max_fold: 3,
        }
    }

    fn origin_main(&self) -> String {
        sh(&self.origin, &["rev-parse", "main"])
    }
}

fn nop(_: &str) {}

#[test]
fn a_clean_batch_merges_and_promotes() {
    let fx = Fx::new();
    let ts = [
        fx.branch("t1", "a.txt", "a\n"),
        fx.branch("t2", "b.txt", "b\n"),
        fx.branch("t3", "c.txt", "c\n"),
    ];
    let b = fx.batch(None, true);
    let a = b.assemble(&ts, &nop).unwrap();
    assert_eq!(a.merged, [1, 2, 3]);
    assert!(a.ejected.is_empty() && a.already.is_empty());
    assert_eq!(b.union_diff(&a), ["a.txt", "b.txt", "c.txt"]);
    let f = b.finish(&a, &mut |_| true, &nop).unwrap().unwrap();
    let Finish::Promoted { commit, folds } = f else {
        panic!("{f:?}")
    };
    assert_eq!(folds, 0);
    assert_eq!(fx.origin_main(), commit);
}

#[test]
fn a_conflicting_task_is_ejected_and_the_rest_land() {
    let fx = Fx::new();
    let t1 = fx.branch("t1", "shared.txt", "from one\n");
    let t2 = fx.branch("t2", "shared.txt", "from two\n");
    let t3 = fx.branch("t3", "c.txt", "c\n");
    let b = fx.batch(None, true);
    let a = b.assemble(&[t1, t2, t3], &nop).unwrap();
    assert_eq!(a.merged, [1, 3]);
    assert_eq!(a.ejected.len(), 1);
    assert_eq!(a.ejected[0].id, 2);
    assert_eq!(a.ejected[0].files, "shared.txt");
    // The merge was undone: the tree is clean and holds only the others.
    assert_eq!(sh(&fx.int_wt, &["status", "--porcelain"]), "");
    assert_eq!(
        std::fs::read_to_string(fx.int_wt.join("shared.txt")).unwrap(),
        "from one\n"
    );
}

#[test]
fn the_resolver_can_resolve_a_conflict() {
    let fx = Fx::new();
    let t1 = fx.branch("t1", "shared.txt", "from one\n");
    let t2 = fx.branch("t2", "shared.txt", "from two\n");
    let b = fx.batch(Some("printf 'both\\n' > shared.txt"), true);
    let a = b.assemble(&[t1, t2], &nop).unwrap();
    assert_eq!(a.merged, [1, 2]);
    assert_eq!(a.resolved, [2]);
    assert!(a.ejected.is_empty());
    assert_eq!(
        std::fs::read_to_string(fx.int_wt.join("shared.txt")).unwrap(),
        "both\n"
    );
    assert_eq!(sh(&fx.int_wt, &["status", "--porcelain"]), "");
}

#[test]
fn a_resolver_that_fails_or_leaves_conflicts_ejects_only_that_task() {
    let fx = Fx::new();
    let t1 = fx.branch("t1", "shared.txt", "from one\n");
    let t2 = fx.branch("t2", "shared.txt", "from two\n");
    for cmd in ["exit 1", "true"] {
        let b = fx.batch(Some(cmd), true);
        let a = b.assemble(&[t1.clone(), t2.clone()], &nop).unwrap();
        assert_eq!((a.merged.clone(), a.ejected.len()), (vec![1], 1), "{cmd}");
        assert_eq!(sh(&fx.int_wt, &["status", "--porcelain"]), "");
    }
}

#[test]
fn assembling_again_gives_the_same_batch() {
    let fx = Fx::new();
    let ts = [
        fx.branch("t1", "a.txt", "a\n"),
        fx.branch("t2", "b.txt", "b\n"),
    ];
    let b = fx.batch(None, true);
    let first = b.assemble(&ts, &nop).unwrap();
    // Dirt from a crash: an untracked file and a stray edit.
    std::fs::write(fx.int_wt.join("junk"), "x").unwrap();
    std::fs::write(fx.int_wt.join("a.txt"), "changed").unwrap();
    let again = b.assemble(&ts, &nop).unwrap();
    assert_eq!(again.merged, first.merged);
    assert_eq!(again.base_tip, first.base_tip);
    assert_eq!(b.union_diff(&again), b.union_diff(&first));
    assert!(!fx.int_wt.join("junk").exists());
}

#[test]
fn a_task_already_in_the_base_is_not_landed_twice() {
    let fx = Fx::new();
    let ts = [
        fx.branch("t1", "a.txt", "a\n"),
        fx.branch("t2", "b.txt", "b\n"),
    ];
    let b = fx.batch(None, true);
    let a = b.assemble(&ts, &nop).unwrap();
    b.finish(&a, &mut |_| true, &nop).unwrap().unwrap();
    let landed = fx.origin_main();
    // A crash after promote: the same batch is assembled and finished again.
    let a2 = b.assemble(&ts, &nop).unwrap();
    assert_eq!(a2.already, [1, 2]);
    assert!(a2.merged.is_empty());
    let f = b.finish(&a2, &mut |_| true, &nop).unwrap().unwrap();
    assert!(matches!(f, Finish::Promoted { .. }));
    assert_eq!(fx.origin_main(), landed, "the base did not move again");
}

#[test]
fn finishing_twice_without_reassembling_does_not_double_land() {
    let fx = Fx::new();
    let ts = [fx.branch("t1", "a.txt", "a\n")];
    let b = fx.batch(None, true);
    let a = b.assemble(&ts, &nop).unwrap();
    b.finish(&a, &mut |_| true, &nop).unwrap().unwrap();
    let landed = fx.origin_main();
    b.finish(&a, &mut |_| true, &nop).unwrap().unwrap();
    assert_eq!(fx.origin_main(), landed);
}

#[test]
fn a_moved_base_is_folded_in_and_reverified() {
    let fx = Fx::new();
    let ts = [fx.branch("t1", "a.txt", "a\n")];
    let b = fx.batch(None, true);
    let a = b.assemble(&ts, &nop).unwrap();
    // Someone lands first.
    let other = fx.root.join("other");
    sh(
        &fx.root,
        &[
            "clone",
            "-q",
            fx.origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    write_commit(&other, "z.txt", "z\n");
    sh(&other, &["push", "-q", "origin", "HEAD:main"]);

    let mut moved_seen = vec![];
    let f = b
        .finish(
            &a,
            &mut |m| {
                moved_seen = m.to_vec();
                true
            },
            &nop,
        )
        .unwrap()
        .unwrap();
    let Finish::Promoted { commit, folds } = f else {
        panic!("{f:?}")
    };
    assert_eq!(folds, 1);
    assert_eq!(moved_seen, ["z.txt"]);
    assert_eq!(fx.origin_main(), commit);
    // Both are in the base.
    sh(&fx.origin, &["cat-file", "-e", "main:z.txt"]);
    sh(&fx.origin, &["cat-file", "-e", "main:a.txt"]);
}

#[test]
fn red_after_the_fold_does_not_promote() {
    let fx = Fx::new();
    let ts = [fx.branch("t1", "a.txt", "a\n")];
    let b = fx.batch(None, true);
    let a = b.assemble(&ts, &nop).unwrap();
    let other = fx.root.join("other");
    sh(
        &fx.root,
        &[
            "clone",
            "-q",
            fx.origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    write_commit(&other, "z.txt", "z\n");
    sh(&other, &["push", "-q", "origin", "HEAD:main"]);
    let before = fx.origin_main();
    let r = b.finish(&a, &mut |_| false, &nop).unwrap();
    assert!(r.is_err());
    assert_eq!(fx.origin_main(), before);
}

#[test]
fn a_moved_base_that_conflicts_is_reported() {
    let fx = Fx::new();
    let ts = [fx.branch("t1", "shared.txt", "mine\n")];
    let b = fx.batch(None, true);
    let a = b.assemble(&ts, &nop).unwrap();
    let other = fx.root.join("other");
    sh(
        &fx.root,
        &[
            "clone",
            "-q",
            fx.origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    write_commit(&other, "shared.txt", "theirs\n");
    sh(&other, &["push", "-q", "origin", "HEAD:main"]);
    let f = b.finish(&a, &mut |_| true, &nop).unwrap().unwrap();
    assert!(
        matches!(f, Finish::Conflict(ref s) if s.contains("shared.txt")),
        "{f:?}"
    );
}

#[test]
fn without_a_remote_the_local_base_moves_by_compare_and_swap() {
    let fx = Fx::new();
    // The repo checkout has main; move it off so the branch is free to update.
    sh(&fx.repo, &["checkout", "-q", "--detach"]);
    let ts = [fx.branch("t1", "a.txt", "a\n")];
    let b = fx.batch(None, false);
    let a = b.assemble(&ts, &nop).unwrap();
    let Finish::Promoted { commit, .. } = b.finish(&a, &mut |_| true, &nop).unwrap().unwrap()
    else {
        panic!()
    };
    assert_eq!(sh(&fx.repo, &["rev-parse", "main"]), commit);
}

#[test]
fn a_checked_out_local_base_is_refused() {
    let fx = Fx::new();
    let ts = [fx.branch("t1", "a.txt", "a\n")];
    let b = fx.batch(None, false);
    let a = b.assemble(&ts, &nop).unwrap();
    let f = b.finish(&a, &mut |_| true, &nop).unwrap().unwrap();
    assert!(matches!(f, Finish::LocalBaseCheckedOut(_)), "{f:?}");
}
