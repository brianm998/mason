// SPDX-License-Identifier: GPL-3.0-or-later

//! Scheduled CI runs (s175, decision 0170): the `ci` kind of `[schedule.<name>]`. Dispatches a
//! slow workflow on the base branch when it matters, follows the run, reruns its failed jobs once
//! and files tasks for the reds and the flakes. Off unless a project's overlay has the schedule.
//! The rules are in `mason_core::ci_schedule`; this is the reading, the filing and the keeping.
//! The followed run is kept in the schedule's row (`schedules.follow`), so a restart loses nothing.

use crate::ci_watch::{emit_task, named_keys};
use crate::config::ProjectCfg;
use crate::daemon::Daemon;
use crate::schedule::{CiSpec, Spec};
use mason_adapters::github::{ActionsJob, GitHub, WorkflowRun};
use mason_core::ci_schedule::{
    self as rules, Failed, Follow, Group, Kind, Phase, Second, Seen, Step,
};
use mason_core::*;
use mason_store::ScheduleRow;
use serde_json::{json, Value};
use std::sync::Arc;

/// Lines of a failed job's log kept for its task.
const LOG_LINES: usize = 40;

/// The CI host. The GitHub adapter is one; tests give another.
pub trait Host {
    fn runs(&self, workflow: &str, branch: &str) -> Result<Vec<WorkflowRun>, String>;
    fn dispatch(
        &self,
        workflow: &str,
        git_ref: &str,
        inputs: &[(String, String)],
    ) -> Result<(), String>;
    /// The jobs of a run's latest attempt.
    fn jobs(&self, run: u64) -> Result<Vec<ActionsJob>, String>;
    fn log_tail(&self, job: u64) -> Option<String>;
    fn rerun_failed(&self, run: u64) -> Result<(), String>;
}

impl Host for GitHub {
    fn runs(&self, workflow: &str, branch: &str) -> Result<Vec<WorkflowRun>, String> {
        self.workflow_file_runs(workflow, branch)
            .map_err(|e| e.to_string())
    }
    fn dispatch(
        &self,
        workflow: &str,
        git_ref: &str,
        inputs: &[(String, String)],
    ) -> Result<(), String> {
        self.dispatch_workflow(workflow, git_ref, inputs)
            .map_err(|e| e.to_string())
    }
    fn jobs(&self, run: u64) -> Result<Vec<ActionsJob>, String> {
        self.run_jobs(run).map_err(|e| e.to_string())
    }
    fn log_tail(&self, job: u64) -> Option<String> {
        self.job_log_tail(job, LOG_LINES)
    }
    fn rerun_failed(&self, run: u64) -> Result<(), String> {
        GitHub::rerun_failed(self, run).map_err(|e| e.to_string())
    }
}

/// What the decision needs from git.
pub trait Repo {
    fn fetch(&self) -> Result<(), String>;
    /// The base branch's tip.
    fn tip(&self) -> Result<String, String>;
    /// A file's text at the tip.
    fn file_at_tip(&self, path: &str) -> Result<String, String>;
    /// Files that differ from `sha` to the tip; `None` if `sha` is not in the repository.
    fn changed_since(&self, sha: &str) -> Option<Vec<String>>;
}

struct Git<'a> {
    d: &'a Daemon,
    p: &'a ProjectCfg,
    pid: ProjectId,
}

impl Repo for Git<'_> {
    fn fetch(&self) -> Result<(), String> {
        self.d.fetch_remote(self.pid)
    }
    fn tip(&self) -> Result<String, String> {
        mason_landing::git::run(&self.p.path, &["rev-parse", &self.p.base_ref()])
            .map(|s| s.trim().to_string())
            .map_err(|e| e.to_string())
    }
    fn file_at_tip(&self, path: &str) -> Result<String, String> {
        mason_landing::git::run(
            &self.p.path,
            &["show", &format!("{}:{path}", self.p.base_ref())],
        )
        .map_err(|e| e.to_string())
    }
    fn changed_since(&self, sha: &str) -> Option<Vec<String>> {
        mason_landing::git::run(
            &self.p.path,
            &["cat-file", "-e", &format!("{sha}^{{commit}}")],
        )
        .ok()?;
        let out = mason_landing::git::run(
            &self.p.path,
            &["diff", "--name-only", sha, &self.p.base_ref()],
        )
        .ok()?;
        Some(out.lines().map(String::from).collect())
    }
}

// ---- the followed run, kept in the schedule's row ------------------------------------------------------------

fn phase_name(p: Phase) -> &'static str {
    match p {
        Phase::Locate => "locate",
        Phase::Await1 => "await1",
        Phase::Await2 => "await2",
    }
}

pub fn encode(f: &Follow) -> String {
    json!({
        "run": f.run_id, "phase": phase_name(f.phase), "at": f.at, "before": f.before,
        "failures": f.failures.iter().map(|x| json!({
            "job": x.job, "conclusion": x.conclusion, "log": x.log, "infra": x.infra,
        })).collect::<Vec<_>>(),
    })
    .to_string()
}

pub fn decode(text: &str) -> Option<Follow> {
    let v: Value = serde_json::from_str(text).ok()?;
    let phase = match v["phase"].as_str()? {
        "locate" => Phase::Locate,
        "await1" => Phase::Await1,
        "await2" => Phase::Await2,
        _ => return None,
    };
    let text_of = |x: &Value| x.as_str().unwrap_or_default().to_string();
    Some(Follow {
        run_id: v["run"].as_u64()?,
        phase,
        at: v["at"].as_u64()?,
        before: v["before"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .collect(),
        failures: v["failures"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| Failed {
                job: text_of(&x["job"]),
                conclusion: text_of(&x["conclusion"]),
                log: text_of(&x["log"]),
                infra: x["infra"].as_str().map(String::from),
            })
            .collect(),
    })
}

/// The followed run as the status shows it, or `None` when nothing is followed.
pub fn follow_view(row: &ScheduleRow) -> Option<Value> {
    let f = decode(&row.follow)?;
    Some(json!({
        "run": (f.run_id != 0).then_some(f.run_id),
        "phase": phase_name(f.phase),
        "since": f.at,
        "waiting_for": match f.phase {
            Phase::Locate => "the dispatched run to appear",
            Phase::Await1 => "the first attempt to finish",
            Phase::Await2 => "the rerun of the failed jobs to finish",
        },
    }))
}

// ---- time ---------------------------------------------------------------------------------------------------

/// The local offset from UTC in minutes (`date +%z`); 0 if it cannot be read.
fn local_offset_min() -> i32 {
    let out = std::process::Command::new("/bin/date")
        .arg("+%z")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    let t = out.trim();
    if t.len() != 5 {
        return 0;
    }
    let sign = if t.starts_with('-') { -1 } else { 1 };
    match (t[1..3].parse::<i32>(), t[3..5].parse::<i32>()) {
        (Ok(h), Ok(m)) => sign * (h * 60 + m),
        _ => 0,
    }
}

/// Is this `ci` schedule due? A followed run is looked at on every tick; otherwise at its time
/// of day, or every so often.
pub fn is_due(row: &ScheduleRow, spec: &Spec, now: u64) -> bool {
    if !row.follow.is_empty() {
        return true;
    }
    match spec.ci.as_ref().and_then(|c| c.at) {
        Some(at) => rules::due_at(row.last_run, now, at, local_offset_min()),
        None => mason_core::introspect::due(Some(row.last_run), now, row.every_ms),
    }
}

// ---- running it ----------------------------------------------------------------------------------------------------

/// Run a `ci` schedule: step the followed run if there is one, else decide and dispatch. Returns
/// what happened and whether the schedule itself ran (a followed run that only moved did not).
pub fn run(
    d: &Arc<Daemon>,
    project: &ProjectCfg,
    spec: &Spec,
    row: &mut ScheduleRow,
    now: u64,
) -> (String, bool) {
    let Some(ci) = spec.ci.as_ref() else {
        return ("not a ci schedule".into(), true);
    };
    let Some(pid) = d.project_id_by_name(&project.id) else {
        return ("no such project".into(), true);
    };
    let what = format!("schedule `{}`", spec.name);
    let host = match crate::ci_watch::source(project, &what) {
        Ok((gh, _)) => gh,
        Err(e) => return (e, true),
    };
    if !row.follow.is_empty() {
        return (follow(d, pid, ci, &spec.name, &host, row, now), false);
    }
    let repo = Git { d, p: project, pid };
    (dispatch(d, pid, ci, &host, &repo, row, now), true)
}

fn branch_of(d: &Daemon, pid: ProjectId, ci: &CiSpec) -> Result<String, String> {
    let p = d.project(pid).ok_or("no such project")?;
    Ok(ci.git_ref.clone().unwrap_or_else(|| p.base.clone()))
}

fn infos(runs: &[WorkflowRun]) -> Vec<rules::RunInfo> {
    runs.iter()
        .map(|r| rules::RunInfo {
            id: r.id,
            sha: r.sha.clone(),
            created_ms: r.created_ms,
            completed: r.status == "completed",
            conclusion: r.conclusion.clone(),
            event: r.event.clone(),
            title: r.title.clone(),
            attempt: r.attempt,
        })
        .collect()
}

/// Decide, and dispatch when it is worth a run. Starts following the new run.
pub fn dispatch(
    d: &Arc<Daemon>,
    pid: ProjectId,
    ci: &CiSpec,
    host: &dyn Host,
    repo: &dyn Repo,
    row: &mut ScheduleRow,
    now: u64,
) -> String {
    let mut go = || -> Result<String, String> {
        let branch = branch_of(d, pid, ci)?;
        repo.fetch()?;
        let tip = repo.tip()?;
        let globs = match (&ci.paths_from, ci.paths.is_empty()) {
            (Some(f), _) => Some(
                rules::relevant_globs(&repo.file_at_tip(f)?).map_err(|e| format!("{e} in {f}"))?,
            ),
            (None, false) => Some(ci.paths.clone()),
            (None, true) => None,
        };
        let runs = host.runs(&ci.workflow, &branch)?;
        let decision = rules::decide(
            &infos(&runs),
            &ci.run_name,
            &tip,
            globs.as_deref(),
            &|sha| repo.changed_since(sha),
        );
        if !decision.dispatch {
            return Ok(format!("not dispatched: {}", decision.reason));
        }
        host.dispatch(&ci.workflow, &branch, &ci.inputs)?;
        row.follow = encode(&Follow {
            run_id: 0,
            phase: Phase::Locate,
            at: now,
            before: runs.iter().map(|r| r.id).collect(),
            failures: vec![],
        });
        Ok(format!(
            "dispatched {} on {branch}: {}",
            ci.workflow, decision.reason
        ))
    };
    go().unwrap_or_else(|e| format!("could not dispatch: {e}"))
}

fn is_green(c: &str) -> bool {
    matches!(c, "success" | "neutral" | "skipped")
}

/// Move the followed run one step. Nothing here waits: the next tick looks again.
pub fn follow(
    d: &Arc<Daemon>,
    pid: ProjectId,
    ci: &CiSpec,
    name: &str,
    host: &dyn Host,
    row: &mut ScheduleRow,
    now: u64,
) -> String {
    let Some(mut f) = decode(&row.follow) else {
        row.follow.clear();
        return "the followed run's state could not be read; dropped".into();
    };
    let branch = match branch_of(d, pid, ci) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let runs = match host.runs(&ci.workflow, &branch) {
        Ok(r) => r,
        Err(e) => return format!("could not look at the runs: {e}"),
    };
    if f.phase == Phase::Locate {
        let new = runs
            .iter()
            .filter(|r| !f.before.contains(&r.id) && r.event == "workflow_dispatch")
            .filter(|r| ci.run_name.is_empty() || r.title == ci.run_name)
            .max_by_key(|r| r.id);
        if let Some(r) = new {
            f.run_id = r.id;
            f.phase = Phase::Await1;
        }
    }
    let run = (f.phase != Phase::Locate)
        .then(|| runs.iter().find(|r| r.id == f.run_id))
        .flatten();
    let seen = run.map(|r| Seen {
        completed: r.status == "completed",
        attempt: r.attempt,
    });
    let say = match (rules::step(&f, seen, now), run) {
        (Step::Wait, _) => {
            row.follow = encode(&f);
            return match f.phase {
                Phase::Locate => "waiting for the dispatched run to appear".into(),
                _ => format!("following run {}", f.run_id),
            };
        }
        (Step::Drop, _) => format!("dropped run {}: it did not finish in time", f.run_id),
        (Step::ByHand, _) => format!(
            "run {} was rerun by hand; not classified (its first attempt's failures are gone)",
            f.run_id
        ),
        (Step::Check, Some(_)) => match check(host, &mut f) {
            Ok(Some(say)) => {
                row.follow = encode(&f);
                return say;
            }
            Ok(None) => format!("run {} is green", f.run_id),
            Err(e) => {
                // Not asked again until it answers; give up when it never does.
                if now.saturating_sub(f.at) > rules::GIVE_UP_MS {
                    row.follow.clear();
                    return format!("dropped run {}: {e}", f.run_id);
                }
                row.follow = encode(&f);
                return format!("run {}: {e}; will try again", f.run_id);
            }
        },
        (Step::Classify, Some(r)) => {
            match classify_and_file(d, pid, ci, name, host, &f, r, &branch) {
                Ok(say) => say,
                Err(e) => {
                    if now.saturating_sub(f.at) > rules::GIVE_UP_MS {
                        row.follow.clear();
                        return format!("dropped run {}: {e}", f.run_id);
                    }
                    row.follow = encode(&f);
                    return format!("run {}: {e}; will try again", f.run_id);
                }
            }
        }
        (Step::Check | Step::Classify, None) => {
            row.follow = encode(&f);
            return "waiting".into();
        }
    };
    row.follow.clear();
    say
}

/// The first attempt finished. `Ok(None)` when it was green; otherwise the failures are taken
/// (the rerun replaces them) and the failed jobs rerun.
fn check(host: &dyn Host, f: &mut Follow) -> Result<Option<String>, String> {
    let jobs = host.jobs(f.run_id)?;
    let red: Vec<&ActionsJob> = jobs
        .iter()
        .filter(|j| j.conclusion.as_deref().is_some_and(|c| !is_green(c)))
        .collect();
    if red.is_empty() {
        return Ok(None);
    }
    f.failures = red
        .iter()
        .map(|j| {
            let conclusion = j.conclusion.clone().unwrap_or_default();
            let log = j
                .conclusion
                .as_deref()
                .filter(|c| *c == "failure")
                .and_then(|_| host.log_tail(j.id));
            Failed {
                infra: mason_adapters::ci_infra::judge(&conclusion, true, log.as_deref(), &[]),
                log: log.unwrap_or_default(),
                job: j.name.clone(),
                conclusion,
            }
        })
        .collect();
    host.rerun_failed(f.run_id)?;
    f.phase = Phase::Await2;
    Ok(Some(format!(
        "run {}: {} job(s) failed; rerunning them once",
        f.run_id,
        f.failures.len()
    )))
}

fn fence(s: &str) -> String {
    // Never close the task's own fence.
    s.replace("```", "'''")
}

fn tails(legs: &[rules::Leg], second: bool) -> String {
    legs.iter()
        .map(|l| {
            let log = if second { &l.second_log } else { &l.first_log };
            let log = if log.trim().is_empty() {
                "(no log available)"
            } else {
                log
            };
            format!("### {}\n```\n{}\n```", l.job, fence(log))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn title_of(workflow: &str, g: &Group, base: &str) -> String {
    let what = match g.kind {
        Kind::Red => "red twice",
        Kind::Flaky => "flaky",
    };
    let keys: Vec<String> = g
        .legs
        .iter()
        .map(|l| mason_core::ci_watch::key(workflow, &l.job))
        .collect();
    let full = format!("{} {what} on {base}", keys.join(", "));
    if full.chars().count() <= 110 {
        full
    } else {
        format!(
            "{} {what} on {base} ({} legs)",
            mason_core::ci_watch::key(workflow, &g.group),
            g.legs.len()
        )
    }
}

fn body_of(name: &str, workflow: &str, g: &Group, run: &WorkflowRun, base: &str) -> String {
    let keys: Vec<String> = g
        .legs
        .iter()
        .map(|l| format!("- `{}`", mason_core::ci_watch::key(workflow, &l.job)))
        .collect();
    let (what, tail_of, which) = match g.kind {
        Kind::Red => (
            "failed on the first attempt and again when its failed jobs were rerun once, so it is not a one-off",
            tails(&g.legs, true),
            "the rerun",
        ),
        Kind::Flaky => (
            "failed on the first attempt and passed when its failed jobs were rerun once, so it is flaky. Find the cause (a race, a fixed sleep, a timeout too tight for the runner, state shared between tests) and fix it; a retry or a longer timeout alone is not a fix",
            tails(&g.legs, false),
            "the failed first attempt",
        ),
    };
    format!(
        "`{}` {what}.\n\nJobs:\n{}\n\nRun: {}\nCommit: {}\nBranch: `{base}`\n\nLog tail of {which}:\n\n{tail_of}\n\nFiled by the scheduled CI run `{name}`.",
        mason_core::ci_watch::key(workflow, &g.group),
        keys.join("\n"),
        run.url,
        run.sha,
    )
}

/// The rerun finished: judge each failed job, then file by group.
#[allow(clippy::too_many_arguments)]
fn classify_and_file(
    d: &Arc<Daemon>,
    pid: ProjectId,
    ci: &CiSpec,
    name: &str,
    host: &dyn Host,
    f: &Follow,
    run: &WorkflowRun,
    base: &str,
) -> Result<String, String> {
    let jobs = host.jobs(f.run_id)?;
    let second: Vec<Second> = f
        .failures
        .iter()
        .filter_map(|x| {
            let j = jobs.iter().find(|j| j.name == x.job)?;
            let c = j.conclusion.clone();
            let red = c.as_deref().is_some_and(|c| !is_green(c));
            let log = (c.as_deref() == Some("failure"))
                .then(|| host.log_tail(j.id))
                .flatten();
            Some(Second {
                job: j.name.clone(),
                done: c.is_some(),
                infra: red
                    .then(|| {
                        mason_adapters::ci_infra::judge(
                            c.as_deref().unwrap_or(""),
                            true,
                            log.as_deref(),
                            &[],
                        )
                    })
                    .flatten(),
                conclusion: c,
                log: log.unwrap_or_default(),
            })
        })
        .collect();
    let judged = rules::classify(&f.failures, &second);
    let grouped = rules::group(&judged, &ci.group_by);
    let workflow = run.workflow.as_str();
    let keys_of = |g: &Group| -> Vec<String> {
        let mut k: Vec<String> = g
            .legs
            .iter()
            .map(|l| mason_core::ci_watch::key(workflow, &l.job))
            .collect();
        k.push(mason_core::ci_watch::key(workflow, &g.group));
        k
    };
    let all: Vec<String> = grouped.groups.iter().flat_map(keys_of).collect();
    let named = named_keys(d, pid, &all);
    let sel = rules::select(grouped.groups.clone(), ci.max_tickets, &|g| {
        keys_of(g).iter().any(|k| named.contains_key(k))
    });
    let mut filed = vec![];
    let mut failed = vec![];
    for g in &sel.file {
        let priority = match g.kind {
            Kind::Red => Priority::Normal,
            Kind::Flaky => Priority::Low,
        };
        match emit_task(
            d,
            pid,
            title_of(workflow, g, base),
            body_of(name, workflow, g, run, base),
            priority,
            false,
        ) {
            Ok(Some(t)) => filed.push(format!(
                "#{t} ({} {})",
                if g.kind == Kind::Red { "red" } else { "flaky" },
                g.group
            )),
            Ok(None) => failed.push(g.group.clone()),
            Err(e) => {
                tracing::warn!(schedule = name, %e, "could not file a task for a scheduled CI run");
                failed.push(g.group.clone());
            }
        }
    }
    let mut parts = vec![format!(
        "run {}: {}",
        f.run_id,
        if filed.is_empty() {
            "nothing filed".to_string()
        } else {
            format!("filed {}", filed.join(", "))
        }
    )];
    if !sel.covered.is_empty() {
        parts.push(format!(
            "already named by an open task or issue: {}",
            sel.covered
                .iter()
                .map(|g| g.group.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !sel.left.is_empty() {
        parts.push(format!(
            "over the cap of {}, left unfiled: {}",
            ci.max_tickets,
            sel.left
                .iter()
                .map(|g| g.group.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !failed.is_empty() {
        parts.push(format!("could not file: {}", failed.join(", ")));
    }
    if grouped.infra + grouped.unfinished > 0 {
        parts.push(format!(
            "{} job(s) cancelled, infrastructure or unfinished, not filed",
            grouped.infra + grouped.unfinished
        ));
    }
    Ok(parts.join("; "))
}
