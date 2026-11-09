// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason init`, `adopt` and `remove` end to end, against throwaway git repositories and a
//! throwaway config directory. No daemon, no network, no agent: the prove step is shown to
//! say what is missing rather than to run.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SAYING: &str = "This is only a starting point. You can change anything, any time, in the monitor or in `.mason/`.";

struct Fixture {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    config: PathBuf,
    data: PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn fixture(files: &[(&str, &str)]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("My Repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "hello\n").unwrap();
    for (p, t) in files {
        let f = repo.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    }
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "init"]);
    Fixture {
        config: dir.path().join("c"),
        data: dir.path().join("d"),
        repo,
        _dir: dir,
    }
}

impl Fixture {
    fn mason(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mason"))
            .args([
                "--config-dir",
                self.config.to_str().unwrap(),
                "--data-dir",
                self.data.to_str().unwrap(),
            ])
            .args(args)
            .current_dir(&self.repo)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    }
    fn overlay(&self) -> PathBuf {
        self.config.join("projects/my-repo.toml")
    }
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn ok(o: &Output) -> String {
    assert!(o.status.success(), "{}", text(o));
    text(o)
}

fn refused(o: &Output, why: &str) {
    assert!(!o.status.success(), "should have been refused: {}", text(o));
    assert!(text(o).contains(why), "wanted {why:?} in {}", text(o));
}

fn on_setup(f: &Fixture, path: &str) -> Option<String> {
    Command::new("git")
        .args(["show", &format!("mason/setup:{path}")])
        .current_dir(&f.repo)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
}

#[test]
fn yes_runs_without_a_terminal_and_writes_the_branch_the_overlay_and_the_notes() {
    let f = fixture(&[]);
    let out = ok(&f.mason(&[
        "init",
        "--workflow",
        "solo",
        "--yes",
        "--no-prove",
        "--suites",
        "skip",
    ]));
    assert!(out.contains(SAYING), "{out}");
    // The saying comes at the workflow step: after the detection, before the written files.
    assert!(
        out.find("Found a git repository").unwrap() < out.find(SAYING).unwrap()
            && out.find(SAYING).unwrap() < out.find("Written:").unwrap()
    );
    let mason = on_setup(&f, ".mason/mason.toml").expect(".mason/mason.toml on the setup branch");
    assert!(
        mason.starts_with("# SPDX-License-Identifier: MIT") && mason.contains("[step.develop]")
    );
    assert!(on_setup(&f, ".mason/landing.toml")
        .unwrap()
        .contains("mode = \"direct\""));
    let notes = on_setup(&f, "AGENTS.md").unwrap();
    assert!(notes.contains("## Filing work for the pipeline") && notes.contains("explored: true"));
    // Working tree and current branch untouched; nothing pushed (there is no remote to push to).
    assert!(!f.repo.join(".mason").exists() && !f.repo.join("AGENTS.md").exists());
    assert_eq!(git(&f.repo, &["branch", "--show-current"]).trim(), "main");
    assert_eq!(git(&f.repo, &["status", "--porcelain"]), "");
    assert_eq!(
        git(&f.repo, &["worktree", "list"]).lines().count(),
        1,
        "the temporary worktree is gone"
    );
    let overlay = std::fs::read_to_string(f.overlay()).unwrap();
    assert!(overlay.contains("base = \"main\"") && overlay.contains("test = \"true\""));
    assert!(std::fs::read_to_string(f.config.join("resources.toml"))
        .unwrap()
        .contains("claude = 1"));
    // The no-tests case is said, not hidden.
    assert!(out.contains("proves nothing"), "{out}");
}

#[test]
fn every_flag_changes_what_is_written() {
    let f = fixture(&[("CLAUDE.md", "# Mine\n")]);
    let out = ok(&f.mason(&[
        "init",
        "--yes",
        "--no-prove",
        "--id",
        "custom",
        "--tracker",
        "local",
        "--agent",
        "codex",
        "--agent-bin",
        "/opt/codex",
        "--sessions",
        "3",
        "--workflow",
        "human-gated",
        "--suites",
        "skip",
        "--base",
        "main",
        "--landing",
        "direct",
        "--alerts",
        "sound,speech,push",
    ]));
    let overlay = std::fs::read_to_string(f.config.join("projects/custom.toml")).unwrap();
    assert!(
        overlay.contains("provider = \"codex\"") && overlay.contains("codex_bin = \"/opt/codex\""),
        "{overlay}"
    );
    assert!(on_setup(&f, ".mason/mason.toml")
        .unwrap()
        .contains("[step.plan-ok]"));
    // The notes go to CLAUDE.md because that is the file there; no AGENTS.md is invented.
    assert!(on_setup(&f, "CLAUDE.md")
        .unwrap()
        .starts_with("# Mine\n\n## Filing work"));
    assert!(on_setup(&f, "AGENTS.md").is_none());
    let notify = std::fs::read_to_string(f.config.join("notify.toml")).unwrap();
    assert!(notify.contains("sound = 0") && notify.contains("speech = 0"));
    assert!(out.contains("push needs an account"), "{out}");
    assert!(
        out.contains("sessions for codex"),
        "no [llm] file is written for codex: {out}"
    );
    assert!(!f.config.join("resources.toml").exists());
}

#[test]
fn sessions_alerts_none_and_the_landing_flag_are_applied() {
    let f = fixture(&[("AGENTS.md", "# Agents\n")]);
    ok(&f.mason(&[
        "init",
        "--yes",
        "--no-prove",
        "--suites",
        "skip",
        "--sessions",
        "4",
        "--alerts",
        "none",
        "--workflow",
        "batched-ci",
        "--landing",
        "direct",
        "--base",
        "main",
    ]));
    assert!(std::fs::read_to_string(f.config.join("resources.toml"))
        .unwrap()
        .contains("claude = 4"));
    let notify = std::fs::read_to_string(f.config.join("notify.toml")).unwrap();
    assert!(notify.contains("monitor = 0") && !notify.contains("sound"));
    let landing = on_setup(&f, ".mason/landing.toml").unwrap();
    assert!(
        landing.contains("mode = \"direct\"") && !landing.contains("window"),
        "batching is off with direct landing: {landing}"
    );
    assert!(on_setup(&f, "AGENTS.md")
        .unwrap()
        .starts_with("# Agents\n\n## Filing work"));
}

#[test]
fn a_github_remote_unlocks_pr_landing_and_the_host_section() {
    let f = fixture(&[]);
    git(
        &f.repo,
        &["remote", "add", "origin", "git@github.com:acme/widgets.git"],
    );
    ok(&f.mason(&[
        "init",
        "--yes",
        "--no-prove",
        "--suites",
        "skip",
        "--workflow",
        "reviewed-pr",
        "--tracker",
        "github",
    ]));
    let overlay = std::fs::read_to_string(f.overlay()).unwrap();
    assert!(
        overlay.contains("remote = \"origin\"")
            && overlay.contains("[host]")
            && overlay.contains("repo = \"acme/widgets\"")
            && overlay.contains("[tracker]")
    );
    assert!(on_setup(&f, ".mason/landing.toml")
        .unwrap()
        .contains("mode = \"pr-each\""));
}

#[test]
fn each_refusal_says_what_to_do_and_writes_nothing() {
    let f = fixture(&[]);
    // Not a terminal, no --yes, no flags: it names the flag instead of guessing.
    refused(&f.mason(&["init"]), "--yes");
    refused(
        &f.mason(&["init", "--yes", "--workflow", "fancy"]),
        "--workflow",
    );
    refused(
        &f.mason(&["init", "--yes", "--tracker", "github"]),
        "GitHub remote",
    );
    refused(
        &f.mason(&["init", "--yes", "--landing", "pr"]),
        "GitHub remote",
    );
    refused(
        &f.mason(&["init", "--yes", "--landing", "direct-ci"]),
        "GitHub remote",
    );
    refused(
        &f.mason(&["init", "--yes", "--sessions", "0"]),
        "--sessions",
    );
    refused(
        &f.mason(&["init", "--yes", "--base", "nonesuch", "--no-prove"]),
        "does not exist",
    );
    refused(
        &f.mason(&["init", "--yes", "--bogus"]),
        "unexpected argument",
    );
    refused(&f.mason(&["init", "--yes", "--sessions"]), "needs a value");
    assert!(!f.overlay().exists() && !f.config.exists());
    assert_eq!(git(&f.repo, &["branch", "--list", "mason/*"]), "");
    // Not a repository at all.
    let o = Command::new(env!("CARGO_BIN_EXE_mason"))
        .args(["--config-dir", f.config.to_str().unwrap(), "init", "--yes"])
        .current_dir(f.config.parent().unwrap())
        .output()
        .unwrap();
    refused(&o, "git init");
    // No commits.
    let empty = tempfile::tempdir().unwrap();
    git(empty.path(), &["init", "-q", "-b", "main"]);
    let o = Command::new(env!("CARGO_BIN_EXE_mason"))
        .args([
            "--config-dir",
            f.config.to_str().unwrap(),
            "init",
            "--yes",
            "--no-prove",
        ])
        .current_dir(empty.path())
        .output()
        .unwrap();
    refused(&o, "no commits");
}

#[test]
fn a_second_init_neither_duplicates_the_notes_nor_overwrites_without_force() {
    let f = fixture(&[]);
    ok(&f.mason(&["init", "--yes", "--no-prove", "--suites", "skip"]));
    refused(
        &f.mason(&["init", "--yes", "--no-prove", "--suites", "skip"]),
        "already exists",
    );
    // Another id, same branch: refused for the branch.
    refused(
        &f.mason(&[
            "init",
            "--yes",
            "--no-prove",
            "--suites",
            "skip",
            "--id",
            "other",
        ]),
        "mason/setup already exists",
    );
    assert!(!f.config.join("projects/other.toml").exists());
    // Merge the setup, then init again: .mason/ is there, so it is refused and pointed at adopt.
    git(&f.repo, &["merge", "-q", "--ff-only", "mason/setup"]);
    refused(
        &f.mason(&[
            "init",
            "--yes",
            "--no-prove",
            "--suites",
            "skip",
            "--force-not-a-flag",
        ]),
        "unexpected argument",
    );
    refused(
        &f.mason(&[
            "init",
            "--yes",
            "--no-prove",
            "--suites",
            "skip",
            "--id",
            "other",
        ]),
        "mason adopt",
    );
    // --force replaces it, and the Filing section is still there exactly once.
    ok(&f.mason(&["init", "--yes", "--no-prove", "--suites", "skip", "--force"]));
    let notes = on_setup(&f, "AGENTS.md").unwrap();
    assert_eq!(
        notes.matches("## Filing work for the pipeline").count(),
        1,
        "{notes}"
    );
}

#[test]
fn suites_are_run_once_and_a_vacuous_one_is_dropped() {
    // A repository with a Makefile only: no suite can be proposed, and that is said.
    let f = fixture(&[("Makefile", "test:\n\ttrue\n")]);
    let out = ok(&f.mason(&["init", "--yes", "--no-prove"]));
    assert!(
        out.contains("no suite could be proposed") && out.contains("proves nothing"),
        "{out}"
    );
    assert!(on_setup(&f, ".mason/baselines.toml").is_none());
    // Python with one passing unittest: proposed, run, parsed, recorded.
    let g = fixture(&[("requirements.txt", ""), ("test_ok.py", "import unittest\nclass T(unittest.TestCase):\n    def test_one(self):\n        self.assertTrue(True)\n")]);
    if Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        let out = ok(&g.mason(&["init", "--yes", "--no-prove"]));
        assert!(
            out.contains("python: 1 test(s) found, 1 passed, 0 failed"),
            "{out}"
        );
        let b = on_setup(&g, ".mason/baselines.toml").unwrap();
        assert!(
            b.contains("[suite.python]")
                && b.contains("executed = 1")
                && b.contains("green = true"),
            "{b}"
        );
        let m = on_setup(&g, ".mason/mason.toml").unwrap();
        assert!(m.contains("suites = [\"python\"]") && m.contains("[suite.python]"));
        assert!(std::fs::read_to_string(g.overlay_named())
            .unwrap()
            .contains("unittest"));
    }
}

impl Fixture {
    fn overlay_named(&self) -> PathBuf {
        self.config.join("projects/my-repo.toml")
    }
}

#[test]
fn a_failing_baseline_is_reported_and_kept() {
    let f = fixture(&[("requirements.txt", ""), ("test_bad.py", "import unittest\nclass T(unittest.TestCase):\n    def test_one(self):\n        self.assertTrue(False)\n")]);
    if !Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        return;
    }
    let out = ok(&f.mason(&["init", "--yes", "--no-prove"]));
    assert!(
        out.contains("NOT green") && out.contains("red before any change"),
        "{out}"
    );
    assert!(on_setup(&f, ".mason/baselines.toml")
        .unwrap()
        .contains("green = false"));
}

#[test]
fn the_ci_trigger_is_checked_and_offered_only_on_the_branch() {
    let wf = ".github/workflows/ci.yml";
    let f = fixture(&[(wf, "on:\n  push:\n    branches: [main]\n")]);
    git(
        &f.repo,
        &["remote", "add", "origin", "https://github.com/acme/widgets"],
    );
    let out = ok(&f.mason(&[
        "init",
        "--yes",
        "--no-prove",
        "--suites",
        "skip",
        "--landing",
        "direct-ci",
    ]));
    assert!(
        out.contains("direct-ci will not work yet") && out.contains("mason/ci/**"),
        "{out}"
    );
    assert!(on_setup(&f, ".github/workflows/mason-ci.yml").is_none());
    assert!(std::fs::read_to_string(f.overlay())
        .unwrap()
        .contains("[ci]"));
    let g = fixture(&[(wf, "on:\n  push:\n    branches: [main]\n")]);
    git(
        &g.repo,
        &["remote", "add", "origin", "https://github.com/acme/widgets"],
    );
    let out = ok(&g.mason(&[
        "init",
        "--yes",
        "--no-prove",
        "--suites",
        "skip",
        "--landing",
        "direct-ci",
        "--ci-workflow",
        "yes",
    ]));
    assert!(
        out.contains("added .github/workflows/mason-ci.yml"),
        "{out}"
    );
    assert!(on_setup(&g, ".github/workflows/mason-ci.yml")
        .unwrap()
        .contains("mason/ci/**"));
    assert!(
        !g.repo.join(".github/workflows/mason-ci.yml").exists(),
        "the working tree is untouched"
    );
    let h = fixture(&[(wf, "on:\n  workflow_dispatch:\n")]);
    git(
        &h.repo,
        &["remote", "add", "origin", "https://github.com/acme/widgets"],
    );
    assert!(ok(&h.mason(&[
        "init",
        "--yes",
        "--no-prove",
        "--suites",
        "skip",
        "--landing",
        "direct-ci"
    ]))
    .contains("already runs"));
}

#[test]
fn prove_says_what_is_missing_instead_of_pretending() {
    let f = fixture(&[]);
    let out = ok(&f.mason(&["init", "--yes", "--suites", "skip"]));
    assert!(
        out.contains("Prove it:") && out.contains("has no .mason/ yet"),
        "{out}"
    );
    git(&f.repo, &["merge", "-q", "--ff-only", "mason/setup"]);
    refused(&f.mason(&["init", "--prove-only"]), "masond is not running");
}

#[test]
fn adopt_registers_an_existing_mason_folder_and_never_writes_it() {
    let f = fixture(&[]);
    // Nothing to adopt.
    refused(&f.mason(&["adopt"]), "mason init");
    let def = include_str!("../../../templates/solo-fast/mason.toml")
        .replace("# suites: mason init fills this in\n", "");
    std::fs::create_dir_all(f.repo.join(".mason")).unwrap();
    std::fs::write(f.repo.join(".mason/mason.toml"), &def).unwrap();
    ok(&f.mason(&["adopt"]));
    assert_eq!(
        std::fs::read_to_string(f.repo.join(".mason/mason.toml")).unwrap(),
        def,
        ".mason/ is byte for byte as it was"
    );
    assert!(!f.repo.join(".mason/landing.toml").exists());
    assert!(std::fs::read_to_string(f.overlay())
        .unwrap()
        .contains("path = "));
    // Registered already: refused, overlay as it was.
    let before = std::fs::read_to_string(f.overlay()).unwrap();
    refused(&f.mason(&["adopt", "--test", "make"]), "already registered");
    assert_eq!(std::fs::read_to_string(f.overlay()).unwrap(), before);
    // A broken .mason/mason.toml is said before anything is written.
    let g = fixture(&[(".mason/mason.toml", "schema = 1\nstart = \"nope\"\n")]);
    refused(&g.mason(&["adopt"]), "does not parse");
    assert!(!g.overlay().exists());
}

#[test]
fn remove_lists_first_then_deletes_what_is_ours_and_leaves_the_rest() {
    let f = fixture(&[("AGENTS.md", "# Mine\n\nKeep this.\n")]);
    ok(&f.mason(&["init", "--yes", "--no-prove", "--suites", "skip"]));
    git(&f.repo, &["merge", "-q", "--ff-only", "mason/setup"]);
    assert!(f.repo.join(".mason").exists());
    // Without --yes: a list, exit 2, nothing changed.
    let o = f.mason(&["remove"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(
        text(&o).contains("Would remove") && f.overlay().exists() && f.repo.join(".mason").exists()
    );
    ok(&f.mason(&["remove", "--yes"]));
    assert!(!f.overlay().exists() && !f.repo.join(".mason").exists());
    assert_eq!(
        std::fs::read_to_string(f.repo.join("AGENTS.md")).unwrap(),
        "# Mine\n\nKeep this.\n"
    );
    // Left alone, and said.
    let out = text(&f.mason(&["remove", "--yes"]));
    assert!(out.contains("Nothing to remove"), "{out}");
    assert!(
        git(&f.repo, &["branch", "--list", "mason/setup"]).contains("mason/setup"),
        "the setup branch is not deleted"
    );
    assert!(f.config.join("resources.toml").exists());
}

#[test]
fn remove_keeps_notes_that_were_edited_and_deletes_a_file_that_was_only_ours() {
    let f = fixture(&[]);
    ok(&f.mason(&["init", "--yes", "--no-prove", "--suites", "skip"]));
    git(&f.repo, &["merge", "-q", "--ff-only", "mason/setup"]);
    let notes = std::fs::read_to_string(f.repo.join("AGENTS.md")).unwrap();
    std::fs::write(
        f.repo.join("AGENTS.md"),
        notes.replace("explored: true", "explored: yes"),
    )
    .unwrap();
    let out = ok(&f.mason(&["remove", "--yes"]));
    assert!(
        out.contains("kept: the Filing section") && f.repo.join("AGENTS.md").exists(),
        "{out}"
    );
    // Untouched notes in a file that held nothing else: the file goes.
    let g = fixture(&[]);
    ok(&g.mason(&["init", "--yes", "--no-prove", "--suites", "skip"]));
    git(&g.repo, &["merge", "-q", "--ff-only", "mason/setup"]);
    ok(&g.mason(&["remove", "--yes"]));
    assert!(!g.repo.join("AGENTS.md").exists() && !g.repo.join(".mason").exists());
}

#[test]
fn doctor_warns_when_mason_on_path_is_someone_elses() {
    let f = fixture(&[]);
    let bin = f.config.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = bin.join("mason");
    std::fs::write(&fake, "#!/bin/sh\necho 'Bricks are great: mason_cli 0.1'\n").unwrap();
    Command::new("chmod")
        .args(["+x", fake.to_str().unwrap()])
        .status()
        .unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let o = Command::new(env!("CARGO_BIN_EXE_mason"))
        .args([
            "--config-dir",
            f.config.to_str().unwrap(),
            "--data-dir",
            f.data.to_str().unwrap(),
            "doctor",
        ])
        .env("PATH", path)
        .output()
        .unwrap();
    let out = text(&o);
    assert!(
        out.contains("is not mason-pipeline")
            && out.contains("Bricks are great")
            && out.contains("dart pub global deactivate"),
        "{out}"
    );
    // With no mason on PATH at all, it says so instead.
    let o = Command::new(env!("CARGO_BIN_EXE_mason"))
        .args([
            "--config-dir",
            f.config.to_str().unwrap(),
            "--data-dir",
            f.data.to_str().unwrap(),
            "doctor",
        ])
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert!(
        text(&o).contains("`mason` is not on PATH") || text(&o).contains("is not mason-pipeline"),
        "{}",
        text(&o)
    );
}
