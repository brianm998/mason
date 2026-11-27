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
use mason_core::full_test as full;
use mason_core::*;
use mason_store::{FullTestRow, ScheduleRow};
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
    /// Cancel a run that is still going.
    fn cancel_run(&self, run: u64) -> Result<(), String>;
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
    fn cancel_run(&self, run: u64) -> Result<(), String> {
        GitHub::cancel_run(self, run).map_err(|e| e.to_string())
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
pub(crate) fn local_offset_min() -> i32 {
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
    let ci = spec.ci.as_ref();
    match ci.and_then(|c| c.at) {
        Some(at) => rules::due_at(row.last_run, now, at, local_offset_min()),
        // A schedule with only `when_quiet` has no clock of its own.
        None if ci.is_some_and(|c| !c.every && c.when_quiet.is_some()) => false,
        None => mason_core::introspect::due(Some(row.last_run), now, row.every_ms),
    }
}

// ---- the record of full tests ------------------------------------------------------------------

pub(crate) fn tested(rows: &[FullTestRow]) -> Vec<full::Tested> {
    rows.iter()
        .map(|r| full::Tested {
            commit: r.commit.clone(),
            trigger: r.trigger.clone(),
            started_ms: r.started_ms,
            finished_ms: r.finished_ms,
            verdict: r.verdict.clone(),
        })
        .collect()
}

/// A time as the status writes it: `01:00` today, `2026-11-20 01:00` before.
pub fn when_text(ms: u64, now: u64) -> String {
    let off = i64::from(local_offset_min()) * 60_000;
    let local = ms as i64 + off;
    let days = local.div_euclid(86_400_000);
    let mins = local.rem_euclid(86_400_000) / 60_000;
    let hm = format!("{:02}:{:02}", mins / 60, mins % 60);
    if days == (now as i64 + off).div_euclid(86_400_000) {
        return hm;
    }
    // Civil date from days since 1970-01-01.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year}-{month:02}-{day:02} {hm}")
}

fn project_name(d: &Daemon, pid: ProjectId) -> String {
    d.project(pid).map(|p| p.id.clone()).unwrap_or_default()
}

/// End the running full test of a schedule, if one is recorded.
fn finish_full(d: &Daemon, pid: ProjectId, schedule: &str, verdict: &str, link: &str) {
    let project = project_name(d, pid);
    let rows = d.store.full_tests(&project, schedule).unwrap_or_default();
    if let Some(r) = rows.iter().find(|r| r.finished_ms == 0) {
        let _ = d
            .store
            .finish_full_test(r.id, crate::daemon::now_ms(), verdict, link);
    }
}

/// The record says a test is running but nothing is followed (it was lost): it was cut off.
fn cut_off_lost(d: &Daemon, pid: ProjectId, schedule: &str) {
    finish_full(d, pid, schedule, full::CUT_OFF, "");
}

/// The tip is fully tested, or a full test of it is running, or the changes since the last one
/// select no suite; otherwise a full test is worth starting.
pub fn decide_full(
    d: &Daemon,
    pid: ProjectId,
    schedule: &str,
    tip: &str,
    repo: &dyn Repo,
    now: u64,
) -> full::Decision {
    let rows = d
        .store
        .full_tests(&project_name(d, pid), schedule)
        .unwrap_or_default();
    full::decide(
        tip,
        &tested(&rows),
        &|sha| repo.changed_since(sha),
        &|files| !crate::affected::reads_none(files),
        &|ms| when_text(ms, now),
    )
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
    let repo = Git { d, p: project, pid };
    let mut took = String::new();
    if !row.follow.is_empty() {
        let manual = d
            .intro
            .triggers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&crate::schedule::row_name(&project.id, &spec.name))
            .is_some_and(|(t, _)| t == full::MANUAL);
        if manual {
            if let Some(say) = take_over_outdated(d, pid, ci, &spec.name, &host, &repo, row) {
                took = format!("{say}; ");
            }
        }
    }
    if !row.follow.is_empty() {
        return (
            follow(d, pid, ci, &spec.name, &host, &repo, row, now),
            false,
        );
    }
    let (trigger, force) = d
        .intro
        .triggers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&crate::schedule::row_name(&project.id, &spec.name))
        .unwrap_or((full::NIGHTLY.into(), false));
    let go = Go {
        schedule: &spec.name,
        trigger: &trigger,
        force,
    };
    let said = dispatch_as(d, pid, ci, &host, &repo, row, now, &go);
    // A deferred run keeps its slot: the next tick asks again.
    let decided = !said.starts_with(DEFERRED);
    (format!("{took}{said}"), decided)
}

/// A manual run asked while a rerun is followed: when the rerun is of a commit develop has moved
/// past, cancel it and drop the follow, so a run on the current head can be dispatched. A rerun
/// of the current head is left alone. Says what it did, or `None` when it did nothing.
pub fn take_over_outdated(
    d: &Daemon,
    pid: ProjectId,
    ci: &CiSpec,
    name: &str,
    host: &dyn Host,
    repo: &dyn Repo,
    row: &mut ScheduleRow,
) -> Option<String> {
    let f = decode(&row.follow)?;
    if f.phase != Phase::Await2 {
        return None;
    }
    let branch = branch_of(d, pid, ci).ok()?;
    let runs = host.runs(&ci.workflow, &branch).ok()?;
    let run = runs.iter().find(|r| r.id == f.run_id)?;
    repo.fetch().ok()?;
    let tip = repo.tip().ok()?;
    if tip == run.sha {
        return None;
    }
    if run.status != "completed" {
        host.cancel_run(f.run_id).ok()?;
    }
    finish_full(d, pid, name, full::CUT_OFF, "");
    row.follow.clear();
    Some(format!(
        "cancelled the rerun of run {} (commit {} is behind develop's head); dispatching on the current head",
        f.run_id,
        run.sha.chars().take(9).collect::<String>()
    ))
}

// ---- the `suites` kind: MASON's own full test (#783) --------------------------------------------

/// Run a `suites` schedule: every listed suite whole on the base tip, here or on GitHub. The
/// same triggers, record and skip rule as the `ci` kind ([`decide_full`]); the run is the debt
/// payment's per-suite run ([`crate::suites::run_whole`]), judged the same way.
pub fn run_suites(d: &Arc<Daemon>, project: &ProjectCfg, spec: &Spec, now: u64) -> String {
    let Some(ci) = spec.ci.as_ref() else {
        return "not a suites schedule".into();
    };
    let Some(pid) = d.project_id_by_name(&project.id) else {
        return "no such project".into();
    };
    let repo = Git { d, p: project, pid };
    let (trigger, force) = d
        .intro
        .triggers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&crate::schedule::row_name(&project.id, &spec.name))
        .unwrap_or((full::NIGHTLY.into(), false));
    let go = Go {
        schedule: &spec.name,
        trigger: &trigger,
        force,
    };
    suites_as(d, pid, ci, &repo, now, &go)
}

/// Decide with the record, run the suites, write the verdict, file what went red or flaked.
pub fn suites_as(
    d: &Arc<Daemon>,
    pid: ProjectId,
    ci: &CiSpec,
    repo: &dyn Repo,
    now: u64,
    asked: &Go,
) -> String {
    use crate::suites::WholeState;
    let go = || -> Result<String, String> {
        repo.fetch()?;
        let tip = repo.tip()?;
        // Nothing runs from an earlier daemon, so a row still open was cut off by a restart.
        cut_off_lost(d, pid, asked.schedule);
        let record = decide_full(d, pid, asked.schedule, &tip, repo, now);
        if !record.start && !asked.force {
            return Ok(format!("not run: {}", record.reason));
        }
        let project = project_name(d, pid);
        let _ = d.store.add_full_test(&FullTestRow {
            project,
            schedule: asked.schedule.to_string(),
            commit: tip.clone(),
            trigger: asked.trigger.to_string(),
            started_ms: now,
            ..Default::default()
        });
        let whole = match crate::suites::run_whole(d, pid, &ci.suites, &tip, asked.force, &|| {
            !d.is_shutting_down()
        }) {
            Ok(w) => w,
            Err(e) => {
                finish_full(d, pid, asked.schedule, full::CUT_OFF, "");
                return Err(e);
            }
        };
        let verdict = if whole.red() { full::RED } else { full::GREEN };
        // The range names the last green before this commit, so it is read before the row ends.
        let range = range_note(d, pid, asked.schedule, &whole.tip);
        finish_full(d, pid, asked.schedule, verdict, "");
        let base = d.project(pid).map(|p| p.base.clone()).unwrap_or_default();
        let mut filed = vec![];
        for s in &whole.suites {
            let (what, priority) = match s.state {
                WholeState::Red => ("red", Priority::Normal),
                WholeState::Flaky => ("flaky", Priority::Low),
                _ => continue,
            };
            let key = format!("suite `{}` {what} on {base}", s.name);
            if !named_keys(d, pid, std::slice::from_ref(&key)).is_empty() {
                filed.push(format!("{} {what} (already named by an open task)", s.name));
                continue;
            }
            let failing = if s.failing.is_empty() {
                String::new()
            } else {
                format!("\n\nFailing: {}.", s.failing.join(", "))
            };
            let body = format!(
                "The scheduled full test `{}` ran suite `{}` whole on `{base}` at {} and it was {what} (a red test is rerun once before it counts; a flake passed on that rerun).{failing}\n\n{}\n\n{range}\n\nFiled by the schedule `{}`.",
                asked.schedule,
                s.name,
                whole.tip,
                s.note,
                asked.schedule,
            );
            match emit_task(d, pid, key, body, priority, false) {
                Ok(Some(t)) => filed.push(format!("#{t} ({} {what})", s.name)),
                _ => filed.push(format!("{} {what} (could not file)", s.name)),
            }
        }
        let counts = |st: WholeState| whole.suites.iter().filter(|s| s.state == st).count();
        Ok(format!(
            "ran {} suite(s) on {}: {verdict} ({} green, {} already green, {} flaky, {} excused, {} red){}",
            whole.suites.len(),
            full::short(&whole.tip),
            counts(WholeState::Green),
            counts(WholeState::AlreadyGreen),
            counts(WholeState::Flaky),
            counts(WholeState::Excused),
            counts(WholeState::Red),
            if filed.is_empty() {
                String::new()
            } else {
                format!("; filed {}", filed.join(", "))
            }
        ))
    };
    go().unwrap_or_else(|e| format!("could not run: {e}"))
}

/// Why and how a full test is being asked for.
pub struct Go<'a> {
    pub schedule: &'a str,
    /// `nightly`, `quiet` or `manual`.
    pub trigger: &'a str,
    /// Start whatever the record says.
    pub force: bool,
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

/// Decide, and dispatch when it is worth a run. Starts following the new run. The schedule's
/// own timer asked: the trigger is `nightly`.
pub fn dispatch(
    d: &Arc<Daemon>,
    pid: ProjectId,
    ci: &CiSpec,
    host: &dyn Host,
    repo: &dyn Repo,
    row: &mut ScheduleRow,
    now: u64,
) -> String {
    let name = row.name.rsplit('/').next().unwrap_or("").to_string();
    let go = Go {
        schedule: &name,
        trigger: full::NIGHTLY,
        force: false,
    };
    dispatch_as(d, pid, ci, host, repo, row, now, &go)
}

/// What a deferred full-CI dispatch says first (s1218).
const DEFERRED: &str = "deferred: ";

/// Whether a non-forced full-CI run should wait (s1218): task work waits for GitHub runner
/// units, or the `gha` pool is used beyond `share_pct` percent of `limit`. `None` means start.
/// A run already dispatched cannot be capped; this only decides when it starts.
pub fn full_start_deferred(
    used: u32,
    limit: u32,
    share_pct: u32,
    task_waiting: u32,
) -> Option<String> {
    if task_waiting > 0 {
        return Some(format!(
            "{task_waiting} task request(s) wait for GitHub runner units"
        ));
    }
    if u64::from(used) * 100 > u64::from(limit) * u64::from(share_pct) {
        return Some(format!(
            "{used} of {limit} gha units are in use, above the full-CI share of {share_pct}%"
        ));
    }
    None
}

/// [`dispatch`] with the trigger named. Both triggers and the manual one come through here, so
/// one rule decides: the record of full tests first, then what the CI host knows.
#[allow(clippy::too_many_arguments)]
pub fn dispatch_as(
    d: &Arc<Daemon>,
    pid: ProjectId,
    ci: &CiSpec,
    host: &dyn Host,
    repo: &dyn Repo,
    row: &mut ScheduleRow,
    now: u64,
    asked: &Go,
) -> String {
    let mut go = || -> Result<String, String> {
        let branch = branch_of(d, pid, ci)?;
        repo.fetch()?;
        let tip = repo.tip()?;
        // Nothing is followed (the caller followed first), so a row still running was lost.
        cut_off_lost(d, pid, asked.schedule);
        let record = decide_full(d, pid, asked.schedule, &tip, repo, now);
        if !record.start && !asked.force {
            return Ok(format!("not dispatched: {}", record.reason));
        }
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
        if !decision.dispatch && !asked.force {
            return Ok(format!("not dispatched: {}", decision.reason));
        }
        if !asked.force {
            let (used, limit, share) = d.broker.gha_use();
            if let Some(why) =
                full_start_deferred(used, limit, share, d.broker.gha_task_waiting() as u32)
            {
                return Ok(format!("{DEFERRED}{why}"));
            }
        }
        host.dispatch(&ci.workflow, &branch, &ci.inputs)?;
        let _ = d.store.add_full_test(&FullTestRow {
            project: project_name(d, pid),
            schedule: asked.schedule.to_string(),
            commit: tip.clone(),
            trigger: asked.trigger.to_string(),
            started_ms: now,
            ..Default::default()
        });
        row.follow = encode(&Follow {
            run_id: 0,
            phase: Phase::Locate,
            at: now,
            before: runs.iter().map(|r| r.id).collect(),
            failures: vec![],
        });
        Ok(format!(
            "dispatched {} on {branch} ({}): {}",
            ci.workflow,
            asked.trigger,
            if decision.dispatch {
                decision.reason
            } else {
                format!("forced; {}", decision.reason)
            }
        ))
    };
    go().unwrap_or_else(|e| format!("could not dispatch: {e}"))
}

fn is_green(c: &str) -> bool {
    matches!(c, "success" | "neutral" | "skipped")
}

/// Move the followed run one step. Nothing here waits: the next tick looks again.
#[allow(clippy::too_many_arguments)]
pub fn follow(
    d: &Arc<Daemon>,
    pid: ProjectId,
    ci: &CiSpec,
    name: &str,
    host: &dyn Host,
    repo: &dyn Repo,
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
        (Step::Drop, _) => {
            finish_full(d, pid, name, full::CUT_OFF, "");
            format!("dropped run {}: it did not finish in time", f.run_id)
        }
        (Step::ByHand, _) => {
            finish_full(d, pid, name, full::CUT_OFF, "");
            format!(
                "run {} was rerun by hand; not classified (its first attempt's failures are gone)",
                f.run_id
            )
        }
        (Step::Check, Some(r)) if r.conclusion.as_deref() == Some("cancelled") => {
            // Someone cancelled the run (a job cut off at its time limit leaves the run
            // `failure`, not `cancelled`): nothing failed, so nothing is rerun or filed.
            finish_full(d, pid, name, full::CUT_OFF, "");
            format!("run {} was cancelled; not rerun, nothing filed", f.run_id)
        }
        (Step::Check, Some(r)) => {
            let outdated = repo.fetch().is_ok() && repo.tip().is_ok_and(|t| t != r.sha);
            match check(host, &mut f, outdated) {
                Ok(Checked::Rerunning(say)) => {
                    row.follow = encode(&f);
                    return say;
                }
                Ok(Checked::Superseded(say)) => {
                    finish_full(d, pid, name, full::CUT_OFF, "");
                    say
                }
                Ok(Checked::Green) => {
                    finish_full(
                        d,
                        pid,
                        name,
                        full::GREEN,
                        run.map_or("", |r| r.url.as_str()),
                    );
                    format!("run {} is green", f.run_id)
                }
                Err(e) => {
                    // Not asked again until it answers; give up when it never does.
                    if now.saturating_sub(f.at) > rules::GIVE_UP_MS {
                        finish_full(d, pid, name, full::CUT_OFF, "");
                        row.follow.clear();
                        return format!("dropped run {}: {e}", f.run_id);
                    }
                    row.follow = encode(&f);
                    return format!("run {}: {e}; will try again", f.run_id);
                }
            }
        }
        (Step::Classify, Some(r)) => {
            match classify_and_file(d, pid, ci, name, host, &f, r, &branch) {
                Ok((say, verdict)) => {
                    finish_full(d, pid, name, verdict, &r.url);
                    say
                }
                Err(e) => {
                    if now.saturating_sub(f.at) > rules::GIVE_UP_MS {
                        finish_full(d, pid, name, full::CUT_OFF, "");
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
fn check(host: &dyn Host, f: &mut Follow, outdated: bool) -> Result<Checked, String> {
    let jobs = host.jobs(f.run_id)?;
    let red: Vec<&ActionsJob> = jobs
        .iter()
        .filter(|j| j.conclusion.as_deref().is_some_and(|c| !is_green(c)))
        .collect();
    if red.is_empty() {
        return Ok(Checked::Green);
    }
    if outdated {
        // Develop has moved on: a rerun would spend runners on code that may be fixed. The
        // schedule dispatches on the current head at its next chance.
        return Ok(Checked::Superseded(format!(
            "run {}: {} job(s) failed, but develop has moved past its commit; superseded, not rerun, nothing filed",
            f.run_id,
            red.len()
        )));
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
    Ok(Checked::Rerunning(format!(
        "run {}: {} job(s) failed; rerunning them once",
        f.run_id,
        f.failures.len()
    )))
}

/// What looking at a finished first attempt found.
enum Checked {
    Green,
    Rerunning(String),
    Superseded(String),
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
) -> Result<(String, &'static str), String> {
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
    let range = range_note(d, pid, name, &run.sha);
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
            format!("{}\n\n{range}", body_of(name, workflow, g, run, base)),
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
    let verdict = if grouped.groups.iter().any(|g| g.kind == Kind::Red) {
        full::RED
    } else if grouped.groups.is_empty() && grouped.infra + grouped.unfinished > 0 {
        full::CUT_OFF
    } else {
        full::GREEN
    };
    Ok((parts.join("; "), verdict))
}

/// For a red ticket: the commits since the last green full test and the tasks landed in them
/// whose changes select suites, so the developer starts from a short list. Best effort: a
/// repository that cannot answer says so instead of failing the filing.
fn range_note(d: &Daemon, pid: ProjectId, schedule: &str, sha: &str) -> String {
    let Some(p) = d.project(pid) else {
        return String::new();
    };
    let rows = d
        .store
        .full_tests(&project_name(d, pid), schedule)
        .unwrap_or_default();
    let Some(green) = rows
        .iter()
        .find(|r| r.verdict == full::GREEN && r.commit != sha)
    else {
        return "Since the last green full test: none is on record, so there is no range to name."
            .into();
    };
    let range = format!("{}..{sha}", green.commit);
    let git = |args: &[&str]| mason_landing::git::run(&p.path, args).map_err(|e| e.to_string());
    let count = git(&["rev-list", "--count", &range]).map(|s| s.trim().to_string());
    let merges = git(&["log", "--merges", "--format=%H\t%s", &range]);
    let (Ok(count), Ok(merges)) = (count, merges) else {
        return format!(
            "Since the last green full test {} the range {range} could not be read here.",
            full::short(&green.commit)
        );
    };
    let mut selecting = vec![];
    let mut quiet = vec![];
    for line in merges.lines() {
        let Some((hash, subject)) = line.split_once('\t') else {
            continue;
        };
        let tasks = full::tasks_in(&[subject.to_string()]);
        let Some(t) = tasks.first() else { continue };
        let files = git(&["diff", "--name-only", &format!("{hash}^1"), hash])
            .map(|o| o.lines().map(String::from).collect::<Vec<_>>())
            .unwrap_or_default();
        if crate::affected::reads_none(&files) {
            quiet.push(*t);
        } else {
            selecting.push(*t);
        }
    }
    let list = |v: &[u64]| {
        v.iter()
            .take(30)
            .map(|t| format!("#{t}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "Since the last green full test ({} at {}): {count} commit(s), `{range}`.\nTasks landed in it whose changes select suites: {}.\nLanded in it with only docs or other paths no suite reads: {}.",
        full::short(&green.commit),
        when_text(green.finished_ms, crate::daemon::now_ms()),
        if selecting.is_empty() { "none found".into() } else { list(&selecting) },
        if quiet.is_empty() { "none".into() } else { list(&quiet) },
    )
}

// ---- showing the record ----------------------------------------------------------------------

/// What `mason status`, `mason schedule list` and the monitor show for one full-test schedule.
pub fn full_view(d: &Daemon, project: &ProjectCfg, spec: &Spec) -> Value {
    let now = crate::daemon::now_ms();
    let Some(ci) = spec.ci.as_ref() else {
        return Value::Null;
    };
    let rows = d
        .store
        .full_tests(&project.id, &spec.name)
        .unwrap_or_default();
    let last = rows
        .iter()
        .find(|r| r.finished_ms != 0 && (r.verdict == full::GREEN || r.verdict == full::RED));
    let running = rows.iter().find(|r| r.finished_ms == 0);
    let git = |args: &[&str]| mason_landing::git::run(&project.path, args).ok();
    let base = project.base_ref();
    let (commits, tasks) = match last {
        Some(l) => {
            let range = format!("{}..{base}", l.commit);
            (
                git(&["rev-list", "--count", &range]).and_then(|s| s.trim().parse::<u64>().ok()),
                git(&["log", "--format=%s", &range]).map(|o| {
                    full::tasks_in(&o.lines().map(String::from).collect::<Vec<_>>()).len()
                }),
            )
        }
        None => (None, None),
    };
    let name = crate::schedule::row_name(&project.id, &spec.name);
    let quiet_min = ci.when_quiet.and_then(|_| {
        d.intro
            .quiet
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&name)
            .and_then(|t| t.quiet_min(now))
    });
    let next = full::next_text(ci.at, ci.when_quiet, quiet_min);
    let mut line = match last {
        Some(l) => format!(
            "last full test: {} at {} {} ({}); {} commit(s), {} task(s) landed since",
            full::short(&l.commit),
            when_text(l.finished_ms, now),
            l.verdict,
            l.trigger,
            commits.map_or("?".into(), |n| n.to_string()),
            tasks.map_or("?".into(), |n| n.to_string()),
        ),
        None => "last full test: none on record".to_string(),
    };
    if let Some(r) = running {
        line += &format!(
            "; running now: {} ({}) since {}",
            full::short(&r.commit),
            r.trigger,
            when_text(r.started_ms, now)
        );
    }
    line += &format!("; next: {next}");
    json!({
        "project": project.id,
        "schedule": spec.name,
        "line": line,
        "next": next,
        "last": last.map(|l| json!({
            "commit": l.commit, "finished_ms": l.finished_ms, "verdict": l.verdict,
            "trigger": l.trigger, "link": l.link,
        })),
        "running": running.map(|r| json!({
            "commit": r.commit, "started_ms": r.started_ms, "trigger": r.trigger,
        })),
        "commits_since": commits,
        "tasks_since": tasks,
        "quiet_min": quiet_min,
    })
}

/// [`full_view`] for a schedule row (`project/name`); null for anything else.
pub fn full_view_of_row(d: &Daemon, row: &ScheduleRow) -> Value {
    if row.kind != "ci" && row.kind != "suites" {
        return Value::Null;
    }
    let Some((p, s)) = row.name.split_once('/') else {
        return Value::Null;
    };
    d.project_id_by_name(p)
        .and_then(|pid| d.project(pid))
        .and_then(|pc| {
            pc.schedules()
                .into_iter()
                .find(|x| x.name == s)
                .map(|spec| full_view(d, pc, &spec))
        })
        .unwrap_or(Value::Null)
}

/// Every full-test schedule's view, for `mason status`.
pub fn full_views(d: &Daemon) -> Vec<Value> {
    let mut out = vec![];
    for p in d.projects().values() {
        for spec in p.schedules() {
            if spec.kind == "ci" || spec.kind == "suites" {
                out.push(full_view(d, p, &spec));
            }
        }
    }
    out
}

#[cfg(test)]
mod share_tests {
    use super::full_start_deferred;

    #[test]
    fn waiting_task_work_defers() {
        assert!(full_start_deferred(0, 16, 50, 1).is_some());
    }

    #[test]
    fn use_over_the_share_defers_and_at_the_share_starts() {
        assert!(full_start_deferred(9, 16, 50, 0).is_some());
        assert!(full_start_deferred(8, 16, 50, 0).is_none());
    }

    #[test]
    fn idle_pool_with_no_waiters_starts() {
        assert!(full_start_deferred(0, 16, 50, 0).is_none());
    }

    #[test]
    fn a_zero_limit_starts_only_when_nothing_is_used() {
        assert!(full_start_deferred(0, 0, 50, 0).is_none());
        assert!(full_start_deferred(1, 0, 50, 0).is_some());
    }
}
