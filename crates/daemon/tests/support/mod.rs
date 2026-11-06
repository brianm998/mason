// SPDX-License-Identifier: GPL-3.0-or-later
#![allow(dead_code)]

//! A scratch world for end-to-end tests: a bare "origin", a clone, a project
//! file pointing at the scripted stand-in for `claude`, and a daemon config.

use mason_core::*;
use mason_daemon::config::DaemonConfig;
use mason_daemon::Daemon;
use mason_landing::git;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A binary from another package. Cargo only builds (and rebuilds) every
/// package's binaries for `cargo test --workspace`, so build it, once per run:
/// a stale binary would silently test old behaviour.
pub fn bin(package: &str, name: &str) -> PathBuf {
    static BUILT: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let exe = std::env::current_exe().unwrap();
    let target = exe.parent().unwrap().parent().unwrap();
    let mut built = BUILT.lock().unwrap();
    if !built.iter().any(|b| b == name) {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let ok = std::process::Command::new(cargo)
            .args(["build", "-q", "-p", package, "--bin", name])
            .status()
            .unwrap()
            .success();
        assert!(ok, "could not build {name}");
        built.push(name.to_string());
    }
    target.join(name)
}

pub struct Scratch {
    pub root: tempfile::TempDir,
    pub origin: PathBuf,
    pub repo: PathBuf,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub script: PathBuf,
}

pub const TASK_SCRIPT_COMMIT: &str =
    "echo hello > hello.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm 'add hello'";

pub fn sh(cwd: &Path, args: &[&str]) -> String {
    git::run(cwd, args).unwrap_or_else(|e| panic!("{e}"))
}

impl Scratch {
    pub fn new(test_cmd: &str, script: Value) -> Scratch {
        let root = tempfile::Builder::new()
            .prefix("ms")
            .tempdir_in("/tmp")
            .unwrap();
        let r = std::fs::canonicalize(root.path()).unwrap();
        let origin = r.join("o.git");
        let repo = r.join("repo");
        std::fs::create_dir_all(&origin).unwrap();
        sh(&origin, &["init", "-q", "--bare", "-b", "main"]);
        sh(&r, &["clone", "-q", origin.to_str().unwrap(), "repo"]);
        sh(&repo, &["checkout", "-q", "-b", "main"]);
        std::fs::write(repo.join("README"), "scratch\n").unwrap();
        sh(&repo, &["add", "-A"]);
        sh(&repo, &["commit", "-q", "-m", "init"]);
        sh(&repo, &["push", "-q", "origin", "main"]);
        // The owner's checkout sits on another branch, as it would while MASON works.
        sh(&repo, &["checkout", "-q", "-b", "owner-work"]);

        let config_dir = r.join("c");
        std::fs::create_dir_all(config_dir.join("projects")).unwrap();
        let script_path = r.join("script.json");
        std::fs::write(&script_path, script.to_string()).unwrap();
        let s = Scratch {
            root,
            origin,
            repo,
            config_dir,
            data_dir: r.join("d"),
            script: script_path,
        };
        s.write_project("scratch", test_cmd, "");
        s
    }

    pub fn write_project(&self, id: &str, test_cmd: &str, extra: &str) {
        let toml = format!(
            "path = \"{repo}\"\nbase = \"main\"\nremote = \"origin\"\ntest = \"{test}\"\nclaude_bin = \"{fake}\"\n\
             step_timeout_s = 120\ntest_timeout_s = 60\n{extra}\n[agent_env]\nFAKE_CLAUDE_SCRIPT = \"{script}\"\n",
            repo = self.repo.display(),
            test = test_cmd,
            fake = bin("mason-adapters", "fake-claude").display(),
            script = self.script.display(),
        );
        std::fs::write(
            self.config_dir.join("projects").join(format!("{id}.toml")),
            toml,
        )
        .unwrap();
    }

    pub fn config(&self) -> DaemonConfig {
        let mut c = DaemonConfig::new(
            &self.data_dir,
            &self.config_dir,
            bin("mason-step-host", "step-host"),
            bin("mason-adapters", "mason-mcp"),
        );
        c.sweep_interval = Duration::from_secs(3600);
        c.heartbeat = Duration::from_millis(100);
        c.hold_retry = Duration::from_millis(300);
        c
    }

    pub fn start(&self) -> Arc<Daemon> {
        Daemon::start(self.config()).expect("daemon starts")
    }

    pub fn origin_log(&self) -> Vec<String> {
        sh(&self.origin, &["log", "--format=%s", "main"])
            .lines()
            .map(String::from)
            .collect()
    }
}

pub fn explore_turn() -> Value {
    json!([{"call": {"tool": "submit_plan", "args": {
        "summary": "add hello.txt", "files": ["hello.txt"], "approach": "write the file",
        "acceptance_criteria": ["hello.txt exists"]}}}])
}

pub fn develop_turn() -> Value {
    json!([
        {"sh": TASK_SCRIPT_COMMIT},
        {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "$HEAD"}}}
    ])
}

/// The script for the happy path: one explore turn, one develop turn.
pub fn happy_script() -> Value {
    json!({"steps": {"explore": [explore_turn()], "develop": [develop_turn()]}})
}

pub fn file_task(d: &Arc<Daemon>, title: &str) -> TaskId {
    let pid = d.project_id_by_name("scratch").unwrap();
    let a = d
        .emit(EventKind::TaskFiled {
            project: pid,
            title: title.into(),
            body: format!("Please do: {title}"),
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            explored: false,
            proposed: false,
            blockers: vec![],
            not_before: None,
        })
        .unwrap();
    a.effects
        .iter()
        .find_map(|e| {
            if let Effect::TaskCreated { task } = e {
                Some(*task)
            } else {
                None
            }
        })
        .unwrap()
}

pub fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !f() {
        assert!(
            Instant::now() < end,
            "timed out after {secs}s waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn state_of(d: &Arc<Daemon>, t: TaskId) -> State {
    d.world().unwrap().tasks[&t].state
}

pub fn wait_state(d: &Arc<Daemon>, t: TaskId, want: State, secs: u64) {
    wait_for(&format!("task {t} to reach {want:?}"), secs, || {
        let s = state_of(d, t);
        if s != want && matches!(s, State::Failed | State::Canceled | State::Landed) && s != want {
            let evs: Vec<String> = d
                .store
                .read_since(0, 1000)
                .unwrap()
                .iter()
                .map(|e| format!("{}: {:?}", e.seq, e.event.kind))
                .collect();
            panic!(
                "task {t} ended in {s:?} instead of {want:?}\nevents:\n{}\nlogs:\n{}",
                evs.join("\n"),
                mason_daemon::http::render_logs(d, t.0)
            );
        }
        s == want
    });
}
