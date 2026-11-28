// SPDX-License-Identifier: GPL-3.0-or-later

//! scripts/run-changed-exit-scripts.sh against a throwaway repository with fake exit scripts:
//! the run's time budget (`MASON_RUN_BUDGET_S`) ends it red with a verdict for every script
//! instead of being killed with none, and passes are kept between runs of the same tree under
//! `MASON_TASK_CACHE`. Outcomes only: no assertion reads the clock. The fakes are shell scripts;
//! no binary is copied.

use std::path::{Path, PathBuf};
use std::process::Command;

fn runner() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/run-changed-exit-scripts.sh")
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .expect("git runs")
        .success();
    assert!(ok, "git {args:?} failed");
}

/// A repository whose last commit adds the fake scripts `(name, body)`; its base is the one before.
struct Repo {
    dir: tempfile::TempDir,
    base: String,
}

impl Repo {
    fn new(scripts: &[(&str, &str)]) -> Repo {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path();
        git(p, &["init", "-q", "."]);
        git(p, &["config", "user.email", "t@example.invalid"]);
        git(p, &["config", "user.name", "t"]);
        git(p, &["config", "commit.gpgsign", "false"]);
        std::fs::write(p.join("README"), "base\n").unwrap();
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "base"]);
        let base = Command::new("git")
            .current_dir(p)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let base = String::from_utf8(base.stdout).unwrap().trim().to_string();
        std::fs::create_dir_all(p.join("scripts/exit")).unwrap();
        for (name, body) in scripts {
            std::fs::write(
                p.join(format!("scripts/exit/{name}.sh")),
                format!("#!/usr/bin/env bash\n{body}\n"),
            )
            .unwrap();
        }
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "scripts"]);
        Repo { dir, base }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Runs the runner with `env` set; returns (stdout, exit code).
    fn run(&self, env: &[(&str, &str)]) -> (String, i32) {
        let mut c = Command::new("bash");
        c.arg(runner()).current_dir(self.path());
        for k in [
            "MASON_RUN_BUDGET_S",
            "MASON_RUN_BUDGET_MARGIN_S",
            "MASON_RUN_BUDGET_FLOOR_S",
            "MASON_TASK_CACHE",
            "MASON_EXIT_CACHE",
            "MASON_EXIT_SHARD",
            "MASON_EXIT_SCRIPT_LIMIT_S",
            "MASON_EXIT_REPEAT",
            "MASON_EXIT_TREE_KEY",
            "MASON_EXIT_PASSES",
        ] {
            c.env_remove(k);
        }
        c.env("MASON_BASE", &self.base);
        for (k, v) in env {
            c.env(k, v);
        }
        let o = c.output().expect("the runner starts");
        (
            String::from_utf8_lossy(&o.stdout).into_owned(),
            o.status.code().unwrap_or(-1),
        )
    }
}

fn lines_with(out: &str, prefix: &str) -> Vec<String> {
    out.lines()
        .filter(|l| l.starts_with(prefix))
        .map(String::from)
        .collect()
}

fn runs_of(mark: &Path, id: &str) -> usize {
    std::fs::read_to_string(mark)
        .unwrap_or_default()
        .lines()
        .filter(|l| *l == id)
        .count()
}

const NOT_RUN: &str = "(not run: out of time; run the check again to continue)";

#[test]
fn a_budget_too_small_for_any_script_reports_each_as_not_run_and_exits_red() {
    let r = Repo::new(&[("s10", "exit 0"), ("s11", "exit 0"), ("s12", "exit 0")]);
    // One second left is under the 60 s floor, whatever the machine's speed.
    let (out, code) = r.run(&[
        ("MASON_RUN_BUDGET_S", "1"),
        ("MASON_RUN_BUDGET_MARGIN_S", "0"),
    ]);
    assert_ne!(code, 0, "{out}");
    for id in ["s10", "s11", "s12"] {
        assert!(
            out.lines().any(|l| l == format!("FAIL {id} {NOT_RUN}")),
            "{out}"
        );
    }
    assert!(lines_with(&out, "PASS").is_empty(), "{out}");
    assert!(
        out.lines()
            .any(|l| l == "exit-scripts: 0 passed, 0 failed, 3 not run (time budget 1 s)"),
        "{out}"
    );
}

#[test]
fn the_script_after_the_one_that_used_the_time_is_not_run() {
    // The first script sleeps past what the floor leaves; the budget forces the stop.
    let r = Repo::new(&[("s10", "sleep 5"), ("s11", "exit 0"), ("s12", "exit 0")]);
    let (out, code) = r.run(&[
        ("MASON_RUN_BUDGET_S", "8"),
        ("MASON_RUN_BUDGET_MARGIN_S", "0"),
        ("MASON_RUN_BUDGET_FLOOR_S", "4"),
    ]);
    assert_ne!(code, 0, "{out}");
    assert!(out.lines().any(|l| l == "PASS s10"), "{out}");
    assert!(
        out.lines().any(|l| l == format!("FAIL s11 {NOT_RUN}")),
        "{out}"
    );
    assert!(
        out.lines().any(|l| l == format!("FAIL s12 {NOT_RUN}")),
        "{out}"
    );
    assert!(
        out.lines()
            .any(|l| l == "exit-scripts: 1 passed, 0 failed, 2 not run (time budget 8 s)"),
        "{out}"
    );
}

#[test]
fn a_script_is_held_to_the_time_left_and_the_runner_ends_by_itself() {
    let r = Repo::new(&[("s10", "sleep 300"), ("s11", "exit 0")]);
    let (out, code) = r.run(&[
        ("MASON_RUN_BUDGET_S", "4"),
        ("MASON_RUN_BUDGET_MARGIN_S", "0"),
        ("MASON_RUN_BUDGET_FLOOR_S", "2"),
    ]);
    assert_ne!(code, 0, "{out}");
    assert!(
        out.lines().any(|l| l.starts_with("FAIL s10 (over ")),
        "{out}"
    );
    assert!(
        out.lines().any(|l| l == format!("FAIL s11 {NOT_RUN}")),
        "{out}"
    );
    assert!(
        out.lines()
            .any(|l| l == "exit-scripts: 0 passed, 1 failed, 1 not run (time budget 4 s)"),
        "{out}"
    );
}

#[test]
fn passes_are_kept_for_the_same_tree_and_a_fail_is_not() {
    let r = Repo::new(&[
        ("s10", "echo s10 >> \"$MASON_TEST_MARK\""),
        ("s11", "echo s11 >> \"$MASON_TEST_MARK\""),
        ("s12", "echo s12 >> \"$MASON_TEST_MARK\"; exit 1"),
    ]);
    let cache = tempfile::tempdir().unwrap();
    let marks = tempfile::tempdir().unwrap();
    let mark = marks.path().join("mark");
    let env = [
        ("MASON_TASK_CACHE", cache.path().to_str().unwrap()),
        ("MASON_EXIT_CACHE", "1"),
        ("MASON_TEST_MARK", mark.to_str().unwrap()),
    ];
    let (out, code) = r.run(&env);
    assert_ne!(code, 0, "{out}");
    assert!(out.lines().any(|l| l == "PASS s10"), "{out}");
    assert!(out.lines().any(|l| l == "PASS s11"), "{out}");
    assert!(out.lines().any(|l| l == "FAIL s12"), "{out}");

    let (out, code) = r.run(&env);
    assert_ne!(code, 0, "{out}");
    assert!(out.lines().any(|l| l == "PASS s10 (cached)"), "{out}");
    assert!(out.lines().any(|l| l == "PASS s11 (cached)"), "{out}");
    assert!(out.lines().any(|l| l == "FAIL s12"), "{out}");
    assert_eq!(runs_of(&mark, "s10"), 1);
    assert_eq!(runs_of(&mark, "s11"), 1);
    assert_eq!(runs_of(&mark, "s12"), 2, "a fail runs again");
}

#[test]
fn a_change_to_the_tree_or_to_the_script_runs_it_again() {
    let r = Repo::new(&[("s10", "echo s10 >> \"$MASON_TEST_MARK\"")]);
    let cache = tempfile::tempdir().unwrap();
    let marks = tempfile::tempdir().unwrap();
    let mark = marks.path().join("mark");
    let env = [
        ("MASON_TASK_CACHE", cache.path().to_str().unwrap()),
        ("MASON_EXIT_CACHE", "1"),
        ("MASON_TEST_MARK", mark.to_str().unwrap()),
    ];
    let p = r.path();
    let (_, code) = r.run(&env);
    assert_eq!(code, 0);
    let (out, _) = r.run(&env);
    assert!(out.lines().any(|l| l == "PASS s10 (cached)"), "{out}");
    assert_eq!(runs_of(&mark, "s10"), 1);

    // A tracked file edited.
    std::fs::write(p.join("README"), "changed\n").unwrap();
    let (out, code) = r.run(&env);
    assert_eq!(code, 0);
    assert!(out.lines().any(|l| l == "PASS s10"), "{out}");
    assert_eq!(runs_of(&mark, "s10"), 2);

    // An untracked file added.
    std::fs::write(p.join("new.txt"), "x\n").unwrap();
    let (out, _) = r.run(&env);
    assert!(out.lines().any(|l| l == "PASS s10"), "{out}");
    assert_eq!(runs_of(&mark, "s10"), 3);

    // The script itself edited (the tree changes with it, and the key holds its content too).
    let mut body = std::fs::read_to_string(p.join("scripts/exit/s10.sh")).unwrap();
    body.push_str("# edited\n");
    std::fs::write(p.join("scripts/exit/s10.sh"), body).unwrap();
    let (out, _) = r.run(&env);
    assert!(out.lines().any(|l| l == "PASS s10"), "{out}");
    assert_eq!(runs_of(&mark, "s10"), 4);

    // A new commit.
    git(p, &["add", "-A"]);
    git(p, &["commit", "-q", "-m", "more"]);
    let (out, _) = r.run(&env);
    assert!(out.lines().any(|l| l == "PASS s10"), "{out}");
    assert_eq!(runs_of(&mark, "s10"), 5);
    // And the same tree again is kept.
    let (out, _) = r.run(&env);
    assert!(out.lines().any(|l| l == "PASS s10 (cached)"), "{out}");
}

#[test]
fn a_cached_pass_is_shown_even_when_the_budget_is_spent() {
    let r = Repo::new(&[("s10", "exit 0"), ("s11", "exit 0")]);
    let cache = tempfile::tempdir().unwrap();
    let env = [
        ("MASON_TASK_CACHE", cache.path().to_str().unwrap()),
        ("MASON_EXIT_CACHE", "1"),
    ];
    let (_, code) = r.run(&env);
    assert_eq!(code, 0);
    let (out, code) = r.run(&[
        ("MASON_TASK_CACHE", cache.path().to_str().unwrap()),
        ("MASON_EXIT_CACHE", "1"),
        ("MASON_RUN_BUDGET_S", "1"),
    ]);
    assert_eq!(code, 0, "{out}");
    assert!(out.lines().any(|l| l == "PASS s10 (cached)"), "{out}");
    assert!(out.lines().any(|l| l == "PASS s11 (cached)"), "{out}");
    assert!(
        out.lines()
            .any(|l| l == "exit-scripts: 2 passed, 0 failed, 0 not run (time budget 1 s)"),
        "{out}"
    );
}

#[test]
fn without_a_budget_or_a_cache_the_output_is_what_it_was() {
    let r = Repo::new(&[("s10", "exit 0"), ("s11", "exit 0")]);
    let (out, code) = r.run(&[]);
    assert_eq!(code, 0);
    assert_eq!(out, "PASS s10\nPASS s11\n");
    // A cache directory that does not exist (a remote run's coordinator path) is not used.
    let (out, code) = r.run(&[
        ("MASON_TASK_CACHE", "/nonexistent/mason/cache"),
        ("MASON_EXIT_CACHE", "1"),
    ]);
    assert_eq!(code, 0);
    assert_eq!(out, "PASS s10\nPASS s11\n");

    let r = Repo::new(&[("s10", "echo boom; exit 1"), ("s11", "exit 0")]);
    let (out, code) = r.run(&[]);
    assert_ne!(code, 0);
    assert_eq!(out, "FAIL s10\n    | boom\nPASS s11\n");
}

#[test]
fn a_test_step_keeps_no_passes_without_the_check_opt_in() {
    // A test step has MASON_TASK_CACHE but not MASON_EXIT_CACHE: its verdict counts only what
    // it ran, so nothing is recorded or read.
    let r = Repo::new(&[("s10", "echo s10 >> \"$MASON_TEST_MARK\"")]);
    let cache = tempfile::tempdir().unwrap();
    let marks = tempfile::tempdir().unwrap();
    let mark = marks.path().join("mark");
    let env = [
        ("MASON_TASK_CACHE", cache.path().to_str().unwrap()),
        ("MASON_TEST_MARK", mark.to_str().unwrap()),
    ];
    let (out, code) = r.run(&env);
    assert_eq!(code, 0, "{out}");
    let (out, code) = r.run(&env);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out, "PASS s10\n");
    assert_eq!(runs_of(&mark, "s10"), 2);
    assert!(
        std::fs::read_dir(cache.path()).unwrap().next().is_none(),
        "nothing written to the task cache"
    );
}

#[test]
fn a_runner_off_this_mac_skips_the_passes_the_coordinator_sends_and_reports_new_ones() {
    let r = Repo::new(&[
        ("s10", "echo s10 >> \"$MASON_TEST_MARK\""),
        ("s11", "echo s11 >> \"$MASON_TEST_MARK\""),
        ("s12", "echo s12 >> \"$MASON_TEST_MARK\""),
    ]);
    let marks = tempfile::tempdir().unwrap();
    let mark = marks.path().join("mark");
    let base = [
        ("MASON_EXIT_CACHE", "1"),
        ("MASON_EXIT_TREE_KEY", "coordinator-tree"),
        ("MASON_TEST_MARK", mark.to_str().unwrap()),
    ];
    // The first run reports the key of each pass.
    let (out, code) = r.run(&base);
    assert_eq!(code, 0, "{out}");
    let keys: Vec<String> = lines_with(&out, "MASON-EXIT-KEY ")
        .iter()
        .map(|l| l.trim_start_matches("MASON-EXIT-KEY ").to_string())
        .collect();
    assert_eq!(keys.len(), 3, "{out}");
    assert_eq!(runs_of(&mark, "s10"), 1);

    // The coordinator sends the first script's key: the rest run, that one does not.
    let sent = keys[0].clone();
    let mut env = base.to_vec();
    env.push(("MASON_EXIT_PASSES", sent.as_str()));
    let (out, code) = r.run(&env);
    assert_eq!(code, 0, "{out}");
    assert!(out.lines().any(|l| l == "PASS s10 (cached)"), "{out}");
    assert!(out.lines().any(|l| l == "PASS s11"), "{out}");
    assert_eq!(runs_of(&mark, "s10"), 1, "a kept pass is not run again");
    assert_eq!(runs_of(&mark, "s11"), 2);

    // A key made for another tree matches nothing.
    let mut env = base.to_vec();
    env[1] = ("MASON_EXIT_TREE_KEY", "another-tree");
    env.push(("MASON_EXIT_PASSES", sent.as_str()));
    let (out, _) = r.run(&env);
    assert!(!out.contains("(cached)"), "{out}");

    // A test step (no MASON_EXIT_CACHE) ignores the keys and reports none.
    let env = [
        ("MASON_EXIT_TREE_KEY", "coordinator-tree"),
        ("MASON_EXIT_PASSES", sent.as_str()),
        ("MASON_TEST_MARK", mark.to_str().unwrap()),
    ];
    let (out, _) = r.run(&env);
    assert!(
        !out.contains("(cached)") && !out.contains("MASON-EXIT-KEY"),
        "{out}"
    );
}
