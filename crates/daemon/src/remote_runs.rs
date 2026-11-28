// SPDX-License-Identifier: GPL-3.0-or-later

//! The remote runs MASON has on GitHub or on a node, for the monitor's "Remote runs" section
//! (#767): what owns
//! each run, where it is (waiting for a pool unit, queued, running, done), its jobs and its last
//! update. A run is entered here when it asks for a pool unit and when its hold is written, and
//! kept for ten minutes after it ends. The registry lives in this process; a daemon that
//! restarts has no history, only the runs it adopts again.
//!
//! Jobs are read by the run's own poll (`remote::poll`), never by a second loop here.

use crate::daemon::{now_ms, Daemon};
use mason_protocol::{GithubJob, GithubRun, GithubRunList};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A finished run stays in the list this long.
pub const KEEP_MS: u64 = 10 * 60 * 1000;

/// How long past its runner limit a run that was picked up may still excuse its step's silence:
/// the poll cancels it at the limit and then waits for its output.
const LIMIT_MARGIN_MS: u64 = 10 * 60 * 1000;

#[derive(Clone, Debug, Default)]
pub struct Job {
    pub name: String,
    /// queued, running or done.
    pub state: String,
    pub conclusion: String,
    pub step: String,
}

#[derive(Clone, Debug, Default)]
struct Entry {
    /// The daemon (its hosts directory) the run belongs to.
    scope: String,
    id: String,
    /// The owning task, 0 when none (a landing batch).
    task: u64,
    owner: String,
    kind: String,
    suite: String,
    runs_on: String,
    shards: u32,
    /// The run's record, which says what GitHub run it is and where it stands.
    record: Option<PathBuf>,
    waiting: bool,
    since_ms: u64,
    updated_ms: u64,
    finished_ms: Option<u64>,
    outcome: String,
    jobs: Vec<Job>,
    /// The runner limit, counted from `picked_ms`.
    limit_ms: u64,
    /// When a runner took the run, and when a poll last succeeded and found it still going.
    picked_ms: Option<u64>,
    polled_ms: Option<u64>,
}

static REG: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

fn reg() -> std::sync::MutexGuard<'static, Vec<Entry>> {
    REG.lock().unwrap_or_else(|e| e.into_inner())
}

/// What kind of run a host id names: a landing batch (`b480-…`), a debt, or a task's step.
fn classify(id: &str, key: &str, kind: &str) -> (u64, String, String) {
    let task = key
        .strip_prefix("task ")
        .and_then(|n| n.parse::<u64>().ok())
        .or_else(|| crate::sweep::parse_host_id(id).map(|(t, _)| t));
    if let Some(n) = task {
        let owner = if kind == "debt" {
            format!("debt #{n}")
        } else {
            format!("#{n}")
        };
        return (n, owner, shown(kind));
    }
    match crate::sweep::parse_batch_host_id(id) {
        Some(n) => {
            let k = if id.contains("lint") {
                "landing lint"
            } else {
                "landing suite"
            };
            (0, format!("b{n}"), k.to_string())
        }
        None if kind == "debt" => (0, id.to_string(), shown(kind)),
        None => (0, id.to_string(), shown(kind)),
    }
}

/// The words for a run's kind: the holds say `check`, `test_step`, `batch` or `debt`.
fn shown(kind: &str) -> String {
    match kind {
        "check" => "agent check",
        "test_step" => "test step",
        "debt" => "debt payment",
        k => k,
    }
    .to_string()
}

/// A request for a pool unit that has not been granted: removed when dropped.
pub struct WaitGuard {
    scope: String,
    id: String,
}

/// Note that `task`'s `kind` of run waits for a runner pool unit.
pub fn waiting(d: &Daemon, task: u64, kind: &str, owner: &str) -> WaitGuard {
    let scope = d.cfg.hosts_dir().display().to_string();
    let id = format!("wait-{task}-{owner}-{kind}");
    let now = now_ms();
    reg().push(Entry {
        scope: scope.clone(),
        id: id.clone(),
        task,
        owner: owner.to_string(),
        kind: kind.to_string(),
        waiting: true,
        since_ms: now,
        updated_ms: now,
        ..Default::default()
    });
    WaitGuard { scope, id }
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        reg().retain(|e| !(e.scope == self.scope && e.id == self.id));
    }
}

/// A run went out: enter it (replacing a waiting entry of the same owner).
#[allow(clippy::too_many_arguments)]
pub fn started(
    scope: &Path,
    id: &str,
    key: &str,
    kind: &str,
    suite: &str,
    runs_on: &str,
    shards: u32,
    record: &Path,
    limit_ms: u64,
) {
    let scope = scope.display().to_string();
    let (task, owner, kind) = classify(id, key, kind);
    let now = now_ms();
    let mut r = reg();
    r.retain(|e| !(e.scope == scope && e.waiting && e.task == task && e.owner == owner));
    r.retain(|e| !(e.scope == scope && e.id == id));
    r.push(Entry {
        scope,
        id: id.to_string(),
        task,
        owner,
        kind,
        suite: suite.to_string(),
        runs_on: runs_on.to_string(),
        shards: shards.max(1),
        record: Some(record.to_path_buf()),
        since_ms: now,
        updated_ms: now,
        limit_ms,
        ..Default::default()
    });
}

/// The run's poll succeeded at `now` and found it still going; `picked_ms` is when a runner
/// took it, if one has.
pub fn polled(record: &Path, picked_ms: Option<u64>, now: u64) {
    for e in reg()
        .iter_mut()
        .filter(|e| e.record.as_deref() == Some(record))
    {
        e.picked_ms = picked_ms;
        e.polled_ms = Some(now);
    }
}

/// How long a step silent for `silent_ms` counts as silent when it waits on a run: a run that a
/// runner took, that is within its limit (plus a margin), counts from its last successful poll.
/// Otherwise `silent_ms` stands.
fn quiet_ms(
    silent_ms: u64,
    picked_ms: Option<u64>,
    polled_ms: Option<u64>,
    limit_ms: u64,
    now: u64,
) -> u64 {
    match (picked_ms, polled_ms) {
        (Some(picked), Some(polled))
            if now
                < picked
                    .saturating_add(limit_ms)
                    .saturating_add(LIMIT_MARGIN_MS) =>
        {
            silent_ms.min(now.saturating_sub(polled))
        }
        _ => silent_ms,
    }
}

/// [`quiet_ms`] over the unfinished GitHub runs of `task` under `scope`: the least silent one
/// speaks for the step.
pub fn remote_quiet_ms(scope: &Path, task: u64, silent_ms: u64, now: u64) -> u64 {
    let scope = scope.display().to_string();
    reg()
        .iter()
        .filter(|e| e.scope == scope && e.task == task)
        .filter(|e| e.finished_ms.is_none() && !e.waiting && !is_node_run(e))
        .map(|e| quiet_ms(silent_ms, e.picked_ms, e.polled_ms, e.limit_ms, now))
        .fold(silent_ms, u64::min)
}

/// `task` has a request for a GitHub runner pool unit that has not been granted.
pub fn pool_waiting(scope: &Path, task: u64) -> bool {
    let scope = scope.display().to_string();
    reg()
        .iter()
        .any(|e| e.scope == scope && e.task == task && e.waiting)
}

/// One short line for each unfinished run of `task` under `scope`: kind, where it runs, whether it
/// waits for a runner, and the age of its last successful poll. Evidence for `stuck:long`.
pub fn open_of(scope: &Path, task: u64, now: u64) -> Vec<String> {
    let scope = scope.display().to_string();
    reg()
        .iter()
        .filter(|e| e.scope == scope && e.task == task && e.finished_ms.is_none())
        .map(|e| {
            let poll = e.polled_ms.map_or("never polled".to_string(), |p| {
                format!("polled {} min ago", now.saturating_sub(p) / 60_000)
            });
            let on = if e.runs_on.is_empty() {
                "no runner yet"
            } else {
                e.runs_on.as_str()
            };
            format!("{} on {on}, waiting {}, {poll}", e.kind, e.waiting)
        })
        .collect()
}

/// The run ended: how, from its record (collected with code 0 is green).
pub fn finished(scope: &Path, id: &str, outcome: Option<&str>) {
    let scope = scope.display().to_string();
    let now = now_ms();
    let mut r = reg();
    for e in r.iter_mut().filter(|e| e.scope == scope && e.id == id) {
        e.finished_ms = Some(now);
        e.updated_ms = now;
        e.outcome = outcome.unwrap_or("cancelled").to_string();
    }
}

/// The jobs the run's poll just read, with the time.
pub fn set_jobs(record: &Path, jobs: Vec<Job>) {
    let now = now_ms();
    for e in reg()
        .iter_mut()
        .filter(|e| e.record.as_deref() == Some(record))
    {
        if e.jobs
            .iter()
            .map(|j| (&j.state, &j.step))
            .collect::<Vec<_>>()
            != jobs.iter().map(|j| (&j.state, &j.step)).collect::<Vec<_>>()
        {
            e.updated_ms = now;
        }
        e.jobs = jobs.clone();
    }
}

/// A run that is on a node: `runs_on` is "node <name>".
fn is_node_run(e: &Entry) -> bool {
    e.runs_on.starts_with("node ")
}

/// A run went out to the node `node`: enter it, as [`started`] does for GitHub. It is told apart
/// by its `runs_on`, "node <name>", and has no GitHub run, so no jobs and no link.
pub fn node_started(scope: &Path, id: &str, suite: &str, node: &str, record: &Path) {
    let kind = if crate::sweep::parse_batch_host_id(id).is_some() {
        "batch"
    } else if id.contains("check") {
        "check"
    } else {
        "test_step"
    };
    started(
        scope,
        id,
        "",
        kind,
        suite,
        &format!("node {node}"),
        1,
        record,
        0,
    );
}

/// The record's stage, for the run's own line.
fn stage(record: &Path) -> (String, String, Option<u64>) {
    match crate::remote::run_stage(record) {
        Some(v) => v,
        None => ("queued".into(), String::new(), None),
    }
}

fn mins(s: u64) -> String {
    if s < 90 {
        format!("{s} s")
    } else {
        format!("{} min", s / 60)
    }
}

/// The section for the home view (empty when no project runs on GitHub or on a node).
pub fn view(
    d: &Daemon,
    rows: &dyn Fn(u64) -> Option<(String, String, String, String)>,
    project: &str,
) -> GithubRunList {
    let now = now_ms();
    let scope = d.cfg.hosts_dir().display().to_string();
    let enabled = crate::remote::any_remote(d) || d.nodes.cfg(&d.cfg.config_dir).enabled;
    let mut mine: Vec<Entry> = {
        let mut r = reg();
        r.retain(|e| {
            e.finished_ms
                .is_none_or(|f| now.saturating_sub(f) < KEEP_MS)
        });
        r.iter().filter(|e| e.scope == scope).cloned().collect()
    };
    // A project filter keeps the runs of that project's tasks (the rows say which); a landing
    // batch has no task and is kept.
    mine.retain(|e| project.is_empty() || e.task == 0 || rows(e.task).is_some());
    // Running ones first (the oldest first), then the waiting in the order they will start
    // (the order they asked), then the finished.
    let rank = |e: &Entry| (e.finished_ms.is_some() as u8, e.waiting as u8, e.since_ms);
    mine.sort_by_key(rank);
    let runs: Vec<GithubRun> = mine
        .iter()
        .map(|e| {
            let (title, task_state, activity, project_name) = rows(e.task).unwrap_or_default();
            let (st, url, picked) = match (&e.record, e.waiting) {
                // A run on a node has no GitHub run to look at: it is running from when it went.
                _ if is_node_run(e) && !e.waiting => ("running".into(), String::new(), None),
                (Some(rec), false) => stage(rec),
                _ => ("waiting".into(), String::new(), None),
            };
            let state = if e.finished_ms.is_some() {
                e.outcome.clone()
            } else {
                st
            };
            let since = if e.finished_ms.is_some() {
                e.finished_ms.unwrap_or(now)
            } else {
                picked.unwrap_or(e.since_ms)
            };
            let elapsed_s = if e.finished_ms.is_some() {
                0
            } else {
                now.saturating_sub(since) / 1000
            };
            let jobs: Vec<GithubJob> = e
                .jobs
                .iter()
                .map(|j| GithubJob {
                    name: j.name.clone(),
                    state: j.state.clone(),
                    conclusion: j.conclusion.clone(),
                    step: j.step.clone(),
                })
                .collect();
            let done = e.jobs.iter().filter(|j| j.state == "done").count();
            let progress = if e.finished_ms.is_some() || e.waiting {
                String::new()
            } else {
                let step = e
                    .jobs
                    .iter()
                    .find(|j| !j.step.is_empty())
                    .map(|j| j.step.clone());
                let mut p = if state == "running" {
                    format!("running {}", mins(elapsed_s))
                } else {
                    String::new()
                };
                if e.jobs.len() > 1 {
                    p = format!("{done} of {} jobs done", e.jobs.len());
                }
                if let Some(s) = step {
                    p = format!("{p}, step {s}");
                }
                p
            };
            GithubRun {
                id: e.id.clone(),
                task: e.task,
                title,
                owner: e.owner.clone(),
                task_state,
                task_activity: activity,
                kind: e.kind.clone(),
                suite: e.suite.clone(),
                runs_on: e.runs_on.clone(),
                shards: e.shards,
                state,
                elapsed_s,
                url,
                jobs,
                progress,
                updated_ago_s: now.saturating_sub(e.updated_ms) / 1000,
                project: project_name,
            }
        })
        .collect();
    let used = d.broker.used(mason_scheduler::GHA);
    let limit = d.broker.limit(mason_scheduler::GHA).unwrap_or(0);
    let waiting = runs.iter().filter(|r| r.state == "waiting").count() as u32;
    GithubRunList {
        enabled,
        pool_used: used,
        pool_limit: limit,
        waiting,
        runs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mason_core::health::{judge, Facts, Limits};

    const MIN: u64 = 60 * 1000;
    const LIMIT: u64 = 90 * MIN;
    const PICKED: u64 = 1_000 * MIN;

    fn reported(silent_ms: u64) -> bool {
        let x = Facts {
            silent_hosts: vec![(7, silent_ms)],
            ..Default::default()
        };
        judge(&x, &Limits::default())
            .iter()
            .any(|f| f.key == "stuck:host:7")
    }

    /// A registry scope of the test's own, with one run of task 7 in it.
    fn run(name: &str) -> (PathBuf, PathBuf) {
        let scope = PathBuf::from(format!("/nowhere/{name}/hosts"));
        let record = PathBuf::from(format!("/nowhere/{name}/t7r1-u.remote"));
        started(
            &scope,
            "t7r1-u",
            "task 7",
            "test_step",
            "u",
            "ubuntu-latest",
            1,
            &record,
            LIMIT,
        );
        (scope, record)
    }

    #[test]
    fn a_picked_up_run_inside_its_limit_is_not_silent() {
        let (scope, record) = run("inside");
        let now = PICKED + 60 * MIN;
        polled(&record, Some(PICKED), now - 2 * MIN);
        let quiet = remote_quiet_ms(&scope, 7, 60 * MIN, now);
        assert_eq!(quiet, 2 * MIN);
        assert!(!reported(quiet));
    }

    #[test]
    fn pool_waiting_lasts_from_waiting_to_the_guard_drop() {
        let (scope, _) = run("poolwait");
        // A started run is not a pool wait.
        assert!(!pool_waiting(&scope, 7));
        let id = "wait-7-#7-test_step".to_string();
        reg().push(Entry {
            scope: scope.display().to_string(),
            id: id.clone(),
            task: 7,
            waiting: true,
            ..Default::default()
        });
        let guard = WaitGuard {
            scope: scope.display().to_string(),
            id,
        };
        assert!(pool_waiting(&scope, 7));
        assert!(!pool_waiting(&scope, 8));
        drop(guard);
        assert!(!pool_waiting(&scope, 7));
    }

    #[test]
    fn a_run_past_its_limit_and_margin_is_still_silent() {
        let (scope, record) = run("past");
        let now = PICKED + LIMIT + LIMIT_MARGIN_MS;
        polled(&record, Some(PICKED), now - MIN);
        let quiet = remote_quiet_ms(&scope, 7, 100 * MIN, now);
        assert_eq!(quiet, 100 * MIN);
        assert!(reported(quiet));
        // Just inside the margin it is excused.
        assert_eq!(remote_quiet_ms(&scope, 7, 100 * MIN, now - 1), MIN - 1);
    }

    #[test]
    fn polls_that_keep_failing_are_reported() {
        let (scope, record) = run("failing");
        polled(&record, Some(PICKED), PICKED);
        let quiet = remote_quiet_ms(&scope, 7, 45 * MIN, PICKED + 31 * MIN);
        assert_eq!(quiet, 31 * MIN);
        assert!(reported(quiet));
    }

    #[test]
    fn only_a_live_picked_up_run_of_the_task_excuses_silence() {
        let (scope, record) = run("others");
        let now = PICKED + 50 * MIN;
        let quiet = |task| remote_quiet_ms(&scope, task, 40 * MIN, now);
        // Never polled, then still queued on GitHub.
        assert_eq!(quiet(7), 40 * MIN);
        polled(&record, None, now);
        assert_eq!(quiet(7), 40 * MIN);
        // A local step has no run, and another daemon's run is not this one's.
        polled(&record, Some(PICKED), now);
        assert_eq!(quiet(8), 40 * MIN);
        assert!(reported(quiet(8)));
        let elsewhere = Path::new("/nowhere/elsewhere/hosts");
        assert_eq!(remote_quiet_ms(elsewhere, 7, 40 * MIN, now), 40 * MIN);
        assert_eq!(quiet(7), 0);
        finished(&scope, "t7r1-u", Some("green"));
        assert_eq!(quiet(7), 40 * MIN);
    }
}
