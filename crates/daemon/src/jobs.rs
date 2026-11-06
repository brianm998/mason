// SPDX-License-Identifier: GPL-3.0-or-later

//! The jobs behind the fixed graph: explore, develop, test, review and land.
//! One job per open step run, resumable: starting it again after a restart
//! finds the work where it was left (worktree, agent session, test host,
//! landing) and carries on.
//!
//! Crash-injection points used here (see `crate::crash`):
//! `job_started:<State>`, `before_event:<State>`, `land_after_fetch`,
//! `land_after_fold`, `land_after_ff`, `land_after_push`, `land_before_event`,
//! `land_after_event`.

use crate::config::ProjectCfg;
use crate::daemon::Daemon;
use mason_adapter_api::contract::{CheckHandler, CheckRun, ContractHandle, Validator};
use mason_adapter_api::{AgentAdapter, SessionSpec, StepKind};
use mason_adapters::claude::ClaudeAdapter;
use mason_adapters::hosted::HostLaunch;
use mason_adapters::runner::{
    hold_note, FailReason, HoldReason, RunnerConfig, StepEnd, StepReport, StepRunner,
};
use mason_core::*;
use mason_landing::direct::{DirectLanding, LandError};
use mason_landing::git::{self, CatchUp};
use mason_step_host::client::{collect_to_file, ClientError, HostClient};
use mason_step_host::launcher::{spawn_host, SpawnOptions};
use mason_step_host::protocol::{CURRENT, PREVIOUS};
use serde_json::Value;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Everything a job needs to know about its task, read once at the start.
pub struct Ctx {
    pub d: Arc<Daemon>,
    pub task: TaskId,
    pub run: StepRunId,
    pub step: State,
    pub attempt: u32,
    pub project: ProjectCfg,
    pub project_id: ProjectId,
    pub title: String,
    pub body: String,
    pub trust: TrustSource,
    pub wt: PathBuf,
    pub branch: String,
}

impl Ctx {
    fn emit(&self, kind: EventKind) -> bool {
        self.d.emit_for_run(self.task, self.run, kind)
    }

    fn fail(&self, reason: impl Into<String>) {
        self.d.crash.point(&format!("before_event:{:?}", self.step));
        self.emit(EventKind::StepFailed {
            task: self.task,
            reason: reason.into(),
        });
    }

    fn succeed(&self) {
        self.d.crash.point(&format!("before_event:{:?}", self.step));
        self.emit(EventKind::StepSucceeded { task: self.task });
    }

    fn task_dir(&self) -> PathBuf {
        let p = self.d.task_dir(self.task);
        let _ = std::fs::create_dir_all(&p);
        p
    }
}

fn ctx(d: &Arc<Daemon>, task: TaskId, step: State, run: StepRunId) -> Option<Ctx> {
    let w = d.world().ok()?;
    let t = w.tasks.get(&task)?;
    let r = w.step_runs.get(&run)?;
    let project = d.project(t.project)?.clone();
    Some(Ctx {
        d: d.clone(),
        task,
        run,
        step,
        attempt: r.attempt,
        wt: project.worktree(&d.cfg, task.0),
        branch: project.branch(task.0),
        project,
        project_id: t.project,
        title: t.title.clone(),
        body: t.body.clone(),
        trust: t.trust,
    })
}

pub fn run(d: &Arc<Daemon>, task: TaskId, step: State, run: StepRunId) {
    let Some(c) = ctx(d, task, step, run) else {
        // The task's project is no longer configured.
        d.emit_for_run(
            task,
            run,
            EventKind::StepFailed {
                task,
                reason: "the task's project is not configured".into(),
            },
        );
        return;
    };
    d.crash.point(&format!("job_started:{step:?}"));
    match step {
        State::Exploring => agent_job(&c, StepKind::Explore),
        State::Developing => agent_job(&c, StepKind::Develop),
        State::Testing => test_job(&c),
        // The fixed graph has no review step yet: it passes straight through.
        State::Review => c.succeed(),
        State::Landing => land_job(&c),
        _ => {}
    }
}

/// Get the worktree ready: fetch, create it on the task's branch, and bring
/// the branch up to date with the base. Idempotent.
fn prepare(c: &Ctx) -> Result<(), String> {
    let lock = c.d.repo_lock(c.project_id);
    let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
    let repo = &c.project.path;
    if let Some(r) = &c.project.remote {
        git::run(repo, &["fetch", "--prune", r]).map_err(|e| e.to_string())?;
    }
    let base = c.project.base_ref();
    git::ensure_worktree(repo, &c.wt, &c.branch, &base).map_err(|e| e.to_string())?;
    match git::catch_up(&c.wt, &base) {
        Ok(CatchUp::Conflict(files)) => Err(format!(
            "folding in the base conflicts in: {}",
            files.trim()
        )),
        Ok(_) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Run `sh -c cmd` in `cwd`, giving up after `timeout`. Returns whether it
/// exited 0 and the last lines of its output.
pub fn run_shell(cmd: &str, cwd: &Path, timeout: Duration) -> (bool, String) {
    let out = tempfile_path();
    let file = match std::fs::File::create(&out) {
        Ok(f) => f,
        Err(e) => return (false, format!("cannot capture output: {e}")),
    };
    let err = file.try_clone();
    let child = Command::new("sh")
        .args(["-c", cmd])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(file)
        .stderr(err.map_or_else(|_| Stdio::null(), Stdio::from))
        .process_group(0)
        .spawn();
    let Ok(mut child) = child else {
        return (false, "could not start the command".into());
    };
    let end = Instant::now() + timeout;
    let ok = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.success(),
            Ok(None) if Instant::now() >= end => {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(child.id() as i32),
                    nix::sys::signal::Signal::SIGKILL,
                );
                let _ = child.wait();
                break false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break false,
        }
    };
    let text = std::fs::read_to_string(&out).unwrap_or_default();
    let _ = std::fs::remove_file(&out);
    (ok, tail(&text, 40))
}

fn tempfile_path() -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "mason-sh-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ))
}

pub fn tail(text: &str, lines: usize) -> String {
    let v: Vec<&str> = text.lines().collect();
    v[v.len().saturating_sub(lines)..].join("\n")
}

/// `run_check` for the develop step: the only check is the project's tests.
struct TestChecks {
    cmd: String,
    wt: PathBuf,
    timeout: Duration,
}

impl CheckHandler for TestChecks {
    fn run(&self, name: &str) -> CheckRun {
        if name != "tests" {
            return CheckRun::Unknown;
        }
        let (passed, out) = run_shell(&self.cmd, &self.wt, self.timeout);
        CheckRun::Done {
            passed,
            summary: format!("Last output:\n{out}"),
        }
    }
}

/// Proof before acceptance: the submitted commit really is the tip of the
/// task's branch, the tree is clean, and the branch has work in it.
fn change_validator(wt: PathBuf, base_ref: String, branch: String) -> Validator {
    Box::new(move |tool, args| {
        if tool != "submit_change" {
            return Ok(());
        }
        let mut problems = vec![];
        let head = args["head"].as_str().unwrap_or_default();
        let tip = git::rev_parse(&wt, "HEAD").unwrap_or_default();
        match git::rev_parse(&wt, head) {
            Err(_) => problems.push(format!("head: {head:?} is not a commit in this repository")),
            Ok(h) if h != tip => problems.push(format!(
                "head: {head} is not the tip of your branch ({branch} is at {}); commit your work and submit the hash from `git rev-parse HEAD`",
                &tip[..tip.len().min(12)]
            )),
            Ok(_) => {}
        }
        let dirty = git::run(&wt, &["status", "--porcelain"]).unwrap_or_default();
        if !dirty.trim().is_empty() {
            problems.push(format!(
                "the tree has uncommitted changes; commit or discard them:\n{}",
                tail(&dirty, 10)
            ));
        }
        match git::commits_ahead(&wt, &base_ref, "HEAD") {
            Ok(0) => problems.push(format!(
                "the branch has no commits beyond {base_ref}; there is nothing to submit"
            )),
            Err(e) => problems.push(format!("cannot compare with {base_ref}: {e}")),
            Ok(_) => {}
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    })
}

fn adapter(c: &Ctx) -> ClaudeAdapter {
    let bin = c
        .project
        .claude_bin
        .clone()
        .unwrap_or_else(|| PathBuf::from("claude"));
    ClaudeAdapter::new(bin, c.d.cfg.mcp_bin.clone(), c.d.cfg.agent_state_dir()).with_host(
        HostLaunch {
            host_bin: c.d.cfg.host_bin.clone(),
            root: c.d.cfg.hosts_dir(),
            heartbeat: c.d.cfg.heartbeat,
            linger: Duration::from_secs(600),
        },
    )
}

fn agent_job(c: &Ctx, kind: StepKind) {
    if let Err(why) = prepare(c) {
        return c.fail(why);
    }
    let key = format!("t{}r{}", c.task.0, c.run.0);
    let dir = c.task_dir();
    let plan: Option<Value> = std::fs::read_to_string(dir.join("plan.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let feedback = std::fs::read_to_string(dir.join("feedback.txt")).ok();
    let prompt = match kind {
        StepKind::Develop => crate::prompts::develop_prompt(
            c.task.0,
            &c.title,
            &c.body,
            c.trust,
            &c.project,
            plan.as_ref(),
            feedback.as_deref(),
        ),
        _ => crate::prompts::explore_prompt(c.task.0, &c.title, &c.body, c.trust, &c.project),
    };

    let checks = Box::new(TestChecks {
        cmd: c.project.test_cmd.clone(),
        wt: c.wt.clone(),
        timeout: c.project.test_timeout,
    });
    let validator = (kind == StepKind::Develop)
        .then(|| change_validator(c.wt.clone(), c.project.base_ref(), c.branch.clone()));
    let contract = ContractHandle::with_validator(kind, checks, validator);
    if let Err(e) = contract.serve(&c.d.cfg.sockets_dir().join(format!("{key}.sock"))) {
        return c.fail(format!("cannot serve the step contract: {e}"));
    }

    let adapter = adapter(c);
    let mut spec = SessionSpec::new(kind, &prompt, &c.wt);
    spec.model = c.project.model.clone();
    spec.env = c.project.agent_env.clone();
    spec.tool_timeout = c.project.tool_timeout;
    spec.deadline = Some(c.project.step_timeout);
    spec.run_key = Some(key.clone());

    let runner = StepRunner::new(&adapter, RunnerConfig::default());
    let mut report = match adapter.reattach(&key, spec.clone()) {
        Ok(Some(h)) => runner.adopt(h, spec.clone(), &contract),
        _ => runner.run(spec.clone(), &contract),
    };

    // A provider hold is a wait, never a failure: show it, wait, resume.
    while let StepEnd::Hold(reason) = &report.end {
        let on = match reason {
            HoldReason::UsageLimit { resets_at } => format!(
                "usage limit (resets {})",
                resets_at.as_deref().unwrap_or("at an unknown time")
            ),
            HoldReason::ProviderDown { .. } => "provider outage".to_string(),
        };
        if !c.emit(EventKind::WaitStarted { task: c.task, on }) {
            return;
        }
        std::thread::sleep(c.d.cfg.hold_retry);
        if !c.emit(EventKind::WaitEnded { task: c.task }) {
            return;
        }
        report = runner.resume(report, hold_note(), spec.clone(), &contract);
    }
    contract.shutdown();
    finish_agent(c, kind, report, &dir);
}

fn finish_agent(c: &Ctx, kind: StepKind, report: StepReport, dir: &Path) {
    c.d.crash.point(&format!("before_event:{:?}", c.step));
    let task = c.task;
    match report.end {
        StepEnd::Accepted { submission, .. } => {
            if kind == StepKind::Explore {
                let _ = std::fs::write(dir.join("plan.json"), submission.to_string());
            } else {
                let _ = std::fs::remove_file(dir.join("feedback.txt"));
            }
            c.emit(EventKind::StepSucceeded { task });
        }
        StepEnd::ExplorationRequested { reason } => {
            c.emit(EventKind::ExplorationRequested { task, reason });
        }
        StepEnd::Asked { question } => {
            let q = question["ask"]
                .as_str()
                .unwrap_or("(no question text)")
                .to_string();
            c.emit(EventKind::QuestionAsked { task, question: q });
        }
        // A long check is not used by the fixed graph; if one appears, say so.
        StepEnd::Suspended { check_id } => {
            c.emit(EventKind::StepFailed {
                task,
                reason: format!("unexpected suspended check {check_id}"),
            });
        }
        StepEnd::Failed(why) => {
            let reason = match why {
                FailReason::NoResult { nudges } => format!(
                    "the agent ended its turn {} times without submitting",
                    nudges + 1
                ),
                FailReason::Timeout => "the step ran out of time".to_string(),
                FailReason::Crash { detail } => format!("the agent failed: {detail}"),
            };
            c.emit(EventKind::StepFailed { task, reason });
        }
        StepEnd::Hold(_) => {} // handled by the caller's loop
    }
}

fn test_job(c: &Ctx) {
    if let Err(why) = prepare_worktree_only(c) {
        return c.fail(why);
    }
    let id = format!("t{}r{}-test", c.task.0, c.run.0);
    let hosts = c.d.cfg.hosts_dir();
    let dir = hosts.join(&id);
    if !dir.join("host.kv").exists() {
        let o = SpawnOptions {
            host_bin: c.d.cfg.host_bin.clone(),
            root: hosts.clone(),
            id: id.clone(),
            cmd: vec!["sh".into(), "-c".into(), c.project.test_cmd.clone()],
            cwd: Some(c.wt.clone()),
            env: vec![],
            clear_env: false,
            stdin: None,
            linger: Duration::from_secs(600),
            heartbeat: c.d.cfg.heartbeat,
        };
        if let Err(e) = spawn_host(&o) {
            return c.fail(format!("cannot start the test run: {e}"));
        }
    }
    // Stop the run if it takes too long.
    let done = Arc::new(AtomicBool::new(false));
    {
        let (done, sock, limit) = (done.clone(), dir.join("sock"), c.project.test_timeout);
        std::thread::spawn(move || {
            let end = Instant::now() + limit;
            while Instant::now() < end {
                if done.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if let Ok(mut h) = HostClient::connect(&sock, PREVIOUS, CURRENT, "daemon") {
                let _ = h.stop(1000);
            }
        });
    }
    let log = c.task_dir().join(format!("test-r{}.log", c.run.0));
    let result = collect_to_file(
        &dir,
        &log,
        &c.task_dir().join(format!("test-r{}.exit", c.run.0)),
    );
    done.store(true, Ordering::SeqCst);
    let rec = match result {
        Ok(r) => r,
        Err(ClientError::HostLost { .. }) => {
            return c.fail("the test run was lost (its host died)")
        }
        Err(e) => return c.fail(format!("cannot collect the test result: {e}")),
    };
    if rec.code == 0 && rec.signal == 0 {
        let _ = std::fs::remove_file(c.task_dir().join("feedback.txt"));
        return c.succeed();
    }
    let output = tail(&std::fs::read_to_string(&log).unwrap_or_default(), 60);
    let how = if rec.signal != 0 {
        format!("killed by signal {}", rec.signal)
    } else {
        format!("exit status {}", rec.code)
    };
    let rounds = c.attempt;
    if rounds >= c.project.max_test_rounds {
        return c.fail(format!("tests still red after {rounds} rounds ({how})"));
    }
    let _ = std::fs::write(
        c.task_dir().join("feedback.txt"),
        format!("tests failed ({how}). Last output:\n{output}"),
    );
    c.d.crash.point("before_event:Testing");
    c.emit(EventKind::SentBack {
        task: c.task,
        reason: format!("tests failed ({how})"),
    });
}

fn prepare_worktree_only(c: &Ctx) -> Result<(), String> {
    let lock = c.d.repo_lock(c.project_id);
    let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
    git::ensure_worktree(&c.project.path, &c.wt, &c.branch, &c.project.base_ref())
        .map_err(|e| e.to_string())
}

fn land_job(c: &Ctx) {
    if let Err(why) = prepare_worktree_only(c) {
        return c.fail(why);
    }
    let landing_wt = c.project.landing_worktree(&c.d.cfg);
    let lock = c.d.repo_lock(c.project_id);
    let result = {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        let l = DirectLanding {
            repo: &c.project.path,
            task_wt: &c.wt,
            task_branch: &c.branch,
            landing_wt: &landing_wt,
            base: &c.project.base,
            remote: c.project.remote.as_deref(),
        };
        let mut verify = || run_shell(&c.project.test_cmd, &c.wt, c.project.test_timeout).0;
        l.land(&mut verify, &|p| c.d.crash.point(p))
    };
    match result {
        Ok(_) => {
            c.d.crash.point("land_before_event");
            c.succeed();
            c.d.crash.point("land_after_event");
            cleanup(&c.d, c.task, true);
        }
        Err(LandError::RedAfterFold) => {
            let _ = std::fs::write(
                c.task_dir().join("feedback.txt"),
                "Tests were red after folding in the latest base branch. Fix the interaction.",
            );
            c.emit(EventKind::SentBack {
                task: c.task,
                reason: "tests are red after folding in the new base".into(),
            });
        }
        Err(LandError::Conflict(files)) => {
            let _ = std::fs::write(
                c.task_dir().join("feedback.txt"),
                format!("The base branch moved and now conflicts with your change in: {files}"),
            );
            c.emit(EventKind::SentBack {
                task: c.task,
                reason: format!("conflict with the new base in: {}", files.trim()),
            });
        }
        Err(e) => c.fail(format!("landing failed: {e}")),
    }
}

/// Remove a task's worktree and, if asked, its branch. Safe to repeat.
pub fn cleanup(d: &Arc<Daemon>, task: TaskId, delete_branch: bool) {
    let Ok(w) = d.world() else { return };
    let Some(t) = w.tasks.get(&task) else { return };
    let Some(p) = d.project(t.project) else {
        return;
    };
    let lock = d.repo_lock(t.project);
    let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
    let wt = p.worktree(&d.cfg, task.0);
    git::remove_worktree(&p.path, &wt);
    if delete_branch {
        let b = p.branch(task.0);
        if git::branch_exists(&p.path, &b) {
            let _ = git::run(&p.path, &["branch", "-D", &b]);
        }
    }
}
