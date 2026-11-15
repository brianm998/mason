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

/// Audit, then pair what was dropped with what replaced it, as the review step is shown.
fn pairs(base: &str, result: &str) -> (Vec<String>, Vec<mason_landing::audit::Replaced>) {
    let repo = Repo::new();
    let p = &repo.path;
    let mb = commit(p, "f.rs", START);
    let b = commit(p, "f.rs", base);
    sh(p, &["checkout", "-q", "-b", "task", &mb]);
    let t = commit(p, "f.rs", START);
    let r = commit(p, "f.rs", result);
    let d = audit(p, &mb, &b, &t, None, &r, &[]).unwrap();
    let names = d.iter().map(|x| x.line.clone()).collect();
    (
        names,
        mason_landing::audit::replacements(p, &b, &r, &d).unwrap(),
    )
}

#[test]
fn a_landed_line_rewritten_in_place_is_paired_and_settles_by_removes() {
    let base = "fn run() {\n    setup();\n    pin_value(\"alpha\", 1);\n    finish();\n}\n";
    let result = "fn run() {\n    setup();\n    pin_value(\"beta\", 2);\n    finish();\n}\n";
    let (dropped, pairs) = pairs(base, result);
    assert_eq!(dropped, vec!["    pin_value(\"alpha\", 1);"]);
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].replacement, "    pin_value(\"beta\", 2);");
    // The reviewer's approval declares it: the audit then reports nothing.
    let repo = Repo::new();
    let d = repo.run(
        base,
        START,
        result,
        &[Removal {
            file: "f.rs".into(),
            line: pairs[0].line.clone(),
        }],
    );
    assert!(d.is_empty());
}

#[test]
fn a_landed_line_simply_lost_has_no_replacement() {
    let base = "fn run() {\n    setup();\n    pin_value(\"alpha\", 1);\n    finish();\n}\n";
    let (dropped, pairs) = pairs(base, START);
    assert_eq!(dropped.len(), 1);
    assert!(pairs.is_empty());
}

#[test]
fn a_line_the_task_moved_to_a_file_it_split_off_is_kept() {
    // The base adds a test to f.rs; the task splits f.rs, moving everything, the new test
    // too, into g.rs.
    let p = Repo::new();
    let mb = commit(&p.path, "f.rs", START);
    let b = commit(
        &p.path,
        "f.rs",
        "fn run() {\n    setup();\n    audit_me(1, 2);\n    finish();\n}\n",
    );
    sh(&p.path, &["checkout", "-q", "-b", "task", &mb]);
    let t = commit(&p.path, "f.rs", "// moved to g.rs\n");
    std::fs::write(
        p.path.join("g.rs"),
        "fn run() {\n    setup();\n    audit_me(1, 2);\n    finish();\n}\n",
    )
    .unwrap();
    sh(&p.path, &["add", "-A"]);
    sh(&p.path, &["commit", "-q", "-m", "split"]);
    let r = sh(&p.path, &["rev-parse", "HEAD"]);
    let d = audit(&p.path, &mb, &b, &t, None, &r, &[]).unwrap();
    assert!(d.is_empty(), "{d:?}");
}

#[test]
fn a_dropped_line_found_only_in_a_file_the_task_did_not_change_is_still_dropped() {
    // h.rs holds the same line from the start; the task never touches it.
    let p = Repo::new();
    std::fs::write(p.path.join("h.rs"), "    audit_me(1, 2);\n").unwrap();
    let mb = commit(&p.path, "f.rs", START);
    let b = commit(
        &p.path,
        "f.rs",
        "fn run() {\n    setup();\n    audit_me(1, 2);\n    finish();\n}\n",
    );
    sh(&p.path, &["checkout", "-q", "-b", "task", &mb]);
    let t = commit(&p.path, "f.rs", "fn run() {\n    own();\n}\n");
    std::fs::write(p.path.join("g.rs"), "unrelated(1, 2, 3);\n").unwrap();
    sh(&p.path, &["add", "-A"]);
    sh(&p.path, &["commit", "-q", "-m", "more"]);
    let r = sh(&p.path, &["rev-parse", "HEAD"]);
    let d = audit(&p.path, &mb, &b, &t, None, &r, &[]).unwrap();
    assert_eq!(lines(&d), ["    audit_me(1, 2);"]);
}
