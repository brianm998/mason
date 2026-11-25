// SPDX-License-Identifier: GPL-3.0-or-later

//! A diagnosis for a task that failed on its own code (s250, decision 0245).
//!
//! When a task fails with class `code`, one read-only agent session (tier `light`, at most two
//! per task, counted in the store) looks at it in the task's worktree: the title and body, the
//! plan, why it failed and what each round was told, what earlier rounds tried and gave up, the
//! tails of the last test logs and `git diff <base>...HEAD --stat`. It ends with one call,
//! `submit_diagnosis`:
//!
//! * `retry`: run again from a step, with a note that the next round's prompt shows once;
//! * `explore`: the plan does not hold, back to exploring with the reason;
//! * `close`: already done or a duplicate, with evidence;
//! * `ask`: only the owner can decide, recommendation first.
//!
//! A `retry` is applied by itself when the failure was not the code (any project), or when the
//! diagnosis says no code change is needed, at most once per task per day. Every other verdict waits as a pending diagnosis: the alert says what it suggests, and the owner
//! applies or dismisses it (`mason task diagnosis`). A failed task cannot hold a question in the
//! engine, so the card is that alert and these two actions, not a `QuestionAsked`.

use crate::daemon::{now_ms, Daemon};
use mason_adapter_api::contract::{ContractHandle, NoChecks};
use mason_adapter_api::{SessionSpec, StepKind};
use mason_adapters::runner::{RunnerConfig, StepEnd, StepRunner};
use mason_core::*;
use mason_store::DiagnosisRow;
use serde_json::{json, Value};
use std::sync::Arc;

/// Sessions per task, ever.
pub const MAX_PER_TASK: usize = 2;
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

/// The words of a pending diagnosis, as the alert and `mason task diagnosis` show them.
pub fn card_text(task: u64, id: u64, verdict: &str, a: &Value) -> String {
    let s = |k: &str| a[k].as_str().unwrap_or("").trim().to_string();
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
    tried: Option<&str>,
    logs: &str,
    diffstat: &str,
    steps: &[String],
) -> String {
    format!(
        "Task #{number} failed on its own code and no one is watching. You are the diagnosis: \
         look at the worktree you are in (you may not change any file) and say what should \
         happen next. End with exactly one call of `submit_diagnosis`.\n\n\
         Verdicts: `retry` (run again from one of the steps {steps}; give a note for the next \
         agent: what went wrong and what to do differently; set `no_code_change` only if the code is \
         right and running it again is expected to pass, and MASON then retries by itself), `explore` (the plan does not hold: \
         give the reason), `close` (the work is already done, or a duplicate of another task: \
         give the evidence, and `duplicate_of` for a duplicate) or `ask` (only the owner can \
         decide: one question and your recommendation).\n\n\
         # The task\n{title}\n\n{body}\n\n\
         # The plan\n{plan}\n\n\
         # Why it failed\n{reason}\n\n\
         # What each round was told\n{history}\n\n{tried}\
         # The end of the last test logs\n{logs}\n\n\
         # git diff --stat against the base\n{diffstat}\n",
        steps = steps.join(", "),
        plan = plan.map_or("(none)".to_string(), |p| p.to_string()),
        tried = tried.map_or(String::new(), |t| format!("{t}\n\n")),
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

/// A task has just failed with a class: start a diagnosis if it is about the code and the task
/// has had fewer than two.
pub fn on_failed(d: &Arc<Daemon>, task: TaskId, run: StepRunId) {
    let d = d.clone();
    let _ = std::thread::Builder::new()
        .name(format!("diagnose-{}", task.0))
        .spawn(move || {
            let r =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| diagnose(&d, task, run)));
            if r.is_err() {
                tracing::error!(task = task.0, "the diagnosis panicked");
            }
        });
}

fn diagnose(d: &Arc<Daemon>, task: TaskId, run: StepRunId) {
    let Ok(w) = d.world() else { return };
    let Some(t) = w.tasks.get(&task) else { return };
    if t.failure_class != Some(FailClass::Code) || t.state != State::Failed {
        return;
    }
    let Some(project) = d.project(t.project) else {
        return;
    };
    if !project.diagnose {
        return;
    }
    // Counted before the session: a restart in the middle does not allow a third.
    let so_far = d
        .store
        .diagnoses(Some(task.0))
        .map_or(MAX_PER_TASK, |v| v.len());
    if !may_run(so_far) {
        return;
    }
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
        crate::prompts::tried_block(&crate::tried::rounds(&dir)).as_deref(),
        &log_tails(&dir, 3, 40),
        &diffstat,
        &steps,
    );
    let agent = c.project.agent();
    let eff = crate::tiermap::effective(&d.cfg.config_dir, agent.provider, &c.project.overlay_text);
    let (mut spec, ran_on) = session_spec(&text, &c.wt, &c.project, &eff);
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
        );
        return;
    };
    let mut submission = submission;
    if let Some(o) = submission.as_object_mut() {
        o.insert("ran_on".into(), json!(ran_on));
    }
    record(d, &c, submission);
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

/// Store a submitted diagnosis, and apply it or leave it as a card.
fn record(d: &Arc<Daemon>, c: &crate::jobs::Ctx, a: Value) {
    record_verdict(d, c.task, a);
}

/// The same for a task by number: looks up its project and failure class.
pub fn record_verdict(d: &Arc<Daemon>, task: TaskId, a: Value) {
    let verdict = a["verdict"].as_str().unwrap_or("none").to_string();
    let now = now_ms();
    let Ok(w) = d.world() else { return };
    let Some(t) = w.tasks.get(&task) else { return };
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
        no_code_change: a["no_code_change"].as_bool().unwrap_or(false),
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
        .add_diagnosis(task, &verdict, &a.to_string(), status, now)
    else {
        return;
    };
    if action == Action::Apply {
        let _ = d.store.set_diagnosis_status(id, "applied", now);
        match apply_args(d, task, &verdict, &a, true) {
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
        _ => Err("an `ask` has nothing to apply: answer it yourself, or dismiss it".into()),
    }
}

/// Apply a pending diagnosis the owner chose.
pub fn apply(d: &Arc<Daemon>, row: &DiagnosisRow) -> Result<String, String> {
    if row.status != "pending" {
        return Err(format!("diagnosis #{} is {}", row.id, row.status));
    }
    let a: Value = serde_json::from_str(&row.args).unwrap_or(json!({}));
    let r = apply_args(d, row.task, &row.verdict, &a, false)?;
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
            let a: Value = serde_json::from_str(&r.args).unwrap_or(json!({}));
            (
                format!("diagnosis:{}", r.id),
                "needs_you".to_string(),
                format!(
                    "{}. `mason task diagnosis {} apply|dismiss {}`",
                    card_text(r.task, r.id, &r.verdict, &a),
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
            let a: Value = serde_json::from_str(&r.args).unwrap_or(json!({}));
            (
                format!("diagnosis-applied:{}", r.id),
                "environment".to_string(),
                format!(
                    "Applied by itself: {}",
                    card_text(r.task, r.id, &r.verdict, &a)
                ),
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

    #[test]
    fn a_task_gets_two_sessions_and_no_more() {
        assert!(may_run(0) && may_run(1));
        assert!(!may_run(2) && !may_run(5));
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
            Some("=== Earlier rounds tried ===\n- a dead end"),
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
            "=== Earlier rounds tried ===\n- a dead end\n\n# The end of the last test logs",
            "FAILED",
            "src/a.rs",
            "explore, develop",
            "submit_diagnosis",
        ] {
            assert!(p.contains(want), "{want}");
        }
    }
}
