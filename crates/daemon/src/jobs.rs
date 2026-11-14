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
use mason_adapter_api::contract::{CheckHandler, CheckRun, CheckScope, ContractHandle, Validator};
use mason_adapter_api::{AgentAdapter, SessionSpec, StepKind};
use mason_adapters::claude::ClaudeAdapter;
use mason_adapters::codex::CodexAdapter;
use mason_adapters::hosted::HostLaunch;
use mason_adapters::runner::{
    hold_note, worth_a_stronger_model, FailReason, RunnerConfig, StepEnd, StepReport, StepRunner,
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
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The resources a step holds, shared with its `run_check`, which gives them
/// back while the tests run.
pub type StepLease = Arc<std::sync::Mutex<Option<crate::sched::Lease>>>;

/// How many times a task may be sent back for a conflict with the base before
/// it fails. These do not count against a step's `max_rounds` (s69).
pub(crate) const CONFLICT_ROUNDS: u32 = 5;

/// One line per conflict send-back in the task's directory: `<run>\t<reason>`.
pub(crate) const CONFLICTS_FILE: &str = "conflicts.txt";

/// The step runs recorded in a task directory's [`CONFLICTS_FILE`].
pub(crate) fn conflict_runs(task_dir: &Path) -> Vec<u64> {
    let mut v: Vec<u64> = std::fs::read_to_string(task_dir.join(CONFLICTS_FILE))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split('\t').next()?.trim().parse().ok())
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Every send-back's feedback, in order, for the escalated last round.
const FEEDBACK_HISTORY: &str = "feedback-history.txt";

/// Is the next test round the last one the test step will give (another red would fail the
/// task), after at least one red round? `done`: test rounds that went red so far.
pub(crate) fn last_round_ahead(done: u32, max_rounds: u32) -> bool {
    done >= 1 && done + 1 >= max_rounds
}

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
    pub lease: StepLease,
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
        let round = self.round();
        self.conclude_round(outcome, reason, feedback, round)
    }

    /// Which round of this step this run is, as `max_rounds` counts them: its
    /// runs since the owner last restarted the task (a retry, or explore-anyway),
    /// leaving out the runs that ended in a conflict with the base
    /// ([`Ctx::conflict`]). A retried task gets its rounds again, and a base that
    /// moved is not held against the change. Falls back to the attempt number.
    pub(crate) fn round(&self) -> u32 {
        let Ok(w) = self.d.world() else {
            return self.attempt;
        };
        let conflicts = self.conflict_runs();
        let mut runs: Vec<&StepRun> = w
            .step_runs
            .values()
            .filter(|r| r.task == self.task && r.id.0 <= self.run.0)
            .collect();
        runs.sort_by_key(|r| r.id.0);
        let restart = runs
            .iter()
            .rev()
            .find(|r| {
                r.id != self.run
                    && matches!(
                        r.outcome,
                        Some(StepOutcome::Failed | StepOutcome::Superseded | StepOutcome::Canceled)
                    )
            })
            .map_or(0, |r| r.id.0);
        let same = |r: &StepRun| {
            if self.step.name.is_empty() || r.name.is_empty() {
                r.step == self.state
            } else {
                r.name == self.step.name
            }
        };
        let n = runs
            .iter()
            .filter(|r| r.id.0 > restart && same(r))
            .filter(|r| r.id == self.run || !conflicts.contains(&r.id.0))
            .count() as u32;
        n.max(1)
    }

    /// How many rounds of the test step (the one that sends back to this step) went red since
    /// the owner last restarted the task, leaving out conflict runs, as `round` counts them.
    /// `None` when no test step sends work back here.
    fn test_rounds_done(&self) -> Option<(u32, u32)> {
        let test = self.def.steps.iter().find(|s| {
            s.kind == StepType::Test && s.on_red.as_deref() == Some(self.step.name.as_str())
        })?;
        let max = self.project.max_test_rounds.or(test.max_rounds)?;
        let w = self.d.world().ok()?;
        let conflicts = self.conflict_runs();
        let mut runs: Vec<&StepRun> = w
            .step_runs
            .values()
            .filter(|r| r.task == self.task && r.id.0 < self.run.0)
            .collect();
        runs.sort_by_key(|r| r.id.0);
        let restart = runs
            .iter()
            .rev()
            .find(|r| {
                matches!(
                    r.outcome,
                    Some(StepOutcome::Failed | StepOutcome::Superseded | StepOutcome::Canceled)
                )
            })
            .map_or(0, |r| r.id.0);
        let done = runs
            .iter()
            .filter(|r| r.id.0 > restart && r.name == test.name)
            .filter(|r| !conflicts.contains(&r.id.0))
            .filter(|r| matches!(r.outcome, Some(StepOutcome::SentBack)))
            .count() as u32;
        Some((done, max))
    }

    /// The change conflicts with the base (or with the rest of its landing
    /// batch): back to the step's `on_red`, with the note for the agent, and
    /// not counted as a red round, since the change is not what is wrong. A
    /// task that has conflicted [`CONFLICT_ROUNDS`] times fails, so a base the
    /// agent cannot keep up with still ends.
    pub(crate) fn conflict(&self, reason: String, note: String) {
        let mut past = self.conflict_runs();
        if !past.contains(&self.run.0) {
            if past.len() as u32 >= CONFLICT_ROUNDS {
                return self.fail(format!(
                    "{reason}; still conflicting with the base after {CONFLICT_ROUNDS} merges"
                ));
            }
            let line = format!("{}\t{}\n", self.run.0, reason.replace('\n', " "));
            let path = self.task_dir().join(CONFLICTS_FILE);
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
            past.push(self.run.0);
        }
        // Round 0 is under every limit: a conflict always goes back while there is an `on_red`.
        self.conclude_round(Outcome::Red, reason, Some(note), 0)
    }

    /// The step runs that ended in a conflict send-back ([`Ctx::conflict`]).
    pub(crate) fn conflict_runs(&self) -> Vec<u64> {
        conflict_runs(&self.d.task_dir(self.task))
    }

    fn conclude_round(
        &self,
        outcome: Outcome,
        reason: String,
        feedback: Option<String>,
        round: u32,
    ) {
        self.d
            .crash
            .point(&format!("before_event:{:?}", self.state));
        let rounds = self.res.max_rounds.as_ref().map(|a| a.value);
        // The history the detectors read (Part 14): how this attempt ended, and why.
        let _ = self
            .d
            .store
            .record_attempt(&mason_core::introspect::Attempt {
                task: self.task.0,
                project: self.project.id.clone(),
                step: self.step.name.clone(),
                outcome: match outcome {
                    Outcome::Green => "green",
                    Outcome::Red => "red",
                    Outcome::Fail => "fail",
                }
                .into(),
                signature: mason_core::introspect::signature(&reason),
                reason: reason.clone(),
                retry_n: self.attempt,
                at: now_ms(),
            });
        match after(&self.def, &self.step, outcome, round, rounds) {
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
                    let _ = std::fs::write(self.task_dir().join("feedback.txt"), &f);
                    // Every round's feedback is kept for the last round's fresh session (s251).
                    let entry = format!(
                        "## After run {} ({}): {}\n{}\n\n",
                        self.run.0, self.step.name, reason, f
                    );
                    let _ = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(self.task_dir().join(FEEDBACK_HISTORY))
                        .and_then(|mut h| std::io::Write::write_all(&mut h, entry.as_bytes()));
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
                // Hand the resources back before the task shows as failed, so nobody
                // can see a failed task still holding its device.
                self.release_lease();
                let class = Some(FailClass::classify(self.state, &reason));
                self.emit(EventKind::StepFailed {
                    task: self.task,
                    reason,
                    class,
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
        lease: Arc::new(std::sync::Mutex::new(None)),
    })
}

pub fn run(d: &Arc<Daemon>, task: TaskId, step: State, run: StepRunId) {
    run_inner(d, task, step, run);
    // A task that was the account's probe is not any longer once its job is over.
    crate::holds::task_done(d, task.0);
}

fn run_inner(d: &Arc<Daemon>, task: TaskId, step: State, run: StepRunId) {
    let c = match ctx(d, task, run) {
        Ok(c) => c,
        Err(why) => {
            d.emit_for_run(
                task,
                run,
                EventKind::StepFailed {
                    task,
                    reason: why,
                    class: Some(FailClass::Code),
                },
            );
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
    if c.step.kind == StepType::Agent {
        // A held account starts no new session; the task waits without a lease.
        if get_lease(&c).is_err() {
            return;
        }
    } else if c.step.kind != StepType::Approval
        && builds_first(&c).is_none()
        && !(c.step.kind == StepType::Test
            && !c.step.suites.is_empty()
            && crate::suites::builds_per_suite(&c.project, &c.def, &c.step, &c.branch))
    {
        // A test step with a build queues phase by phase in `command_job`, and one whose
        // suites build, suite by suite in `suites::run`.
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
        (
            "MASON_TASK_CACHE".into(),
            crate::taskcache::own_dir(&c.d, c.task.0),
        ),
        ("MASON_TITLE".into(), c.title.clone()),
        ("MASON_PROJECT".into(), c.project.id.clone()),
        ("MASON_WORKTREE".into(), c.wt.to_string_lossy().into_owned()),
        ("MASON_BRANCH".into(), c.branch.clone()),
        ("MASON_BASE".into(), c.project.base_ref()),
        ("MASON_STEP".into(), c.step.name.clone()),
        ("MASON_ATTEMPT".into(), c.attempt.to_string()),
        // What the step already holds from the broker. Here that is the step's own
        // `uses`; each command is told what is leased for it (`command_job` and the
        // suites override this), so a command that would queue for the same class
        // (`mason run --needs`) can tell it need not, and does not wait on itself.
        ("MASON_HELD".into(), held_text(&c.step.uses)),
        (
            "MASON_RESULT_FILE".into(),
            result_file(c).to_string_lossy().into_owned(),
        ),
        (
            "MASON_PROGRESS_FILE".into(),
            progress_file(c).to_string_lossy().into_owned(),
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

/// Where a script step may append progress, one JSON object per line:
/// `{"done": n, "total": m, "unit": "...", "message": "..."}` (every field optional).
/// A line that is not JSON is the message.
fn progress_file(c: &Ctx) -> PathBuf {
    c.task_dir()
        .join(format!("progress-{}-r{}.jsonl", c.step.name, c.run.0))
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
            if !c.emit(EventKind::WaitStarted {
                task: c.task,
                on,
                class: Some("pause".into()),
            }) {
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

/// How often a task waiting on a held account looks again.
const HOLD_POLL: Duration = Duration::from_millis(100);

fn still_open(c: &Ctx) -> bool {
    c.d.world()
        .map(|w| w.open_run(c.task) == Some(c.run))
        .unwrap_or(true)
}

/// Wait while the step's account is held (see `holds`). Says why in the task's
/// wait, once; no lease is held and no time is charged. `Err` when the task
/// went away meanwhile.
fn wait_open(c: &Ctx) -> Result<(), ()> {
    use crate::holds::Gate;
    let mut announced = false;
    let mut n = 0u32;
    loop {
        match crate::holds::gate(&c.d, &c.project.account, c.task.0) {
            Gate::Open | Gate::Probe => break,
            Gate::Wait(on) => {
                if !announced {
                    announced = true;
                    if !c.emit(EventKind::WaitStarted {
                        task: c.task,
                        on,
                        class: Some("environment".into()),
                    }) {
                        return Err(());
                    }
                }
                std::thread::sleep(HOLD_POLL);
                n += 1;
                if n.is_multiple_of(5) && !still_open(c) {
                    return Err(());
                }
            }
        }
    }
    if announced {
        c.emit(EventKind::WaitEnded { task: c.task });
    }
    Ok(())
}

/// The agent step's session slot, once its account is open. A hold that
/// begins while the step queues for the slot sends it back to waiting.
fn get_lease(c: &Ctx) -> Result<(), ()> {
    loop {
        wait_open(c)?;
        let have = c.lease.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        if !have {
            match acquire(c) {
                Ok(l) => *c.lease.lock().unwrap_or_else(|e| e.into_inner()) = Some(l),
                Err(_) => return Err(()),
            }
        }
        if matches!(
            crate::holds::gate(&c.d, &c.project.account, c.task.0),
            crate::holds::Gate::Wait(_)
        ) {
            c.release_lease();
            continue;
        }
        return Ok(());
    }
}

/// Wait for this step's resources (Part 7). Says why in the task's wait, once,
/// and ends the wait when they are granted.
fn acquire(c: &Ctx) -> Result<crate::sched::Lease, crate::sched::Cancelled> {
    let account = (c.step.kind == StepType::Agent).then_some(c.project.account.as_str());
    // A definition names what it uses; a name nobody declared gets its
    // definition-given limit (or one slot).
    let mut needs = crate::sched::needs_of(&c.step.uses, account);
    if c.step.kind == StepType::Test && !c.step.suites.is_empty() {
        // The step holds what its suites (and their environments) use.
        let more = crate::suites::needs_for(&c.project, &c.def, &c.step, &c.branch);
        needs.exclusive |= more.exclusive;
        for (k, v) in more.amounts {
            let have = needs.amounts.entry(k).or_insert(0);
            *have = (*have).max(v);
        }
    }
    acquire_needs(c, needs)
}

/// Wait for `needs`, ranked as this step's work.
pub(crate) fn acquire_needs(
    c: &Ctx,
    needs: mason_scheduler::Needs,
) -> Result<crate::sched::Lease, crate::sched::Cancelled> {
    acquire_needs_while(c, needs, &|| true)
}

/// Like `acquire_needs`, but the wait also ends (`Cancelled`) as soon as `wanted`
/// says no: a suite whose skip arrives while it waits for its slot.
pub(crate) fn acquire_needs_while(
    c: &Ctx,
    needs: mason_scheduler::Needs,
    wanted: &dyn Fn() -> bool,
) -> Result<crate::sched::Lease, crate::sched::Cancelled> {
    let d = &c.d;
    // Device pools are sized by what the provider lists, never by a default.
    if needs.amounts.keys().any(|n| n.starts_with("device:")) {
        d.targets.declare_all(d);
    }
    for name in needs.amounts.keys() {
        if !name.starts_with("llm:") && !name.starts_with("device:") {
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
            && wanted()
    };
    let mut waited = false;
    let asked = needs.clone();
    let lease = d.broker.acquire(
        c.task,
        &format!("#{}", c.task.0),
        rank,
        needs,
        &alive,
        &mut |why| {
            waited = c.emit(EventKind::WaitStarted {
                task: c.task,
                on: crate::targets::wait_text(d, &asked, why),
                class: Some(crate::targets::wait_class(d, &asked, c.task)),
            });
        },
    )?;
    if waited {
        c.emit(EventKind::WaitEnded { task: c.task });
    }
    Ok(lease)
}

/// What a test step runs: its command, and what builds the tests first, if anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TestCommands {
    /// Run first, holding one `compile` slot.
    pub build: Option<String>,
    /// Then this, holding the test step's `uses` without `compile` (all of
    /// `uses` when there is no build).
    pub test: String,
}

/// The one `compile` slot a build holds.
pub(crate) fn build_uses() -> std::collections::BTreeMap<String, u32> {
    std::collections::BTreeMap::from([(mason_scheduler::COMPILE.to_string(), 1)])
}

/// What a test run holds once its build is done: the test step's `uses`
/// without `compile`. The build compiled; the run that follows only runs.
pub(crate) fn tests_uses(
    uses: &std::collections::BTreeMap<String, u32>,
) -> std::collections::BTreeMap<String, u32> {
    uses.iter()
        .filter(|(k, n)| k.as_str() != mason_scheduler::COMPILE && **n > 0)
        .map(|(k, n)| (k.clone(), *n))
        .collect()
}

/// The resource a scoped `run_check` takes instead of `test-suite`, when the definition declares it.
pub(crate) const TEST_SMALL: &str = "test-small";

/// `uses` for a scoped check: `test-small` in place of `test-suite` when the definition declares
/// `[resource.test-small]`. Otherwise `uses` unchanged. Full checks and the test step never call this.
pub(crate) fn small_lane(
    def: &Definition,
    mut uses: std::collections::BTreeMap<String, u32>,
) -> std::collections::BTreeMap<String, u32> {
    if !def.resources.contains_key(TEST_SMALL) {
        return uses;
    }
    if let Some(n) = uses.remove("test-suite") {
        uses.insert(TEST_SMALL.to_string(), n);
    }
    uses
}

/// `MASON_HELD`: the classes a command already holds, so that one which would
/// queue for the same class (`mason run --needs`) can tell it need not.
pub(crate) fn held_text(uses: &std::collections::BTreeMap<String, u32>) -> String {
    uses.iter()
        .filter(|(_, n)| **n > 0)
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// `MASON_HELD` for a lease: its classes, plus `exclusive` when it holds the machine.
pub(crate) fn held_needs_text(needs: &mason_scheduler::Needs) -> String {
    let mut text = held_text(&needs.amounts);
    if needs.exclusive && !needs.amounts.contains_key("exclusive") {
        if !text.is_empty() {
            text.push(',');
        }
        text.push_str("exclusive");
    }
    text
}

/// A test step's build, when it has one: its command runs in two phases.
fn builds_first(c: &Ctx) -> Option<String> {
    (c.step.kind == StepType::Test && c.step.suites.is_empty())
        .then(|| c.res.build.as_ref().map(|a| a.value.clone()))
        .flatten()
}

/// The commands of the definition's test step for this project, layered exactly as
/// a test step resolves them: the definition, the project's local overlay, the
/// owner's runtime overrides (`mason config set`) and, for a task, its own
/// overrides. `run_check` and landing's legacy suite use this, so they run the
/// same commands the test step runs.
pub(crate) fn test_commands(
    d: &Daemon,
    def: &mason_definition::Definition,
    pid: ProjectId,
    task: Option<TaskId>,
) -> Option<TestCommands> {
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
    let r = resolve(def, step, &d.overlay(pid), &runtime, &task_overrides, now).ok()?;
    Some(TestCommands {
        test: r.command?.value,
        build: r.build.map(|a| a.value).filter(|_| step.suites.is_empty()),
    })
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
    // The fetch has a lock of its own: a stalled remote holds up other fetches,
    // not the worktrees, merges and cleanup that take the repository lock.
    // A fetch that fails (the network dropped, the remote is down) decides nothing: the
    // last-known base is still there, and landing folds in the fresh base anyway.
    if let Err(why) = c.d.fetch_remote(c.project_id) {
        let base = c.project.base_ref();
        if git::rev_parse(&c.project.path, &base).is_err() {
            return Err(format!(
                "fetching the remote failed ({why}) and there is no last-known {base} to work from"
            ));
        }
        tracing::warn!(
            task = c.task.0,
            "fetching the remote failed ({why}); going on from the last-known {base}"
        );
    }
    wait_out_environment(c, "preparing the worktree", || prepare_locked(c))
}

fn prepare_locked(c: &Ctx) -> Result<Option<String>, git::GitError> {
    let lock = c.d.repo_lock(c.project_id);
    let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
    let repo = &c.project.path;
    let base = c.project.base_ref();
    git::ensure_worktree(repo, &c.wt, &c.branch, &base)?;
    // Uncommitted changes are an agent's work in progress: a step re-entered after
    // the daemon restarted (a self-update, say) finds its agent's edits not yet
    // committed. Merging the base into them fails ("your local changes would be
    // overwritten") and failed the task; leave the branch as it is, and the next
    // step folds the base in once the work is committed.
    // `--no-optional-locks`: a plain `git status` refreshes the index and takes
    // `index.lock` to do it. An agent still winding down in this worktree (its step
    // just superseded) then fails its own `git commit` on that lock.
    let dirty = git::run(&c.wt, &["--no-optional-locks", "status", "--porcelain"])?;
    if !dirty.trim().is_empty() {
        tracing::info!(
            task = c.task.0,
            "worktree has uncommitted changes; not folding in the base now"
        );
        return Ok(None);
    }
    match git::catch_up(&c.wt, &base)? {
        CatchUp::Conflict(files) => Ok(Some(files.trim().to_string())),
        _ => Ok(None),
    }
}

/// Whether a git failure is the environment's (a deadline passed, a lock is held) and not a
/// verdict on the task or the repository.
pub(crate) fn environmental(e: &git::GitError) -> bool {
    if e.timed_out {
        return true;
    }
    let s = e.stderr.to_lowercase();
    s.contains("index.lock")
        || s.contains("packed-refs.lock")
        || s.contains("another git process")
        || (s.contains(".lock") && (s.contains("file exists") || s.contains("unable to create")))
}

static ENV_WAIT_LIMIT_MS: AtomicU64 = AtomicU64::new(0);
static ENV_BACKOFF_MS: AtomicU64 = AtomicU64::new(0);

/// Shorten how long a step waits out an environmental git failure, and the first backoff
/// (tests). Zeros restore the defaults: 15 minutes, starting at one second.
pub fn set_env_wait(limit: Duration, first_backoff: Duration) {
    ENV_WAIT_LIMIT_MS.store(limit.as_millis() as u64, Ordering::SeqCst);
    ENV_BACKOFF_MS.store(first_backoff.as_millis() as u64, Ordering::SeqCst);
}

fn env_wait_limit() -> Duration {
    match ENV_WAIT_LIMIT_MS.load(Ordering::SeqCst) {
        0 => Duration::from_secs(15 * 60),
        ms => Duration::from_millis(ms),
    }
}

fn env_backoff() -> Duration {
    match ENV_BACKOFF_MS.load(Ordering::SeqCst) {
        0 => Duration::from_secs(1),
        ms => Duration::from_millis(ms),
    }
}

/// Run `f`; while it fails for environmental reasons (see [`environmental`]) say the step is
/// waiting, back off and try again, for up to 15 minutes. Nothing is charged to the task
/// meanwhile. A real error is returned at once; so is the last one when the time runs out,
/// naming what failed.
pub(crate) fn wait_out_environment<T>(
    c: &Ctx,
    what: &str,
    mut f: impl FnMut() -> Result<T, git::GitError>,
) -> Result<T, String> {
    let started = Instant::now();
    let limit = env_wait_limit();
    let mut delay = env_backoff();
    let mut waited = false;
    loop {
        match f() {
            Ok(v) => {
                if waited {
                    c.emit(EventKind::WaitEnded { task: c.task });
                }
                return Ok(v);
            }
            Err(e) if environmental(&e) => {
                if started.elapsed() >= limit {
                    return Err(format!(
                        "{what}: still failing after {} s of waiting: {e}",
                        started.elapsed().as_secs()
                    ));
                }
                if !waited {
                    waited = true;
                    tracing::warn!(task = c.task.0, "{what}: {e}; waiting");
                    let on = format!("git is busy or not answering ({what}): {e}");
                    if !c.emit(EventKind::WaitStarted {
                        task: c.task,
                        on,
                        class: Some("environment".into()),
                    }) {
                        return Err(format!("{what}: the step was superseded while waiting"));
                    }
                }
                std::thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_secs(30));
                let alive =
                    c.d.world()
                        .map(|w| w.open_run(c.task) == Some(c.run))
                        .unwrap_or(true);
                if !alive {
                    return Err(format!("{what}: the step was superseded while waiting"));
                }
            }
            Err(e) => return Err(format!("{what}: {e}")),
        }
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
    wait_out_environment(c, "preparing the worktree", || {
        let lock = c.d.repo_lock(c.project_id);
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::ensure_worktree(&c.project.path, &c.wt, &c.branch, &c.project.base_ref())
    })
}

/// Run `sh -c cmd` in `cwd`, giving up after `timeout`. Returns whether it
/// exited 0 and the last lines of its output.
pub fn run_shell(cmd: &str, cwd: &Path, timeout: Duration) -> (bool, String) {
    run_shell_env(cmd, cwd, timeout, &[])
}

/// [`run_shell`] with extra environment variables.
fn run_shell_env(
    cmd: &str,
    cwd: &Path,
    timeout: Duration,
    env: &[(String, String)],
) -> (bool, String) {
    let out = tempfile_path();
    let file = match std::fs::File::create(&out) {
        Ok(f) => f,
        Err(e) => return (false, format!("cannot capture output: {e}")),
    };
    let err = file.try_clone();
    let mut command = Command::new("sh");
    command
        .args(["-c", cmd])
        .current_dir(cwd)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(file)
        .stderr(err.map_or_else(|_| Stdio::null(), Stdio::from));
    mason_platform::process::group_leader(&mut command);
    let child = command.spawn();
    let Ok(mut child) = child else {
        return (false, "could not start the command".into());
    };
    let end = Instant::now() + timeout;
    let ok = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.success(),
            Ok(None) if Instant::now() >= end => {
                let _ = mason_platform::process::signal_group(
                    child.id(),
                    mason_platform::process::Signal::Kill,
                );
                let _ = child.wait();
                break false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break false,
        }
    };
    let text = read_tail_lossy(&out).unwrap_or_default();
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

/// Read a text file, replacing bytes that are not valid UTF-8 with U+FFFD.
/// A missing or unreadable file is an error, as with `read_to_string`.
pub fn read_lossy(path: &Path) -> std::io::Result<String> {
    Ok(String::from_utf8_lossy(&std::fs::read(path)?).into_owned())
}

/// How much of the end of a log `read_tail_lossy` looks at.
pub const TAIL_WINDOW: u64 = 1 << 20;

/// The last `TAIL_WINDOW` bytes of a text file, read lossily. When the file is
/// longer, the partial first line is dropped.
pub fn read_tail_lossy(path: &Path) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let cut = len > TAIL_WINDOW;
    if cut {
        f.seek(SeekFrom::Start(len - TAIL_WINDOW))?;
    }
    let mut buf = Vec::new();
    f.take(TAIL_WINDOW).read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    if cut {
        if let Some(i) = text.find('\n') {
            return Ok(text[i + 1..].to_string());
        }
    }
    Ok(text.into_owned())
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
    cmd: Option<TestCommands>,
    wt: PathBuf,
    timeout: Duration,
    slots: CheckSlots,
    /// The step's deadline grows by the time the agent spent queued for a check.
    extra: mason_adapter_api::DeadlineExtra,
    id_prefix: String,
    log_dir: PathBuf,
    /// The test step's suites, when `run_check` runs those (some are bound to
    /// `development`); otherwise it runs the one command.
    suites: Option<DevSuites>,
    task: u64,
    run: u64,
    /// A develop step's project and base: before a check the base is folded in when that is
    /// safe (`basefold`). `None` for other steps.
    base_fold: Option<(ProjectId, String)>,
}

/// What `run_check` needs to pick suites: the test step, and where the base is.
struct DevSuites {
    step: mason_definition::Step,
    base_ref: String,
}

impl DevSuites {
    fn of(def: &mason_definition::Definition, base_ref: String) -> Option<DevSuites> {
        let step = def.steps.iter().find(|s| s.kind == StepType::Test)?;
        let any = def
            .testing
            .bound(mason_testing::When::Development)
            .iter()
            .any(|s| step.suites.contains(&s.name));
        (!step.suites.is_empty() && any).then(|| DevSuites {
            step: step.clone(),
            base_ref,
        })
    }
}

impl TestChecks {
    /// `run_check` on a suites step: the suites bound to `development` that the impact map
    /// selects for the worktree's changes, each after its own build, reported one by one.
    fn run_suites(&self, dev: &DevSuites, scope: &CheckScope) -> CheckRun {
        use std::collections::BTreeMap;
        let over = || CheckRun::Done {
            passed: false,
            summary: STEP_OVER.into(),
        };
        let def = self.slots.def.clone();
        let paths = crate::suites::worktree_paths(&self.wt, &dev.base_ref);
        let sels = crate::suites::plan_for(
            &def,
            &dev.step,
            paths.clone(),
            mason_testing::When::Development,
        );
        // Packages the diff touches, found once and only if a suite asks for them.
        let mut touched: Option<Option<Vec<String>>> = None;
        let mut narrowed: Vec<String> = vec![];
        let project_test = self.cmd.as_ref().map(|c| c.test.as_str());
        let tree = crate::hostedcheck::tree_key(&self.wt);
        let spec = |prefix: String, cmd: &str, uses: &BTreeMap<String, u32>, limit: Duration| {
            crate::hostedcheck::CheckSpec {
                id_prefix: prefix,
                log_dir: self.log_dir.clone(),
                cmd: cmd.to_string(),
                cwd: self.wt.clone(),
                env: vec![("MASON_HELD".to_string(), held_text(uses))],
                limit,
                tree: tree.clone(),
            }
        };
        let mut lines: Vec<String> = vec![];
        let mut failed = false;
        let mut built: Vec<(String, bool)> = vec![];
        let mut any = false;
        for sel in &sels {
            if !sel.selected {
                lines.push(format!("{}: not run, {}", sel.suite.name, sel.why));
                continue;
            }
            any = true;
            let name = sel.suite.name.clone();
            let mut suite = match crate::suites::with_project_test(&sel.suite, project_test) {
                Ok(s) => s,
                Err(why) => {
                    failed = true;
                    lines.push(format!("{name}: FAILED, {why}"));
                    continue;
                }
            };
            let limit = suite.timeout.unwrap_or(self.timeout).min(self.timeout);
            let before = suite.command.clone();
            // Only this develop-phase check narrows. The test step runs `command` whole.
            if !scope.is_empty() {
                let mut packages = |scope: &CheckScope| -> Option<Vec<String>> {
                    if !scope.packages.is_empty() {
                        return Some(scope.packages.clone());
                    }
                    touched
                        .get_or_insert_with(|| {
                            paths
                                .as_deref()
                                .and_then(|p| crate::scope::changed_packages(&self.wt, p))
                        })
                        .clone()
                };
                if let (false, Some(t)) = (scope.tests.is_empty(), &suite.rerun) {
                    suite.command = t.replace(
                        "{tests}",
                        &scope
                            .tests
                            .iter()
                            .map(|n| shell_word(n))
                            .collect::<Vec<_>>()
                            .join(" "),
                    );
                    narrowed.push(format!(
                        "{name}: only tests matching {}",
                        scope.tests.join(", ")
                    ));
                } else if let (true, Some(dc)) = (
                    scope.changed || !scope.packages.is_empty(),
                    suite.dev_command.clone(),
                ) {
                    match packages(scope) {
                        Some(pk) => {
                            let flags = crate::scope::flags(&pk);
                            suite.command = dc.replace("{packages}", &flags);
                            if let Some(db) = &suite.dev_build {
                                suite.build = Some(db.replace("{packages}", &flags));
                            }
                            narrowed.push(format!(
                                "{name}: {} packages ({})",
                                pk.len(),
                                pk.join(", ")
                            ));
                        }
                        None => lines.push(format!(
                            "{name}: no packages found for your changes, so it runs whole"
                        )),
                    }
                } else {
                    lines.push(format!(
                        "{name}: cannot be narrowed that way, so it runs whole"
                    ));
                }
            }
            if let Some(b) = &suite.build {
                let known = built.iter().find(|(c, _)| c == b).map(|(_, ok)| *ok);
                let ok = match known {
                    Some(ok) => {
                        if !ok {
                            lines.push(format!("{name}: FAILED, the build it shares failed"));
                        }
                        ok
                    }
                    None => {
                        let bspec = spec(
                            format!("{}-{name}-build", self.id_prefix),
                            b,
                            &build_uses(),
                            limit,
                        );
                        let queue = !crate::hostedcheck::pending(&self.slots.d, &bspec);
                        let queued = Instant::now();
                        let swapped = self.slots.swap_for(queue, &build_uses());
                        self.extra.add(queued.elapsed());
                        let Ok(lease) = swapped else {
                            return over();
                        };
                        let res = crate::hostedcheck::run(&self.slots.d, &bspec);
                        drop(lease);
                        let ok = matches!(&res, Ok(o) if o.passed);
                        if !ok {
                            lines.push(match &res {
                                Ok(o) => format!(
                                    "{name}: FAILED, its build {}. Last output:\n{}",
                                    if o.timed_out { "timed out" } else { "failed" },
                                    o.output
                                ),
                                Err(e) => format!("{name}: FAILED, its build gave no result: {e}"),
                            });
                        }
                        built.push((b.clone(), ok));
                        ok
                    }
                };
                if !ok {
                    failed = true;
                    continue;
                }
            }
            let mut uses = if suite.build.is_some() {
                tests_uses(&self.slots.test_uses)
            } else {
                self.slots.test_uses.clone()
            };
            for (k, v) in &suite.uses {
                if !k.starts_with("device:") && !k.starts_with("llm:") {
                    let have = uses.entry(k.clone()).or_insert(0);
                    *have = (*have).max(*v);
                }
            }
            // A command that was narrowed is short: it takes the small lane when the definition
            // declares one, so it does not queue behind a full run.
            if suite.command != before {
                uses = small_lane(&def, uses);
            }
            let rspec = spec(
                format!("{}-{name}", self.id_prefix),
                &suite.command,
                &uses,
                limit,
            );
            let queue = !crate::hostedcheck::pending(&self.slots.d, &rspec);
            let queued = Instant::now();
            let swapped = self.slots.swap_for(queue, &uses);
            self.extra.add(queued.elapsed());
            let Ok(lease) = swapped else {
                return over();
            };
            let res = crate::hostedcheck::run(&self.slots.d, &rspec);
            drop(lease);
            match res {
                Ok(o) if o.passed => lines.push(format!("{name}: passed")),
                Ok(o) => {
                    failed = true;
                    lines.push(format!(
                        "{name}: FAILED{}. Last output:\n{}",
                        if o.timed_out { " (timed out)" } else { "" },
                        o.output
                    ));
                    if let Some(h) = known_header(&self.slots.d, &self.log_dir, &o.id, &name) {
                        lines.push(format!("{name}: {h}"));
                    }
                }
                Err(e) => {
                    failed = true;
                    lines.push(format!("{name}: FAILED, no result: {e}"));
                }
            }
        }
        self.slots.take_agent_slot_back();
        if !any {
            lines.insert(
                0,
                "No suite is selected for your changes, so none ran.".into(),
            );
        }
        if !narrowed.is_empty() {
            lines.push(format!(
                "Scoped ({}): this green is not the test step's. The test step runs the whole \
                 workspace; run one unscoped run_check before you submit.",
                narrowed.join("; ")
            ));
        }
        CheckRun::Done {
            passed: !failed,
            summary: lines.join("\n"),
        }
    }
}

/// A header for `run_check`'s output naming the failing tests that are known red (not the
/// agent's to fix) and their fixes: read from the whole log of the run `id`, `scope` being
/// the suite or test step the ledger keeps them under.
fn known_header(d: &Daemon, log_dir: &Path, id: &str, scope: &str) -> Option<String> {
    let text = read_whole_log(&log_dir.join(format!("{id}.log")))?;
    crate::knownred::header(&text, &crate::knownred::known_for(&d.store, scope))
}

/// A name from a validated scope (letters, digits, `_ : . - /`), quoted all the same.
fn shell_word(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

impl CheckHandler for TestChecks {
    fn run(&self, name: &str) -> CheckRun {
        self.run_scoped(name, &CheckScope::default())
    }

    fn run_scoped(&self, name: &str, scope: &CheckScope) -> CheckRun {
        let note = match (&self.base_fold, name == "tests") {
            (Some((pid, base)), true) => {
                let fold = {
                    let lock = self.slots.d.repo_lock(*pid);
                    let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
                    crate::basefold::fold(&self.wt, base)
                };
                if !matches!(fold, crate::basefold::BaseFold::Current) {
                    tracing::info!(
                        task = self.task,
                        run = self.run,
                        ?fold,
                        "run_check: base fold"
                    );
                }
                crate::basefold::note(base, &fold)
            }
            _ => None,
        };
        match (note, self.check(name, scope)) {
            (Some(n), CheckRun::Done { passed, summary }) => CheckRun::Done {
                passed,
                summary: format!("{n}\n\n{summary}"),
            },
            (_, r) => r,
        }
    }
}

impl TestChecks {
    /// The open debt that skips this task's test step, if the owner skipped it: the
    /// agent's own checks then run nothing either (s264).
    fn skipped_by_owner(&self) -> Option<u64> {
        let step = self.slots.test_step.as_deref()?;
        crate::suites::skip_for(&self.slots.d, TaskId(self.task), "", step)
    }

    fn check(&self, name: &str, scope: &CheckScope) -> CheckRun {
        // On 2026-11-13 the owner skipped testing for every active task to drain the queue,
        // but their agents kept queueing `run_check` for compile slots. A skip before the
        // check, or one that ends its wait for a slot (`CheckSlots::acquire`), is answered
        // with what happened, not run.
        if name == "tests" {
            if let Some(debt) = self.skipped_by_owner() {
                return not_run_skipped(debt);
            }
        }
        let r = self.check_unskipped(name, scope);
        // Only a check whose wait was ended is replaced; one that ran keeps its result.
        let ended = matches!(&r, CheckRun::Done { passed: false, summary } if summary == STEP_OVER);
        match (name == "tests" && ended, self.skipped_by_owner()) {
            (true, Some(debt)) => {
                // A skip that ended the wait left the agent without its slot.
                if self
                    .slots
                    .lease
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_none()
                {
                    self.slots.take_agent_slot_back();
                }
                not_run_skipped(debt)
            }
            _ => r,
        }
    }

    fn check_unskipped(&self, name: &str, scope: &CheckScope) -> CheckRun {
        if name == "tests" {
            if let Some(dev) = &self.suites {
                return self.run_suites(dev, scope);
            }
        }
        let (true, Some(cmds)) = (name == "tests", &self.cmd) else {
            return CheckRun::Unknown;
        };
        // No suites: the one command has no way to narrow, so it runs whole, and a green of
        // it is the full command's green (the reuse below is of that only).
        let whole = if scope.is_empty() {
            ""
        } else {
            "Scope ignored: this project has no suite that can narrow, so the whole command ran.\n"
        };
        let over = || CheckRun::Done {
            passed: false,
            summary: STEP_OVER.into(),
        };
        // The exact content tree and command already went green (here, in the test step or in
        // landing's re-test): the answer is that result, and nothing runs.
        let key = crate::cmdreuse::command_key(cmds.build.as_deref(), &cmds.test);
        let ctree = crate::cmdreuse::content_tree(&self.wt);
        if let Some(src) = ctree
            .as_deref()
            .and_then(|t| crate::cmdreuse::green_for(&self.slots.d, t, &key))
        {
            return CheckRun::Done {
                passed: true,
                summary: format!(
                    "Green for this exact tree and command already ({src}); the tests were not run again."
                ),
            };
        }
        // Under a step host, so that a restarted daemon finds the run again.
        let tree = crate::hostedcheck::tree_key(&self.wt);
        let spec = |prefix: String, cmd: &str, uses: &std::collections::BTreeMap<String, u32>| {
            crate::hostedcheck::CheckSpec {
                id_prefix: prefix,
                log_dir: self.log_dir.clone(),
                cmd: cmd.to_string(),
                cwd: self.wt.clone(),
                env: vec![("MASON_HELD".to_string(), held_text(uses))],
                limit: self.timeout,
                tree: tree.clone(),
            }
        };
        // A build first, holding one `compile` slot; the tests then hold the
        // rest of what the test step uses.
        let uses = match &cmds.build {
            None => self.slots.test_uses.clone(),
            Some(build) => {
                let bspec = spec(format!("{}build", self.id_prefix), build, &build_uses());
                let queue = !crate::hostedcheck::pending(&self.slots.d, &bspec);
                let queued = Instant::now();
                let swapped = self.slots.swap_for(queue, &build_uses());
                self.extra.add(queued.elapsed());
                let Ok(lease) = swapped else {
                    return over();
                };
                let res = crate::hostedcheck::run(&self.slots.d, &bspec);
                drop(lease);
                match res {
                    Ok(o) if o.passed => {}
                    Ok(o) => {
                        self.slots.take_agent_slot_back();
                        return CheckRun::Done {
                            passed: false,
                            summary: if o.timed_out {
                                format!(
                                    "Building the tests did not finish within {} s and was stopped. Last output:\n{}",
                                    self.timeout.as_secs(),
                                    o.output
                                )
                            } else {
                                format!("Building the tests failed. Last output:\n{}", o.output)
                            },
                        };
                    }
                    Err(e) => {
                        self.slots.take_agent_slot_back();
                        return CheckRun::Done {
                            passed: false,
                            summary: format!("Building the tests did not give a result: {e}"),
                        };
                    }
                }
                tests_uses(&self.slots.test_uses)
            }
        };
        let spec = spec(self.id_prefix.clone(), &cmds.test, &uses);
        // The same call sent again after a restart attaches to the run already
        // going (or takes its result) at once: it starts nothing, so it does not
        // queue behind other work for a slot the run it joins already took.
        let queue = !crate::hostedcheck::pending(&self.slots.d, &spec);
        let queued = Instant::now();
        let swapped = self.slots.swap_for(queue, &uses);
        self.extra.add(queued.elapsed());
        let Ok(test_lease) = swapped else {
            return over();
        };
        let res = crate::hostedcheck::run(&self.slots.d, &spec);
        drop(test_lease);
        self.slots.take_agent_slot_back();
        if let (Ok(o), Some(t)) = (&res, &ctree) {
            if o.passed {
                let src = format!("run_check of task #{}, run {}", self.task, self.run);
                crate::cmdreuse::record_green(&self.slots.d, t, &key, &src);
            }
        }
        let header = match &res {
            Ok(o) if !o.passed => {
                let step = self.slots.test_step.as_deref().unwrap_or("test");
                known_header(&self.slots.d, &self.log_dir, &o.id, step)
                    .map(|h| format!("{h}\n\n"))
                    .unwrap_or_default()
            }
            _ => String::new(),
        };
        match res {
            Ok(o) => CheckRun::Done {
                passed: o.passed,
                summary: header
                    + &if o.timed_out {
                        format!(
                        "The tests did not finish within {} s and were stopped. Last output:\n{}",
                        self.timeout.as_secs(),
                        o.output
                    )
                    } else {
                        format!("{whole}Last output:\n{}", o.output)
                    },
            },
            Err(e) => CheckRun::Done {
                passed: false,
                summary: format!("The check did not give a result: {e}"),
            },
        }
    }
}

/// What `run_check` holds while it runs. The agent does nothing but wait for a
/// check, so its LLM session slot goes to a task that can use it, and the check
/// queues for what the test step `uses`, as the test step does. The time spent
/// in that queue is outside the check's timeout.
struct CheckSlots {
    d: Arc<Daemon>,
    task: TaskId,
    run: StepRunId,
    def: Arc<Definition>,
    step: String,
    account: String,
    agent_uses: std::collections::BTreeMap<String, u32>,
    /// The test step's `uses`, without device pools: a check runs no suite.
    test_uses: std::collections::BTreeMap<String, u32>,
    /// The test step's name: a check queues as the test step's work, which it
    /// is, not as the develop step's. Ranked as develop it came after every test
    /// and landing of every other task, and waited hours for a slot.
    test_step: Option<String>,
    lease: StepLease,
}

impl CheckSlots {
    fn new(c: &Ctx) -> CheckSlots {
        let test = c.def.steps.iter().find(|s| s.kind == StepType::Test);
        let test_uses = test
            .map(|s| s.uses.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|(k, n)| *n > 0 && !k.starts_with("device:") && !k.starts_with("llm:"))
            .collect();
        CheckSlots {
            d: c.d.clone(),
            task: c.task,
            run: c.run,
            def: c.def.clone(),
            step: c.step.name.clone(),
            account: c.project.account.clone(),
            agent_uses: c.step.uses.clone(),
            test_uses,
            test_step: test.map(|s| s.name.clone()),
            lease: c.lease.clone(),
        }
    }

    fn acquire(
        &self,
        needs: mason_scheduler::Needs,
        as_step: &str,
        end_on_skip: bool,
    ) -> Result<crate::sched::Lease, crate::sched::Cancelled> {
        let d = &self.d;
        let t = d
            .world()
            .ok()
            .and_then(|w| w.tasks.get(&self.task).cloned())
            .ok_or(crate::sched::Cancelled)?;
        let rank = d.rank_for(&t, &self.def, as_step);
        let (d2, task, run) = (d.clone(), self.task, self.run);
        let test_step = self.test_step.clone().filter(|_| end_on_skip);
        // A skip of the task's test step ends a check's wait too: the check then says so.
        let alive = move || {
            d2.world()
                .map(|w| w.open_run(task) == Some(run))
                .unwrap_or(true)
                && test_step
                    .as_deref()
                    .is_none_or(|s| crate::suites::skip_for(&d2, task, "", s).is_none())
        };
        // The wait shows on the task, as a step's does: a check that queued
        // used to leave the task reading "running" for hours.
        let asked = needs.clone();
        let mut waited = false;
        let lease = d.broker.acquire(
            self.task,
            &format!("#{}", self.task.0),
            rank,
            needs,
            &alive,
            &mut |why| {
                waited = d.emit_for_run(
                    task,
                    run,
                    EventKind::WaitStarted {
                        task,
                        on: crate::targets::wait_text(d, &asked, why),
                        class: Some(crate::targets::wait_class(d, &asked, task)),
                    },
                );
            },
        );
        if waited {
            d.emit_for_run(task, run, EventKind::WaitEnded { task });
        }
        lease
    }

    /// Give the agent's slot back (if it still holds it) and, if `queue`,
    /// queue for `uses`: what the tests use, or a build's `compile` slot.
    fn swap_for(
        &self,
        queue: bool,
        uses: &std::collections::BTreeMap<String, u32>,
    ) -> Result<Option<crate::sched::Lease>, crate::sched::Cancelled> {
        let held = self.lease.lock().unwrap_or_else(|e| e.into_inner()).take();
        if held.is_some() {
            drop(held);
            self.d.admit();
        }
        if !queue || uses.is_empty() {
            return Ok(None);
        }
        for name in uses.keys() {
            self.d
                .broker
                .declare(name, self.def.resources.get(name).copied().unwrap_or(1));
        }
        // Ranked as the agent's own step, not the test step: a task that has reached
        // its test step is further along and goes first (s262). The check used to
        // rank as the test step so that a long wait could not run the agent out of
        // time; the wait is now added back to the agent's deadline (`extra`), and on
        // 2026-11-13 develop checks tied with test steps and took every compile
        // slot ahead of four tasks already in test.
        self.acquire(
            crate::sched::needs_of(uses, None),
            check_rank_step(&self.step, self.test_step.as_deref()),
            true,
        )
        .map(Some)
    }

    /// Queue for the agent's slot again, before the agent is told the result.
    fn take_agent_slot_back(&self) {
        self.d.admit();
        let needs = crate::sched::needs_of(&self.agent_uses, Some(&self.account));
        if let Ok(l) = self.acquire(needs, &self.step, false) {
            *self.lease.lock().unwrap_or_else(|e| e.into_inner()) = Some(l);
        }
    }
}

/// The step a `run_check` is ranked as: the agent's own, never the test step it
/// runs the commands of (see `CheckSlots::swap_for`).
fn check_rank_step<'a>(agent_step: &'a str, _test_step: Option<&'a str>) -> &'a str {
    agent_step
}

/// What a check says when its wait for a slot was ended.
const STEP_OVER: &str = "The step is over; the check did not run.";

/// What a `run_check` says when the owner skipped the task's testing (s264).
fn not_run_skipped(debt: u64) -> CheckRun {
    CheckRun::Done {
        passed: true,
        summary: format!(
            "Not run: the owner skipped testing for this task (debt #{debt}); the tests run \
             once on the base after it lands. Nothing was built or tested, so this says \
             nothing about your change: finish it and submit."
        ),
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
    weakening: Option<WeakeningCheck>,
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
        let dirty =
            git::run(&wt, &["--no-optional-locks", "status", "--porcelain"]).unwrap_or_default();
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
        // The scan landing runs, now, before the test step. Where the project sends a
        // weakening back to development, the submission is refused with each finding; where
        // it holds for the owner, the submission is accepted and the findings are kept as a
        // note (landing holds it).
        if let Some(w) = &weakening {
            if let Ok((found, notes)) =
                crate::selfguard::submission_scan(&wt, &base_ref, &w.base, "HEAD")
            {
                if !found.is_empty() {
                    match w.holds {
                        crate::config::Holds::Review => {
                            for f in &found {
                                problems.push(format!(
                                    "the change weakens a check, which MASON does not land: {}. Restore it, or reach the same goal without weakening it",
                                    crate::selfguard::describe_finding(f)
                                ));
                            }
                        }
                        crate::config::Holds::Owner => {
                            let list: Vec<String> = found
                                .iter()
                                .map(crate::selfguard::describe_finding)
                                .collect();
                            let _ = std::fs::write(
                                &w.note_file,
                                format!(
                                    "Findings the owner will be asked about at landing:\n{}\n",
                                    list.join("\n")
                                ),
                            );
                        }
                    }
                }
                let _ = notes; // lines in files the change adds: not a finding
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    })
}

/// Wrap `inner` so a `submit_change` whose head tree was red on the test step for this
/// command, and has not changed since, is refused with the failing tests and the run.
fn refuse_known_red(inner: Validator, d: Arc<Daemon>, wt: PathBuf, key: String) -> Validator {
    Box::new(move |tool, args| {
        let mut problems = inner(tool, args).err().unwrap_or_default();
        if tool == "submit_change" {
            if let Ok(tree) = git::run(&wt, &["rev-parse", "HEAD^{tree}"]) {
                if let Some(why) = crate::cmdreuse::known_red(&d, tree.trim(), &key) {
                    problems.push(why);
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

/// What `submit_change` needs to run the weakening scan: the project's base branch, and what
/// its `[self] holds` says to do with a finding.
struct WeakeningCheck {
    base: String,
    holds: crate::config::Holds,
    note_file: PathBuf,
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
    // The last round the test step will give runs on the stronger model, in a fresh session that
    // is shown every earlier round's feedback (s251).
    let escalate_to: Option<String> = if kind == StepKind::Develop {
        let agent = c.project.agent();
        match (agent.escalate_model, c.test_rounds_done()) {
            (Some(m), Some((done, max))) if last_round_ahead(done, max) => Some(m),
            _ => None,
        }
    } else {
        None
    };
    if escalate_to.is_some() {
        if let Ok(h) = std::fs::read_to_string(dir.join(FEEDBACK_HISTORY)) {
            if !h.trim().is_empty() {
                let note = format!(
                    "This is your last round: another red test round fails the task. This session starts fresh; the feedback of every earlier round follows.\n\n{h}"
                );
                feedback = Some(match feedback {
                    Some(f) if !f.trim().is_empty() => format!("{f}\n\n{note}"),
                    _ => note,
                });
            }
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
        // A base that cannot be read right now is waited for; it must not look like
        // a base with no numbered files.
        if let Err(why) = wait_out_environment(c, "reading the base's numbered files", || {
            crate::numbers::base_paths(&c.project)
        }) {
            return c.fail(why);
        }
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
    let tests_cmd = test_commands(&c.d, &c.def, c.project_id, Some(c.task));
    let reuse_key = tests_cmd
        .as_ref()
        .map(|t| crate::cmdreuse::command_key(t.build.as_deref(), &t.test));
    let deadline_extra = mason_adapter_api::DeadlineExtra::default();
    let checks = Box::new(TestChecks {
        cmd: tests_cmd,
        wt: c.wt.clone(),
        timeout: c.project.test_timeout,
        slots: CheckSlots::new(c),
        extra: deadline_extra.clone(),
        id_prefix: format!("t{}r{}-check", c.task.0, c.run.0),
        log_dir: c.task_dir(),
        suites: DevSuites::of(&c.def, c.project.base_ref()),
        task: c.task.0,
        run: c.run.0,
        base_fold: (kind == StepKind::Develop).then(|| (c.project_id, c.project.base_ref())),
    });
    let validator = (kind == StepKind::Develop).then(|| {
        change_validator(
            c.wt.clone(),
            c.project.base_ref(),
            c.branch.clone(),
            c.d.caps_for(&c.project),
            c.project.self_cfg.as_ref().map(|s| WeakeningCheck {
                base: c.project.base.clone(),
                holds: s.holds,
                note_file: c
                    .task_dir()
                    .join(format!("weakening-note-r{}.txt", c.run.0)),
            }),
        )
    });
    // A head whose tree was red on the test step, unchanged since, is not submitted again.
    let validator = match (validator, reuse_key) {
        (Some(v), Some(key)) => Some(refuse_known_red(v, c.d.clone(), c.wt.clone(), key)),
        (v, _) => v,
    };
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
    spec.deadline_extra = deadline_extra;
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
        escalated: std::cell::Cell::new(false),
    };
    spec.model = launched.model.clone();
    if let Some(m) = escalate_to.filter(|m| spec.model.as_deref() != Some(m.as_str())) {
        *rec.moved_from.borrow_mut() = spec.model.clone().unwrap_or_default();
        rec.escalated.set(true);
        spec.model = Some(m);
    }
    spec.effort = launched.effort.clone();
    {
        let (d, run) = (c.d.clone(), c.run.0);
        spec.on_model = Some(Arc::new(move |m| {
            let _ = d.store.set_reported_model(run, m);
        }));
    }
    {
        let (d, acct, task) = (c.d.clone(), c.project.account.clone(), c.task.0);
        spec.on_live = Some(Arc::new(move || crate::holds::probe_ok(&d, &acct, task)));
    }
    spec.env = c.project.agent_env.clone();
    // Whoever the agent runs `mason run` for: it ranks the request as this
    // task's pipeline work. Set last so a project's agent_env cannot name
    // another task.
    spec.env.retain(|(k, _)| k != "MASON_TASK");
    spec.env.push(("MASON_TASK".into(), c.task.0.to_string()));
    spec.env.retain(|(k, _)| k != "MASON_TASK_CACHE");
    spec.env.push((
        "MASON_TASK_CACHE".into(),
        crate::taskcache::own_dir(&c.d, c.task.0),
    ));
    spec.tool_timeout = tool_timeout_for(kind, &c.project);
    spec.deadline = Some(
        c.res
            .budget
            .as_ref()
            .map_or(c.project.step_timeout, |a| a.value),
    );
    // A `run_check` may queue for a long time before it runs, so no tool call is
    // cut off before the step's own deadline would have ended it.
    if kind == StepKind::Develop {
        spec.tool_timeout = spec.tool_timeout.max(spec.deadline.unwrap_or_default());
    }
    spec.run_key = Some(key.clone());
    spec.tools = c.step.tools.clone();
    spec.idle = Some(mason_adapter_api::IdlePolicy {
        alert_after: c.project.agent_idle_alert,
        stop_after: c.project.agent_idle_stop,
        report: {
            let (d, run, task, step) = (c.d.clone(), c.run.0, c.task.0, c.step.name.clone());
            Arc::new(move |ms| {
                let mut m = d.silent.lock().unwrap_or_else(|e| e.into_inner());
                match ms {
                    Some(silent_ms) => {
                        m.insert(
                            run,
                            crate::daemon::SilentAgent {
                                task,
                                step: step.clone(),
                                silent_ms,
                            },
                        );
                    }
                    None => {
                        m.remove(&run);
                    }
                }
            })
        },
    });
    {
        let (d, run) = (c.d.clone(), c.run);
        spec.progress = Some(Arc::new(move |stream, off, bytes| {
            d.progress_observe(run, stream, off, bytes)
        }));
    }
    {
        let (d, run) = (c.d.clone(), c.run);
        let api = c.d.accounts.iter().any(|a| {
            a.name == c.project.account && matches!(a.billing, crate::providers::Billing::Api)
        });
        spec.on_tokens = Some(Arc::new(move |stream, t| {
            d.progress_tokens(
                run,
                stream,
                mason_core::progress::Tokens {
                    input: t.input,
                    output: t.output,
                    cache_read: t.cache_read,
                    cache_write: t.cache_create,
                    // Spend belongs to an api account (decision 0043).
                    cost_nano_usd: t
                        .cost_usd
                        .filter(|_| api)
                        .map(mason_core::progress::Tokens::cost_nano),
                },
            )
        }));
    }
    // The agent's counts, todo list and tool calls reach the live record as they happen.
    c.d.progress_agent(c.run);
    {
        let (d, run) = (c.d.clone(), c.run);
        contract.set_on_progress(Arc::new(move |r| d.progress_agent_report(run, r)));
        let (d, run) = (c.d.clone(), c.run);
        spec.on_agent_event = Some(Arc::new(move |ev| d.progress_agent_event(run, ev)));
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
    let Some(mut report) = settle_all(c, &runner, first, &spec, &contract, &rec) else {
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
        // Held meanwhile: wait for the account before another session starts.
        if get_lease(c).is_err() {
            return;
        }
        rec.write(c, "retry", &spec);
        let again = runner.run(spec.clone(), &contract);
        let Some(r) = settle_all(c, &runner, again, &spec, &contract, &rec) else {
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
    /// The model was raised for the task's last test round (s251).
    escalated: std::cell::Cell<bool>,
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
            } else if self.escalated.get() {
                "escalated for the last round (escalate_model)".into()
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
        let account = &c.project.account;
        // The snapshot first: it may carry the reset the text lacks.
        if let Some(u) = &report.usage {
            crate::usage::record_snapshot(&c.d, account, u);
        }
        // The account is held, not just this task; if this was the probe it is
        // refused and backs off further.
        crate::holds::observe_end(&c.d, account, c.task.0, reason);
        crate::holds::session_ended(&c.d, account, c.task.0, true);
        // The session slot goes back while the account waits.
        c.release_lease();
        if get_lease(c).is_err() {
            return None;
        }
        rec.write(c, "resume", spec);
        report = runner.resume(report, hold_note(), spec.clone(), contract);
    }
    crate::holds::session_ended(&c.d, &c.project.account, c.task.0, false);
    Some(report)
}

/// Does this crash say the session's transcript is gone, so it cannot be resumed?
fn transcript_gone(detail: &str) -> bool {
    let d = detail.to_lowercase();
    [
        "no conversation found",
        "no rollout found",
        "session not found",
        "could not find session",
    ]
    .iter()
    .any(|m| d.contains(m))
}

/// Holds, then lost hosts, until the step has an end that is neither. A lost host
/// (or one that never came up) is not a failure: it is recorded, the lease taken
/// again, and the step continued, with `resume` and a note when the journal gave a
/// session id and from the start when not. At most [`hl::MAX_AGENT_RESTARTS`] times
/// per step run, counted from the invocation directories so a daemon restart does
/// not reset it; one more loss ends the step, naming them all. A resume whose
/// transcript is gone starts fresh, once. `None` when the task went away meanwhile.
fn settle_all(
    c: &Ctx,
    runner: &StepRunner,
    first: StepReport,
    spec: &SessionSpec,
    contract: &ContractHandle,
    rec: &LaunchRec,
) -> Option<StepReport> {
    use crate::hostloss as hl;
    let root = c.d.cfg.hosts_dir();
    let base = format!("t{}r{}", c.task.0, c.run.0);
    let describe = |id: &str| -> (String, u64) {
        if let Some(note) = mason_adapters::hosted::silent_stop(&root.join(id)) {
            return (note, now_ms());
        }
        match mason_step_host::registry::lost_report(&root.join(id)) {
            Ok(r) => (r.to_string(), if r.at_ms > 0 { r.at_ms } else { now_ms() }),
            Err(_) => (format!("host {id} never came up"), now_ms()),
        }
    };
    let mut report = settle_holds(c, runner, first, spec, contract, rec)?;
    let mut resumed = false;
    let mut fresh_after_gone = false;
    let mut silent = false;
    loop {
        let fresh = match &report.end {
            StepEnd::Failed(FailReason::HostLost { detail }) => {
                let lost = mason_adapters::hosted::lost_invocations(&root, &base);
                let newest = lost.last().cloned().unwrap_or_else(|| base.clone());
                let (text, at) = describe(&newest);
                let text = if lost.is_empty() {
                    detail.clone()
                } else {
                    text
                };
                // A host the daemon's own restart took is not the task's failure: it
                // is recorded but not counted toward the restart budget.
                let counted = lost
                    .iter()
                    .filter(|id| {
                        let r = mason_step_host::registry::lost_report(&root.join(id));
                        !r.is_ok_and(|r| hl::is_restart_loss(&root, r.last_heartbeat_ms, r.at_ms))
                    })
                    .count();
                let restart = counted <= hl::MAX_AGENT_RESTARTS;
                hl::record_as(&c.d, Some(c.run), &c.step.name, &newest, at, text, restart);
                if !restart {
                    let all: Vec<String> = lost.iter().map(|id| describe(id).0).collect();
                    report.end = StepEnd::Failed(FailReason::HostLost {
                        detail: hl::agent_lost(&all),
                    });
                    return Some(report);
                }
                // Only what is verifiably the recorded process: the CLI the dead
                // host left running must not work beside its replacement.
                tracing::warn!(host = %newest, "reaping a lost host before its replacement");
                mason_step_host::registry::reap_dir(&root.join(&newest));
                silent = mason_adapters::hosted::silent_stop(&root.join(&newest)).is_some();
                c.release_lease();
                if get_lease(c).is_err() {
                    return None;
                }
                report.session_id.is_none()
            }
            StepEnd::Failed(FailReason::Crash { detail })
                if resumed && !fresh_after_gone && transcript_gone(detail) =>
            {
                fresh_after_gone = true;
                true
            }
            _ => return Some(report),
        };
        if fresh {
            // What the lost session used is kept; the new one starts its own totals.
            record_usage(c, spec, &report);
            rec.write(c, "launch", spec);
            contract.clear_pending();
            report = runner.run(spec.clone(), contract);
            resumed = false;
        } else {
            rec.write(c, "resume", spec);
            let note = if std::mem::take(&mut silent) {
                hl::silent_note()
            } else {
                hl::interrupted_note()
            };
            report = runner.resume(report, note, spec.clone(), contract);
            resumed = true;
        }
        report = settle_holds(c, runner, report, spec, contract, rec)?;
    }
}

fn finish_agent(c: &Ctx, report: StepReport, dir: &Path) {
    // What the agent noticed outside its task (Part 13): filed before the step's own result
    // is recorded, so a proposal that blocks the task is already a task when it waits.
    if !report.proposals.is_empty() {
        crate::proposals::file_from_report(&c.d, c.task, c.project_id, &report.proposals, dir);
    }
    let asked = match &report.end {
        StepEnd::Asked { question } => Some(question.clone()),
        _ => None,
    };
    finish_agent_inner(c, report, dir);
    // The analyst's findings (Part 14), read from the file it wrote in its worktree.
    crate::introspect::ingest_analyst(&c.d, c.task, &c.project.id, &c.title, &c.wt, dir);
    // Triage (off unless the project turns it on) only records what it would do; the
    // question is the owner's either way.
    if let Some(q) = asked {
        let ask = mason_adapter_api::card::Card::from_json(&q)
            .map(|card| card.ask)
            .unwrap_or_else(|| q.as_str().unwrap_or_default().to_string());
        crate::introspect::triage_question(&c.d, &c.project, c.task, &ask);
    }
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
                    // What landing's dropped-line audit reads: the lines removed on purpose.
                    let _ = std::fs::write(dir.join("change.json"), submission.to_string());
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
                // Restarted until it could not be (see `settle_all`); the text names the losses.
                FailReason::HostLost { detail } => detail,
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
                return c.conflict(
                    format!(
                        "the branch conflicts with {} in: {}",
                        c.project.base_ref(),
                        files.replace('\n', ", ")
                    ),
                    note,
                );
            }
            Err(why) => return c.fail(why),
        }
    } else if let Err(why) = prepare_worktree_only(c) {
        return c.fail(why);
    }
    let suffix = if is_test { "test" } else { "cmd" };
    let what = format!("`{}`", c.step.name);
    // The folded tree already went green for this command (a `run_check`, an earlier test
    // step, landing's re-test): nothing new to prove. Exact tree and exact command only.
    let reuse = is_test.then(|| {
        (
            crate::cmdreuse::content_tree(&c.wt),
            crate::cmdreuse::command_key(builds_first(c).as_deref(), &cmd),
        )
    });
    if let Some((Some(tree), key)) = &reuse {
        if let Some(src) = crate::cmdreuse::green_for(&c.d, tree, key) {
            let note =
                format!("green for this exact tree already ({src}); {what} did not run again");
            tracing::info!(task = c.task.0, run = c.run.0, "{note}");
            let _ = std::fs::write(
                c.task_dir().join(format!("{suffix}-r{}.reused", c.run.0)),
                &note,
            );
            c.d.progress_report(c.run, |p, now| {
                p.apply_report(
                    &mason_core::progress::Report {
                        message: Some(note.clone()),
                        ..Default::default()
                    },
                    now,
                )
            });
            return c.green();
        }
    }
    let record = |green: bool, names: &[String]| {
        if let Some((Some(tree), key)) = &reuse {
            let src = format!("test step of task #{}, run {}", c.task.0, c.run.0);
            if green {
                crate::cmdreuse::record_green(&c.d, tree, key, &src);
            } else {
                crate::cmdreuse::record_red(&c.d, tree, key, &src, names);
            }
        }
    };
    // A test step with a build builds first, holding one `compile` slot, and
    // lets it go before the tests run: they hold what the step uses without
    // `compile`, so a long test run leaves the build slot to the next task.
    let held = match builds_first(c) {
        None => c.step.uses.clone(),
        Some(build) => {
            let uses = build_uses();
            if !started(c, "build", 0) {
                match acquire_needs(c, crate::sched::needs_of(&uses, None)) {
                    Ok(l) => *c.lease.lock().unwrap_or_else(|e| e.into_inner()) = Some(l),
                    Err(_) => return,
                }
            }
            let built = run_command_once(c, &build, "build", 0, &uses, &format!("{what}'s build"));
            c.release_lease();
            match built {
                Err(why) => return c.fail(why),
                Ok(CommandRun {
                    verdict:
                        RunVerdict::Red {
                            reason, feedback, ..
                        },
                    ..
                }) => return c.conclude(Outcome::Red, reason, Some(feedback)),
                Ok(_) => {}
            }
            let uses = tests_uses(&c.step.uses);
            let needs = crate::sched::needs_of(&uses, None);
            if (needs.exclusive || !needs.amounts.is_empty()) && !started(c, suffix, 0) {
                match acquire_needs(c, needs) {
                    Ok(l) => *c.lease.lock().unwrap_or_else(|e| e.into_inner()) = Some(l),
                    Err(_) => return,
                }
            }
            uses
        }
    };
    let reruns = if is_test { c.project.test_reruns } else { 0 };
    let filter = c.project.rerun_filter.as_deref();
    let mut earlier: Vec<String> = vec![];
    // The tests that failed in every run so far (None: the first run's names were not
    // readable, or a later red's were not, so nothing is compared and a rerun runs it all),
    // and every test that failed in any run.
    let mut persistent: Option<Vec<String>> = None;
    let mut seen: Vec<String> = vec![];
    let mut this_cmd = cmd.clone();
    // Set when a filtered rerun ran none of the tests it was given: run it all.
    let mut filter_off = false;
    for k in 0..=reruns {
        let run = match run_command_once(c, &this_cmd, suffix, k, &held, &what) {
            Ok(r) => r,
            Err(why) => return c.fail(why),
        };
        let log = run.log.clone();
        match run.verdict {
            RunVerdict::Green => {
                if this_cmd != cmd {
                    let out = read_whole_log(&log).unwrap_or_default();
                    let names = persistent.clone().unwrap_or_default();
                    if !crate::rerun::ran_them(&out, &names) {
                        // It exited green without running what failed: nothing is proved.
                        earlier.push(format!(
                            "run {}: the filtered rerun ran none of {}",
                            k + 1,
                            names.join(", ")
                        ));
                        if k < reruns {
                            filter_off = true;
                            this_cmd = cmd.clone();
                            continue;
                        }
                        return c.conclude(
                            Outcome::Red,
                            format!(
                                "{what} failed ({} runs, the filtered rerun ran none of the failed tests)",
                                k + 1
                            ),
                            Some(format!(
                                "{what} was red, and the rerun with `rerun_filter` ran none of the failing tests ({}), so it proved nothing:\n{}",
                                names.join(", "),
                                earlier.join("\n")
                            )),
                        );
                    }
                }
                let _ = std::fs::remove_file(c.task_dir().join("feedback.txt"));
                if k > 0 {
                    record_flaky(c, &seen, &[]);
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
                // Only the whole command's green counts: a filtered rerun proved less.
                if this_cmd == cmd {
                    record(true, &[]);
                }
                return c.green();
            }
            // Red by its exit status alone: worth running again, and worth reading.
            RunVerdict::Red {
                reason,
                feedback,
                rerun: true,
            } => {
                let names = crate::rerun::failing_names(&read_whole_log(&log).unwrap_or_default());
                let last_names = names.clone();
                if k == 0 {
                    if !names.is_empty() {
                        persistent = Some(names.clone());
                    }
                } else if names.is_empty() {
                    persistent = None;
                } else if let Some(p) = persistent.as_mut() {
                    p.retain(|n| names.contains(n));
                }
                if persistent.is_some() {
                    for n in names {
                        if !seen.contains(&n) {
                            seen.push(n);
                        }
                    }
                }
                // No test failed in two runs: a flake, not this change.
                if persistent.as_ref().is_some_and(|p| p.is_empty()) {
                    let _ = std::fs::remove_file(c.task_dir().join("feedback.txt"));
                    record_flaky(c, &seen, &[]);
                    earlier.push(format!("run {}: {reason}\n{feedback}", k + 1));
                    let note = format!(
                        "`{}` was red, but no test failed in two runs (flaky, not this change): {}\n{}",
                        c.step.name,
                        seen.join(", "),
                        earlier.join("\n")
                    );
                    tracing::warn!(task = c.task.0, run = c.run.0, "{note}");
                    let _ = std::fs::write(
                        c.task_dir().join(format!("{suffix}-r{}.flaky", c.run.0)),
                        &note,
                    );
                    return c.green();
                }
                if k < reruns {
                    earlier.push(format!("run {}: {reason}\n{feedback}", k + 1));
                    this_cmd = if filter_off {
                        cmd.clone()
                    } else {
                        crate::rerun::command(&cmd, filter, persistent.as_deref().unwrap_or(&[]))
                    };
                    use std::io::Write as _;
                    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&log) {
                        let _ = writeln!(
                            f,
                            "\n== MASON: red ({reason}); running {} again ({} of {reruns}) to tell a flaky test from a broken change",
                            if this_cmd == cmd {
                                "it"
                            } else {
                                "only the failed tests"
                            },
                            k + 1
                        );
                    }
                    continue;
                }
                // Red on every run: a test in the known-red ledger, or one that also fails at
                // the merge base, is not this change's (s177, s281). Each is told apart on its
                // own: only the others fail the step.
                let mut not_yours: Vec<String> = vec![];
                // Tests excused above (by the ledger or the base), and those that are left.
                let mut excused_names: Vec<String> = vec![];
                let mut rest_new: Vec<String> = vec![];
                let mut exhausted_why: Option<String> = None;
                if is_test {
                    if let Some(p) = persistent.as_ref().filter(|p| !p.is_empty()) {
                        let known = crate::knownred::known_for(&c.d.store, &c.step.name);
                        let (mut rest, hit) = crate::knownred::split(p, &known);
                        not_yours.extend(crate::knownred::lines(&hit));
                        excused_names.extend(hit.iter().map(|k| k.test.clone()));
                        if !not_yours.is_empty() {
                            let _ = std::fs::write(
                                c.task_dir()
                                    .join(format!("{suffix}-r{}-known.excused", c.run.0)),
                                not_yours.join("\n"),
                            );
                        }
                        if !rest.is_empty() {
                            let base_cmd = if filter_off {
                                cmd.clone()
                            } else {
                                crate::rerun::command(&cmd, filter, &rest)
                            };
                            if let Some(e) = crate::basecheck::excuse(
                                c,
                                suffix,
                                builds_first(c).as_deref(),
                                &base_cmd,
                                &rest,
                            ) {
                                rest.retain(|t| !e.tests.contains(t));
                                excused_names.extend(e.tests);
                                not_yours.extend(e.lines);
                            }
                        }
                        rest_new = rest.clone();
                        if rest.is_empty() && !not_yours.is_empty() {
                            let note = format!(
                                "`{}` was red, but every test that failed on every run is not this change's (known red, or also failing at the merge base; excused):\n{}",
                                c.step.name,
                                not_yours.join("\n")
                            );
                            record_flaky(c, &seen, p);
                            let _ = std::fs::remove_file(c.task_dir().join("feedback.txt"));
                            tracing::warn!(task = c.task.0, run = c.run.0, "{note}");
                            use std::io::Write as _;
                            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&log) {
                                let _ = writeln!(f, "\n== MASON: {note}");
                            }
                            return c.green();
                        }
                    }
                }
                // What failed on every run and is still the change's (s282): kept for the last
                // round, counted across tasks, and on the last round run at the merge base.
                if is_test && !rest_new.is_empty() {
                    crate::exhausted::record_round(c, &rest_new);
                    crate::exhausted::record_persistent(c, &c.step.name, &rest_new);
                    if crate::exhausted::is_last_round(c) {
                        let build = builds_first(c);
                        let cmd_for = |t: &[String]| {
                            if filter_off {
                                cmd.clone()
                            } else {
                                crate::rerun::command(&cmd, filter, t)
                            }
                        };
                        let ex = crate::exhausted::on_last_round(
                            c,
                            &c.step.name,
                            build.as_deref(),
                            &cmd_for,
                            &rest_new,
                        );
                        exhausted_why = ex.why;
                        if !ex.excused.is_empty() {
                            excused_names.extend(ex.excused);
                            not_yours.extend(ex.lines);
                            let left: Vec<&String> = last_names
                                .iter()
                                .filter(|t| !excused_names.contains(t))
                                .collect();
                            if left.is_empty() && exhausted_why.is_none() {
                                let note = format!(
                                    "`{}` was still red on the last round, but what failed every time is red at the merge base too (now in the known-red ledger, fix filed); the step goes on:\n{}",
                                    c.step.name,
                                    not_yours.join("\n")
                                );
                                record_flaky(c, &seen, &excused_names);
                                let _ = std::fs::remove_file(c.task_dir().join("feedback.txt"));
                                tracing::warn!(task = c.task.0, run = c.run.0, "{note}");
                                use std::io::Write as _;
                                if let Ok(mut f) =
                                    std::fs::OpenOptions::new().append(true).open(&log)
                                {
                                    let _ = writeln!(f, "\n== MASON: {note}");
                                }
                                return c.green();
                            }
                        }
                    }
                }
                // Red on every run, by exit status, with the failing tests named: this tree is
                // known red. (A timeout, a signal or a red with no names read is never recorded.)
                if let Some(p) = persistent.as_ref().filter(|p| !p.is_empty()) {
                    record(false, p);
                }
                let (reason, feedback) = match &persistent {
                    Some(p) if k > 0 => {
                        record_flaky(c, &seen, p);
                        (
                            format!(
                                "{reason} ({} runs; failed every time: {})",
                                k + 1,
                                p.join(", ")
                            ),
                            format!(
                                "{feedback}\n\nThese tests failed on all {} runs: {}",
                                k + 1,
                                p.join(", ")
                            ),
                        )
                    }
                    _ if k > 0 => (
                        format!("{reason} ({} runs, red every time)", k + 1),
                        format!("{feedback}\n\nIt was red on all {} runs.", k + 1),
                    ),
                    _ => (reason, feedback),
                };
                let feedback = if not_yours.is_empty() {
                    feedback
                } else {
                    format!(
                        "{feedback}\n\nNot yours (do not work on these; the others above are yours):\n{}",
                        not_yours.join("\n")
                    )
                };
                let reason = match &exhausted_why {
                    Some(w) => format!("{reason}; {w}"),
                    None => reason,
                };
                let feedback = match &exhausted_why {
                    Some(w) => format!("{feedback}\n\nChecked at the base: {w}"),
                    None => feedback,
                };
                return c.conclude(Outcome::Red, reason, Some(feedback));
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

/// A test step's whole log as text, if it is small enough to read: a log cut short could
/// hide a failing name, and a name that is not seen must not make a flake.
fn read_whole_log(path: &Path) -> Option<String> {
    let len = std::fs::metadata(path).ok()?.len();
    if len > 32 << 20 {
        return None;
    }
    std::fs::read(path)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Record each of `tests` that failed in some run but not in every run (`except`) as a
/// flake, as the suites path does; the step's name stands for the suite.
fn record_flaky(c: &Ctx, tests: &[String], except: &[String]) {
    for t in tests.iter().filter(|t| !except.contains(t)) {
        let _ =
            c.d.store
                .record_flaky(&c.step.name, t, c.task.0, crate::daemon::now_ms());
    }
    let flaked: Vec<String> = tests
        .iter()
        .filter(|t| !except.contains(t))
        .cloned()
        .collect();
    crate::autoquarantine::consider(&c.d, c.project_id, &c.step.name, &flaked);
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

/// Why one hosted run did not give a result.
enum Unfinished {
    /// The host was lost with no exit record: nothing was judged.
    Lost(mason_step_host::registry::LostReport),
    Other(String),
}

impl From<String> for Unfinished {
    fn from(s: String) -> Self {
        Unfinished::Other(s)
    }
}

/// Run the command once (the `k`th time). If its host is lost before the command
/// finished, run it once more on a fresh host (id ends `-host1`) and judge that
/// run; a second loss in a row fails the step and names both. A restarted daemon
/// finds the run already started, on whichever host it was.
/// Has the `k`th run of `suffix` already started (a daemon that restarted
/// finds it)? Collecting it starts nothing, so it has nothing to queue for.
fn started(c: &Ctx, suffix: &str, k: u32) -> bool {
    let id = format!("t{}r{}-{suffix}{}", c.task.0, c.run.0, again(k));
    c.d.cfg.hosts_dir().join(id).join("host.kv").exists()
}

/// One run of a step's command, `what` in what it says (`` `test` ``, or
/// `` `test`'s build ``), told that it holds `held`.
fn run_command_once(
    c: &Ctx,
    cmd: &str,
    suffix: &str,
    k: u32,
    held: &std::collections::BTreeMap<String, u32>,
    what: &str,
) -> Result<CommandRun, String> {
    use crate::hostloss as hl;
    let id = format!("t{}r{}-{suffix}{}", c.task.0, c.run.0, again(k));
    let hosts = c.d.cfg.hosts_dir();
    let rerun = hl::rerun_id(&id);
    let mut first = None;
    let mut host = 0;
    if hosts.join(&rerun).join("host.kv").exists() {
        host = 1;
        first = mason_step_host::registry::lost_report(&hosts.join(&id)).ok();
    }
    loop {
        match run_command_on(c, cmd, suffix, k, host, held, what) {
            Ok(r) => return Ok(r),
            Err(Unfinished::Other(e)) => return Err(e),
            Err(Unfinished::Lost(rep)) if host == 0 => {
                hl::record(&c.d, Some(c.run), &c.step.name, &rep, true);
                let note = format!(
                    "\n== MASON: the step host was lost ({rep}); running `{}` again on a fresh host. A lost host is not a result: only the completed re-run is judged.\n",
                    c.step.name
                );
                let log = c
                    .task_dir()
                    .join(format!("{suffix}-r{}{}.log", c.run.0, again(k)));
                use std::io::Write as _;
                if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&log) {
                    let _ = f.write_all(note.as_bytes());
                }
                first = Some(rep);
                host = 1;
            }
            Err(Unfinished::Lost(rep)) => {
                hl::record(&c.d, Some(c.run), &c.step.name, &rep, false);
                return Err(match first {
                    Some(f) => hl::both(&f, &rep),
                    None => hl::lost_text(&rep),
                });
            }
        }
    }
}

/// The command as it runs when `pty` is set: on macOS under `script`, which gives it a
/// pseudo-terminal for stdout and stderr (so `xctest` writes line by line) and passes
/// its exit status through. Elsewhere, or without `pty`, the command unchanged.
pub(crate) fn pty_wrap(pty: bool, cmd: &str) -> String {
    if pty && cfg!(target_os = "macos") {
        format!("script -q /dev/null sh -c '{}'", cmd.replace('\'', "'\\''"))
    } else {
        cmd.to_string()
    }
}

/// One run of the command on host number `host` (0 the first, 1 the fresh one),
/// or collect the run a restarted daemon finds already started.
fn run_command_on(
    c: &Ctx,
    cmd: &str,
    suffix: &str,
    k: u32,
    host: u32,
    held: &std::collections::BTreeMap<String, u32>,
    what: &str,
) -> Result<CommandRun, Unfinished> {
    let hsuf = if host == 0 { "" } else { "-host1" };
    let id = format!("t{}r{}-{suffix}{}{hsuf}", c.task.0, c.run.0, again(k));
    let hosts = c.d.cfg.hosts_dir();
    let dir = hosts.join(&id);
    let limit = c
        .res
        .budget
        .as_ref()
        .map_or(c.project.test_timeout, |a| a.value);
    if !dir.join("host.kv").exists() {
        let _ = std::fs::remove_file(result_file(c));
        let _ = std::fs::remove_file(progress_file(c));
        let mut env = step_env_pairs(c);
        for (k, v) in env.iter_mut() {
            if k == "MASON_HELD" {
                *v = held_text(held);
            }
        }
        let o = SpawnOptions {
            host_bin: c.d.cfg.host_bin.clone(),
            root: hosts.clone(),
            id: id.clone(),
            cmd: vec!["sh".into(), "-c".into(), pty_wrap(c.step.pty, cmd)],
            cwd: Some(c.wt.clone()),
            env,
            clear_env: false,
            stdin: None,
            linger: Duration::from_secs(600),
            heartbeat: c.d.cfg.heartbeat,
        };
        if let Err(e) = spawn_host(&o) {
            return Err(format!("cannot start the command: {e}").into());
        }
    }
    // Stop the run if it takes too long.
    let done = Arc::new(AtomicBool::new(false));
    {
        let (done, sock, hdir) = (done.clone(), dir.join("sock"), dir.clone());
        std::thread::spawn(move || {
            let end = Instant::now() + mason_step_host::registry::remaining_limit(limit, &hdir);
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
    let name = format!("{suffix}-r{}{}{hsuf}", c.run.0, again(k));
    let log = c.task_dir().join(format!("{name}.log"));
    // Silence, judged on the whole tree: continue what is stopped, kill what is stuck.
    let stall = crate::watch::Watch::start(
        &c.d,
        &id,
        log.clone(),
        crate::watch::Thresholds::new(c.project.cont_after, c.project.kill_after),
        {
            let (d, run, what) = (c.d.clone(), c.run, c.step.name.clone());
            move |text| {
                let text = format!("{what}: {text}");
                d.progress_report(run, |p, now| p.set_message(&text, now))
            }
        },
    );
    // Lines and bytes of the output, counted as they arrive. A run found again after a
    // restart first counts what its log holds beyond what the stored record had.
    // Each run of the command is its own stream, since each log starts at 0.
    let phase = if suffix == "build" { "build" } else { "" };
    let stream = format!("{phase}{}{hsuf}", again(k));
    if c.step.pty {
        c.d.progress_terminal(c.run, &stream);
    }
    c.d.progress_catch_up(c.run, &stream, &log);
    // What the command says about itself in `MASON_PROGRESS_FILE`, as it says it.
    let _tail = {
        let (d, run) = (c.d.clone(), c.run);
        crate::progress::FileTail::start(progress_file(c), move |r| {
            d.progress_report(run, |p, now| p.apply_report(&r, now))
        })
    };
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
            return Err(match mason_step_host::registry::lost_report(&dir) {
                Ok(rep) => Unfinished::Lost(rep),
                Err(_) => Unfinished::Other("the command was lost (its host died)".into()),
            })
        }
        Err(e) => return Err(format!("cannot collect the command's result: {e}").into()),
    };
    if let Some(why) = stall.as_ref().and_then(|w| w.stalled()) {
        let output = tail(&read_tail_lossy(&log).unwrap_or_default(), 60);
        return Ok(CommandRun {
            verdict: RunVerdict::Red {
                reason: format!("{what} failed ({why})"),
                feedback: format!("{what} failed ({why}). Last output:\n{output}"),
                rerun: false,
            },
            log,
        });
    }
    // A script may say more than its exit status in a JSON result file.
    let script = match std::fs::read_to_string(result_file(c)) {
        Ok(t) => match read_script_result(&t) {
            Ok(r) => Some(r),
            Err(e) => {
                return Err(format!("{what} wrote a result file that cannot be used: {e}").into())
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
                    reason: format!("{what} failed: {why}"),
                    feedback: format!("{what} failed: {why}"),
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
    let output = tail(&read_tail_lossy(&log).unwrap_or_default(), 60);
    let how = if rec.signal != 0 {
        format!("killed by signal {}", rec.signal)
    } else {
        format!("exit status {}", rec.code)
    };
    Ok(CommandRun {
        verdict: RunVerdict::Red {
            reason: format!("{what} failed ({how})"),
            feedback: format!("{what} failed ({how}). Last output:\n{output}"),
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
            class: Some("approval".into()),
        })
    {
        return;
    }
    // What the step waits on, and since when (its start), for the monitor to show.
    c.d.progress_report(c.run, |p, now| p.set_message("waiting for the owner", now));
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
    // An improvement of MASON's own code without write access: the branch goes to the owner's
    // fork, not to the repository (Part 13).
    if let Ok(Some(row)) = c.d.store.proposal(c.task.0) {
        if row.access == "fork" {
            return crate::improve::land_on_fork(c, &row);
        }
    }
    // Landing is batched per project: join the queue and wait for the verdict.
    let verdict = loop {
        let Some(v) = crate::landing::submit_and_wait(c) else {
            return; // approved, rejected, cancelled or retried while waiting
        };
        if matches!(v, crate::landing::Verdict::Held) {
            // The host refused the push's credentials: landing is held for the project. The task
            // stays here, and joins the queue again once the owner has chosen a key.
            if wait_while_paused(c).is_err() {
                return;
            }
            continue;
        }
        break v;
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
        crate::landing::Verdict::Conflict { reason, feedback } => c.conflict(reason, feedback),
        crate::landing::Verdict::Failed(why) => c.fail(format!("landing failed: {why}")),
        crate::landing::Verdict::Held => {}
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
            class: Some(FailClass::Code),
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
    let landed = t.state == State::Landed;
    {
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
    // The task's build directory goes only when it landed (the others can be revived), and on
    // a thread of its own, outside the lock: deleting gigabytes takes a while (s78).
    if landed {
        crate::taskcache::remove_in_background(d, p, task.0);
    }
    // The directory MASON owns (`MASON_TASK_CACHE`) goes when the task lands or is canceled.
    if landed || t.state == State::Canceled {
        crate::taskcache::remove_own_in_background(d, task.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- s251: the last round -------------------------------------------------------------

    #[test]
    fn the_last_round_is_the_one_before_another_red_would_fail_the_task() {
        // Three test rounds: rounds 1 and 2 are not it, the develop run before test round 3 is.
        assert!(!last_round_ahead(0, 3));
        assert!(!last_round_ahead(1, 3));
        assert!(last_round_ahead(2, 3));
        assert!(last_round_ahead(3, 3));
        // One round only: the first run is never "escalated", there is no earlier feedback.
        assert!(!last_round_ahead(0, 1));
        assert!(last_round_ahead(1, 1));
        assert!(last_round_ahead(1, 2));
        assert!(!last_round_ahead(0, 2));
    }

    // ---- s248: the small lane for scoped checks ------------------------------------------

    fn uses_of(pairs: &[(&str, u32)]) -> std::collections::BTreeMap<String, u32> {
        pairs.iter().map(|(k, n)| (k.to_string(), *n)).collect()
    }

    fn def_with(extra: &str) -> Definition {
        let text = format!(
            "schema = 1\nstart = \"develop\"\n{extra}\n[step.develop]\ntype = \"agent\"\n\
             category = \"developing\"\nagent = \"deep\"\nnext = \"test\"\n\
             [step.test]\ntype = \"test\"\ncategory = \"testing\"\nuses = {{ test-suite = 1 }}\n\
             next = \"land\"\n[step.land]\ntype = \"landing\"\ncategory = \"landing\"\n"
        );
        mason_definition::parse(&text).unwrap_or_else(|p| panic!("{p:?}"))
    }

    #[test]
    fn a_scoped_check_takes_test_small_when_declared() {
        let def = def_with("[resource.test-suite]\ncount = 2\n[resource.test-small]\ncount = 2\n");
        let u = small_lane(&def, uses_of(&[("test-suite", 1)]));
        assert_eq!(u, uses_of(&[("test-small", 1)]));
        // Other classes are untouched.
        let u = small_lane(&def, uses_of(&[("test-suite", 1), ("disk", 3)]));
        assert_eq!(u, uses_of(&[("test-small", 1), ("disk", 3)]));
    }

    #[test]
    fn a_scoped_check_keeps_test_suite_without_a_small_lane() {
        let def = def_with("[resource.test-suite]\ncount = 2\n");
        let u = small_lane(&def, uses_of(&[("test-suite", 1)]));
        assert_eq!(u, uses_of(&[("test-suite", 1)]));
    }

    #[test]
    fn a_develop_check_ranks_behind_a_task_already_in_test() {
        // On 2026-11-13 develop checks ranked as the test step tied with real test
        // steps, so priority and filing age decided, and four tasks in test waited
        // behind every older or higher-priority develop agent.
        let def = mason_definition::parse::parse(
            "schema = 1\nstart = \"develop\"\n\n[step.develop]\ntype = \"agent\"\n\
             category = \"developing\"\nnext = \"test\"\n\n[step.test]\ntype = \"test\"\n\
             category = \"testing\"\nnext = \"land\"\n\n[step.land]\ntype = \"landing\"\n\
             category = \"landing\"\n",
        )
        .unwrap();
        let steps = |s: &str| mason_definition::graph::steps_remaining(&def, s);
        let check = check_rank_step("develop", Some("test"));
        assert_eq!(check, "develop");
        // A high-priority develop check filed long ago, against a normal test step filed later.
        let develop_check = mason_scheduler::Rank::new(false, steps(check), 1, 100, 1, false);
        let test_step = mason_scheduler::Rank::new(false, steps("test"), 2, 900, 2, false);
        assert!(
            test_step < develop_check,
            "the task in test goes first: {test_step:?} vs {develop_check:?}"
        );
    }

    // ---- Task 180: the weakening scan at submit_change -----------------------------------

    fn sh(dir: &Path, args: &[&str]) {
        let o = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }

    /// A repo whose `main` holds `scripts/run.sh`, and a branch that appends `line` to it.
    fn submit_with(
        line: &str,
        holds: Option<crate::config::Holds>,
    ) -> (Result<(), Vec<String>>, PathBuf) {
        let dir = tempfile::tempdir().unwrap().keep();
        sh(&dir, &["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join("scripts/run.sh"), "echo hi\n").unwrap();
        sh(&dir, &["add", "-A"]);
        sh(&dir, &["commit", "-qm", "init"]);
        sh(&dir, &["checkout", "-q", "-b", "work"]);
        std::fs::write(
            dir.join("scripts/run.sh"),
            format!("echo hi\n{line}\n").replace("@OT@", concat!("|", "| true")),
        )
        .unwrap();
        sh(&dir, &["commit", "-qam", "work"]);
        let head = git::rev_parse(&dir, "HEAD").unwrap();
        let note = dir.join("note.txt");
        let v = change_validator(
            dir.clone(),
            "main".into(),
            "work".into(),
            mason_definition::Capabilities::now(),
            holds.map(|holds| WeakeningCheck {
                base: "main".into(),
                holds,
                note_file: note.clone(),
            }),
        );
        (v("submit_change", &serde_json::json!({"head": head})), note)
    }

    #[test]
    fn submit_refuses_a_weakening_where_the_project_sends_it_back() {
        let (r, _) = submit_with("cargo test @OT@", Some(crate::config::Holds::Review));
        let problems = r.unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("scripts/run.sh:2") && p.contains("broadened-skip")),
            "{problems:?}"
        );
        // Closures and counters are not findings.
        let (r, _) = submit_with("n=$(grep -c x f) @OT@", Some(crate::config::Holds::Review));
        assert!(r.is_ok(), "{r:?}");
        // A project that is not self-hosting is not scanned.
        let (r, _) = submit_with("cargo test @OT@", None);
        assert!(r.is_ok());
    }

    #[test]
    fn submit_accepts_a_weakening_with_a_note_where_the_owner_holds() {
        let (r, note) = submit_with("cargo test @OT@", Some(crate::config::Holds::Owner));
        assert!(r.is_ok(), "{r:?}");
        let text = std::fs::read_to_string(note).unwrap();
        assert!(text.contains("scripts/run.sh:2"), "{text}");
    }

    #[test]
    fn without_pty_the_command_is_unchanged() {
        for cmd in ["swift test", "echo 'a b'; exit 3", ""] {
            assert_eq!(pty_wrap(false, cmd), cmd);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn with_pty_the_command_runs_under_script_with_its_own_exit_code() {
        assert_eq!(
            pty_wrap(true, "echo 'hi'"),
            "script -q /dev/null sh -c 'echo '\\''hi'\\'''"
        );
        let run = |cmd: &str| {
            std::process::Command::new("sh")
                .args(["-c", &pty_wrap(true, cmd)])
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap()
        };
        let o = run("echo 'it works'; exit 3");
        assert_eq!(o.status.code(), Some(3));
        assert!(String::from_utf8_lossy(&o.stdout).contains("it works"));
        assert_eq!(run("exit 0").status.code(), Some(0));
    }

    /// What the step host does to a stopped run: SIGTERM to the command's process
    /// group. `script` starts the command in a session of its own, so this checks that
    /// the command is gone anyway.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_stopped_pty_run_leaves_no_command_behind() {
        use std::os::unix::process::CommandExt;
        let dir = std::env::temp_dir().join(format!("mason-pty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pidf = dir.join("pid");
        let cmd = format!(
            "echo $$ > {p}.tmp; mv {p}.tmp {p}; sleep 300",
            p = pidf.display()
        );
        let mut child = std::process::Command::new("sh")
            .args(["-c", &pty_wrap(true, &cmd)])
            .stdin(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = loop {
            if let Ok(t) = std::fs::read_to_string(&pidf) {
                break t.trim().parse::<i32>().unwrap();
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let pg = nix::unistd::Pid::from_raw(child.id() as i32);
        let _ = nix::sys::signal::killpg(pg, nix::sys::signal::Signal::SIGTERM);
        let _ = child.wait();
        let alive =
            |pid: i32| nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok();
        let t0 = Instant::now();
        while alive(pid) && t0.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let left = alive(pid);
        if left {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!left, "the command outlived its stopped run");
    }

    #[test]
    fn locks_and_deadlines_are_environmental_real_errors_are_not() {
        let e = |stderr: &str, timed_out: bool| git::GitError {
            args: vec!["merge".into()],
            code: Some(128),
            stderr: stderr.into(),
            timed_out,
        };
        assert!(environmental(&e("", true)));
        assert!(environmental(&e(
            "fatal: Unable to create '/r/.git/index.lock': File exists.",
            false
        )));
        assert!(environmental(&e(
            "error: cannot lock ref: Unable to create 'packed-refs.lock': File exists",
            false
        )));
        assert!(!environmental(&e("fatal: not a git repository", false)));
        assert!(!environmental(&e(
            "error: pathspec 'x' did not match",
            false
        )));
    }

    #[test]
    fn run_shell_returns_the_tail_of_output_with_a_bad_byte() {
        let dir = tempfile::tempdir().unwrap();
        let (ok, out) = run_shell(
            "printf 'first\\n\\377bad\\nlast line\\n'; exit 1",
            dir.path(),
            Duration::from_secs(20),
        );
        assert!(!ok);
        assert!(
            out.contains("last line") && out.contains("first"),
            "{out:?}"
        );
    }

    #[test]
    fn the_tail_reader_keeps_only_the_last_window() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big");
        let mut data = b"head\n".to_vec();
        data.extend(std::iter::repeat_n(b'x', 2 << 20));
        data.extend(b"\n\xffend\n");
        std::fs::write(&p, &data).unwrap();
        let t = read_tail_lossy(&p).unwrap();
        assert!(t.len() as u64 <= TAIL_WINDOW && t.ends_with("end\n") && !t.starts_with("head"));
        assert!(read_lossy(&p).unwrap().starts_with("head"));
    }

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
