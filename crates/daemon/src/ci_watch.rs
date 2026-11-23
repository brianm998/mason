// SPDX-License-Identifier: GPL-3.0-or-later

//! The base-branch CI watch (s154, decision 0149): reads the CI host's own runs on the base
//! branch, files a task for a new red and, for a red gate, freezes the project's landing until
//! that job is green on the base again. Off unless the definition has `[watch.ci] enabled = true`.
//! The rules are in `mason_core::ci_watch`; this is the reading, the filing and the keeping.

use crate::config::CiKind;
use crate::daemon::{now_ms, Daemon};
use mason_adapter_api::github_budget;
use mason_adapter_api::http::Curl;
use mason_adapters::github::{ActionsJob, ActionsRun, GitHub};
use mason_core::ci_watch::{
    self as rules, Ctx, File, Filed, JobRec, JobState, RecentTask, RunRec, RunRef, Why,
};
use mason_core::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

/// How long a landed or closed task still counts as having covered a red (s551).
pub const LOOKBACK_MS: u64 = 7 * 24 * 3_600_000;

/// Lines of a failed job's log put in the task.
const LOG_LINES: usize = 60;

/// Where the runs come from. The GitHub adapter is one; tests give another.
pub trait Source {
    fn runs(&self, branch: &str) -> Result<Vec<ActionsRun>, String>;
    fn jobs(&self, run: u64) -> Result<Vec<ActionsJob>, String>;
    fn log_tail(&self, job: u64) -> Option<String>;
    /// The annotation messages of a job (where GitHub says it hit its time limit).
    fn annotations(&self, _job: u64) -> Option<String> {
        None
    }
    /// A longer end of the job's log, for finding which tests failed.
    fn log_long(&self, job: u64) -> Option<String> {
        self.log_tail(job)
    }
}

impl Source for GitHub {
    fn runs(&self, branch: &str) -> Result<Vec<ActionsRun>, String> {
        self.base_runs(branch).map_err(|e| e.to_string())
    }
    fn jobs(&self, run: u64) -> Result<Vec<ActionsJob>, String> {
        self.run_jobs(run).map_err(|e| e.to_string())
    }
    fn log_tail(&self, job: u64) -> Option<String> {
        self.job_log_tail(job, LOG_LINES)
    }
    fn log_long(&self, job: u64) -> Option<String> {
        self.job_log_tail(job, 4000)
    }
    fn annotations(&self, job: u64) -> Option<String> {
        self.job_annotations(job)
    }
}

/// Reads each job's long log once per pass; the short tail is its last lines. The signatures
/// and the filed body come from the same fetch.
struct OnceLog<'a> {
    inner: &'a dyn Source,
    logs: std::cell::RefCell<std::collections::HashMap<u64, Option<String>>>,
}

impl Source for OnceLog<'_> {
    fn runs(&self, branch: &str) -> Result<Vec<ActionsRun>, String> {
        self.inner.runs(branch)
    }
    fn jobs(&self, run: u64) -> Result<Vec<ActionsJob>, String> {
        self.inner.jobs(run)
    }
    fn annotations(&self, job: u64) -> Option<String> {
        self.inner.annotations(job)
    }
    fn log_long(&self, job: u64) -> Option<String> {
        self.logs
            .borrow_mut()
            .entry(job)
            .or_insert_with(|| self.inner.log_long(job))
            .clone()
    }
    fn log_tail(&self, job: u64) -> Option<String> {
        let long = self.log_long(job)?;
        let lines: Vec<&str> = long.lines().collect();
        Some(lines[lines.len().saturating_sub(LOG_LINES)..].join("\n"))
    }
}

/// What GitHub writes on a job it cut off at its `timeout-minutes`.
pub const TIMEOUT_NOTE: &str = "exceeded the maximum execution time";

/// The conclusion the rules should see for a job (decision 0660). GitHub reports a job that
/// hit its time limit as `cancelled`, the same as a job of a run a newer push superseded. On
/// the base branch (`cancel-in-progress` is off) a `cancelled` job inside a run that finished
/// `failure` or `success` is not superseded; it is a timeout when its annotations say so.
/// The annotation text is data from GitHub, not a measured duration.
pub fn job_conclusion(
    run_conclusion: Option<&str>,
    job_conclusion: Option<&str>,
    annotations: impl FnOnce() -> Option<String>,
) -> Option<String> {
    if job_conclusion == Some("cancelled")
        && matches!(run_conclusion, Some("failure" | "success"))
        && annotations().is_some_and(|a| a.contains(TIMEOUT_NOTE))
    {
        return Some("timed_out".into());
    }
    job_conclusion.map(String::from)
}

/// The `== scripts/exit/sNN.sh` lines of a log, with whatever timestamp starts each line.
fn script_lines(log: &str) -> Vec<&str> {
    log.lines()
        .filter(|l| l.contains("== scripts/exit/"))
        .collect()
}

/// How a red landing batch stands against the base branch's own CI (s174): each failed job
/// is looked up in the base's latest finished run of the same workflow, and the failing
/// tests of the two logs are compared. A base that cannot be read excuses nothing.
pub fn assess(
    src: &dyn Source,
    base: &str,
    jobs: &[mason_adapter_api::service::JobFailure],
) -> mason_core::red_base::Verdict {
    use mason_core::red_base as rb;
    let Ok(runs) = src.runs(base) else {
        return rb::Verdict::Blame;
    };
    let mut out = vec![];
    for j in jobs {
        // `runs` is newest first: the latest finished run of the workflow.
        let latest = runs
            .iter()
            .find(|r| r.workflow == j.workflow && r.conclusion.is_some());
        let state = match latest {
            None => rb::Base::Unknown,
            Some(r) => match src.jobs(r.id) {
                Err(_) => rb::Base::Unknown,
                Ok(bj) => match bj.iter().find(|b| b.name == j.job) {
                    None => rb::Base::Unknown,
                    Some(b) => match b.conclusion.as_deref() {
                        Some("success" | "neutral" | "skipped") => rb::Base::Green,
                        Some("failure") => rb::Base::Red(src.log_long(b.id)),
                        _ => rb::Base::Unknown,
                    },
                },
            },
        };
        out.push(rb::Job {
            workflow: j.workflow.clone(),
            job: j.job.clone(),
            log: j.log.clone(),
            base: state,
        });
    }
    rb::judge(&out)
}

/// Which of the named CI jobs are red in the base branch's latest finished run that has them
/// (s213): the landing's red ladder does not search for a culprit of a red the base already
/// has. A base that cannot be read, or has no such job, excuses nothing (the job is left out).
pub fn base_red_jobs(src: &dyn Source, base: &str, names: &[String]) -> Vec<String> {
    let Ok(runs) = src.runs(base) else {
        return vec![];
    };
    let finished: Vec<&ActionsRun> = runs.iter().filter(|r| r.conclusion.is_some()).collect();
    names
        .iter()
        .filter(|n| {
            // Newest first: the first finished run that has the job says how it stands.
            let state = finished.iter().take(10).find_map(|r| {
                src.jobs(r.id).ok().and_then(|jobs| {
                    jobs.iter()
                        .find(|b| &b.name == *n)
                        .map(|b| b.conclusion.as_deref() == Some("failure"))
                })
            });
            state == Some(true)
        })
        .cloned()
        .collect()
}

/// What is kept between passes, and what the status shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Saved {
    pub state: rules::State,
    /// Why the last pass could not look, until one can.
    pub error: Option<String>,
    pub checked_at: u64,
}

fn opt(v: Option<u64>) -> Value {
    v.map_or(Value::Null, |n| json!(n))
}

pub fn encode(s: &Saved) -> String {
    let wfs: serde_json::Map<String, Value> = s
        .state
        .workflows
        .iter()
        .map(|(w, ws)| {
            let jobs: serde_json::Map<String, Value> = ws
                .jobs
                .iter()
                .map(|(j, js)| {
                    (
                        j.clone(),
                        json!({
                            "red": js.red, "known": js.known, "red_since": js.red_since,
                            "pending": js.pending,
                            "run": js.last_red.run, "job": js.last_red.job,
                            "sha": js.last_red.sha, "url": js.last_red.url,
                            "ticket": opt(js.ticket), "checked": opt(js.checked),
                            "freeze": js.freeze,
                            "covered": js.covered, "landed_ms": opt(js.landed_ms),
                            "at_ms": js.last_red.at_ms,
                            "timeouts": js.timeouts, "timeout_filed": js.timeout_filed,
                            "signatures": js.signatures,
                            "t_run": js.last_timeout.run, "t_job": js.last_timeout.job,
                            "t_sha": js.last_timeout.sha, "t_url": js.last_timeout.url,
                            "t_at_ms": js.last_timeout.at_ms,
                            "absent": js.absent, "full": js.full,
                        }),
                    )
                })
                .collect();
            (w.clone(), json!({"last_run": ws.last_run, "jobs": jobs}))
        })
        .collect();
    let filed: Vec<Value> = s
        .state
        .filed
        .iter()
        .map(|f| {
            json!({"key": f.key, "task": opt(f.task), "at": f.at, "urgent": f.urgent,
                   "proposed": f.proposed, "auto": f.auto, "url": f.url})
        })
        .collect();
    json!({"workflows": wfs, "filed": filed, "error": s.error, "checked_at": s.checked_at})
        .to_string()
}

pub fn decode(text: &str) -> Saved {
    let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let u = |x: &Value| x.as_u64().unwrap_or(0);
    let mut s = Saved {
        error: v["error"].as_str().map(String::from),
        checked_at: u(&v["checked_at"]),
        ..Saved::default()
    };
    for (w, wv) in v["workflows"].as_object().into_iter().flatten() {
        let mut ws = rules::WorkflowState {
            last_run: u(&wv["last_run"]),
            ..Default::default()
        };
        for (j, jv) in wv["jobs"].as_object().into_iter().flatten() {
            ws.jobs.insert(
                j.clone(),
                JobState {
                    red: jv["red"].as_bool().unwrap_or(false),
                    known: jv["known"].as_bool().unwrap_or(false),
                    red_since: u(&jv["red_since"]),
                    pending: jv["pending"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(u)
                        .collect(),
                    last_red: RunRef {
                        run: u(&jv["run"]),
                        job: u(&jv["job"]),
                        sha: jv["sha"].as_str().unwrap_or_default().to_string(),
                        url: jv["url"].as_str().unwrap_or_default().to_string(),
                        at_ms: u(&jv["at_ms"]),
                    },
                    signatures: jv["signatures"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect(),
                    ticket: jv["ticket"].as_u64(),
                    checked: jv["checked"].as_u64(),
                    freeze: jv["freeze"].as_bool().unwrap_or(false),
                    covered: u(&jv["covered"]),
                    landed_ms: jv["landed_ms"].as_u64(),
                    timeouts: jv["timeouts"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(u)
                        .collect(),
                    timeout_filed: jv["timeout_filed"].as_bool().unwrap_or(false),
                    absent: jv["absent"].as_u64().unwrap_or(0) as u32,
                    full: jv["full"].as_bool().unwrap_or(false),
                    last_timeout: RunRef {
                        run: u(&jv["t_run"]),
                        job: u(&jv["t_job"]),
                        sha: jv["t_sha"].as_str().unwrap_or_default().to_string(),
                        url: jv["t_url"].as_str().unwrap_or_default().to_string(),
                        at_ms: u(&jv["t_at_ms"]),
                    },
                },
            );
        }
        s.state.workflows.insert(w.clone(), ws);
    }
    for f in v["filed"].as_array().into_iter().flatten() {
        s.state.filed.push(Filed {
            key: f["key"].as_str().unwrap_or_default().to_string(),
            task: f["task"].as_u64(),
            at: u(&f["at"]),
            urgent: f["urgent"].as_bool().unwrap_or(false),
            proposed: f["proposed"].as_bool().unwrap_or(false),
            auto: f["auto"].as_bool().unwrap_or(false),
            url: f["url"].as_str().unwrap_or_default().to_string(),
        });
    }
    s
}

fn load(d: &Daemon, project: &str) -> Saved {
    d.store
        .ci_watch_state(project)
        .ok()
        .flatten()
        .map(|t| decode(&t))
        .unwrap_or_default()
}

fn save(d: &Daemon, project: &str, s: &Saved) {
    if let Err(e) = d.store.put_ci_watch_state(project, &encode(s), now_ms()) {
        tracing::warn!(project, "cannot keep the base-branch CI watch state: {e}");
    }
}

/// Whether five minutes have passed since the last look at CI runs for suite proof.
fn ci_proof_due() -> bool {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = now_ms();
    let last = LAST.load(Ordering::SeqCst);
    now.saturating_sub(last) >= 300_000
        && LAST
            .compare_exchange(last, now, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
}

/// Start the watch. One loop looks at every project; a project whose definition has no
/// `[watch.ci]` costs nothing (no call to the CI host).
pub fn spawn(d: &Arc<Daemon>) {
    crate::bg::spawn_loop(d, "ci-watch", move |weak| {
        let Some(d) = weak.upgrade() else {
            return false;
        };
        if let Some(w) = github_budget::wait_s() {
            // The GitHub quota is used up until its reset: nothing is asked, and nothing is
            // logged for it (the alert says it once).
            drop(d);
            std::thread::sleep(Duration::from_secs(w.min(60)));
            return true;
        }
        let mut every = 20;
        for (pid, p) in &d.projects() {
            // Suites the CI jobs cover are recorded green from its runs, at most every five
            // minutes (#774); it needs no `[watch.ci]`.
            if d.current_def(*pid)
                .testing
                .suites
                .iter()
                .any(|s| !s.ci_jobs.is_empty())
                && ci_proof_due()
            {
                if let Ok((gh, _)) = source(p, "ci_jobs") {
                    crate::suites::record_ci_greens(&d, *pid, &gh);
                }
            }
            let Some(rules) = d.current_def(*pid).watch_ci.clone() else {
                // Switched off: a freeze it held does not outlive it.
                if d.store.clear_ci_freeze(&p.id).unwrap_or(false) {
                    d.landing.wake();
                    d.poke();
                }
                continue;
            };
            let gh = match source(p, "[watch.ci]") {
                Ok((gh, poll_s)) => {
                    every = every.min(poll_s.max(1));
                    gh
                }
                Err(e) => {
                    let mut s = load(&d, &p.id);
                    if s.error.as_deref() != Some(e.as_str()) {
                        s.error = Some(e);
                        save(&d, &p.id, &s);
                    }
                    continue;
                }
            };
            if let Err(e) = pass(&d, *pid, &rules, &gh, now_ms()) {
                let e = match github_budget::exhausted_until() {
                    Some(t) => crate::bg::quota_text(t),
                    None => {
                        tracing::warn!(project = %p.id, "base-branch CI watch: {e}");
                        e
                    }
                };
                let mut s = load(&d, &p.id);
                s.error = Some(e);
                s.checked_at = now_ms();
                save(&d, &p.id, &s);
            }
        }
        drop(d);
        // The CI history is the first to slow down when the quota runs short.
        std::thread::sleep(Duration::from_secs(github_budget::paced(
            every,
            github_budget::Priority::Background,
        )));
        true
    });
}

/// The GitHub Actions host of a project, or why there is none: the watch reads runs, so
/// it needs the project's `[ci]` to be `github-actions`.
pub(crate) fn source(p: &crate::config::ProjectCfg, what: &str) -> Result<(GitHub, u64), String> {
    match p.services.ci.as_ref() {
        Some(c) => match &c.kind {
            CiKind::GithubActions {
                api, repo, token, ..
            } => Ok((
                GitHub::new(
                    Arc::new(Curl::default()),
                    api,
                    repo,
                    crate::services::token(token)?,
                ),
                c.poll_s,
            )),
            _ => Err(format!(
                "{what} needs a github-actions [ci] section; this project's [ci] is another kind"
            )),
        },
        None => Err(format!(
            "{what} needs a github-actions [ci] section; this project has none"
        )),
    }
}

/// State saved before s928 keyed jobs by the run's display title (a commit message) instead of
/// the workflow's name. Such an entry is dropped, to be rebuilt from the next runs under the
/// workflow's name: one whose name has a line break, or whose last run is in `runs` under a
/// different workflow name. Returns the names dropped.
pub fn migrate_keys(saved: &mut Saved, runs: &[ActionsRun]) -> Vec<String> {
    let stale: Vec<String> = saved
        .state
        .workflows
        .iter()
        .filter(|(w, ws)| {
            w.contains('\n')
                || runs
                    .iter()
                    .any(|r| r.id == ws.last_run && r.workflow != **w)
        })
        .map(|(w, _)| w.clone())
        .collect();
    for w in &stale {
        saved.state.workflows.remove(w);
    }
    stale
}

/// One look at the base branch's runs, one pass of the rules, and the filing and freezing that
/// follow. State is kept only when everything it decided was done, so a failed pass is repeated.
pub fn pass(
    d: &Arc<Daemon>,
    pid: ProjectId,
    r: &rules::Rules,
    src: &dyn Source,
    now: u64,
) -> Result<(), String> {
    let once = OnceLog {
        inner: src,
        logs: Default::default(),
    };
    let src: &dyn Source = &once;
    let p = d.project(pid).ok_or("no such project")?;
    let mut saved = load(d, &p.id);
    let runs = src.runs(&p.base)?;
    migrate_keys(&mut saved, &runs);
    let mut recs: Vec<RunRec> = runs
        .iter()
        .map(|a| RunRec {
            id: a.id,
            workflow: a.workflow.clone(),
            sha: a.sha.clone(),
            url: a.url.clone(),
            created_ms: a.created_ms,
            conclusion: a.conclusion.clone(),
            // A dispatched run is the full one (s800): push runs lack its jobs.
            full: a.event == "workflow_dispatch",
            jobs: vec![],
        })
        .collect();
    for id in rules::needs_jobs(&saved.state, r, &recs) {
        let jobs = src.jobs(id)?;
        if let Some(rec) = recs.iter_mut().find(|x| x.id == id) {
            let rc = rec.conclusion.clone();
            rec.jobs = jobs
                .into_iter()
                .map(|j| JobRec {
                    id: j.id,
                    conclusion: job_conclusion(rc.as_deref(), j.conclusion.as_deref(), || {
                        src.annotations(j.id)
                    }),
                    // What failed, for the title and for finding the same failure again.
                    signatures: if j.conclusion.as_deref() == Some("failure") {
                        src.log_long(j.id)
                            .map(|l| rules::signatures(&l))
                            .unwrap_or_default()
                    } else {
                        vec![]
                    },
                    name: j.name,
                })
                .collect();
        }
    }
    let (named, open_auto, open_approved) = who_names(d, pid, &saved, &recs);
    let (recent, landed) = recent_tasks(d, pid, now);
    let named_sigs = who_names_signatures(d, pid, &saved, &recs);
    let files = rules::step(
        &mut saved.state,
        &recs,
        &Ctx {
            rules: r,
            now_ms: now,
            named: &named,
            open_auto,
            open_approved,
            recent: &recent,
            landed: &landed,
            named_sigs: &named_sigs,
        },
    );
    for f in &files {
        let mut f = f.clone();
        if f.auto && names_trust_file(&f, src) {
            // A trust file stays with the owner, whatever the rule says.
            f.auto = false;
            f.proposed = true;
        }
        let f = &f;
        let task = file_task(d, pid, &p.base, f, src)?;
        rules::filed(&mut saved.state, f, task, now, &f.at.url);
    }
    // The freeze: held while a gate job is red on the base, whatever else lands.
    let frozen = if r.freeze_on_gate {
        rules::freezing(&saved.state)
    } else {
        vec![]
    };
    if frozen.is_empty() {
        if d.store.clear_ci_freeze(&p.id).unwrap_or(false) {
            d.landing.wake();
            d.poke();
        }
    } else {
        let names: Vec<&str> = frozen.iter().map(|(k, _)| k.as_str()).collect();
        let reason = format!(
            "{} red on {} (the base branch's own CI)",
            names.join(", "),
            p.base
        );
        let fix = frozen.iter().find_map(|(_, t)| *t);
        d.store
            .set_ci_freeze(&p.id, &reason, fix, now)
            .map_err(|e| e.to_string())?;
        d.poke();
    }
    saved.error = None;
    saved.checked_at = now;
    save(d, &p.id, &saved);
    Ok(())
}

/// Tasks of the project that landed or closed within the lookback, and when those that landed
/// with a change did. A task whose body cites a run, or whose title names a job, has covered it.
pub(crate) fn recent_tasks(
    d: &Daemon,
    pid: ProjectId,
    now: u64,
) -> (Vec<RecentTask>, BTreeMap<u64, u64>) {
    let mut recent = vec![];
    let mut landed = BTreeMap::new();
    if let Ok(w) = d.world() {
        for t in w.tasks.values() {
            if t.project != pid
                || !t.state.is_terminal()
                || now.saturating_sub(t.state_entered_at.0) > LOOKBACK_MS
            {
                continue;
            }
            if t.state == State::Landed && t.landed == Some(LandedOutcome::Merged) {
                landed.insert(t.id.0, t.state_entered_at.0);
            }
            recent.push(RecentTask {
                id: t.id.0,
                title: t.title.clone(),
                body: t.body.clone(),
            });
        }
    }
    (recent, landed)
}

/// The open tasks and issues that name each job, and how many auto-filed tasks are open.
fn who_names(
    d: &Daemon,
    pid: ProjectId,
    saved: &Saved,
    recs: &[RunRec],
) -> (BTreeMap<String, Option<u64>>, u32, u32) {
    let mut keys: Vec<String> = vec![];
    for (w, ws) in &saved.state.workflows {
        for j in ws.jobs.keys() {
            keys.push(rules::key(w, j));
            keys.push(rules::group_key(w, j));
        }
    }
    for r in recs {
        for j in &r.jobs {
            keys.push(rules::key(&r.workflow, &j.name));
            keys.push(rules::group_key(&r.workflow, &j.name));
        }
    }
    keys.sort();
    keys.dedup();
    let named = named_keys(d, pid, &keys);
    let mut open_auto = 0;
    let mut open_approved = 0;
    if let Ok(w) = d.world() {
        let mine: std::collections::BTreeSet<u64> =
            saved.state.filed.iter().filter_map(|f| f.task).collect();
        open_auto = w
            .tasks
            .values()
            .filter(|t| t.project == pid && !t.state.is_terminal())
            .filter(|t| mine.contains(&t.id.0))
            .count() as u32;
        let approved: std::collections::BTreeSet<u64> = saved
            .state
            .filed
            .iter()
            .filter(|f| f.auto)
            .filter_map(|f| f.task)
            .collect();
        open_approved = w
            .tasks
            .values()
            .filter(|t| t.project == pid && !t.state.is_terminal())
            .filter(|t| approved.contains(&t.id.0))
            .count() as u32;
    }
    (named, open_auto, open_approved)
}

/// Whether a red's ticket names a trust file (`protect::PROTECTED`): a failing script or test
/// that is one, or a path in the log excerpt the ticket quotes.
fn names_trust_file(f: &File, src: &dyn Source) -> bool {
    let (_, text) = failure_text(src, f.at.job);
    let paths = f.signatures.iter().map(String::as_str).chain(
        text.split(|c: char| c.is_whitespace() || "`'\"():,;".contains(c))
            .filter(|w| w.contains('/')),
    );
    paths
        .map(|p| p.trim_start_matches("./"))
        .any(|p| crate::protect::protected_because(p, &[]).is_some())
}

/// The open tasks and issues that name a failing script or test of a red job. An open task
/// counts when its title names it, or when the watch filed it (its body is then the log of a
/// red); an open issue by its title or body. A task about something else that happens to
/// quote a script name in its body is not taken for a ticket.
fn who_names_signatures(
    d: &Daemon,
    pid: ProjectId,
    saved: &Saved,
    recs: &[RunRec],
) -> BTreeMap<String, Option<u64>> {
    let mut sigs: Vec<String> = vec![];
    for ws in saved.state.workflows.values() {
        for js in ws.jobs.values().filter(|j| j.red) {
            sigs.extend(js.signatures.iter().cloned());
        }
    }
    for r in recs {
        for j in &r.jobs {
            sigs.extend(j.signatures.iter().cloned());
        }
    }
    sigs.sort();
    sigs.dedup();
    let mut named = BTreeMap::new();
    if sigs.is_empty() {
        return named;
    }
    let mine: std::collections::BTreeSet<u64> =
        saved.state.filed.iter().filter_map(|f| f.task).collect();
    if let Ok(w) = d.world() {
        for t in w
            .tasks
            .values()
            .filter(|t| t.project == pid && !t.state.is_terminal())
        {
            for s in &sigs {
                if rules::mentions(&t.title, s)
                    || (mine.contains(&t.id.0) && rules::mentions(&t.body, s))
                {
                    named.entry(s.clone()).or_insert(Some(t.id.0));
                }
            }
        }
    }
    if let Some(p) = d.project(pid) {
        for it in d.store.tracker_items(&p.id).unwrap_or_default() {
            if !it.open {
                continue;
            }
            for s in &sigs {
                if rules::mentions(&it.title, s) || rules::mentions(&it.body, s) {
                    named.entry(s.clone()).or_insert(None);
                }
            }
        }
    }
    named
}

/// Which of `keys` an open task (by its title) or an open tracker issue (by title or body)
/// already names; `Some(task)` for a task, `None` for an issue. Shared with the scheduled CI
/// runs, so a red one of them filed is not filed again by the other.
pub(crate) fn named_keys(
    d: &Daemon,
    pid: ProjectId,
    keys: &[String],
) -> BTreeMap<String, Option<u64>> {
    let mut named = BTreeMap::new();
    if let Ok(w) = d.world() {
        let open: Vec<&Task> = w
            .tasks
            .values()
            .filter(|t| t.project == pid && !t.state.is_terminal())
            .collect();
        for k in keys {
            if let Some(t) = open.iter().find(|t| t.title.contains(k.as_str())) {
                named.insert(k.clone(), Some(t.id.0));
            }
        }
    }
    if let Some(p) = d.project(pid) {
        for it in d.store.tracker_items(&p.id).unwrap_or_default() {
            if !it.open {
                continue;
            }
            for k in keys {
                if (it.title.contains(k.as_str()) || it.body.contains(k.as_str()))
                    && !named.contains_key(k)
                {
                    named.insert(k.clone(), None);
                }
            }
        }
    }
    named
}

fn file_task(
    d: &Arc<Daemon>,
    pid: ProjectId,
    base: &str,
    f: &File,
    src: &dyn Source,
) -> Result<Option<u64>, String> {
    if f.why == Why::Timeout {
        return file_timeout(d, pid, base, f, src);
    }
    // Shards of one matrix job go by their group key, so one ticket names them all.
    let key = f.group();
    let why = match f.why {
        Why::Gate => "the gate job went red".to_string(),
        Why::Agreed => "it was red on more than one run in a row".to_string(),
        Why::LongRed => "it has stayed red past the long-red threshold".to_string(),
        Why::Timeout => String::new(),
    };
    let (heading, tail) = failure_text(src, f.at.job);
    let shorts: Vec<&str> = f.signatures.iter().map(|s| rules::short(s)).collect();
    let failing = if shorts.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nFailing: {}",
            f.signatures
                .iter()
                .map(|s| format!("`{s}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let shards = if f.also.is_empty() {
        String::new()
    } else {
        format!("\nAlso red in the same run: {}", f.also.join(", "))
    };
    let repeats = if f.repeats.is_empty() {
        String::new()
    } else {
        let list: Vec<String> = f
            .repeats
            .iter()
            .map(|r| {
                format!(
                    "#{} (landed {}): {}",
                    r.task,
                    ago_text(now_ms().saturating_sub(r.landed_ms)),
                    r.title
                )
            })
            .collect();
        let ids: Vec<String> = f
            .repeats
            .iter()
            .map(|r| {
                format!(
                    "#{} (landed {})",
                    r.task,
                    ago_text(now_ms().saturating_sub(r.landed_ms))
                )
            })
            .collect();
        let named: Vec<&str> = f
            .repeats
            .iter()
            .flat_map(|r| r.signatures.iter().map(|s| rules::short(s)))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        format!(
            "\n\nRepeat failure: {} changed {} and it still fails on `{base}`; find why \
             their fix did not hold.\n{}",
            ids.join(" and "),
            named.join(", "),
            list.join("\n")
        )
    };
    let body = format!(
        "`{key}` is red on `{base}`: {why}.{failing}{shards}{repeats}\n\nRun: {}\nCommit: {}\n\n\
         {heading}:\n```\n{tail}\n```\n\nFound by the base-branch CI watch.",
        f.at.url, f.at.sha
    );
    let what = if shorts.is_empty() {
        String::new()
    } else if shorts.len() > 6 {
        format!(
            ": {}, and {} more",
            shorts[..6].join(", "),
            shorts.len() - 6
        )
    } else {
        format!(": {}", shorts.join(", "))
    };
    emit_task(
        d,
        pid,
        format!("{key} red on {base}{what}"),
        body,
        if f.urgent {
            Priority::Urgent
        } else {
            Priority::Normal
        },
        f.proposed,
    )
}

/// What a red job's task quotes: the error lines of the whole log, or, if only the end of the
/// log can be read, that tail.
fn failure_text(src: &dyn Source, job: u64) -> (&'static str, String) {
    if let Some(log) = src.log_long(job) {
        let e = rules::error_excerpt(&log, LOG_LINES);
        if !e.trim().is_empty() {
            return ("Error lines from the failed step", e);
        }
    }
    (
        "Failed log tail",
        src.log_tail(job)
            .unwrap_or_else(|| "(the log could not be read)".into()),
    )
}

fn ago_text(ms: u64) -> String {
    let m = ms / 60_000;
    match m {
        0..=59 => format!("{m}m ago"),
        60..=2879 => format!("{}h ago", m / 60),
        _ => format!("{}d ago", m / 1440),
    }
}

/// The task for a job that keeps hitting its time limit: the run, the limit GitHub gave, and
/// the end of its log, with the exit-script lines (and their timestamps) so the slow ones are
/// named.
fn file_timeout(
    d: &Arc<Daemon>,
    pid: ProjectId,
    base: &str,
    f: &File,
    src: &dyn Source,
) -> Result<Option<u64>, String> {
    let key = f.key();
    let note = src.annotations(f.at.job).unwrap_or_default();
    let limit = note
        .lines()
        .find(|l| l.contains(TIMEOUT_NOTE))
        .unwrap_or("(GitHub's annotation could not be read)");
    let tail = src
        .log_tail(f.at.job)
        .unwrap_or_else(|| "(the log could not be read)".into());
    let scripts = src
        .log_long(f.at.job)
        .map(|l| script_lines(&l).join("\n"))
        .filter(|s| !s.is_empty())
        .map(|s| {
            format!(
                "\n\nExit scripts started (with their timestamps), last ones last:\n```\n{s}\n```"
            )
        })
        .unwrap_or_default();
    let body = format!(
        "`{key}` was cut off at its time limit on `{base}` in more than one run in a row.\n\n\
         Limit: {limit}\nRun: {}\nCommit: {}\n\nLast lines before the cut:\n```\n{tail}\n```{scripts}\n\n\
         Found by the base-branch CI watch.",
        f.at.url, f.at.sha
    );
    emit_task(
        d,
        pid,
        format!("{key} exceeds its time limit on {base}"),
        body,
        Priority::Normal,
        f.proposed,
    )
}

/// File one owner-trust task for a red job. Shared with the scheduled CI runs.
pub(crate) fn emit_task(
    d: &Arc<Daemon>,
    pid: ProjectId,
    title: String,
    body: String,
    priority: Priority,
    proposed: bool,
) -> Result<Option<u64>, String> {
    let a = d
        .emit(EventKind::TaskFiled {
            project: pid,
            title,
            body,
            priority,
            trust: TrustSource::Owner,
            explored: false,
            proposed,
            blockers: vec![],
            not_before: None,
        })
        .map_err(|e| e.to_string())?;
    Ok(a.effects.iter().find_map(|e| match e {
        Effect::TaskCreated { task } => Some(task.0),
        _ => None,
    }))
}

/// The watch as the status shows it: what is watched, what waits for agreement, what was filed.
/// The held-back part of the view. Open tickets are being fixed: a line each. Landed ones
/// wait for a finished run: one quiet line, no banner. (A newer red run after the landing is
/// not held back at all.)
pub fn held_view(state: &rules::State, r: &rules::Rules) -> Value {
    let lines = rules::held_lines(state, r);
    let held: Vec<Value> = lines
        .iter()
        .filter(|l| !l.landed)
        .map(|l| json!({"job": l.key, "run": l.run, "task": l.ticket, "text": l.open_text()}))
        .collect();
    let unconfirmed: Vec<Value> = lines
        .iter()
        .filter(|l| l.landed)
        .map(|l| json!({"job": l.key, "run": l.run, "task": l.ticket, "text": l.name()}))
        .collect();
    json!({"held_back": held, "unconfirmed": unconfirmed,
           "unconfirmed_text": rules::unconfirmed_text(&lines)})
}

pub fn view(d: &Daemon, pid: ProjectId) -> Value {
    let Some(p) = d.project(pid) else {
        return Value::Null;
    };
    let Some(r) = d.current_def(pid).watch_ci.clone() else {
        return json!({"enabled": false});
    };
    let s = load(d, &p.id);
    let mut watched: Vec<String> = s
        .state
        .workflows
        .keys()
        .filter(|w| !r.excluded(w))
        .cloned()
        .collect();
    for w in &r.workflows {
        if !watched.contains(w) && !r.excluded(w) {
            watched.push(w.clone());
        }
    }
    watched.sort();
    let pending: Vec<Value> = rules::pending(&s.state, &r)
        .into_iter()
        .map(|(k, n)| json!({"job": k, "red_runs": n, "agreement": r.agreement}))
        .collect();
    let filed: Vec<Value> = s
        .state
        .filed
        .iter()
        .rev()
        .map(|f| {
            json!({"job": f.key, "task": f.task, "at": f.at, "urgent": f.urgent,
                   "proposed": f.proposed, "auto": f.auto, "url": f.url})
        })
        .collect();
    let freeze = d
        .store
        .freezes()
        .unwrap_or_default()
        .into_iter()
        .find(|f| f.project == p.id && f.source == "ci")
        .map(|f| json!({"reason": f.reason, "fix_task": f.fix_task, "since": f.since}));
    let held = held_view(&s.state, &r);
    let timeouts: Vec<Value> = rules::timeouts(&s.state, &r)
        .into_iter()
        .map(|(k, n, filed)| json!({"job": k, "timed_out_runs": n, "agreement": r.agreement, "filed": filed}))
        .collect();
    json!({
        "enabled": true,
        "held_back": held["held_back"],
        "unconfirmed": held["unconfirmed"],
        "unconfirmed_text": held["unconfirmed_text"],
        "workflows": watched,
        "excluded": r.exclude,
        "gate": r.gate,
        "freeze_on_gate": r.freeze_on_gate,
        "pending": pending,
        "timeouts": timeouts,
        "filed": filed,
        "freeze": freeze,
        "error": s.error,
        "checked_at": s.checked_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(&'static str);
    impl Source for Fake {
        fn runs(&self, _: &str) -> Result<Vec<ActionsRun>, String> {
            Ok(vec![])
        }
        fn jobs(&self, _: u64) -> Result<Vec<ActionsJob>, String> {
            Ok(vec![])
        }
        fn log_tail(&self, _: u64) -> Option<String> {
            Some("tail noise".into())
        }
        fn log_long(&self, _: u64) -> Option<String> {
            Some(self.0.into())
        }
    }

    #[test]
    fn the_filed_text_has_the_error_line_not_the_tail() {
        let log = include_str!("../../core/tests/fixtures/ci_logs/lint_spdx.log");
        let (h, t) = failure_text(&Fake(log), 1);
        assert_eq!(h, "Error lines from the failed step");
        assert!(
            t.contains("missing SPDX-License-Identifier: GPL-3.0-or-later in scripts/exit/s653.sh")
        );
        assert!(!t.contains("tail noise"));
    }

    const NOTE: &str = "The job has exceeded the maximum execution time of 1h0m0s";

    #[test]
    fn a_cancelled_job_in_a_finished_run_with_the_timeout_note_is_timed_out() {
        for run in ["failure", "success"] {
            let c = job_conclusion(Some(run), Some("cancelled"), || Some(NOTE.into()));
            assert_eq!(c.as_deref(), Some("timed_out"), "{run}");
        }
    }

    #[test]
    fn other_cancellations_stay_cancelled() {
        // No timeout note on the job.
        let c = job_conclusion(Some("failure"), Some("cancelled"), || {
            Some("canceled".into())
        });
        assert_eq!(c.as_deref(), Some("cancelled"));
        // A run a newer push superseded.
        let c = job_conclusion(Some("cancelled"), Some("cancelled"), || Some(NOTE.into()));
        assert_eq!(c.as_deref(), Some("cancelled"));
        // Annotations that cannot be read.
        let c = job_conclusion(Some("failure"), Some("cancelled"), || None);
        assert_eq!(c.as_deref(), Some("cancelled"));
        // Only a cancelled job is looked at.
        let c = job_conclusion(Some("failure"), Some("failure"), || panic!("not read"));
        assert_eq!(c.as_deref(), Some("failure"));
    }
}
