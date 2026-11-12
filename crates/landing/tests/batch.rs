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
            removes: vec![],
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
fn a_task_conflicting_only_with_a_mate_is_deferred_and_the_rest_land() {
    let fx = Fx::new();
    let t1 = fx.branch("t1", "shared.txt", "from one\n");
    let t2 = fx.branch("t2", "shared.txt", "from two\n");
    let t3 = fx.branch("t3", "c.txt", "c\n");
    let b = fx.batch(None, true);
    let a = b.assemble(&[t1, t2, t3], &nop).unwrap();
    assert_eq!(a.merged, [1, 3]);
    assert!(a.ejected.is_empty());
    assert_eq!(a.deferred.len(), 1);
    assert_eq!(a.deferred[0].id, 2);
    assert_eq!(a.deferred[0].with, [1]);
    assert_eq!(a.deferred[0].files, "shared.txt");
    // The merge was undone: the tree is clean and holds only the others.
    assert_eq!(sh(&fx.int_wt, &["status", "--porcelain"]), "");
    assert_eq!(
        std::fs::read_to_string(fx.int_wt.join("shared.txt")).unwrap(),
        "from one\n"
    );
}

#[test]
fn a_task_conflicting_with_the_base_is_ejected() {
    let fx = Fx::new();
    let t1 = fx.branch("t1", "shared.txt", "from one\n");
    let t2 = fx.branch("t2", "shared.txt", "from two\n");
    let t3 = fx.branch("t3", "c.txt", "c\n");
    // t1 lands elsewhere; the base now conflicts with t2, and t1 is not in the batch.
    let b = fx.batch(None, true);
    let a = b.assemble(&[t1], &nop).unwrap();
    b.finish(&a, &mut |_| true, &nop).unwrap().unwrap();
    let a = b.assemble(&[t2, t3], &nop).unwrap();
    assert_eq!(a.merged, [3]);
    assert!(a.deferred.is_empty());
    assert_eq!(a.ejected.len(), 1);
    assert_eq!(a.ejected[0].id, 2);
    assert_eq!(a.ejected[0].files, "shared.txt");
}

#[test]
fn a_deferred_task_goes_first_in_the_next_batch_when_its_mate_did_not_land() {
    let fx = Fx::new();
    let t1 = fx.branch("t1", "shared.txt", "from one\n");
    let t2 = fx.branch("t2", "shared.txt", "from two\n");
    let b = fx.batch(None, true);
    let a = b.assemble(&[t1, t2.clone()], &nop).unwrap();
    assert_eq!(a.deferred.len(), 1);
    // t1 fails verification and never lands; t2 assembles alone, cleanly.
    let a = b.assemble(&[t2], &nop).unwrap();
    assert_eq!(a.merged, [2]);
    assert!(a.deferred.is_empty() && a.ejected.is_empty());
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
fn a_resolver_that_fails_or_leaves_conflicts_sets_aside_only_that_task() {
    let fx = Fx::new();
    let t1 = fx.branch("t1", "shared.txt", "from one\n");
    let t2 = fx.branch("t2", "shared.txt", "from two\n");
    for cmd in ["exit 1", "true"] {
        let b = fx.batch(Some(cmd), true);
        let a = b.assemble(&[t1.clone(), t2.clone()], &nop).unwrap();
        assert_eq!((a.merged.clone(), a.deferred.len()), (vec![1], 1), "{cmd}");
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

/// Push a commit to the remote's main from a second clone.
fn land_elsewhere(fx: &Fx, file: &str) {
    let other = fx.root.join("other");
    if !other.exists() {
        sh(
            &fx.root,
            &[
                "clone",
                "-q",
                fx.origin.to_str().unwrap(),
                other.to_str().unwrap(),
            ],
        );
    }
    sh(&other, &["pull", "-q", "origin", "main"]);
    write_commit(&other, file, "x\n");
    sh(&other, &["push", "-q", "origin", "HEAD:main"]);
}

#[test]
fn the_lock_is_not_held_while_reverify_runs() {
    use std::sync::atomic::{AtomicI32, Ordering::SeqCst};
    let fx = Fx::new();
    let ts = [fx.branch("t1", "a.txt", "a\n")];
    let b = fx.batch(None, true);
    let a = b.assemble(&ts, &nop).unwrap();
    land_elsewhere(&fx, "z.txt");
    let held = AtomicI32::new(0);
    let took = AtomicI32::new(0);
    struct G<'a>(&'a AtomicI32);
    impl Drop for G<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, SeqCst);
        }
    }
    let lock = || -> mason_landing::batch::Guard<'_> {
        held.fetch_add(1, SeqCst);
        took.fetch_add(1, SeqCst);
        Box::new(G(&held))
    };
    let mut ran = 0;
    let f = b
        .finish_with(
            &a,
            &lock,
            &mut |_| {
                ran += 1;
                assert_eq!(held.load(SeqCst), 0, "the lock is held during reverify");
                true
            },
            &nop,
        )
        .unwrap()
        .unwrap();
    assert!(matches!(f, Finish::Promoted { folds: 1, .. }), "{f:?}");
    assert_eq!(ran, 1);
    assert_eq!(
        took.load(SeqCst),
        2,
        "one hold for the fold, one for the promote"
    );
    assert_eq!(held.load(SeqCst), 0);
}

#[test]
fn a_base_that_moves_during_reverify_is_folded_and_verified_again() {
    {
        let fx = Fx::new();
        let ts = [fx.branch("t1", "a.txt", "a\n")];
        let b = fx.batch(None, true);
        let a = b.assemble(&ts, &nop).unwrap();
        land_elsewhere(&fx, "z.txt");
        let mut verified: Vec<(String, Vec<String>)> = vec![];
        let wt = fx.int_wt.clone();
        let f = b
            .finish(
                &a,
                &mut |m| {
                    // The tree verified is the tree in the worktree right now.
                    verified.push((git::rev_parse(&wt, "HEAD").unwrap(), m.to_vec()));
                    if verified.len() == 1 {
                        land_elsewhere(&fx, "y.txt");
                    }
                    true
                },
                &nop,
            )
            .unwrap()
            .unwrap();
        let Finish::Promoted { commit, folds } = f else {
            panic!("{f:?}")
        };
        assert_eq!(folds, 2);
        assert_eq!(verified.len(), 2, "re-verified after the second move");
        assert_eq!(verified[1].1, ["y.txt"]);
        // What was promoted is exactly what was last verified.
        assert_eq!(verified[1].0, commit);
        assert_eq!(fx.origin_main(), commit);
        for f in ["a.txt", "z.txt", "y.txt"] {
            sh(&fx.origin, &["cat-file", "-e", &format!("main:{f}")]);
        }
    }
}

#[test]
fn a_base_that_keeps_moving_during_reverify_is_never_promoted_unverified() {
    let fx = Fx::new();
    let ts = [fx.branch("t1", "a.txt", "a\n")];
    let b = fx.batch(None, true);
    let a = b.assemble(&ts, &nop).unwrap();
    land_elsewhere(&fx, "z0.txt");
    let mut n = 0;
    let f = b
        .finish(
            &a,
            &mut |_| {
                n += 1;
                land_elsewhere(&fx, &format!("m{n}.txt"));
                true
            },
            &nop,
        )
        .unwrap()
        .unwrap();
    assert!(matches!(f, Finish::BaseKeptMoving), "{f:?}");
    let _ = sh(&fx.origin, &["rev-parse", "main"]);
    assert!(
        git::run(&fx.origin, &["cat-file", "-e", "main:a.txt"]).is_err(),
        "an unverified tree reached the base"
    );
}

/// A task branched before the base landed `landed_line` next to the line it changed.
fn conflicting_with_landed_line(fx: &Fx) -> (BatchTask, BatchTask) {
    let t1 = fx.branch("t1", "shared.txt", "mine\n");
    let t3 = fx.branch("t3", "c.txt", "c\n");
    write_commit(&fx.repo, "shared.txt", "one\nlanded_line(1, 2);\n");
    sh(&fx.repo, &["push", "-q", "origin", "main"]);
    (t1, t3)
}

#[test]
fn a_resolution_that_drops_a_landed_line_is_undone_and_the_rest_land() {
    let fx = Fx::new();
    let (t1, t3) = conflicting_with_landed_line(&fx);
    let b = fx.batch(Some("printf 'mine\\n' > shared.txt"), true);
    let a = b.assemble(&[t1, t3], &nop).unwrap();
    assert_eq!(a.merged, [3]);
    assert!(a.resolved.is_empty() && a.ejected.is_empty());
    assert_eq!(a.unkept.len(), 1);
    assert_eq!(a.unkept[0].id, 1);
    assert_eq!(a.unkept[0].lines[0].line, "landed_line(1, 2);");
    assert_eq!(a.unkept[0].lines[0].file, "shared.txt");
    // The merge was undone: clean tree, the base's content.
    assert_eq!(sh(&fx.int_wt, &["status", "--porcelain"]), "");
    assert_eq!(
        std::fs::read_to_string(fx.int_wt.join("shared.txt")).unwrap(),
        "one\nlanded_line(1, 2);\n"
    );
    let f = b.finish(&a, &mut |_| true, &nop).unwrap().unwrap();
    assert!(matches!(f, Finish::Promoted { .. }), "{f:?}");
    assert_eq!(sh(&fx.origin, &["show", "main:c.txt"]), "c");
}

#[test]
fn a_resolution_that_keeps_the_landed_line_is_accepted() {
    let fx = Fx::new();
    let (t1, _) = conflicting_with_landed_line(&fx);
    let b = fx.batch(
        Some("printf 'mine\\nlanded_line(1, 2);\\n' > shared.txt"),
        true,
    );
    let a = b.assemble(&[t1], &nop).unwrap();
    assert_eq!((a.merged.clone(), a.resolved.clone()), (vec![1], vec![1]));
    assert!(a.unkept.is_empty());
}

#[test]
fn a_listed_removal_lets_the_resolution_through() {
    use mason_landing::audit::Removal;
    let fx = Fx::new();
    let (mut t1, _) = conflicting_with_landed_line(&fx);
    t1.removes = vec![Removal {
        file: "shared.txt".into(),
        line: "landed_line(1, 2);".into(),
    }];
    let b = fx.batch(Some("printf 'mine\\n' > shared.txt"), true);
    let a = b.assemble(&[t1], &nop).unwrap();
    assert_eq!(a.resolved, [1]);
    assert!(a.unkept.is_empty());
}
