// SPDX-License-Identifier: GPL-3.0-or-later

//! A diagnosis for a failed task (s250, decision 0245; s1048, decision 1041).
//!
//! When a task enters Failed, one read-only agent session (tier `light`) looks at it in the task's
//! worktree: the title and body, the plan, why it failed and what each round was told, the tails of
//! the last test logs and `git diff <base>...HEAD --stat`. It ends with one call,
//! `submit_diagnosis`: a decision card of 2 to 4 options, exactly one recommended, each with an
//! action the owner's choice carries out:
//!
//! * `retry`: run again from a step, with a note the next round's prompt shows once;
//! * `explore`: the plan does not hold, back to exploring with the reason;
//! * `close`: already done or a duplicate, with evidence;
//! * `cancel`.
//!
//! The card is stored on the diagnosis row (`card`, with each option's action next to it). The
//! **recommended** option's `retry` is applied by itself when the failure was not the code (any
//! project), or when the diagnosis says no code change is needed, at most once per task per day.
//! Everything else waits as a pending card: the alert says what it suggests, and the owner applies
//! or dismisses it (`mason task diagnosis`). A failed task cannot hold a question in the engine, so
//! the card is that alert and these actions, not a `QuestionAsked`.
//!
//! Budget: one diagnosis per entry into Failed (`state_entered_at`), at most [`MAX_PER_TASK`] per
//! task, a session without a verdict counted. The owner may ask for one on a failed task
//! (`POST /v1/tasks/{id}/diagnose`) within the same budget. Rows of the older single-verdict
//! shape stay readable.

use crate::daemon::{now_ms, Daemon};
use mason_adapter_api::card::{OPTION_MAX, WHY_MAX};
use mason_adapter_api::contract::{ContractHandle, NoChecks};
use mason_adapter_api::{SessionSpec, StepKind};
use mason_adapters::runner::{RunnerConfig, StepEnd, StepRunner};
use mason_core::*;
use mason_store::DiagnosisRow;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Sessions per task, ever (one per failure, so a task that fails again after the owner acted on a
/// diagnosis gets a new one).
pub const MAX_PER_TASK: usize = 6;
/// A self project's automatic retry, at most once per task in this long.
pub const AUTO_WINDOW_MS: u64 = 86_400_000;

/// What to do with a submitted diagnosis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Apply it now (the self project's retry, within its daily limit).
    Apply,
    /// Wait for the owner to apply or dismiss it.
    Card,
}

/// What `decide` looks at.
#[derive(Clone, Copy, Debug)]
pub struct Facts<'a> {
    /// The project is MASON's own (`[self]`).
    pub is_self: bool,
    /// The task's failure class.
    pub class: FailClass,
    pub verdict: &'a str,
    /// The diagnosis says the code needs no change (`no_code_change`).
    pub no_code_change: bool,
    /// An automatic retry of this task was applied within a day.
    pub applied_recently: bool,
    /// The automatic retry loop already has a retry coming for this failure (`autoretry::pending`).
    pub auto_retry_pending: bool,
}

/// Only a `retry` is applied by itself, and then only when the failure was not the task's code
/// (any project), or when it was and the diagnosis says no code change is needed, once per task
/// per day. Everything else is a card.
pub fn decide(f: Facts) -> Action {
    if f.verdict != "retry" {
        return Action::Card;
    }
    match f.class {
        // The retry loop makes this one; a second here would be two for one failure.
        FailClass::Outside if f.auto_retry_pending => Action::Card,
        FailClass::Outside => Action::Apply,
        FailClass::Code if f.no_code_change && !f.applied_recently => Action::Apply,
        FailClass::Code => Action::Card,
    }
}

/// Whether one more session may run for a task that has had `so_far`.
pub fn may_run(so_far: usize) -> bool {
    so_far < MAX_PER_TASK
}

/// Whether the failure that began at `failed_at` may be diagnosed, given the task's rows: not
/// twice for one failure, and not past [`MAX_PER_TASK`]. A row with no failure time (an older
/// one) never matches a failure.
pub fn budget(rows: &[DiagnosisRow], failed_at: u64) -> Result<(), String> {
    if !may_run(rows.len()) {
        return Err(format!(
            "this task has had {} diagnoses, the most there are",
            MAX_PER_TASK
        ));
    }
    if failed_at != 0 && rows.iter().any(|r| r.failed_at == failed_at) {
        return Err("this failure has been diagnosed already".into());
    }
    Ok(())
}

/// A diagnosis that is running now. Task numbers are only unique within one daemon, and a
/// process (a test binary) may hold several, so the data directory is part of the key.
struct Running {
    data_dir: PathBuf,
    task: u64,
    started_ms: u64,
    ran_on: String,
}

impl Running {
    fn is(&self, data_dir: &Path, task: u64) -> bool {
        self.task == task && self.data_dir == data_dir
    }
}

static RUNNING: std::sync::Mutex<Vec<Running>> = std::sync::Mutex::new(Vec::new());

/// A diagnosis the daemon of `data_dir` is running for `task`: when it started and on what
/// model and effort.
pub fn running(data_dir: &Path, task: u64) -> Option<(u64, String)> {
    RUNNING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|r| r.is(data_dir, task))
        .map(|r| (r.started_ms, r.ran_on.clone()))
}

/// Takes a task's running mark, and gives it back when dropped.
struct RunMark {
    data_dir: PathBuf,
    task: u64,
}

impl RunMark {
    fn take(data_dir: &Path, task: u64) -> Option<RunMark> {
        let mut v = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if v.iter().any(|r| r.is(data_dir, task)) {
            return None;
        }
        v.push(Running {
            data_dir: data_dir.to_path_buf(),
            task,
            started_ms: now_ms(),
            ran_on: String::new(),
        });
        Some(RunMark {
            data_dir: data_dir.to_path_buf(),
            task,
        })
    }
    fn ran_on(&self, text: &str) {
        let mut v = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = v.iter_mut().find(|r| r.is(&self.data_dir, self.task)) {
            r.ran_on = text.to_string();
        }
    }
}

impl Drop for RunMark {
    fn drop(&mut self) {
        RUNNING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|r| !r.is(&self.data_dir, self.task));
    }
}

/// The recommended option of a submission, as the verdict (its action) and the arguments to carry
/// it out (the option, with the submission's `summary` and `no_code_change`). An older submission
/// is its own verdict and arguments.
pub fn recommended(a: &Value) -> (String, Value) {
    let Some(opts) = a["options"].as_array() else {
        return (
            a["verdict"].as_str().unwrap_or("none").to_string(),
            a.clone(),
        );
    };
    let Some(o) = opts
        .iter()
        .find(|o| o["recommended"].as_bool() == Some(true))
    else {
        return ("none".into(), a.clone());
    };
    let mut args = o.clone();
    for k in ["summary", "no_code_change"] {
        args[k] = a[k].clone();
    }
    (o["action"].as_str().unwrap_or("none").to_string(), args)
}

/// The decision card of a submission with options: `Card::to_json`, each option carrying its
/// action arguments under `action`. `None` for an older single-verdict submission.
pub fn build_card(id: u64, task: u64, a: &Value) -> Option<Value> {
    use mason_adapter_api::card::{
        truncate, Card, CardOption, Origin, EVIDENCE_LINES, EVIDENCE_LINE_MAX,
    };
    let opts = a["options"].as_array()?;
    let card = Card {
        ask: format!("Task #{task} failed: what should happen next?"),
        why: truncate(a["summary"].as_str().unwrap_or("").trim(), WHY_MAX),
        options: opts
            .iter()
            .map(|o| CardOption {
                label: truncate(o["label"].as_str().unwrap_or("").trim(), OPTION_MAX),
                recommended: o["recommended"].as_bool() == Some(true),
            })
            .collect(),
        evidence: a["evidence_lines"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| l.as_str())
            .take(EVIDENCE_LINES)
            .map(|l| truncate(l, EVIDENCE_LINE_MAX))
            .collect(),
        links: vec![],
        key: format!("diagnosis:{id}"),
        origin: Origin::Agent,
        problems: vec![],
        multi_select: false,
    };
    let mut v = card.to_json();
    for (i, o) in opts.iter().enumerate() {
        let mut act = serde_json::Map::new();
        for k in [
            "action",
            "step",
            "note",
            "reason",
            "kind",
            "duplicate_of",
            "evidence",
        ] {
            if !o[k].is_null() {
                act.insert(k.into(), o[k].clone());
            }
        }
        v["options"][i]["action"] = Value::Object(act);
    }
    Some(v)
}

/// The words of a pending diagnosis, as the alert and `mason task diagnosis` show them. A
/// submission with options reads as the card; an older one as its single verdict.
pub fn card_text(task: u64, id: u64, verdict: &str, a: &Value) -> String {
    let s = |k: &str| a[k].as_str().unwrap_or("").trim().to_string();
    if let Some(opts) = a["options"].as_array() {
        let list: Vec<String> = opts
            .iter()
            .enumerate()
            .map(|(i, o)| {
                format!(
                    "{}. {}{}",
                    i + 1,
                    o["label"].as_str().unwrap_or("").trim(),
                    if o["recommended"].as_bool() == Some(true) {
                        " (recommended)"
                    } else {
                        ""
                    }
                )
            })
            .collect();
        return format!(
            "Task #{task} failed. Diagnosis #{id}: {}. Options: {}",
            s("summary"),
            list.join(" / ")
        );
    }
    let what = match verdict {
        "retry" => format!(
            "run it again{} with this note: {}",
            if s("step").is_empty() {
                String::new()
            } else {
                format!(" from `{}`", s("step"))
            },
            s("note")
        ),
        "explore" => format!("send it back to exploring: {}", s("reason")),
        "close" => format!(
            "close it ({}{}): {}",
            s("kind"),
            a["duplicate_of"]
                .as_u64()
                .map_or(String::new(), |n| format!(" of #{n}")),
            s("evidence")
        ),
        _ => {
            let mut options = vec![format!("{} (recommended)", s("recommendation"))];
            options.extend(
                a["alternatives"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|x| x.as_str().map(String::from)),
            );
            format!("{}: {}", s("question"), options.join(" / "))
        }
    };
    format!(
        "Task #{task} failed on its own code. Diagnosis #{id}: {}. Suggested: {what}",
        s("summary")
    )
}

/// The words of a stored diagnosis: read from its card when it has one.
pub fn row_text(r: &DiagnosisRow) -> String {
    let a: Value = serde_json::from_str(&r.args).unwrap_or(json!({}));
    let card = serde_json::from_str::<Value>(&r.card)
        .ok()
        .and_then(|v| mason_adapter_api::card::Card::from_json(&v));
    match card {
        Some(c) => format!(
            "Task #{} failed. Diagnosis #{}: {}. Options: {}",
            r.task,
            r.id,
            c.why,
            c.options
                .iter()
                .enumerate()
                .map(|(i, o)| format!(
                    "{}. {}{}",
                    i + 1,
                    o.label,
                    if o.recommended { " (recommended)" } else { "" }
                ))
                .collect::<Vec<_>>()
                .join(" / ")
        ),
        None => card_text(r.task, r.id, &r.verdict, &a),
    }
}

/// The tail of each of the newest log files under a task's directory.
pub(crate) fn log_tails(dir: &std::path::Path, files: usize, lines: usize) -> String {
    let mut logs: Vec<(std::time::SystemTime, std::path::PathBuf)> = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n != "host") {
                    stack.push(p);
                }
            } else if p.extension().is_some_and(|x| x == "log") {
                if let Ok(t) = e.metadata().and_then(|m| m.modified()) {
                    logs.push((t, p));
                }
            }
        }
    }
    logs.sort_by_key(|l| std::cmp::Reverse(l.0));
    logs.iter()
        .take(files)
        .map(|(_, p)| {
            let text = std::fs::read_to_string(p).unwrap_or_default();
            format!(
                "--- {}\n{}",
                p.file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default(),
                crate::jobs::tail(&text, lines)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The prompt: what the agent is shown. Pure over its inputs.
#[allow(clippy::too_many_arguments)]
pub fn prompt(
    number: u64,
    title: &str,
    body: &str,
    plan: Option<&Value>,
    reason: &str,
    history: &str,
    logs: &str,
    diffstat: &str,
    steps: &[String],
) -> String {
    format!(
        "Task #{number} failed and no one is watching. You are the diagnosis: \
         look at the worktree you are in (you may not change any file) and put the owner's choice \
         to them as a decision card. End with exactly one call of `submit_diagnosis`: a `summary` \
         of what went wrong and why, and 2 to 4 `options` the owner can choose between, exactly \
         one `recommended`. Each option must be complete enough to carry out without further \
         questions, and has one `action`: `retry` (run again from one of the steps {steps}; give a \
         `note` for the next agent: what went wrong and what to do differently), `explore` (the \
         plan does not hold: give the `reason`), `close` (the work is already done, or a \
         duplicate of another task: give the `evidence`, and `duplicate_of` for a duplicate) or \
         `cancel`. The note in a `retry` or `explore` option is what the next agent is told, so \
         splitting the task, landing a partial change or re-scoping are written as notes on those \
         actions. Say in the summary why the recommended option beats the others. Set \
         `no_code_change` only if the code is right and running it again is expected to pass; \
         MASON then retries by itself. Put the decisive lines in `evidence_lines`.\n\n\
         # The task\n{title}\n\n{body}\n\n\
         # The plan\n{plan}\n\n\
         # Why it failed\n{reason}\n\n\
         # What each round was told\n{history}\n\n\
         # The end of the last test logs\n{logs}\n\n\
         # git diff --stat against the base\n{diffstat}\n",
        steps = steps.join(", "),
        plan = plan.map_or("(none)".to_string(), |p| p.to_string()),
        history = if history.trim().is_empty() {
            "(nothing was recorded)"
        } else {
            history
        },
        logs = if logs.trim().is_empty() {
            "(no logs)"
        } else {
            logs
        },
        diffstat = if diffstat.trim().is_empty() {
            "(no change on the branch)"
        } else {
            diffstat
        },
    )
}

/// A task has just entered Failed: start a diagnosis unless the automatic retry loop has a retry
/// coming, the project has them off, or the budget is spent.
pub fn on_failed(d: &Arc<Daemon>, task: TaskId, run: StepRunId) {
    if let Err(why) = start(d, task, run, false) {
        tracing::debug!(task = task.0, "no diagnosis: {why}");
    }
}

/// Start a diagnosis of a failed task. `manual`: the owner asked, so a pending automatic retry and
/// the project's switch do not stop it; the budget and a running session still do.
pub fn start(d: &Arc<Daemon>, task: TaskId, run: StepRunId, manual: bool) -> Result<(), String> {
    let failed_at = {
        let w = d.world().map_err(|e| e.to_string())?;
        let t = w.tasks.get(&task).ok_or("no such task")?;
        if t.state != State::Failed {
            return Err(format!("#{} is not failed", task.0));
        }
        if !manual {
            if crate::autoretry::pending(d, t) {
                return Err("an automatic retry is coming".into());
            }
            if !d.project(t.project).is_some_and(|p| p.diagnose) {
                return Err("the project has diagnoses off".into());
            }
        }
        t.state_entered_at.0
    };
    // Counted before the session, and the mark taken before the thread: two callers do not start
    // two sessions.
    let rows = d.store.diagnoses(Some(task.0)).map_err(|e| e.to_string())?;
    budget(&rows, failed_at)?;
    let mark = RunMark::take(&d.cfg.data_dir, task.0).ok_or("a diagnosis is already running")?;
    let d = d.clone();
    std::thread::Builder::new()
        .name(format!("diagnose-{}", task.0))
        .spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                diagnose(&d, task, run, failed_at, &mark)
            }));
            if r.is_err() {
                tracing::error!(task = task.0, "the diagnosis panicked");
            }
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn diagnose(d: &Arc<Daemon>, task: TaskId, run: StepRunId, failed_at: u64, mark: &RunMark) {
    let Ok(w) = d.world() else { return };
    let Some(t) = w.tasks.get(&task) else { return };
    if t.state != State::Failed {
        return;
    }
    if d.project(t.project).is_none() {
        return;
    }
    let so_far = d
        .store
        .diagnoses(Some(task.0))
        .map_or(MAX_PER_TASK, |v| v.len());
    let reason = t.failure.clone().unwrap_or_default();
    let (title, body) = (t.title.clone(), t.body.clone());
    let steps: Vec<String> = crate::jobs::task_step_names(d, task);
    let Ok(c) = crate::jobs::ctx(d, task, run) else {
        return;
    };
    let dir = c.task_dir();
    let plan: Option<Value> = std::fs::read_to_string(dir.join("plan.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let history = std::fs::read_to_string(dir.join("feedback-history.txt")).unwrap_or_default();
    let base = c.project.base_ref();
    let diffstat = mason_landing::git::run(&c.wt, &["diff", "--stat", &format!("{base}...HEAD")])
        .unwrap_or_default();
    let text = prompt(
        task.0,
        &title,
        &body,
        plan.as_ref(),
        &reason,
        &history,
        &log_tails(&dir, 3, 40),
        &diffstat,
        &steps,
    );
    let agent = c.project.agent();
    let eff = crate::tiermap::effective(&d.cfg.config_dir, agent.provider, &c.project.overlay_text);
    let (mut spec, ran_on) = session_spec(&text, &c.wt, &c.project, &eff);
    mark.ran_on(&ran_on);
    let adapter = crate::jobs::adapter(&c);
    let runner = StepRunner::new(adapter.as_ref(), RunnerConfig::default());
    let contract = ContractHandle::new(StepKind::Diagnose, Box::new(NoChecks));
    let key = format!("t{}r{}diag{}", task.0, run.0, so_far);
    if contract
        .serve(&d.cfg.sockets_dir().join(format!("{key}.sock")))
        .is_err()
    {
        return;
    }
    spec.env = c.project.agent_env.clone();
    spec.env.retain(|(k, _)| k != "MASON_TASK");
    spec.env.push(("MASON_TASK".into(), task.0.to_string()));
    spec.tool_timeout = std::time::Duration::from_secs(120);
    spec.deadline = Some(std::time::Duration::from_secs(900));
    spec.run_key = Some(key);
    // The same session slot an agent step takes; the diagnosis waits for it like any request and
    // gives it back when it ends.
    let Ok(_slot) = crate::jobs::acquire_llm(&c) else {
        contract.shutdown();
        return;
    };
    let report = runner.run(spec, &contract);
    drop(_slot);
    contract.shutdown();
    let StepEnd::Accepted { submission, .. } = report.end else {
        tracing::info!(task = task.0, "the diagnosis ended without a verdict");
        // Counted all the same, so a session that never answers is not tried again and again.
        let _ = d.store.add_diagnosis(
            task.0,
            "none",
            &json!({"ran_on": ran_on}).to_string(),
            "none",
            now_ms(),
            failed_at,
        );
        return;
    };
    let mut submission = submission;
    if let Some(o) = submission.as_object_mut() {
        o.insert("ran_on".into(), json!(ran_on));
    }
    record_verdict(d, c.task, submission);
}

/// The session the diagnosis runs, with the model and effort the project, the provider file
/// and the built-in table settle on, and the words that say what they were.
pub fn session_spec(
    text: &str,
    wt: &std::path::Path,
    project: &crate::config::ProjectCfg,
    eff: &crate::tiermap::Effective,
) -> (SessionSpec, String) {
    let (entry, from) = crate::tiermap::diagnose_entry(
        eff,
        project.agent().provider,
        (
            project.diagnose_model.clone(),
            project.diagnose_effort.clone(),
        ),
    );
    let mut spec = SessionSpec::new(StepKind::Diagnose, text, wt);
    spec.model = entry.model.clone();
    spec.effort = entry.effort.clone();
    (spec, mason_definition::tiers::diagnose_text(&entry, from))
}

/// Store a submitted diagnosis, and apply its recommended option or leave the card pending.
pub fn record_verdict(d: &Arc<Daemon>, task: TaskId, a: Value) {
    let (verdict, rec_args) = recommended(&a);
    let now = now_ms();
    let Ok(w) = d.world() else { return };
    let Some(t) = w.tasks.get(&task) else { return };
    let failed_at = t.state_entered_at.0;
    let recent = d
        .store
        .diagnoses(Some(task.0))
        .unwrap_or_default()
        .iter()
        .any(|r| r.status == "applied" && now.saturating_sub(r.decided_ms) < AUTO_WINDOW_MS);
    let action = decide(Facts {
        is_self: d.project(t.project).is_some_and(|p| p.self_cfg.is_some()),
        class: t.failure_class.unwrap_or(FailClass::Code),
        verdict: &verdict,
        no_code_change: rec_args["no_code_change"].as_bool().unwrap_or(false),
        applied_recently: recent,
        auto_retry_pending: crate::autoretry::pending(d, t),
    });
    drop(w);
    let task = task.0;
    let status = if action == Action::Apply {
        "applied"
    } else {
        "pending"
    };
    let Ok(id) = d
        .store
        .add_diagnosis(task, &verdict, &a.to_string(), status, now, failed_at)
    else {
        return;
    };
    if let Some(card) = build_card(id, task, &a) {
        let _ = d.store.set_diagnosis_card(id, &card.to_string());
    }
    if action == Action::Apply {
        let _ = d.store.set_diagnosis_status(id, "applied", now);
        match apply_args(d, task, &verdict, &rec_args, true) {
            Ok(what) => tracing::info!(task, "diagnosis #{id} applied by itself: {what}"),
            Err(e) => {
                tracing::warn!(task, "the diagnosis could not be applied: {e}");
                let _ = d.store.set_diagnosis_status(id, "pending", 0);
            }
        }
    }
    d.poke();
}

/// Do what a diagnosis says. `auto`: the daemon does it by itself.
pub fn apply_args(
    d: &Arc<Daemon>,
    task: u64,
    verdict: &str,
    a: &Value,
    auto: bool,
) -> Result<String, String> {
    let s = |k: &str| {
        a[k].as_str()
            .map(str::to_string)
            .filter(|x| !x.trim().is_empty())
    };
    let retry = |to: Option<String>, message: Option<String>| {
        let latest = d
            .world()
            .map(|w| crate::http::latest_run(&w, TaskId(task)))
            .unwrap_or(0);
        crate::http::retry_task(
            d,
            task,
            crate::http::RetryAsk {
                to,
                seen: Some(latest),
                auto,
                message,
                // The owner applying a diagnosis is the owner's retry.
                forget_red: !auto,
                ..Default::default()
            },
        )
        .map(|(to, step)| format!("retried from `{step}` ({})", mason_store::state_name(to)))
        .map_err(|(_, e)| e.0["error"].as_str().unwrap_or("refused").to_string())
    };
    match verdict {
        "retry" => retry(s("step"), s("note")),
        "explore" => {
            let first = {
                let w = d.world().map_err(|e| e.to_string())?;
                let t = w.tasks.get(&TaskId(task)).ok_or("no such task")?;
                crate::jobs::first_step_in(d, t, State::Exploring)
            };
            retry(first, s("reason"))
        }
        "close" => {
            // A failed task has no step to close from: retry puts it in one, then it is closed.
            retry(None, None)?;
            let reason = format!(
                "{}: {}",
                s("kind").unwrap_or_default(),
                s("evidence").unwrap_or_default()
            );
            d.emit(EventKind::ClosedNoChange {
                task: TaskId(task),
                reason,
            })
            .map_err(|e| e.to_string())?;
            Ok("closed with no change".into())
        }
        "cancel" => {
            d.emit(EventKind::Canceled {
                task: TaskId(task),
                delete_branch: false,
            })
            .map_err(|e| e.to_string())?;
            Ok("canceled".into())
        }
        _ => Err("this diagnosis has nothing to apply: decide yourself, or dismiss it".into()),
    }
}

/// Apply a pending diagnosis the owner chose.
pub fn apply(d: &Arc<Daemon>, row: &DiagnosisRow) -> Result<String, String> {
    if row.status != "pending" {
        return Err(format!("diagnosis #{} is {}", row.id, row.status));
    }
    let a: Value = serde_json::from_str(&row.args).unwrap_or(json!({}));
    let (verdict, args) = recommended(&a);
    let r = apply_args(d, row.task, &verdict, &args, false)?;
    let _ = d.store.set_diagnosis_status(row.id, "applied", now_ms());
    Ok(r)
}

pub fn dismiss(d: &Arc<Daemon>, row: &DiagnosisRow) -> Result<(), String> {
    if row.status != "pending" {
        return Err(format!("diagnosis #{} is {}", row.id, row.status));
    }
    d.store
        .set_diagnosis_status(row.id, "dismissed", now_ms())
        .map_err(|e| e.to_string())?;
    d.poke();
    Ok(())
}

/// Whether a pending diagnosis still needs the owner (s359): its task is failed, and has been
/// since before the diagnosis was written. A task that was retried, landed, was canceled or
/// failed again since has moved on, and the diagnosis speaks of a failure that is gone.
fn still_needs_you(task: Option<&Task>, diagnosed_ms: u64) -> bool {
    task.is_some_and(|t| t.state == State::Failed && t.state_entered_at.0 <= diagnosed_ms)
}

/// The alerts for pending diagnoses: one each, until applied or dismissed, or until the
/// task has moved on from the failure it diagnosed.
pub fn alerts(d: &Daemon) -> Vec<(String, String, String)> {
    let Ok(w) = d.world() else {
        return vec![];
    };
    d.store
        .diagnoses(None)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.status == "pending")
        .filter(|r| still_needs_you(w.tasks.get(&TaskId(r.task)), r.at_ms))
        .map(|r| {
            (
                format!("diagnosis:{}", r.id),
                "needs_you".to_string(),
                format!(
                    "{}. `mason task diagnosis {} apply|dismiss {}`",
                    row_text(&r),
                    r.task,
                    r.id
                ),
            )
        })
        .chain(applied_alerts(d))
        .collect()
}

/// How long an alert says what a diagnosis applied by itself.
const APPLIED_ALERT_MS: u64 = 3_600_000;

/// One alert for each diagnosis the daemon applied by itself in the last hour, naming what it did.
fn applied_alerts(d: &Daemon) -> Vec<(String, String, String)> {
    let now = now_ms();
    d.store
        .diagnoses(None)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.status == "applied" && now.saturating_sub(r.decided_ms) < APPLIED_ALERT_MS)
        .map(|r| {
            (
                format!("diagnosis-applied:{}", r.id),
                "environment".to_string(),
                format!("Applied by itself: {}", row_text(&r)),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diagnosis_needs_you_only_while_its_task_is_still_in_the_failure_it_diagnosed() {
        // 2026-11-14: the monitor kept speaking diagnoses of tasks that had been retried,
        // landed or canceled since (#197, #211, #213...) (s359).
        let mut t = Task {
            id: TaskId(197),
            project: ProjectId(1),
            title: String::new(),
            body: String::new(),
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            enqueued_at: Timestamp(0),
            state_entered_at: Timestamp(1_000),
            state: State::Failed,
            activity: Activity::Idle,
            landed: None,
            explored: false,
            blockers: Default::default(),
            not_before: None,
            paused: false,
            failures: 1,
            attempts: Default::default(),
            decision: None,
            cancel_keeps_branch: false,
            failure: None,
            failure_class: None,
            def: String::new(),
            step: String::new(),
            step_attempts: Default::default(),
            route: None,
            explore_returns: 0,
            approved: false,
            force_explore: false,
        };
        assert!(still_needs_you(Some(&t), 2_000), "failed, diagnosed after");
        assert!(
            !still_needs_you(Some(&t), 500),
            "failed again since the diagnosis"
        );
        t.state = State::Testing;
        assert!(!still_needs_you(Some(&t), 2_000), "retried");
        t.state = State::Landed;
        assert!(!still_needs_you(Some(&t), 2_000), "landed");
        assert!(!still_needs_you(None, 2_000), "no such task");
    }

    fn facts(is_self: bool, class: FailClass, verdict: &str) -> Facts<'_> {
        Facts {
            is_self,
            class,
            verdict,
            no_code_change: false,
            applied_recently: false,
            auto_retry_pending: false,
        }
    }

    #[test]
    fn a_retry_is_applied_by_itself_when_the_failure_was_not_the_code() {
        for is_self in [true, false] {
            assert_eq!(
                decide(facts(is_self, FailClass::Outside, "retry")),
                Action::Apply
            );
            // The retry loop has one coming: not a second.
            let mut f = facts(is_self, FailClass::Outside, "retry");
            f.auto_retry_pending = true;
            assert_eq!(decide(f), Action::Card);
            for v in ["explore", "close", "ask", "none"] {
                assert_eq!(decide(facts(is_self, FailClass::Outside, v)), Action::Card);
            }
        }
    }

    #[test]
    fn a_code_failure_retry_is_applied_only_when_no_code_change_is_needed() {
        for is_self in [true, false] {
            assert_eq!(
                decide(facts(is_self, FailClass::Code, "retry")),
                Action::Card,
                "without no_code_change, self or not"
            );
            let mut f = facts(is_self, FailClass::Code, "retry");
            f.no_code_change = true;
            assert_eq!(decide(f), Action::Apply);
            f.applied_recently = true;
            assert_eq!(decide(f), Action::Card, "once a day");
            for v in ["explore", "close", "ask", "none"] {
                let mut f = facts(is_self, FailClass::Code, v);
                f.no_code_change = true;
                assert_eq!(decide(f), Action::Card, "{v}");
            }
        }
    }

    fn row(failed_at: u64) -> DiagnosisRow {
        DiagnosisRow {
            id: 1,
            task: 1,
            at_ms: 0,
            verdict: "none".into(),
            args: "{}".into(),
            status: "none".into(),
            decided_ms: 0,
            card: String::new(),
            failed_at,
        }
    }

    #[test]
    fn a_task_gets_six_sessions_and_no_more() {
        assert!(may_run(0) && may_run(5));
        assert!(!may_run(6) && !may_run(9));
        let rows: Vec<_> = (1..=5).map(|i| row(i * 100)).collect();
        assert!(budget(&rows, 600).is_ok(), "the sixth");
        let rows: Vec<_> = (1..=6).map(|i| row(i * 100)).collect();
        assert!(budget(&rows, 700).is_err(), "the seventh is refused");
    }

    #[test]
    fn one_failure_is_diagnosed_once_and_a_second_failure_gets_a_new_diagnosis() {
        let rows = vec![row(100)];
        assert!(budget(&rows, 100).is_err(), "the same failure");
        assert!(budget(&rows, 200).is_ok(), "failed again after a retry");
        assert!(budget(&[], 100).is_ok());
        // A row with no failure time (an older one) does not stand for any failure.
        assert!(budget(&[row(0)], 100).is_ok());
    }

    #[test]
    fn a_session_with_no_verdict_still_counts() {
        // `none` rows are rows: they are counted by the same budget.
        let mut r = row(100);
        r.verdict = "none".into();
        assert!(budget(&[r], 100).is_err());
    }

    #[test]
    fn a_running_mark_is_taken_once_and_given_back() {
        let (a, b) = (Path::new("/data/a"), Path::new("/data/b"));
        let m = RunMark::take(a, 990_001).expect("first");
        m.ran_on("model x on high");
        assert_eq!(
            running(a, 990_001).map(|r| r.1),
            Some("model x on high".into())
        );
        assert!(RunMark::take(a, 990_001).is_none(), "refused while running");
        // Another daemon's task of the same number is another task.
        assert!(running(b, 990_001).is_none());
        drop(RunMark::take(b, 990_001).expect("another daemon's"));
        assert!(running(a, 990_001).is_some());
        drop(m);
        assert!(running(a, 990_001).is_none());
        assert!(RunMark::take(a, 990_001).is_some());
    }

    fn submission(rec: &str) -> Value {
        let opt = |label: &str, action: &str, extra: Value| {
            let mut o = json!({"label": label, "action": action, "recommended": action == rec});
            for (k, v) in extra.as_object().unwrap() {
                o[k] = v.clone();
            }
            o
        };
        json!({"summary": "the fixture is gone", "no_code_change": false,
               "evidence_lines": ["error: no fixture"],
               "options": [
                   opt("Retry with the fixture", "retry", json!({"note": "restore it", "step": "develop"})),
                   opt("Re-scope", "explore", json!({"reason": "drop the fixture"})),
                   opt("Cancel", "cancel", json!({}))]})
    }

    #[test]
    fn the_recommended_option_decides_and_the_card_holds_every_options_action() {
        let (verdict, args) = recommended(&submission("explore"));
        assert_eq!(verdict, "explore");
        assert_eq!(args["reason"], "drop the fixture");
        let c = build_card(9, 4, &submission("retry")).unwrap();
        assert_eq!(c["key"], "diagnosis:9");
        assert_eq!(c["options"].as_array().unwrap().len(), 3);
        assert_eq!(c["options"][0]["recommended"], true);
        assert_eq!(c["options"][0]["action"]["note"], "restore it");
        assert_eq!(c["options"][2]["action"]["action"], "cancel");
        assert_eq!(c["evidence"][0], "error: no fixture");
        let parsed = mason_adapter_api::card::Card::from_json(&c).unwrap();
        assert_eq!(parsed.options.len(), 3);
        // An older submission has no card and is its own verdict.
        let old = json!({"verdict": "retry", "summary": "s", "note": "n"});
        assert!(build_card(1, 1, &old).is_none());
        assert_eq!(recommended(&old).0, "retry");
        let t = card_text(4, 9, "retry", &submission("retry"));
        assert!(
            t.contains("1. Retry with the fixture (recommended) / 2. Re-scope"),
            "{t}"
        );
    }

    #[test]
    fn a_recommended_cancel_or_explore_is_never_applied_by_itself() {
        for (rec, class) in [
            ("cancel", FailClass::Outside),
            ("cancel", FailClass::Code),
            ("explore", FailClass::Outside),
            ("explore", FailClass::Code),
        ] {
            let (verdict, _) = recommended(&submission(rec));
            let mut f = facts(true, class, &verdict);
            f.no_code_change = true;
            assert_eq!(decide(f), Action::Card, "{rec} {class:?}");
        }
        let (verdict, _) = recommended(&submission("retry"));
        assert_eq!(
            decide(facts(false, FailClass::Outside, &verdict)),
            Action::Apply
        );
    }

    #[test]
    fn the_card_says_what_is_suggested_with_the_recommendation_first() {
        let a = json!({"summary": "the test needs the fixture", "verdict": "ask",
                       "question": "Keep the old behaviour?", "recommendation": "Keep it",
                       "alternatives": ["Drop it"]});
        let t = card_text(7, 3, "ask", &a);
        assert!(
            t.starts_with("Task #7 failed on its own code. Diagnosis #3:"),
            "{t}"
        );
        assert!(
            t.contains("Keep the old behaviour?: Keep it (recommended) / Drop it"),
            "{t}"
        );
        let r = card_text(
            7,
            4,
            "retry",
            &json!({"summary": "s", "note": "use the fixture", "step": "develop"}),
        );
        assert!(
            r.contains("run it again from `develop` with this note: use the fixture"),
            "{r}"
        );
        let c = card_text(
            7,
            5,
            "close",
            &json!({"summary": "s", "kind": "duplicate", "duplicate_of": 4, "evidence": "same change"}),
        );
        assert!(c.contains("close it (duplicate of #4): same change"), "{c}");
    }

    #[test]
    fn the_prompt_carries_everything_the_agent_is_given() {
        let p = prompt(
            12,
            "Fix x",
            "the body",
            Some(&json!({"summary": "plan"})),
            "still red after 3 rounds",
            "## round 1\nfeedback one",
            "--- t.log\nFAILED",
            " src/a.rs | 2 +-",
            &["explore".into(), "develop".into()],
        );
        for want in [
            "Task #12",
            "the body",
            "\"summary\":\"plan\"",
            "still red after 3 rounds",
            "feedback one",
            "FAILED",
            "src/a.rs",
            "explore, develop",
            "submit_diagnosis",
            "exactly one `recommended`",
            "why the recommended option beats the others",
            "complete enough to carry out without further",
        ] {
            assert!(p.contains(want), "{want}");
        }
    }
}
