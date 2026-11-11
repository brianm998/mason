// SPDX-License-Identifier: GPL-3.0-or-later

//! The dropped-line audit against real repositories: a base commit that adds lines, a task
//! that edits the same file, and a result that keeps or loses what the base added.

use mason_landing::audit::{audit, Dropped, Removal};
use mason_landing::git;
use std::path::{Path, PathBuf};

const START: &str = "fn run() {\n    setup();\n    finish();\n}\n";

struct Repo {
    _d: tempfile::TempDir,
    path: PathBuf,
}

fn sh(cwd: &Path, args: &[&str]) -> String {
    git::run(cwd, args).unwrap_or_else(|e| panic!("{e}"))
}

fn commit(cwd: &Path, file: &str, body: &str) -> String {
    std::fs::write(cwd.join(file), body).unwrap();
    sh(cwd, &["add", "-A"]);
    sh(cwd, &["commit", "-q", "--allow-empty", "-m", "edit"]);
    sh(cwd, &["rev-parse", "HEAD"])
}

impl Repo {
    fn new() -> Repo {
        let d = tempfile::tempdir().unwrap();
        let path = std::fs::canonicalize(d.path()).unwrap();
        sh(&path, &["init", "-q", "-b", "main"]);
        Repo { _d: d, path }
    }

    /// `base` and `task` both start from `START`; the result is a commit on the task's side
    /// holding `result`. The audit is of the base's change against that result.
    fn run(&self, base: &str, task: &str, result: &str, deliberate: &[Removal]) -> Vec<Dropped> {
        let p = &self.path;
        let mb = commit(p, "f.rs", START);
        let b = commit(p, "f.rs", base);
        sh(p, &["checkout", "-q", "-b", "task", &mb]);
        let t = commit(p, "f.rs", task);
        let r = commit(p, "f.rs", result);
        audit(p, &mb, &b, &t, None, &r, deliberate).unwrap()
    }
}

fn lines(d: &[Dropped]) -> Vec<&str> {
    d.iter().map(|x| x.line.as_str()).collect()
}

#[test]
fn a_line_kept_exactly() {
    let base = "fn run() {\n    setup();\n    audit_me(1, 2);\n    finish();\n}\n";
    let task = "fn run() {\n    setup();\n    own();\n    finish();\n}\n";
    let both = "fn run() {\n    setup();\n    audit_me(1, 2);\n    own();\n    finish();\n}\n";
    assert!(Repo::new().run(base, task, both, &[]).is_empty());
}

#[test]
fn a_line_rewrapped_over_two_lines() {
    let base =
        "fn run() {\n    setup();\n    let total = compute(first, second);\n    finish();\n}\n";
    let result = "fn run() {\n    setup();\n    let total =\n        compute(first, second);\n    finish();\n}\n";
    assert!(Repo::new().run(base, START, result, &[]).is_empty());
}

#[test]
fn an_argument_added_on_both_sides() {
    let base =
        "fn run() {\n    setup();\n    let total = compute(first, second);\n    finish();\n}\n";
    let task = "fn run() {\n    setup();\n    finish();\n}\n";
    let result = "fn run() {\n    setup();\n    let total = compute(first, second, third);\n    finish();\n}\n";
    assert!(Repo::new().run(base, task, result, &[]).is_empty());
}

#[test]
fn a_line_the_task_removed_itself() {
    // The base reordered two lines, so its diff adds one of them; the task removed both.
    let base = "fn run() {\n    finish();\n    setup();\n}\n";
    let task = "fn run() {\n    own();\n}\n";
    assert!(Repo::new().run(base, task, task, &[]).is_empty());
}

#[test]
fn a_line_really_dropped() {
    let base = "fn run() {\n    setup();\n    audit_me(1, 2);\n    finish();\n}\n";
    let task = "fn run() {\n    own();\n    finish();\n}\n";
    let r = Repo::new().run(base, task, task, &[]);
    assert_eq!(lines(&r), ["    audit_me(1, 2);"]);
    assert_eq!(r[0].file, "f.rs");
}

#[test]
fn a_listed_removal_is_not_flagged() {
    let base = "fn run() {\n    setup();\n    audit_me(1, 2);\n    finish();\n}\n";
    let task = "fn run() {\n    own();\n    finish();\n}\n";
    let del = [Removal {
        file: "f.rs".into(),
        line: "audit_me(1, 2);".into(),
    }];
    assert!(Repo::new().run(base, task, task, &del).is_empty());
}

#[test]
fn blank_punctuation_and_source_hash_lines_are_ignored() {
    let base =
        "fn run() {\n    setup();\n\n    // source-hash: 1234abcd\n    });\n    finish();\n}\n";
    let task = "fn run() {\n    own();\n    finish();\n}\n";
    assert!(Repo::new().run(base, task, task, &[]).is_empty());
}

#[test]
fn a_file_the_result_deleted_loses_its_lines() {
    let p = Repo::new();
    let mb = commit(&p.path, "f.rs", START);
    let b = commit(&p.path, "f.rs", "fn run() {\n    audit_me(1, 2);\n}\n");
    sh(&p.path, &["checkout", "-q", "-b", "task", &mb]);
    let t = commit(&p.path, "g.rs", "x\n");
    sh(&p.path, &["rm", "-q", "f.rs"]);
    sh(&p.path, &["commit", "-q", "-m", "rm"]);
    let r = sh(&p.path, &["rev-parse", "HEAD"]);
    let d = audit(&p.path, &mb, &b, &t, None, &r, &[]).unwrap();
    assert!(lines(&d).contains(&"    audit_me(1, 2);"));
}

#[test]
fn lines_the_base_itself_removed_later_are_not_the_tasks_to_keep() {
    let p = Repo::new();
    let mb = commit(&p.path, "f.rs", START);
    let b = commit(&p.path, "f.rs", "fn run() {\n    audit_me(1, 2);\n}\n");
    let tip = commit(&p.path, "f.rs", "fn run() {\n    other();\n}\n");
    sh(&p.path, &["checkout", "-q", "-b", "task", &mb]);
    let t = commit(&p.path, "g.rs", "x\n");
    let none = audit(&p.path, &mb, &b, &t, Some(&tip), &t, &[]).unwrap();
    assert!(none.is_empty(), "{none:?}");
    assert_eq!(audit(&p.path, &mb, &b, &t, None, &t, &[]).unwrap().len(), 1);
}
