// SPDX-License-Identifier: GPL-3.0-or-later

//! A real run: the real `claude` CLI, the real MCP bridge, a scratch directory.
//! Ignored by default (it spends a few cents and needs a logged-in CLI), and
//! never part of the automatic suite. Run it by hand:
//!
//!     cargo test -p mason-adapters --test real_claude -- --ignored --nocapture
//!
//! `CLAUDE_BIN` overrides where the CLI is found.

use mason_adapter_api::contract::{ContractHandle, NoChecks};
use mason_adapter_api::{SessionSpec, StepKind};
use mason_adapters::claude::ClaudeAdapter;
use mason_adapters::runner::{RunnerConfig, StepEnd, StepRunner};
use std::path::PathBuf;

fn find_claude() -> PathBuf {
    if let Some(p) = std::env::var_os("CLAUDE_BIN") {
        return p.into();
    }
    let home = std::env::var("HOME").unwrap();
    let base = PathBuf::from(home).join("Library/Application Support/Claude/claude-code");
    let mut found: Vec<PathBuf> = std::fs::read_dir(&base)
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|v| std::fs::read_dir(v.path()).into_iter().flatten().flatten())
        .map(|e| e.path().join("claude.app/Contents/MacOS/claude"))
        .filter(|p| p.exists())
        .collect();
    found.sort();
    found.pop().expect("no claude binary found; set CLAUDE_BIN")
}

#[test]
#[ignore = "spends real tokens; run by hand"]
fn a_real_explore_step_submits_a_plan_through_the_real_bridge() {
    let d = tempfile::tempdir_in("/tmp").unwrap();
    let work = d.path().join("repo");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("hello.txt"), "hello\n").unwrap();
    let _ = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&work)
        .status();

    let adapter = ClaudeAdapter::new(
        find_claude(),
        env!("CARGO_BIN_EXE_mason-mcp"),
        d.path().join("state"),
    );
    let contract = ContractHandle::new(StepKind::Explore, Box::new(NoChecks));
    contract.serve(&d.path().join("c.sock")).unwrap();
    let mut spec = SessionSpec::new(
        StepKind::Explore,
        "Task: add a file goodbye.txt next to hello.txt containing the word goodbye. \
         Read hello.txt, then submit your plan.",
        &work,
    );
    spec.model = Some("haiku".into());
    spec.deadline = Some(std::time::Duration::from_secs(180));
    let r = StepRunner::new(&adapter, RunnerConfig::default()).run(spec, &contract);
    println!("end: {:?}", r.end);
    println!("tools: {:?}", r.tool_calls);
    println!("tokens: {:?}", r.tokens);
    println!("usage: {:?}", r.usage);
    println!("nudges {} rejections {}", r.nudges, r.rejections);
    assert!(
        matches!(&r.end, StepEnd::Accepted { tool, .. } if tool == "submit_plan"),
        "{:?}",
        r.end
    );
    assert!(
        !work.join("goodbye.txt").exists(),
        "an explore step must not change the tree"
    );
}

#[test]
#[ignore = "spends real tokens; run by hand"]
fn a_real_develop_step_edits_commits_and_submits_a_change() {
    let d = tempfile::tempdir_in("/tmp").unwrap();
    let work = d.path().join("repo");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("hello.txt"), "hello\n").unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(&work)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "T"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);

    let adapter = ClaudeAdapter::new(
        find_claude(),
        env!("CARGO_BIN_EXE_mason-mcp"),
        d.path().join("state"),
    );
    let contract = ContractHandle::new(StepKind::Develop, Box::new(NoChecks));
    contract.serve(&d.path().join("c.sock")).unwrap();
    let mut spec = SessionSpec::new(
        StepKind::Develop,
        "Create goodbye.txt containing the word goodbye, commit it with git (git add, git commit), \
         then call submit_change with the new commit hash from `git rev-parse HEAD`.",
        &work,
    );
    spec.model = Some("haiku".into());
    spec.deadline = Some(std::time::Duration::from_secs(240));
    let r = StepRunner::new(&adapter, RunnerConfig::default()).run(spec, &contract);
    println!("end: {:?}", r.end);
    println!("tools: {:?}", r.tool_calls);
    println!("nudges {} rejections {}", r.nudges, r.rejections);
    assert!(
        matches!(&r.end, StepEnd::Accepted { tool, .. } if tool == "submit_change"),
        "{:?}",
        r.end
    );
    assert_eq!(
        std::fs::read_to_string(work.join("goodbye.txt"))
            .unwrap()
            .trim(),
        "goodbye"
    );
}
