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
    let c = t.find("\nconcurrency:").expect("top-level concurrency group");
    let block: String = t[c..].lines().skip(1).take(3).collect::<Vec<_>>().join("\n");
    assert!(block.contains("group: ci-${{ github.ref }}"), "{block}");
    assert!(block.contains("cancel-in-progress: true"), "{block}");
}

#[test]
fn push_triggered_matrices_have_no_macos_for_exit_scripts_or_compose() {
    let t = ci();
    for name in ["exit-scripts", "monitor-compose"] {
        let j = job(&t, name);
        let os = j.lines().find(|l| l.trim_start().starts_with("os:")).unwrap();
        assert!(!os.contains("macos"), "{name} lists macOS: {os}");
    }
}
