// SPDX-License-Identifier: GPL-3.0-or-later

//! The jobs behind a definition's steps. One job per open step run, resumable:
//! starting it again after a restart finds the work where it was left
//! (worktree, agent session, command host, landing) and carries on.
//!
//! Which step runs, and what follows it, comes from the definition the task is
//! pinned to; where it goes next is decided by `mason_definition::graph::after`,
//! the same function the dry run walks.
//!
//! Crash-injection points used here (see `crate::crash`):
//! `job_started:<State>`, `before_event:<State>`, `accepted_persisted`,
//! `land_after_fetch`, `land_after_fold`, `land_after_ff`, `land_after_push`,
//! `land_before_event`, `land_after_event`.

use crate::config::ProjectCfg;
use crate::config::Provider;
use crate::daemon::{now_ms, Daemon};
use mason_adapter_api::contract::{CheckHandler, CheckRun, ContractHandle, Validator};
use mason_adapter_api::{AgentAdapter, SessionSpec, StepKind};
use mason_adapters::claude::ClaudeAdapter;
use mason_adapters::codex::CodexAdapter;
use mason_adapters::hosted::HostLaunch;
use mason_adapters::runner::{
    hold_note, worth_a_stronger_model, FailReason, HoldReason, RunnerConfig, StepEnd, StepReport,
    StepRunner,
};
use mason_adapters::sandbox::{SandboxKind, SandboxPolicy};
use mason_core::*;
use mason_definition::graph::{after, Outcome, Transition};
use mason_definition::layers::{resolve, Resolved, RuntimeOverride};
use mason_definition::validate::contract_kind;
use mason_definition::{Definition, Step, StepType};
use mason_landing::git::{self, CatchUp};
use mason_step_host::client::{collect_to_file_observed, ClientError, HostClient};
use mason_step_host::launcher::{spawn_host, SpawnOptions};
use mason_step_host::protocol::{CURRENT, PREVIOUS};
use serde_json::Value;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Everything a job needs to know, read once at the start.
pub struct Ctx {
    pub d: Arc<Daemon>,
    pub task: TaskId,
    pub run: StepRunId,
    /// The step's category.
    pub state: State,
    /// How many times this step has run for the task, this run included.
    pub attempt: u32,
    pub project: ProjectCfg,
    pub project_id: ProjectId,
    pub title: String,
    pub body: String,
    pub trust: TrustSource,
    pub wt: PathBuf,
    pub branch: String,
    pub def: Arc<Definition>,
    pub step: Step,
    /// The step's settings after the five layers (Part 5).
    pub res: Resolved,
    /// The step's resources, handed back the moment it reports its result
    /// (not when its thread winds down), so the next waiter starts at once.
    pub lease: std::sync::Mutex<Option<crate::sched::Lease>>,
}

impl Ctx {
    /// Hand the step's resources back and let a waiting task have them.
    pub(crate) fn release_lease(&self) {
        let l = self.lease.lock().unwrap_or_else(|e| e.into_inner()).take();
        if l.is_some() {
            drop(l);
            self.d.admit();
        }
    }

    pub(crate) fn emit(&self, kind: EventKind) -> bool {
        self.d.emit_for_run(self.task, self.run, kind)
    }

    /// End the step and let the graph say what follows.
    pub(crate) fn conclude(&self, outcome: Outcome, reason: String, feedback: Option<String>) {
        self.d
            .crash
            .point(&format!("before_event:{:?}", self.state));
        let rounds = self.res.max_rounds.as_ref().map(|a| a.value);
        match after(&self.def, &self.step, outcome, self.attempt, rounds) {
            Transition::Next { step, state } => {
                self.emit(EventKind::StepSucceeded {
                    task: self.task,
                    next: Some(Next { step, state }),
                });
            }
            Transition::Land => {
                self.emit(EventKind::StepSucceeded {
                    task: self.task,
                    next: Some(Next {
                        step: String::new(),
                        state: State::Landed,
                    }),
                });
                // What was skipped and asked to run later runs now.
                crate::suites::after_landing(&self.d, self.task);
            }
            Transition::SendBack { step } => {
                if let Some(f) = feedback {
                    let _ = std::fs::write(self.task_dir().join("feedback.txt"), f);
                }
                self.emit(EventKind::SentBack {
                    task: self.task,
                    reason,
                    to_step: step,
                });
            }
            Transition::Fail(why) => {
                let reason = match (outcome, reason.is_empty()) {
                    (Outcome::Fail, false) => reason,
                    (_, true) => why,
                    (_, false) => format!("{reason}; {why}"),
                };
                self.emit(EventKind::StepFailed {
                    task: self.task,
                    reason,
                });
            }
        }
        self.release_lease();
    }

    pub(crate) fn green(&self) {
        self.conclude(Outcome::Green, String::new(), None);
    }

    pub(crate) fn fail(&self, reason: impl Into<String>) {
        self.conclude(Outcome::Fail, reason.into(), None);
    }

    pub(crate) fn task_dir(&self) -> PathBuf {
        let p = self.d.task_dir(self.task);
        let _ = std::fs::create_dir_all(&p);
        p
    }
}

fn ctx(d: &Arc<Daemon>, task: TaskId, run: StepRunId) -> Result<Ctx, String> {
    let w = d.world().map_err(|e| e.to_string())?;
    let t = w.tasks.get(&task).ok_or("the task is gone")?;
    let r = w.step_runs.get(&run).ok_or("the step run is gone")?;
    let project = d
        .project(t.project)
        .ok_or("the task's project is not configured")?
        .clone();
    let def = d.pinned_def(&t.def).ok_or_else(|| {
        format!(
            "the definition {} this task is pinned to is not in the store",
            t.def
        )
    })?;
    let step = if r.name.is_empty() {
        def.first_in(r.step)
    } else {
        def.step(&r.name)
    }
    .ok_or_else(|| {
        format!(
            "step `{}` is not in the definition this task is pinned to",
            r.name
        )
    })?
    .clone();
    let now = now_ms();
    let runtime: Vec<RuntimeOverride> = d
        .store
        .overrides(&project.id, now)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|o| RuntimeOverride {
            key: o.key,
            value: o.value,
            owner: o.owner,
            expires_at: o.expires_at,
        })
        .collect();
    let task_overrides = d.store.task_overrides(task.0).map_err(|e| e.to_string())?;
    let res = resolve(
        &def,
        &step,
        &d.overlay(t.project),
        &runtime,
        &task_overrides,
        now,
    )
    .map_err(|p| {
        p.iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    Ok(Ctx {
        d: d.clone(),
        task,
        run,
        state: r.step,
        attempt: r.attempt,
        wt: project.worktree(&d.cfg, task.0),
        branch: project.branch(task.0),
        project,
        project_id: t.project,
        title: t.title.clone(),
        body: t.body.clone(),
        trust: t.trust,
        def,
        step,
        res,
        lease: std::sync::Mutex::new(None),
    })
}

pub fn run(d: &Arc<Daemon>, task: TaskId, step: State, run: StepRunId) {
    let c = match ctx(d, task, run) {
        Ok(c) => c,
        Err(why) => {
            d.emit_for_run(task, run, EventKind::StepFailed { task, reason: why });
            return;
        }
    };
    d.crash.point(&format!("job_started:{step:?}"));
    // A pause stops work at the step boundary: nothing new starts until it
    // lifts or runs out. (A running step finishes.)
    if let Err(()) = wait_while_paused(&c) {
        return;
    }
    // Everything the step needs, acquired at once, in rank order across every
    // project. Time spent waiting is outside every timeout below: they all
    // start after this returns. An approval waits for a person, not for the
    // machine, so it takes nothing.
    if c.step.kind != StepType::Approval {
        match acquire(&c) {
            Ok(l) => *c.lease.lock().unwrap_or_else(|e| e.into_inner()) = Some(l),
            Err(_) => return, // the task was cancelled or retried while it waited
        }
    }
    match c.step.kind {
        StepType::Agent => agent_job(&c),
        StepType::Test if !c.step.suites.is_empty() => crate::suites::run(&c),
        StepType::Command | StepType::Test => command_job(&c),
        StepType::Approval => approval_job(&c),
        StepType::Landing => land_job(&c),
        StepType::Plugin => crate::plugins::plugin_job(&c),
        other => c.fail(format!("step type `{}` cannot run yet", other.name())),
    }
}

/// The facts a script step or plugin gets about its task, as `MASON_*` variables.
pub(crate) fn step_env_pairs(c: &Ctx) -> Vec<(String, String)> {
    vec![
        ("MASON_TASK".into(), c.task.0.to_string()),
        ("MASON_TITLE".into(), c.title.clone()),
        ("MASON_PROJECT".into(), c.project.id.clone()),
        ("MASON_WORKTREE".into(), c.wt.to_string_lossy().into_owned()),
        ("MASON_BRANCH".into(), c.branch.clone()),
        ("MASON_BASE".into(), c.project.base_ref()),
        ("MASON_STEP".into(), c.step.name.clone()),
        ("MASON_ATTEMPT".into(), c.attempt.to_string()),
        // What the step already holds from the broker (its `uses`), so a command
        // that would queue for the same class (`mason run --needs`) can tell it
        // need not, and does not wait on itself.
        (
            "MASON_HELD".into(),
            c.step
                .uses
                .iter()
                .filter(|(_, n)| **n > 0)
                .map(|(k, _)| k.as_str())
                .collect::<Vec<_>>()
                .join(","),
        ),
        (
            "MASON_RESULT_FILE".into(),
            result_file(c).to_string_lossy().into_owned(),
        ),
    ]
}

pub(crate) fn step_env(c: &Ctx) -> Value {
    Value::Object(
        step_env_pairs(c)
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect(),
    )
}

/// Where a script step may write its JSON result: `{"passed": bool, "summary": "..."}`.
fn result_file(c: &Ctx) -> PathBuf {
    c.task_dir()
        .join(format!("result-{}-r{}.json", c.step.name, c.run.0))
}

/// What a script step's result file says. `passed` is optional: without it the exit
/// status decides. An unreadable file is an error, not a pass.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ScriptResult {
    pub passed: Option<bool>,
    pub summary: String,
}

pub(crate) fn read_script_result(text: &str) -> Result<ScriptResult, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("it is not JSON ({e})"))?;
    let o = v.as_object().ok_or("it must be a JSON object")?;
    for k in o.keys() {
        if k != "passed" && k != "summary" && k != "data" {
            return Err(format!("unknown key `{k}`; use passed and summary"));
        }
    }
    let passed = match o.get("passed") {
        None => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Err("`passed` must be true or false".into()),
    };
    let summary = match o.get("summary") {
        None => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(_) => return Err("`summary` must be a string".into()),
    };
    Ok(ScriptResult { passed, summary })
}

/// Hold a step back while the owner has paused its project (Part 10). The
/// wait is shown on the task; `Err` means the task went away meanwhile.
fn wait_while_paused(c: &Ctx) -> Result<(), ()> {
    use crate::attention::{paused, What};
    let what = if c.step.kind == StepType::Landing {
        What::Landing
    } else {
        What::Step
    };
    let mut announced = false;
    while let Some(p) = paused(&c.d, &c.project.id, what) {
        if !announced {
            announced = true;
            let scope = if p.project.is_empty() {
                "everything"
            } else {
                &p.project
            };
            let on = format!("paused by {} ({} for {scope})", p.owner, p.mode);
            if !c.emit(EventKind::WaitStarted { task: c.task, on }) {
                return Err(());
            }
        }
        std::thread::sleep(Duration::from_millis(200));
        let alive =
            c.d.world()
                .map(|w| w.open_run(c.task) == Some(c.run))
                .unwrap_or(true);
        if !alive {
            return Err(());
        }
    }
    if announced {
        c.emit(EventKind::WaitEnded { task: c.task });
    }
    Ok(())
}

/// Wait for this step's resources (Part 7). Says why in the task's wait, once,
/// and ends the wait when they are granted.
fn acquire(c: &Ctx) -> Result<crate::sched::Lease, crate::sched::Cancelled> {
    use crate::sched::needs_of;
    let d = &c.d;
    let account = (c.step.kind == StepType::Agent).then_some(c.project.account.as_str());
    // A definition names what it uses; a name nobody declared gets its
    // definition-given limit (or one slot).
    let mut needs = needs_of(&c.step.uses, account);
    if c.step.kind == StepType::Test && !c.step.suites.is_empty() {
        // The step holds what its suites (and their environments) use.
        let more = crate::suites::needs_for(&c.project, &c.def, &c.step, &c.branch);
        needs.exclusive |= more.exclusive;
        for (k, v) in more.amounts {
            let have = needs.amounts.entry(k).or_insert(0);
            *have = (*have).max(v);
        }
    }
    for name in needs.amounts.keys() {
        if !name.starts_with("llm:") {
            d.broker
                .declare(name, c.def.resources.get(name).copied().unwrap_or(1));
        }
    }
    let rank = match d.world().ok().and_then(|w| w.tasks.get(&c.task).cloned()) {
        Some(t) => d.rank_for(&t, &c.def, &c.step.name),
        None => return Err(crate::sched::Cancelled),
    };
    let (d2, task, run) = (d.clone(), c.task, c.run);
    let alive = move || {
        d2.world()
            .map(|w| w.open_run(task) == Some(run))
            .unwrap_or(true)
    };
    let mut waited = false;
    let lease = d.broker.acquire(
        c.task,
        &format!("#{}", c.task.0),
        rank,
        needs,
        &alive,
        &mut |why| {
            waited = c.emit(EventKind::WaitStarted {
                task: c.task,
                on: why.to_string(),
            });
        },
    )?;
    if waited {
        c.emit(EventKind::WaitEnded { task: c.task });
    }
    Ok(lease)
}

/// The command of the definition's test step for this project, layered exactly as
/// a test step resolves it: the definition, the project's local overlay, the
/// owner's runtime overrides (`mason config set`) and, for a task, its own
/// overrides. `run_check` and landing's legacy suite use this, so they run the
/// same command the test step runs.
pub(crate) fn test_command(
    d: &Daemon,
    def: &mason_definition::Definition,
    pid: ProjectId,
    task: Option<TaskId>,
) -> Option<String> {
    let step = def.steps.iter().find(|s| s.kind == StepType::Test)?;
    let project = d.project(pid)?;
    let now = now_ms();
    let runtime: Vec<RuntimeOverride> = d
        .store
        .overrides(&project.id, now)
        .unwrap_or_default()
        .into_iter()
        .map(|o| RuntimeOverride {
            key: o.key,
            value: o.value,
            owner: o.owner,
            expires_at: o.expires_at,
        })
        .collect();
    let task_overrides = task
        .and_then(|t| d.store.task_overrides(t.0).ok())
        .unwrap_or_default();
    resolve(def, step, &d.overlay(pid), &runtime, &task_overrides, now)
        .ok()
        .and_then(|r| r.command.map(|a| a.value))
}

/// Get the worktree ready: fetch, create it on the task's branch, and bring
/// the branch up to date with the base. Idempotent.
///
/// A conflict with the base is not the task's failure: the merge is aborted,
/// the branch is left as it was, and the conflicting files are returned so
/// the step can hand them to the agent (develop resolves them; the read-only
/// steps go on with the branch as it is). Failing here instead would fail
/// every later step the same way, so no agent could ever resolve it.
pub(crate) fn prepare(c: &Ctx) -> Result<Option<String>, String> {
    let lock = c.d.repo_lock(c.project_id);
    let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
    let repo = &c.project.path;
    if let Some(r) = &c.project.remote {
        git::run(repo, &["fetch", "--prune", r]).map_err(|e| e.to_string())?;
    }
    let base = c.project.base_ref();
    git::ensure_worktree(repo, &c.wt, &c.branch, &base).map_err(|e| e.to_string())?;
    // Uncommitted changes are an agent's work in progress: a step re-entered after
    // the daemon restarted (a self-update, say) finds its agent's edits not yet
    // committed. Merging the base into them fails ("your local changes would be
    // overwritten") and failed the task; leave the branch as it is, and the next
    // step folds the base in once the work is committed.
    let dirty = git::run(&c.wt, &["status", "--porcelain"]).map_err(|e| e.to_string())?;
    if !dirty.trim().is_empty() {
        tracing::info!(
            task = c.task.0,
            "worktree has uncommitted changes; not folding in the base now"
        );
        return Ok(None);
    }
    match git::catch_up(&c.wt, &base) {
        Ok(CatchUp::Conflict(files)) => Ok(Some(files.trim().to_string())),
        Ok(_) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// How long one tool call of an agent step may take. A develop step's
/// `run_check` runs the whole test command, which is only cut off at
/// `test_timeout`; a shorter tool timeout made the agent give up on a run that
/// was still going (and then run the tests again by hand, doubling the load).
pub(crate) fn tool_timeout_for(kind: StepKind, project: &ProjectCfg) -> Duration {
    match kind {
        StepKind::Develop => project
            .tool_timeout
            .max(project.test_timeout + Duration::from_secs(60)),
        _ => project.tool_timeout,
    }
}

/// What the develop agent is told when its branch does not merge cleanly
/// with the base.
pub(crate) fn conflict_note(base: &str, files: &str) -> String {
    let list: Vec<String> = files
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| format!("- {l}"))
        .collect();
    format!(
        "Your branch no longer merges cleanly with `{base}`. Before anything else, run \
         `git merge {base}` in your worktree, resolve the conflicts so that both your \
         change and the base's change are kept (read what the base changed with \
         `git log -p HEAD..{base} -- <file>`), build, and commit the merge with \
         `git commit -s`. The conflicting files are:\n{}",
        list.join("\n")
    )
}

pub(crate) fn prepare_worktree_only(c: &Ctx) -> Result<(), String> {
    let lock = c.d.repo_lock(c.project_id);
    let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
    git::ensure_worktree(&c.project.path, &c.wt, &c.branch, &c.project.base_ref())
        .map_err(|e| e.to_string())
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

/// The project's optional classifier: the task body on its stdin, and `clear`
/// on stdout means skip exploring. Anything else, or any failure, means explore.
/// Run `cmd` with `input` on stdin; its stdout if it succeeds within 30 s.
pub fn run_filter(cmd: &str, input: &str) -> Option<String> {
    use std::io::{Read, Write};
    let mut child = Command::new("sh")
        .args(["-c", cmd])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut sin) = child.stdin.take() {
        let _ = sin.write_all(input.as_bytes());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    if let Some(mut so) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = so.read_to_string(&mut s);
            let _ = tx.send(s);
        });
    }
    let out = match rx.recv_timeout(Duration::from_secs(30)) {
        Ok(s) => s,
        Err(_) => {
            let _ = child.kill();
            return None;
        }
    };
    child.wait().ok().filter(|s| s.success()).map(|_| out)
}

pub fn classify(cmd: &str, body: &str) -> bool {
    use std::io::{Read, Write};
    let Ok(mut child) = Command::new("sh")
        .args(["-c", cmd])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut sin) = child.stdin.take() {
        let _ = sin.write_all(body.as_bytes());
    }
    let mut out = String::new();
    let (tx, rx) = std::sync::mpsc::channel();
    if let Some(mut so) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = so.read_to_string(&mut s);
            let _ = tx.send(s);
        });
    }
    match rx.recv_timeout(Duration::from_secs(30)) {
        Ok(s) => out = s,
        Err(_) => {
            let _ = child.kill();
        }
    }
    let ok = child.wait().map(|s| s.success()).unwrap_or(false);
    ok && out.trim().eq_ignore_ascii_case("clear")
}

/// `run_check` for the develop step: the only check is the project's tests.
struct TestChecks {
    cmd: Option<String>,
    wt: PathBuf,
    timeout: Duration,
}

impl CheckHandler for TestChecks {
    fn run(&self, name: &str) -> CheckRun {
        let (true, Some(cmd)) = (name == "tests", &self.cmd) else {
            return CheckRun::Unknown;
        };
        let (passed, out) = run_shell(cmd, &self.wt, self.timeout);
        CheckRun::Done {
            passed,
            summary: format!("Last output:\n{out}"),
        }
    }
}

/// Proof before acceptance: the submitted commit really is the tip of the
/// task's branch, the tree is clean, the branch has work in it, and if the
/// change edits the pipeline's own definition, the new one is valid. (The
/// definition that judges *this* task is the pinned one, never the edited one:
/// a change cannot relax the gates it is judged by.)
fn change_validator(
    wt: PathBuf,
    base_ref: String,
    branch: String,
    caps: mason_definition::Capabilities,
) -> Validator {
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
        let changed = git::run(&wt, &["diff", "--name-only", &format!("{base_ref}...HEAD")])
            .unwrap_or_default();
        if changed.lines().any(|f| f == ".mason/mason.toml") {
            if let Ok(text) = git::run(&wt, &["show", "HEAD:.mason/mason.toml"]) {
                let bad = match mason_definition::parse_with(
                    &text,
                    mason_definition::Plugins::Named(&caps.plugin_names()),
                ) {
                    Err(e) => e,
                    Ok(def) => mason_definition::validate(&def, &caps),
                };
                for p in bad {
                    problems.push(format!(
                        "your change edits .mason/mason.toml, and the new definition is invalid ({p}); it takes effect only after landing, so it must be valid first"
                    ));
                }
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    })
}

/// The agent CLI for this project's provider, under the step host and, if the
/// project asks for one, an OS sandbox.
fn adapter(c: &Ctx) -> Box<dyn AgentAdapter> {
    let agent = c.project.agent();
    let host = HostLaunch {
        host_bin: c.d.cfg.host_bin.clone(),
        root: c.d.cfg.hosts_dir(),
        heartbeat: c.d.cfg.heartbeat,
        linger: Duration::from_secs(600),
    };
    let sandbox = (agent.sandbox != SandboxKind::None).then(|| {
        let mut p = SandboxPolicy::new(agent.sandbox, &c.wt, &c.d.cfg.agent_state_dir());
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            p.extra = [".claude", ".claude.json", ".codex"]
                .iter()
                .map(|f| home.join(f))
                .collect();
        }
        // A worktree's commits are written into the main repository's `.git`.
        p.extra.push(c.project.path.join(".git"));
        p
    });
    match agent.provider {
        Provider::Claude => {
            let bin = c
                .project
                .claude_bin
                .clone()
                .unwrap_or_else(|| PathBuf::from("claude"));
            let mut a = ClaudeAdapter::new(bin, c.d.cfg.mcp_bin.clone(), c.d.cfg.agent_state_dir())
                .with_host(host);
            a.sandbox = sandbox;
            Box::new(a)
        }
        Provider::Codex => {
            let bin = agent.codex_bin.unwrap_or_else(|| PathBuf::from("codex"));
            let mut a = CodexAdapter::new(bin, c.d.cfg.mcp_bin.clone(), c.d.cfg.agent_state_dir())
                .with_host(host);
            a.sandbox = sandbox;
            Box::new(a)
        }
    }
}

fn agent_job(c: &Ctx) {
    let Some(kind) = contract_kind(c.state) else {
        return c.fail(format!(
            "an agent step cannot be in the {:?} category",
            c.state
        ));
    };
    let conflicts = match prepare(c) {
        Ok(conflicts) => conflicts,
        Err(why) => return c.fail(why),
    };
    let key = format!("t{}r{}", c.task.0, c.run.0);
    let dir = c.task_dir();
    let plan: Option<Value> = std::fs::read_to_string(dir.join("plan.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let mut feedback = std::fs::read_to_string(dir.join("feedback.txt")).ok();
    if let Some(files) = &conflicts {
        if kind == StepKind::Develop {
            let note = conflict_note(&c.project.base_ref(), files);
            feedback = Some(match feedback {
                Some(f) if !f.trim().is_empty() => format!("{note}\n\n{f}"),
                _ => note,
            });
        }
    }
    let decisions = std::fs::read_to_string(dir.join("decisions.txt")).ok();
    let pack_name = match kind {
        StepKind::Develop => "develop",
        StepKind::Review => "review",
        StepKind::Explore => "explore",
    };
    let prompt = if let Some(pack) = crate::prompts::load_pack(&c.project, pack_name) {
        crate::prompts::from_pack(
            &pack,
            &crate::prompts::PackVars {
                number: c.task.0,
                title: &c.title,
                body: &c.body,
                trust: c.trust,
                project: &c.project,
                plan: plan.as_ref(),
                feedback: feedback.as_deref(),
            },
        )
    } else {
        match kind {
            StepKind::Develop => crate::prompts::develop_prompt(
                c.task.0,
                &c.title,
                &c.body,
                c.trust,
                &c.project,
                plan.as_ref(),
                feedback.as_deref(),
            ),
            StepKind::Review => crate::prompts::review_prompt(
                c.task.0,
                &c.title,
                &c.body,
                c.trust,
                &c.project,
                plan.as_ref(),
            ),
            StepKind::Explore => {
                crate::prompts::explore_prompt(c.task.0, &c.title, &c.body, c.trust, &c.project)
            }
        }
    };

    // The numbers MASON reserved for this task (a task claimed before numbers existed gets
    // them now), stated as facts in every explore and develop prompt.
    let prompt = if kind == StepKind::Review {
        prompt
    } else {
        match c.d.ensure_numbers(c.task) {
            Ok(m) => format!("{prompt}\n\n{}", crate::numbers::prompt_block(&m)),
            Err(why) => return c.fail(format!("no numbers could be reserved: {why}")),
        }
    };

    let prompt = match decisions.filter(|t| !t.trim().is_empty()) {
        Some(d) => format!(
            "{prompt}\n\nThe owner has already answered these questions of yours; do not ask them again:\n{}",
            d.trim_end()
        ),
        None => prompt,
    };

    // `run_check` runs the project's tests: the command of its test step, layered exactly
    // as the test step itself resolves it (runtime and task overrides included).
    let tests_cmd = test_command(&c.d, &c.def, c.project_id, Some(c.task));
    let checks = Box::new(TestChecks {
        cmd: tests_cmd,
        wt: c.wt.clone(),
        timeout: c.project.test_timeout,
    });
    let validator = (kind == StepKind::Develop).then(|| {
        change_validator(
            c.wt.clone(),
            c.project.base_ref(),
            c.branch.clone(),
            c.d.caps_for(&c.project),
        )
    });
    let contract = ContractHandle::with_validator(kind, checks, validator);
    if let Some(tools) = &c.step.tools {
        contract.restrict_tools(tools.clone());
    }
    // A decision card that does not fit gets a cheap rewrite if the project
    // has one; otherwise (or if that fails) the contract rebuilds it.
    if let Some(cmd) = c.project.card_rewriter.clone() {
        contract.set_ask_rewriter(Box::new(move |card, problems| {
            let input = serde_json::json!({"card": card, "problems": problems}).to_string();
            let out = run_filter(&cmd, &input)?;
            serde_json::from_str(out.trim()).ok()
        }));
    }
    // An accepted submission is made durable before the agent is told, so a
    // daemon that dies before recording the step's result still knows it.
    let accepted_file = dir.join(format!("accepted-r{}.json", c.run.0));
    if let Some(v) = std::fs::read_to_string(&accepted_file)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    {
        contract.restore_accepted(v["tool"].as_str().unwrap_or_default(), v["args"].clone());
    }
    {
        let (path, d) = (accepted_file.clone(), c.d.clone());
        contract.set_on_accept(Box::new(move |tool, args| {
            let tmp = path.with_extension("tmp");
            let body = serde_json::json!({"tool": tool, "args": args}).to_string();
            if std::fs::write(&tmp, body).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
            d.crash.point("accepted_persisted");
        }));
    }
    if let Err(e) = contract.serve(&c.d.cfg.sockets_dir().join(format!("{key}.sock"))) {
        return c.fail(format!("cannot serve the step contract: {e}"));
    }

    let adapter = adapter(c);
    let mut spec = SessionSpec::new(kind, &prompt, &c.wt);
    let agent = c.project.agent();
    // Both providers go through the mapping: the requested tier and level
    // become the real model and effort, unless something names them outright.
    let eff =
        crate::tiermap::effective(&c.d.cfg.config_dir, agent.provider, &c.project.overlay_text);
    for p in &eff.problems {
        tracing::warn!(project = %c.project.id, "{p}");
    }
    let launched = mason_definition::tiers::launch(&c.res, &eff.mapping, &agent.ladder);
    let rec = LaunchRec {
        launched: &launched,
        provider: crate::tiermap::name(agent.provider),
        max: agent.max_retries,
        tries: std::cell::Cell::new(0),
        moved_from: std::cell::RefCell::new(String::new()),
    };
    spec.model = launched.model.clone();
    spec.effort = launched.effort.clone();
    {
        let (d, run) = (c.d.clone(), c.run.0);
        spec.on_model = Some(Arc::new(move |m| {
            let _ = d.store.set_reported_model(run, m);
        }));
    }
    spec.env = c.project.agent_env.clone();
    spec.tool_timeout = tool_timeout_for(kind, &c.project);
    spec.deadline = Some(
        c.res
            .budget
            .as_ref()
            .map_or(c.project.step_timeout, |a| a.value),
    );
    spec.run_key = Some(key.clone());
    spec.tools = c.step.tools.clone();
    {
        let (d, run) = (c.d.clone(), c.run);
        spec.progress = Some(Arc::new(move |stream, off, bytes| {
            d.progress_observe(run, stream, off, bytes)
        }));
    }
    // Wired before any reattach, so a session found again sees what was already accepted.
    spec.accepted = contract.flag();
    spec.contract = Some(contract.clone());

    let runner = StepRunner::new(adapter.as_ref(), RunnerConfig::default());
    let first = match adapter.reattach(&key, spec.clone()) {
        Ok(Some(h)) => {
            // The session found again keeps the model and effort it was launched
            // with, whatever the provider file says now; one reattach row is
            // written only when this run has no record at all.
            let before = c.d.store.launches(c.run.0).unwrap_or_default();
            match before.iter().rev().find(|r| r.kind != "retry") {
                Some(r) => {
                    spec.model = Some(r.model.clone()).filter(|m| !m.is_empty());
                    spec.effort = Some(r.effort.clone()).filter(|m| !m.is_empty());
                }
                None => rec.write(c, "reattach", &spec),
            }
            runner.adopt(h, spec.clone(), &contract)
        }
        _ => {
            rec.write(c, "launch", &spec);
            runner.run(spec.clone(), &contract)
        }
    };
    let Some(mut report) = settle_holds(c, &runner, first, &spec, &contract, &rec) else {
        return;
    };
    record_usage(c, &spec, &report);

    // The retry ladder: a step that ended with no result, or whose agent
    // crashed, is tried again from a fresh session on the next model up. Not
    // after a timeout (more model would not be more time), a hold (a wait) or a
    // question (the owner's turn). An empty ladder never retries.
    let mut tries = 0;
    while tries < agent.max_retries && worth_a_stronger_model(&report.end) {
        let Some(next) = mason_definition::builtin::next_up(&agent.ladder, spec.model.as_deref())
        else {
            break;
        };
        tries += 1;
        *rec.moved_from.borrow_mut() = spec.model.clone().unwrap_or_default();
        spec.model = Some(next.to_string());
        spec.run_key = Some(format!("{key}u{tries}"));
        rec.tries.set(tries);
        rec.write(c, "retry", &spec);
        let again = runner.run(spec.clone(), &contract);
        let Some(r) = settle_holds(c, &runner, again, &spec, &contract, &rec) else {
            return;
        };
        record_usage(c, &spec, &r);
        report = r;
    }
    contract.shutdown();
    finish_agent(c, report, &dir);
}

/// What the daemon writes down each time an agent step starts a session.
struct LaunchRec<'a> {
    launched: &'a mason_definition::tiers::Launch,
    provider: &'static str,
    max: u32,
    tries: std::cell::Cell<u32>,
    moved_from: std::cell::RefCell<String>,
}

impl LaunchRec<'_> {
    /// Append a launch row: what was asked for, and what `spec` really passes to the CLI.
    fn write(&self, c: &Ctx, kind: &str, spec: &SessionSpec) {
        let l = self.launched;
        let retried = self.tries.get() > 0;
        let row = mason_store::LaunchRow {
            run: c.run.0,
            task: c.task.0,
            step: c.step.name.clone(),
            kind: kind.into(),
            tier: l.tier.map(|t| t.name().to_string()).unwrap_or_default(),
            level: l.level.name().to_string(),
            req_model: l.mapped.model.clone().unwrap_or_default(),
            req_effort: l.mapped.effort.clone().unwrap_or_default(),
            model: spec.model.clone().unwrap_or_default(),
            effort: spec.effort.clone().unwrap_or_default(),
            provider: self.provider.into(),
            model_from: if retried {
                "the retry ladder".into()
            } else {
                l.model_from.clone()
            },
            effort_from: l.effort_from.clone(),
            retry_n: self.tries.get(),
            retry_max: self.max,
            moved_up_from: self.moved_from.borrow().clone(),
            overridden: l.overridden,
            at_ms: now_ms(),
            ..Default::default()
        };
        if let Err(e) = c.d.store.record_launch(row) {
            tracing::warn!(task = c.task.0, "cannot record the launch: {e}");
        }
    }
}

/// Keep what the attempt reported: the provider's limits as the latest for the
/// account, and the tokens and cost against project, task, step and model.
fn record_usage(c: &Ctx, spec: &SessionSpec, report: &StepReport) {
    if let Some(u) = &report.usage {
        crate::usage::record_snapshot(&c.d, &c.project.account, u);
    }
    if let Some(key) = &spec.run_key {
        let who = crate::usage::Attempt {
            key,
            task: c.task.0,
            project: &c.project.id,
            step: &c.step.name,
            account: &c.project.account,
            model: spec.model.as_deref(),
        };
        crate::usage::record_tokens(&c.d, &who, &report.tokens);
    }
}

/// A provider hold is a wait, never a failure: show it, wait, resume. `None`
/// when the task went away while waiting.
fn settle_holds(
    c: &Ctx,
    runner: &StepRunner,
    mut report: StepReport,
    spec: &SessionSpec,
    contract: &ContractHandle,
    rec: &LaunchRec,
) -> Option<StepReport> {
    while let StepEnd::Hold(reason) = &report.end {
        let on = match reason {
            HoldReason::UsageLimit { resets_at } => format!(
                "usage limit (resets {})",
                resets_at.as_deref().unwrap_or("at an unknown time")
            ),
            HoldReason::ProviderDown { .. } => "provider outage".to_string(),
        };
        if let Some(u) = &report.usage {
            crate::usage::record_snapshot(&c.d, &c.project.account, u);
        }
        if !c.emit(EventKind::WaitStarted { task: c.task, on }) {
            return None;
        }
        std::thread::sleep(c.d.cfg.hold_retry);
        if !c.emit(EventKind::WaitEnded { task: c.task }) {
            return None;
        }
        rec.write(c, "resume", spec);
        report = runner.resume(report, hold_note(), spec.clone(), contract);
    }
    Some(report)
}

fn finish_agent(c: &Ctx, report: StepReport, dir: &Path) {
    finish_agent_inner(c, report, dir);
    // Asked, or the agent gave up: either way the session is free now.
    c.release_lease();
}

fn finish_agent_inner(c: &Ctx, report: StepReport, dir: &Path) {
    let task = c.task;
    match report.end {
        StepEnd::Accepted { submission, .. } => {
            match c.state {
                State::Exploring => {
                    let _ = std::fs::write(dir.join("plan.json"), submission.to_string());
                }
                State::Developing => {
                    let _ = std::fs::remove_file(dir.join("feedback.txt"));
                }
                _ => {}
            }
            // A reviewer who asks for changes is red, not an error.
            if c.state == State::Review && submission["verdict"] == "changes" {
                let findings: Vec<String> = submission["findings"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|f| format!("- {}{}", f["issue"].as_str().unwrap_or(""), f["file"].as_str().map(|p| format!(" ({p})")).unwrap_or_default()))
                    .collect();
                let feedback = format!("The reviewer asked for changes: {}\n{}", submission["summary"].as_str().unwrap_or(""), findings.join("\n"));
                return c.conclude(Outcome::Red, "the reviewer asked for changes".into(), Some(feedback));
            }
            c.green();
        }
        StepEnd::ExplorationRequested { reason } => match c.def.first_in(State::Exploring) {
            Some(first) => {
                c.d.crash.point(&format!("before_event:{:?}", c.state));
                c.emit(EventKind::ExplorationRequested { task, reason, to_step: first.name.clone() });
            }
            None => c.fail(format!("the agent asked to go back to exploring ({reason}), but this definition has no exploring step")),
        },
        StepEnd::Asked { question } => {
            // `question` is already a decision card that fits (the contract
            // rewrote it if it did not). Asked again about something the owner
            // already answered, it is acknowledged, not asked: no loops.
            let card = mason_adapter_api::card::Card::from_json(&question)
                .unwrap_or_else(|| mason_adapter_api::card::fallback(&question, &[]));
            c.d.crash.point(&format!("before_event:{:?}", c.state));
            let earlier = crate::attention::earlier_answer(&c.d, task, &card.key);
            if !c.emit(EventKind::QuestionAsked { task, question: card.to_text() }) {
                return;
            }
            if let Some(prev) = earlier {
                let w = c.d.world().ok();
                let decision = w.as_ref().and_then(|w| w.tasks.get(&task)).and_then(|t| t.decision);
                if let Some(decision) = decision {
                    let answer = format!("(acknowledged, not asked again) {prev}");
                    let step = c.def.first_in(State::Developing).map(|s| s.name.clone()).unwrap_or_default();
                    crate::attention::remember(&c.d, task, &card.ask, &answer);
                    let _ = c.d.emit(EventKind::Answered { task, decision, answer, step });
                    crate::attention::intervene(&c.d, "acknowledged", Some(task), "pipeline", &card.key, "the same question was already answered");
                }
            }
        }
        // A long check is not used yet; if one appears, say so.
        StepEnd::Suspended { check_id } => c.fail(format!("unexpected suspended check {check_id}")),
        StepEnd::Failed(why) => {
            let reason = match why {
                FailReason::NoResult { nudges } => format!("the agent ended its turn {} times without submitting", nudges + 1),
                FailReason::Timeout => "the step ran out of time".to_string(),
                FailReason::Crash { detail } => format!("the agent failed: {detail}"),
            };
            c.fail(reason);
        }
        StepEnd::Hold(_) => {} // handled by the caller's loop
    }
}

/// A command or test step: run `command` in the worktree under a step host, so
/// a daemon restart does not lose the run. Exit 0 is green.
///
/// A test step first brings the branch up to date with the base, so it tests
/// what would land and picks up fixes that reached the base while it waited
/// (a conflict sends it back to develop, where the agent is told to merge).
/// A red test run is run again up to `test_reruns` times: green on a rerun is
/// green, recorded as a flake; red every time is red.
fn command_job(c: &Ctx) {
    let Some(cmd) = c.res.command.as_ref().map(|a| a.value.clone()) else {
        return c.fail("this step has no command: set `command` on the step, or `test` in the project's local overlay");
    };
    let is_test = c.step.kind == StepType::Test;
    if is_test {
        match prepare(c) {
            Ok(None) => {}
            Ok(Some(files)) => {
                let note = conflict_note(&c.project.base_ref(), &files);
                return c.conclude(
                    Outcome::Red,
                    format!(
                        "the branch conflicts with {} in: {}",
                        c.project.base_ref(),
                        files.replace('\n', ", ")
                    ),
                    Some(note),
                );
            }
            Err(why) => return c.fail(why),
        }
    } else if let Err(why) = prepare_worktree_only(c) {
        return c.fail(why);
    }
    let suffix = if is_test { "test" } else { "cmd" };
    let reruns = if is_test { c.project.test_reruns } else { 0 };
    let mut earlier: Vec<String> = vec![];
    for k in 0..=reruns {
        let run = match run_command_once(c, &cmd, suffix, k) {
            Ok(r) => r,
            Err(why) => return c.fail(why),
        };
        match run.verdict {
            RunVerdict::Green => {
                let _ = std::fs::remove_file(c.task_dir().join("feedback.txt"));
                if k > 0 {
                    let note = format!(
                        "`{}` was red, then green when run again (a flaky test, not this change):\n{}",
                        c.step.name,
                        earlier.join("\n")
                    );
                    tracing::warn!(task = c.task.0, run = c.run.0, "{note}");
                    let _ = std::fs::write(
                        c.task_dir().join(format!("{suffix}-r{}.flaky", c.run.0)),
                        &note,
                    );
                }
                return c.green();
            }
            // Red by its exit status alone: worth running again.
            RunVerdict::Red {
                reason,
                feedback,
                rerun: true,
            } if k < reruns => {
                earlier.push(format!("run {}: {reason}\n{feedback}", k + 1));
                use std::io::Write as _;
                if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&run.log) {
                    let _ = writeln!(
                        f,
                        "\n== MASON: red ({reason}); running it again ({} of {reruns}) to tell a flaky test from a broken change",
                        k + 1
                    );
                }
                continue;
            }
            RunVerdict::Red {
                reason, feedback, ..
            } => {
                let (reason, feedback) = if k > 0 {
                    (
                        format!("{reason} ({} runs, red every time)", k + 1),
                        format!("{feedback}\n\nIt was red on all {} runs.", k + 1),
                    )
                } else {
                    (reason, feedback)
                };
                return c.conclude(Outcome::Red, reason, Some(feedback));
            }
        }
    }
}

enum RunVerdict {
    Green,
    /// `rerun`: red only by its exit status (not a timeout, a signal or a
    /// script's own verdict), so running it again may tell a flake apart.
    Red {
        reason: String,
        feedback: String,
        rerun: bool,
    },
}

struct CommandRun {
    verdict: RunVerdict,
    log: PathBuf,
}

/// The name part of the `k`th run of a step's command: "" for the first.
fn again(k: u32) -> String {
    if k == 0 {
        String::new()
    } else {
        format!("-again{k}")
    }
}

/// Run the command once (the `k`th time) under its own step host, or collect
/// the run a restarted daemon finds already started.
fn run_command_once(c: &Ctx, cmd: &str, suffix: &str, k: u32) -> Result<CommandRun, String> {
    let id = format!("t{}r{}-{suffix}{}", c.task.0, c.run.0, again(k));
    let hosts = c.d.cfg.hosts_dir();
    let dir = hosts.join(&id);
    let limit = c
        .res
        .budget
        .as_ref()
        .map_or(c.project.test_timeout, |a| a.value);
    if !dir.join("host.kv").exists() {
        let _ = std::fs::remove_file(result_file(c));
        let o = SpawnOptions {
            host_bin: c.d.cfg.host_bin.clone(),
            root: hosts.clone(),
            id: id.clone(),
            cmd: vec!["sh".into(), "-c".into(), cmd.to_string()],
            cwd: Some(c.wt.clone()),
            env: step_env_pairs(c),
            clear_env: false,
            stdin: None,
            linger: Duration::from_secs(600),
            heartbeat: c.d.cfg.heartbeat,
        };
        if let Err(e) = spawn_host(&o) {
            return Err(format!("cannot start the command: {e}"));
        }
    }
    // Stop the run if it takes too long.
    let done = Arc::new(AtomicBool::new(false));
    {
        let (done, sock) = (done.clone(), dir.join("sock"));
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
    let name = format!("{suffix}-r{}{}", c.run.0, again(k));
    let log = c.task_dir().join(format!("{name}.log"));
    // Lines and bytes of the output, counted as they arrive. A run found again after a
    // restart first counts what its log holds beyond what the stored record had.
    // Each run of the command is its own stream, since each log starts at 0.
    let stream = again(k);
    c.d.progress_catch_up(c.run, &stream, &log);
    let result = collect_to_file_observed(
        &dir,
        &log,
        &c.task_dir().join(format!("{name}.exit")),
        &mut |off, b| c.d.progress_observe(c.run, &stream, off, b),
    );
    done.store(true, Ordering::SeqCst);
    let rec = match result {
        Ok(r) => r,
        Err(ClientError::HostLost { .. }) => {
            return Err("the command was lost (its host died)".into())
        }
        Err(e) => return Err(format!("cannot collect the command's result: {e}")),
    };
    // A script may say more than its exit status in a JSON result file.
    let script = match std::fs::read_to_string(result_file(c)) {
        Ok(t) => match read_script_result(&t) {
            Ok(r) => Some(r),
            Err(e) => {
                return Err(format!(
                    "`{}` wrote a result file that cannot be used: {e}",
                    c.step.name
                ))
            }
        },
        Err(_) => None,
    };
    let exited_ok = rec.code == 0 && rec.signal == 0;
    if let Some(r) = &script {
        if !r.summary.is_empty() {
            let _ = std::fs::write(c.task_dir().join(format!("{name}.result")), &r.summary);
        }
        if r.passed == Some(false) {
            let why = if r.summary.is_empty() {
                "reported a failure".to_string()
            } else {
                r.summary.clone()
            };
            return Ok(CommandRun {
                verdict: RunVerdict::Red {
                    reason: format!("`{}` failed: {why}", c.step.name),
                    feedback: format!("`{}` failed: {why}", c.step.name),
                    rerun: false,
                },
                log,
            });
        }
    }
    let said_pass = script.as_ref().is_some_and(|r| r.passed == Some(true));
    if (exited_ok || said_pass) && rec.signal == 0 {
        return Ok(CommandRun {
            verdict: RunVerdict::Green,
            log,
        });
    }
    let output = tail(&std::fs::read_to_string(&log).unwrap_or_default(), 60);
    let how = if rec.signal != 0 {
        format!("killed by signal {}", rec.signal)
    } else {
        format!("exit status {}", rec.code)
    };
    Ok(CommandRun {
        verdict: RunVerdict::Red {
            reason: format!("`{}` failed ({how})", c.step.name),
            feedback: format!("`{}` failed ({how}). Last output:\n{output}", c.step.name),
            rerun: rec.signal == 0,
        },
        log,
    })
}

/// An approval step waits for the owner. The run stays open, holding no slot,
/// until `mason task approve` or `reject` ends it.
fn approval_job(c: &Ctx) {
    let waiting = Activity::Waiting(WaitReason::On(APPROVAL_WAIT.into()));
    let already =
        c.d.world()
            .ok()
            .is_some_and(|w| w.tasks.get(&c.task).is_some_and(|t| t.activity == waiting));
    if !already
        && !c.emit(EventKind::WaitStarted {
            task: c.task,
            on: APPROVAL_WAIT.into(),
        })
    {
        return;
    }
    loop {
        std::thread::sleep(Duration::from_millis(250));
        match c.d.world() {
            Ok(w) if w.open_run(c.task) == Some(c.run) => {}
            _ => return, // approved, rejected, cancelled or retried
        }
    }
}

fn land_job(c: &Ctx) {
    if let Err(why) = prepare_worktree_only(c) {
        return c.fail(why);
    }
    // MASON's own repository: a change that weakens a check goes back to development, and
    // one that touches its gates is recorded (`[self] holds = "owner"` makes either wait for
    // the owner), by the running version's rules, before it may join the queue.
    match crate::selfguard::gate(c) {
        crate::selfguard::Gate::Gone => return,
        crate::selfguard::Gate::SentBack { reason, feedback } => {
            return c.conclude(Outcome::Red, reason, Some(feedback))
        }
        crate::selfguard::Gate::Go => {}
    }
    // Landing is batched per project: join the queue and wait for the verdict.
    let Some(verdict) = crate::landing::submit_and_wait(c) else {
        return; // approved, rejected, cancelled or retried while waiting
    };
    match verdict {
        crate::landing::Verdict::Landed => {
            c.d.crash.point("land_before_event");
            c.green();
            c.d.crash.point("land_after_event");
            cleanup(&c.d, c.task, true);
            // The base branch just moved, perhaps including this project's own definition.
            let _ = c.d.reload_definition(c.project_id);
        }
        crate::landing::Verdict::SentBack { reason, feedback } => {
            c.conclude(Outcome::Red, reason, Some(feedback))
        }
        crate::landing::Verdict::Failed(why) => c.fail(format!("landing failed: {why}")),
    }
}

/// The owner approves or rejects the approval step a task is waiting in.
/// Approving goes where the graph says; rejecting is red (back to development
/// if the step has `on_red`, otherwise the task fails).
pub fn conclude_approval(
    d: &Arc<Daemon>,
    task: TaskId,
    outcome: Outcome,
    reason: &str,
) -> Result<(), String> {
    let w = d.world().map_err(|e| e.to_string())?;
    let t = w
        .tasks
        .get(&task)
        .ok_or_else(|| format!("no task #{}", task.0))?;
    let run = w
        .open_run(task)
        .ok_or_else(|| format!("task #{} is not in a step ({:?})", task.0, t.state))?;
    let r = &w.step_runs[&run];
    let def = d
        .pinned_def(&t.def)
        .ok_or("the task's definition is not in the store")?;
    let step = if r.name.is_empty() {
        def.first_in(r.step)
    } else {
        def.step(&r.name)
    }
    .ok_or("the step is not in the definition")?;
    if step.kind != StepType::Approval {
        return Err(format!(
            "task #{} is in step `{}` ({}), which is not waiting for approval",
            task.0,
            step.name,
            step.kind.name()
        ));
    }
    let kind = match after(&def, step, outcome, r.attempt, step.max_rounds) {
        Transition::Next { step, state } => EventKind::StepSucceeded {
            task,
            next: Some(Next { step, state }),
        },
        Transition::Land => EventKind::StepSucceeded {
            task,
            next: Some(Next {
                step: String::new(),
                state: State::Landed,
            }),
        },
        Transition::SendBack { step: back } => {
            let _ = std::fs::write(
                d.task_dir(task).join("feedback.txt"),
                format!("The owner rejected the work at `{}`: {reason}", step.name),
            );
            EventKind::SentBack {
                task,
                reason: format!("rejected by the owner: {reason}"),
                to_step: back,
            }
        }
        Transition::Fail(why) => EventKind::StepFailed {
            task,
            reason: format!("rejected by the owner: {reason} ({why})"),
        },
    };
    if d.emit_for_run(task, run, kind) {
        Ok(())
    } else {
        Err("the step ended while you were deciding".into())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_steps_result_file_sets_passed_and_summary() {
        let r = read_script_result(r#"{"passed": false, "summary": "3 rows differ"}"#).unwrap();
        assert_eq!(
            r,
            ScriptResult {
                passed: Some(false),
                summary: "3 rows differ".into()
            }
        );
        // Without `passed` the exit status decides.
        let r = read_script_result(r#"{"summary": "ran"}"#).unwrap();
        assert_eq!((r.passed, r.summary.as_str()), (None, "ran"));
        for bad in [
            "not json",
            "[]",
            r#"{"passed": "yes"}"#,
            r#"{"summary": 3}"#,
            r#"{"pased": true}"#,
        ] {
            assert!(read_script_result(bad).is_err(), "{bad}");
        }
    }
}
