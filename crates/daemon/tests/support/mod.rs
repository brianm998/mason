// SPDX-License-Identifier: GPL-3.0-or-later
#![allow(dead_code)]

//! A scratch world for end-to-end tests: a bare "origin", a clone, a project
//! file pointing at the scripted stand-in for `claude`, and a daemon config.

pub mod lw;

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
    /// Held for as long as this scratch world lives (declared last, so released last).
    _slot: Slot,
}

/// How many scratch worlds (each a real daemon with step hosts, git and a scripted
/// agent) one test binary runs at once. The test harness starts every test of a
/// binary together, one thread per CPU, so `definitions.rs` alone ran 21 daemons at
/// once; with several tasks' tests on the same machine their own deadlines then ran
/// out for want of a CPU. A test waits for a slot before its world exists, so the
/// wait never counts against its deadlines. `MASON_TEST_WORLDS` overrides the number.
fn world_limit() -> usize {
    std::env::var("MASON_TEST_WORLDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or_else(|| {
            let cpus = std::thread::available_parallelism().map_or(4, |n| n.get());
            (cpus / 4).clamp(2, 8)
        })
}

struct Slot;

static WORLDS: (std::sync::Mutex<usize>, std::sync::Condvar) =
    (std::sync::Mutex::new(0), std::sync::Condvar::new());

impl Slot {
    fn take() -> Slot {
        let (m, cv) = &WORLDS;
        let limit = world_limit();
        let mut n = m.lock().unwrap_or_else(|e| e.into_inner());
        // A test that holds one world and makes another could wait for ever if every
        // slot were held that way, so after a while it goes ahead regardless.
        let end = Instant::now() + Duration::from_secs(600);
        while *n >= limit && Instant::now() < end {
            n = cv
                .wait_timeout(n, Duration::from_millis(500))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *n += 1;
        Slot
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let (m, cv) = &WORLDS;
        let mut n = m.lock().unwrap_or_else(|e| e.into_inner());
        *n = n.saturating_sub(1);
        cv.notify_one();
    }
}

/// What the scripted agent commits. Every task's worktree also gets a file of its
/// own, so a task that starts after another has landed still has something to
/// commit, whatever the timing (identical commits would leave it with nothing).
pub const TASK_SCRIPT_COMMIT: &str = "echo hello > hello.txt && echo task > unique-$(basename \"$PWD\").txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm 'add hello'";

/// The same as `TASK_SCRIPT_COMMIT` (kept for the tests that named it).
pub const UNIQUE_COMMIT: &str = TASK_SCRIPT_COMMIT;

pub fn sh(cwd: &Path, args: &[&str]) -> String {
    git::run(cwd, args).unwrap_or_else(|e| panic!("{e}"))
}

impl Scratch {
    pub fn new(test_cmd: &str, script: Value) -> Scratch {
        let slot = Slot::take();
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
            _slot: slot,
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
        // Tests never read the machine's real load.
        c.cpu = Some(mason_daemon::cpu::FakeCpu::new(0));
        // Nor the real disk: a full volume must not hold up the suite.
        c.disk = Some(mason_daemon::disk::FakeDisk::new_gb(1000));
        c
    }

    /// What a daemon that ran before would already have fetched: startup reads the
    /// remote-tracking refs as they are and does no network (the watcher fetches).
    pub fn prefetch(&self) {
        // Only a remote on this disk: a test that points origin at a network
        // address (a silent listener) means to leave it unanswered.
        let url = git::run(&self.repo, &["config", "remote.origin.url"])
            .map(|o| o.trim().to_string())
            .unwrap_or_default();
        if !url.starts_with('/') {
            return;
        }
        let _ = git::run_for(
            &self.repo,
            &["fetch", "-q", "--prune", "origin"],
            Duration::from_secs(120),
        );
    }

    pub fn start(&self) -> Arc<Daemon> {
        self.start_with(|_| {})
    }

    /// Start a daemon after letting `f` adjust its configuration.
    pub fn start_with(&self, f: impl FnOnce(&mut DaemonConfig)) -> Arc<Daemon> {
        self.prefetch();
        let mut c = self.config();
        f(&mut c);
        Daemon::start(c).expect("daemon starts")
    }

    /// Commit files to the remote's `main`, as somebody else would (a landed
    /// change to `.mason/`, a new source file, ...), without touching our clone.
    pub fn push_files(&self, files: &[(&str, &str)], msg: &str) {
        let dir = self.repo.parent().unwrap().join("pusher");
        if !dir.exists() {
            sh(
                self.repo.parent().unwrap(),
                &[
                    "clone",
                    "-q",
                    self.origin.to_str().unwrap(),
                    dir.to_str().unwrap(),
                ],
            );
        }
        sh(&dir, &["pull", "-q", "origin", "main"]);
        for (path, body) in files {
            let p = dir.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        sh(&dir, &["add", "-A"]);
        sh(&dir, &["commit", "-q", "-m", msg]);
        sh(&dir, &["push", "-q", "origin", "HEAD:main"]);
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

/// Like `happy_script`, but every task commits a file of its own (see `UNIQUE_COMMIT`),
/// for tests where tasks run one after another.
pub fn unique_script() -> Value {
    json!({"steps": {"explore": [explore_turn()], "develop": [[
        {"sh": UNIQUE_COMMIT},
        {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "$HEAD"}}}
    ]]}})
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

/// File a task with a body and an explicit explored claim and trust.
pub fn file_task_as(
    d: &Arc<Daemon>,
    title: &str,
    body: &str,
    explored: bool,
    trust: TrustSource,
    proposed: bool,
) -> TaskId {
    let pid = d.project_id_by_name("scratch").unwrap();
    let a = d
        .emit(EventKind::TaskFiled {
            project: pid,
            title: title.into(),
            body: body.into(),
            priority: Priority::Normal,
            trust,
            explored,
            proposed,
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

pub fn task_of(d: &Arc<Daemon>, t: TaskId) -> Task {
    d.world().unwrap().tasks[&t].clone()
}

/// The step names a task ran, in order.
pub fn steps_run(d: &Arc<Daemon>, t: TaskId) -> Vec<String> {
    d.world()
        .unwrap()
        .step_runs
        .values()
        .filter(|r| r.task == t)
        .map(|r| r.name.clone())
        .collect()
}

/// Every wait gets at least this long: a step host can take over a minute to
/// launch on a busy or syspolicyd-stalled Mac, and that is not a test failure.
const MIN_WAIT_SECS: u64 = 900;

pub fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let secs = secs.max(MIN_WAIT_SECS);
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
        if s != want && matches!(s, State::Failed | State::Canceled | State::Landed) {
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

// ---- running the real binaries ----

use std::process::{Child, Command, Output, Stdio};

pub struct Masond {
    pub child: Child,
    data_dir: PathBuf,
}

impl Drop for Masond {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        kill_hosts(&self.data_dir);
    }
}

/// Step hosts outlive the daemon by design; tests must not leak them.
///
/// Only a host whose pids still are the processes the registry recorded (start
/// strings match, the command line names this host directory, the child leads its
/// own group) is signalled; pids 0 and 1 and our own pid and group never are. A
/// registry keeps every host that ever started, and on a busy machine (ten tasks
/// running tests at once) killing a stale pid could take down another task's host.
pub fn kill_hosts(data_dir: &Path) {
    mason_step_host::registry::reap_verified(&data_dir.join("h"));
}

impl Scratch {
    pub fn masond(&self, extra_args: &[&str], env: &[(&str, &str)]) -> Masond {
        self.masond_with(true, extra_args, env)
    }

    /// As `masond`, but with no `--port`: what the port tests need.
    pub fn masond_no_port(&self, extra_args: &[&str]) -> Masond {
        self.masond_with(false, extra_args, &[])
    }

    fn masond_with(&self, free_port: bool, extra_args: &[&str], env: &[(&str, &str)]) -> Masond {
        self.prefetch();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_masond"));
        let cfg = self.config();
        cmd.args(["--data-dir", &self.data_dir.display().to_string()])
            .args(["--config-dir", &self.config_dir.display().to_string()])
            .args(["--host-bin", &cfg.host_bin.display().to_string()])
            .args(["--mcp-bin", &cfg.mcp_bin.display().to_string()])
            .args([
                "--heartbeat-ms",
                "100",
                "--hold-retry-s",
                "1",
                "--sweep-ms",
                "3600000",
            ])
            // A free port, so parallel test daemons never collide on the default.
            .args(if free_port {
                vec!["--port", "0"]
            } else {
                vec![]
            })
            .args(extra_args)
            .stdout(Stdio::null())
            .stderr(match std::env::var("MASON_TEST_LOG") {
                // A path: append the daemon's log there (failures then quote its tail).
                Ok(p) if p.starts_with('/') => std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(p)
                    .map_or(Stdio::null(), Stdio::from),
                Ok(_) => Stdio::inherit(),
                Err(_) => Stdio::null(),
            });
        // Tests never read the machine's real load, and never run a sampler per daemon.
        let cpu = self.root.path().join("cpu-for-masond");
        let _ = std::fs::write(&cpu, "0\n");
        cmd.env("MASON_CPU_FILE", &cpu);
        let disk = self.root.path().join("disk-for-masond");
        let _ = std::fs::write(&disk, "1000\n");
        cmd.env("MASON_DISK_FILE", &disk);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let child = cmd.spawn().expect("masond starts");
        Masond {
            child,
            data_dir: self.data_dir.clone(),
        }
    }

    /// Wait until the daemon has announced itself and answers.
    pub fn wait_daemon(&self, m: &mut Masond) {
        wait_for("masond to answer", 60, || {
            if let Ok(Some(st)) = m.child.try_wait() {
                panic!("masond exited early: {st}");
            }
            self.mason(&["status", "--json"]).status.success()
        });
    }

    pub fn mason(&self, args: &[&str]) -> Output {
        Command::new(bin("mason-pipeline", "mason"))
            .env("MASON_UPDATE_WAIT_S", "8")
            .args(["--data-dir", &self.data_dir.display().to_string()])
            .args(["--config-dir", &self.config_dir.display().to_string()])
            .args(args)
            .output()
            .expect("mason runs")
    }

    pub fn mason_ok(&self, args: &[&str]) -> String {
        let o = self.mason(args);
        assert!(
            o.status.success(),
            "mason {args:?} failed: {}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).to_string()
    }

    pub fn task_json(&self, id: u64) -> Value {
        serde_json::from_str(&self.mason_ok(&["task", "show", &id.to_string(), "--json"])).unwrap()
    }

    pub fn wait_task(&self, id: u64, state: &str, secs: u64) {
        wait_for(&format!("task {id} to be {state}"), secs, || {
            let o = self.mason(&["task", "show", &id.to_string(), "--json"]);
            if !o.status.success() {
                return false; // the daemon may be down or restarting
            }
            let v: Value = serde_json::from_slice(&o.stdout).unwrap_or(Value::Null);
            let now = v["state"].as_str().unwrap_or("");
            if now != state && matches!(now, "failed" | "canceled" | "landed") {
                panic!("task {id} ended {now}, wanted {state}: {v:#}");
            }
            now == state
        });
    }
}

/// With `UPDATE_FIXTURES=1`, save a run's event log as a replay fixture:
/// one `at<TAB>kind<TAB>payload` line per event, in the on-disk text format.
pub fn maybe_record(d: &Arc<Daemon>, name: &str) {
    if std::env::var("UPDATE_FIXTURES").is_err() {
        return;
    }
    let mut out = String::new();
    for e in d.store.read_since(0, 100_000).unwrap() {
        let (kind, payload) = mason_store::encode_event(&e.event.kind);
        out.push_str(&format!("{}\t{kind}\t{payload}\n", e.event.at.0));
    }
    let path = format!(
        "{}/tests/fixtures/{name}.events",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
    std::fs::write(path, out).unwrap();
}
