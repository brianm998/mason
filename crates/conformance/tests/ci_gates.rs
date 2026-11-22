// SPDX-License-Identifier: GPL-3.0-or-later

//! The tree stays `cargo fmt` clean (the landing runs the workspace tests, not fmt, so
//! unformatted code landed on develop until this test existed), and `ci.yml` keeps the
//! rules that stop it using up the account's macOS runners (decision 0204).

use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn workspace_is_fmt_clean() {
    let out = Command::new("cargo")
        .args(["fmt", "--all", "--check"])
        .current_dir(root())
        .output()
        .expect("cargo fmt runs");
    assert!(
        out.status.success(),
        "run `cargo fmt --all`:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

fn ci() -> String {
    std::fs::read_to_string(root().join(".github/workflows/ci.yml")).unwrap()
}

/// The text of one top-level job (from `  name:` to the next job).
fn job(text: &str, name: &str) -> String {
    let head = format!("\n  {name}:\n");
    let start = text.find(&head).unwrap_or_else(|| panic!("no job {name}"));
    let rest = &text[start + head.len()..];
    let end = rest
        .lines()
        .scan(0usize, |pos, l| {
            let at = *pos;
            *pos += l.len() + 1;
            Some((at, l))
        })
        .find(|(_, l)| l.starts_with("  ") && !l.starts_with("   ") && l.trim_end().ends_with(':'))
        .map(|(at, _)| at)
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

#[test]
fn ci_cancels_superseded_runs() {
    let t = ci();
    let c = t
        .find("\nconcurrency:")
        .expect("top-level concurrency group");
    let block: String = t[c..]
        .lines()
        .skip(1)
        .take(3)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(block.contains("group: ci-${{ github.ref }}"), "{block}");
    // Superseded runs are cancelled on every ref except develop, which queues
    // so the base-branch CI watch always has a finished run to read.
    assert!(
        block.contains("cancel-in-progress: ${{ github.ref != 'refs/heads/develop' }}"),
        "{block}"
    );
    // The full run must not share a group with pushes.
    assert!(
        block.contains("github.event_name == 'workflow_dispatch' && 'full' || 'push'"),
        "{block}"
    );
}

#[test]
fn push_trigger_ignores_readme_only_commits() {
    let t = ci();
    let on = &t[t.find("\non:").expect("on:")..t.find("\nconcurrency:").unwrap()];
    let push = &on[on.find("\n  push:").expect("push trigger")..];
    let push = &push[..push[1..]
        .find("\n  pull_request")
        .map_or(push.len(), |i| i + 1)];
    assert!(push.contains("paths-ignore: [\"README.md\"]"), "{push}");
    let pr = &on[on.find("\n  pull_request:").unwrap()..];
    assert!(
        !pr.contains("paths-ignore"),
        "pull_request must not ignore paths"
    );
}

#[test]
fn push_triggered_matrices_have_no_macos_for_exit_scripts_or_compose() {
    let t = ci();
    for name in ["exit-scripts", "monitor-compose"] {
        let j = job(&t, name);
        let os = j
            .lines()
            .find(|l| l.trim_start().starts_with("os:"))
            .unwrap();
        assert!(!os.contains("macos"), "{name} lists macOS: {os}");
    }
}

#[test]
fn the_test_step_has_a_fast_fmt_suite_that_holds_no_slot() {
    // s691: an unformatted change goes back from its own test step, even when the owner
    // skipped `cargo-workspace`, because `fmt` is a suite of its own.
    let text = std::fs::read_to_string(root().join(".mason/mason.toml")).unwrap();
    let step = text.split("[step.test]").nth(1).unwrap();
    let step = step.split("\n[suite.").next().unwrap();
    assert!(step.contains("\"fmt\""), "step.test lists the fmt suite");
    let suite = text.split("[suite.fmt]").nth(1).expect("a fmt suite");
    let suite = suite.split("\n[").next().unwrap();
    assert!(suite.contains("cargo fmt --all -- --check"));
    assert!(suite.contains("level = \"unit\"") && suite.contains("uses = {}"));
    assert!(!suite.contains("build ="), "no build step");
    // The crates line must select both suites; other suites (clippy) may sit beside them.
    let line = text
        .lines()
        .find(|l| l.starts_with("\"crates/**\" ="))
        .expect("a crates impact line");
    assert!(
        line.contains("\"cargo-workspace\"") && line.contains("\"fmt\""),
        "{line}"
    );
}

/// s800: a push to develop runs only the fast jobs; the full matrix is `workflow_dispatch`
/// (and pull requests), started by MASON's full-test schedule; there is no GitHub cron.
#[test]
fn push_runs_only_the_fast_jobs_and_the_full_matrix_is_dispatched() {
    let t = ci();
    assert!(!t.contains("\n  schedule:"), "no GitHub cron");
    assert!(!t.contains("cron:"), "no GitHub cron");
    let on = &t[t.find("\non:").expect("on:")..t.find("\nconcurrency:").unwrap()];
    assert!(on.contains("\n  workflow_dispatch:"), "{on}");
    assert!(on.contains("\n      macos:"), "an input for the macOS legs");
    assert!(t.contains("run-name:") && t.contains("'CI: full'"));
    for name in ["lint", "windows", "dco"] {
        assert!(
            !job(&t, name).contains("event_name != 'push'"),
            "{name} is a fast job"
        );
    }
    for name in [
        "test",
        "exit-scripts",
        "monitor-swift",
        "monitor-compose",
        "device-sample-ios",
        "device-sample-android",
    ] {
        assert!(
            job(&t, name).contains("if: github.event_name != 'push'"),
            "{name} must not run on push"
        );
    }
    for name in ["exit-scripts-macos", "monitor-compose-macos"] {
        assert!(
            job(&t, name).contains("if: github.event_name == 'workflow_dispatch' && inputs.macos"),
            "{name} runs only on a full run with macOS"
        );
    }
}

fn check_workflows(files: &[&std::path::Path]) -> std::process::Output {
    Command::new("bash")
        .arg(root().join("scripts/check-workflows.sh"))
        .args(files)
        .current_dir(root())
        .output()
        .expect("check-workflows.sh runs")
}

/// A workflow GitHub cannot parse fails instantly on every push (s808): every workflow and
/// template parses, and the checker rejects the unquoted `run-name` that broke `ci.yml`.
#[test]
fn every_workflow_parses() {
    let out = check_workflows(&[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(ci()
        .lines()
        .any(|l| l.starts_with("run-name: \"") && l.contains("CI: full")));
}

#[test]
fn workflow_check_rejects_unquoted_colon_scalar() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.yml");
    let good = dir.path().join("good.yml");
    std::fs::write(&bad, "name: x\nrun-name: ${{ 'a: b' }}\non: push\n").unwrap();
    std::fs::write(&good, "name: x\nrun-name: \"${{ 'a: b' }}\"\non: push\n").unwrap();
    assert!(!check_workflows(&[&bad]).status.success());
    assert!(check_workflows(&[&good]).status.success());
}
