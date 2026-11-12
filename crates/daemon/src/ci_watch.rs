// SPDX-License-Identifier: GPL-3.0-or-later

//! The base-branch CI watch (s154, decision 0149): reads the CI host's own runs on the base
//! branch, files a task for a new red and, for a red gate, freezes the project's landing until
//! that job is green on the base again. Off unless the definition has `[watch.ci] enabled = true`.
//! The rules are in `mason_core::ci_watch`; this is the reading, the filing and the keeping.

use crate::config::CiKind;
use crate::daemon::{now_ms, Daemon};
use mason_adapter_api::http::Curl;
use mason_adapters::github::{ActionsJob, ActionsRun, GitHub};
use mason_core::ci_watch::{
    self as rules, Ctx, File, Filed, JobRec, JobState, RunRec, RunRef, Why,
};
use mason_core::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

/// Lines of a failed job's log put in the task.
const LOG_LINES: usize = 60;

/// Where the runs come from. The GitHub adapter is one; tests give another.
pub trait Source {
    fn runs(&self, branch: &str) -> Result<Vec<ActionsRun>, String>;
    fn jobs(&self, run: u64) -> Result<Vec<ActionsJob>, String>;
    fn log_tail(&self, job: u64) -> Option<String>;
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
                   "proposed": f.proposed, "url": f.url})
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
                    },
                    ticket: jv["ticket"].as_u64(),
                    checked: jv["checked"].as_u64(),
                    freeze: jv["freeze"].as_bool().unwrap_or(false),
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

/// Start the watch. One loop looks at every project; a project whose definition has no
/// `[watch.ci]` costs nothing (no call to the CI host).
pub fn spawn(d: &Arc<Daemon>) {
    crate::bg::spawn_loop(d, "ci-watch", move |weak| {
        let Some(d) = weak.upgrade() else {
            return false;
        };
        let mut every = 20;
        for (pid, p) in &d.projects() {
            let Some(rules) = d.current_def(*pid).watch_ci.clone() else {
                // Switched off: a freeze it held does not outlive it.
                if d.store.clear_ci_freeze(&p.id).unwrap_or(false) {
                    d.landing.wake();
                    d.poke();
                }
                continue;
            };
            let gh = match source(p) {
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
                tracing::warn!(project = %p.id, "base-branch CI watch: {e}");
                let mut s = load(&d, &p.id);
                s.error = Some(e);
                s.checked_at = now_ms();
                save(&d, &p.id, &s);
            }
        }
        drop(d);
        std::thread::sleep(Duration::from_secs(every));
        true
    });
}

/// The GitHub Actions host of a project, or why there is none: the watch reads runs, so
/// it needs the project's `[ci]` to be `github-actions`.
fn source(p: &crate::config::ProjectCfg) -> Result<(GitHub, u64), String> {
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
            _ => Err("[watch.ci] needs a github-actions [ci] section; this project's [ci] is another kind".into()),
        },
        None => Err("[watch.ci] needs a github-actions [ci] section; this project has none".into()),
    }
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
    let p = d.project(pid).ok_or("no such project")?;
    let mut saved = load(d, &p.id);
    let runs = src.runs(&p.base)?;
    let mut recs: Vec<RunRec> = runs
        .iter()
        .map(|a| RunRec {
            id: a.id,
            workflow: a.workflow.clone(),
            sha: a.sha.clone(),
            url: a.url.clone(),
            created_ms: a.created_ms,
            conclusion: a.conclusion.clone(),
            jobs: vec![],
        })
        .collect();
    for id in rules::needs_jobs(&saved.state, r, &recs) {
        let jobs = src.jobs(id)?;
        if let Some(rec) = recs.iter_mut().find(|x| x.id == id) {
            rec.jobs = jobs
                .into_iter()
                .map(|j| JobRec {
                    id: j.id,
                    name: j.name,
                    conclusion: j.conclusion,
                })
                .collect();
        }
    }
    let (named, open_auto) = who_names(d, pid, &saved, &recs);
    let files = rules::step(
        &mut saved.state,
        &recs,
        &Ctx {
            rules: r,
            now_ms: now,
            named: &named,
            open_auto,
        },
    );
    for f in &files {
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

/// The open tasks and issues that name each job, and how many auto-filed tasks are open.
fn who_names(
    d: &Daemon,
    pid: ProjectId,
    saved: &Saved,
    recs: &[RunRec],
) -> (BTreeMap<String, Option<u64>>, u32) {
    let mut keys: Vec<String> = vec![];
    for (w, ws) in &saved.state.workflows {
        keys.extend(ws.jobs.keys().map(|j| rules::key(w, j)));
    }
    for r in recs {
        keys.extend(r.jobs.iter().map(|j| rules::key(&r.workflow, &j.name)));
    }
    keys.sort();
    keys.dedup();
    let mut named = BTreeMap::new();
    let mut open_auto = 0;
    if let Ok(w) = d.world() {
        let open: Vec<&Task> = w
            .tasks
            .values()
            .filter(|t| t.project == pid && !t.state.is_terminal())
            .collect();
        for k in &keys {
            if let Some(t) = open.iter().find(|t| t.title.contains(k.as_str())) {
                named.insert(k.clone(), Some(t.id.0));
            }
        }
        let mine: std::collections::BTreeSet<u64> =
            saved.state.filed.iter().filter_map(|f| f.task).collect();
        open_auto = open.iter().filter(|t| mine.contains(&t.id.0)).count() as u32;
    }
    if let Some(p) = d.project(pid) {
        for it in d.store.tracker_items(&p.id).unwrap_or_default() {
            if !it.open {
                continue;
            }
            for k in &keys {
                if (it.title.contains(k.as_str()) || it.body.contains(k.as_str()))
                    && !named.contains_key(k)
                {
                    named.insert(k.clone(), None);
                }
            }
        }
    }
    (named, open_auto)
}

fn file_task(
    d: &Arc<Daemon>,
    pid: ProjectId,
    base: &str,
    f: &File,
    src: &dyn Source,
) -> Result<Option<u64>, String> {
    let key = f.key();
    let why = match f.why {
        Why::Gate => "the gate job went red".to_string(),
        Why::Agreed => "it was red on more than one run in a row".to_string(),
        Why::LongRed => "it has stayed red past the long-red threshold".to_string(),
    };
    let tail = src
        .log_tail(f.at.job)
        .unwrap_or_else(|| "(the log could not be read)".into());
    let body = format!(
        "`{key}` is red on `{base}`: {why}.\n\nRun: {}\nCommit: {}\n\n\
         Failed log tail:\n```\n{tail}\n```\n\nFound by the base-branch CI watch.",
        f.at.url, f.at.sha
    );
    let a = d
        .emit(EventKind::TaskFiled {
            project: pid,
            title: format!("{key} red on {base}"),
            body,
            priority: if f.urgent {
                Priority::Urgent
            } else {
                Priority::Normal
            },
            trust: TrustSource::Owner,
            explored: false,
            proposed: f.proposed,
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
                   "proposed": f.proposed, "url": f.url})
        })
        .collect();
    let freeze = d
        .store
        .freezes()
        .unwrap_or_default()
        .into_iter()
        .find(|f| f.project == p.id && f.source == "ci")
        .map(|f| json!({"reason": f.reason, "fix_task": f.fix_task, "since": f.since}));
    json!({
        "enabled": true,
        "workflows": watched,
        "excluded": r.exclude,
        "gate": r.gate,
        "freeze_on_gate": r.freeze_on_gate,
        "pending": pending,
        "filed": filed,
        "freeze": freeze,
        "error": s.error,
        "checked_at": s.checked_at,
    })
}
