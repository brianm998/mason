// SPDX-License-Identifier: GPL-3.0-or-later

//! Introspection (Part 14): MASON notices its own problems and checks its own fixes.
//!
//! - The daily detector cycle runs the rules of `mason_core::introspect` over the recorded
//!   history (the `step_attempts` table, the event log, debts) and files each finding as a
//!   proposal through `proposals::file`, so the duplicate, evidence and cap rules apply.
//! - The weekly analyst is an agent task. It reads the digest and writes a JSON file in its
//!   worktree; the daemon checks every entry with the same rules before filing it.
//! - Closing the loop: each cycle looks at the findings whose proposal has landed and compares
//!   the metric since then with the metric when it was found.
//! - The digest and the Health report are computed from recorded data only.
//! - Triage records what an optional cheap agent would do with a question, for the owner to
//!   review. It never answers anything.

use crate::config::ProjectCfg;
use crate::daemon::{now_ms, Daemon};
use mason_core::introspect::Verdict;
use mason_core::introspect::*;
use mason_core::*;
use mason_engine::{reduce, World};
use mason_store::{state_name, FindingRow, ScheduleRow, TriageRow};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use toml_edit::DocumentMut;

const DAY_MS: u64 = 24 * 3_600_000;
/// The analyst's tasks start with this, which is how a finished step is known to be one.
pub const ANALYST_TITLE: &str = "Analyse recent pipeline behaviour";
/// The file the analyst writes in its worktree.
pub const ANALYST_FILE: &str = "analyst-result.json";

// ---- configuration -----------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct TriageCfg {
    pub enabled: bool,
    pub percent: u32,
    /// Reads `{"task":N,"question":"..."}` on stdin, prints `{"action","answer","evidence"}`.
    pub command: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cfg {
    pub enabled: bool,
    pub analyst: bool,
    pub window_ms: u64,
    pub th: Thresholds,
    pub triage: TriageCfg,
}

impl Default for Cfg {
    fn default() -> Cfg {
        Cfg {
            enabled: true,
            analyst: true,
            window_ms: DAY_MS,
            th: Thresholds::default(),
            triage: TriageCfg {
                enabled: false,
                percent: 20,
                command: None,
            },
        }
    }
}

/// `[introspect]` and `[triage]` of a project file.
pub fn parse_cfg(id: &str, doc: &DocumentMut) -> Result<Cfg, String> {
    let mut cfg = Cfg::default();
    if let Some(t) = doc.get("introspect").and_then(|v| v.as_table_like()) {
        for (k, v) in t.iter() {
            let at = format!("{id}: `[introspect] {k}`");
            let int = || {
                v.as_integer()
                    .filter(|n| *n >= 1)
                    .map(|n| n as u64)
                    .ok_or_else(|| format!("{at} must be a whole number from 1"))
            };
            let flt = || {
                v.as_float()
                    .or_else(|| v.as_integer().map(|n| n as f64))
                    .filter(|n| (0.0..=1.0).contains(n))
                    .ok_or_else(|| format!("{at} must be a number from 0 to 1"))
            };
            let flag = || {
                v.as_bool()
                    .ok_or_else(|| format!("{at} must be true or false"))
            };
            match k {
                "enabled" => cfg.enabled = flag()?,
                "analyst" => cfg.analyst = flag()?,
                "window" => {
                    cfg.window_ms = v
                        .as_str()
                        .and_then(parse_every)
                        .ok_or_else(|| format!("{at} must be like \"24h\" or \"3d\""))?
                }
                "min_samples" => cfg.th.min_samples = int()?,
                "loop_per_task" => cfg.th.loop_per_task = int()? as usize,
                "loop_per_project" => cfg.th.loop_per_project = int()? as usize,
                "same_signature" => cfg.th.same_signature = int()? as usize,
                "force_skips" => cfg.th.force_skips = int()?,
                "no_result_rate" => cfg.th.no_result_rate = flt()?,
                "wait_share" => cfg.th.wait_share = flt()?,
                "landing_idle_ms" => cfg.th.landing_idle_ms = int()?,
                other => return Err(format!("{id}: unknown key `[introspect] {other}`")),
            }
        }
    }
    if let Some(t) = doc.get("triage").and_then(|v| v.as_table_like()) {
        for (k, v) in t.iter() {
            let at = format!("{id}: `[triage] {k}`");
            match k {
                "enabled" => {
                    cfg.triage.enabled = v
                        .as_bool()
                        .ok_or_else(|| format!("{at} must be true or false"))?
                }
                "sample_percent" => {
                    cfg.triage.percent = v
                        .as_integer()
                        .filter(|n| (0..=100).contains(n))
                        .ok_or_else(|| format!("{at} must be a whole number from 0 to 100"))?
                        as u32
                }
                "command" => {
                    cfg.triage.command = v
                        .as_str()
                        .map(String::from)
                        .filter(|c| !c.trim().is_empty())
                }
                other => return Err(format!("{id}: unknown key `[triage] {other}`")),
            }
        }
        if cfg.triage.enabled && cfg.triage.command.is_none() {
            return Err(format!(
                "{id}: `[triage] enabled = true` needs `command` (the cheap agent to run)"
            ));
        }
    }
    Ok(cfg)
}

impl ProjectCfg {
    /// The introspection settings (validated when the project loaded).
    pub fn introspect(&self) -> Cfg {
        self.overlay_text
            .parse::<DocumentMut>()
            .ok()
            .and_then(|d| parse_cfg(&self.id, &d).ok())
            .unwrap_or_default()
    }
}

/// What the daemon remembers between ticks.
#[derive(Default)]
pub struct Runtime {
    running: Mutex<BTreeSet<String>>,
    /// The last cycle's summary, for the Health page.
    last: Mutex<Option<Value>>,
}

// ---- the tick ----------------------------------------------------------------------------

/// Start the thread that runs the detector cycle, the analyst and the recurring jobs when
/// they are due. Nothing runs at start-up: a job seen for the first time waits one interval.
pub fn spawn(d: &Arc<Daemon>) {
    let every = d.cfg.schedule_tick;
    crate::bg::spawn_loop(d, "introspect", move |weak| {
        std::thread::sleep(every);
        let Some(d) = weak.upgrade() else {
            return false;
        };
        tick(&d);
        true
    });
}

fn ensure_row(
    d: &Daemon,
    name: &str,
    project: &str,
    kind: &str,
    every_ms: u64,
    now: u64,
) -> ScheduleRow {
    // Written only when it is new or its settings changed: a job's own updates are not
    // overwritten by a tick that read the row a moment earlier.
    match d.store.schedule(name).ok().flatten() {
        Some(row) if row.every_ms == every_ms && row.kind == kind => row,
        found => {
            let mut row = found.unwrap_or_else(|| ScheduleRow {
                name: name.into(),
                project: project.into(),
                kind: kind.into(),
                last_run: now,
                ..Default::default()
            });
            row.every_ms = every_ms;
            row.kind = kind.into();
            let _ = d.store.put_schedule(&row);
            row
        }
    }
}

/// One look at what is due.
pub fn tick(d: &Arc<Daemon>) {
    let now = now_ms();
    let mut jobs: Vec<(String, Job)> = vec![];
    let intro = ensure_row(
        d,
        "introspect",
        "",
        "introspect",
        d.cfg.introspect_interval.as_millis() as u64,
        now,
    );
    if due(Some(intro.last_run), now, intro.every_ms) {
        jobs.push(("introspect".into(), Job::Cycle));
    }
    for p in d.projects().values() {
        let cfg = p.introspect();
        if cfg.enabled && cfg.analyst {
            let name = format!("analyst:{}", p.id);
            let row = ensure_row(
                d,
                &name,
                &p.id,
                "analyst",
                d.cfg.analyst_interval.as_millis() as u64,
                now,
            );
            if due(Some(row.last_run), now, row.every_ms) {
                jobs.push((name, Job::Analyst(p.id.clone())));
            }
        }
        for spec in p.schedules() {
            let name = crate::schedule::row_name(&p.id, &spec.name);
            let row = ensure_row(d, &name, &p.id, &spec.kind, spec.every_ms, now);
            let is_due = if spec.kind == "ci" {
                crate::ci_schedule::is_due(&row, &spec, now)
            } else {
                due(Some(row.last_run), now, row.every_ms)
            };
            if is_due {
                jobs.push((name, Job::Schedule(p.id.clone(), spec.name.clone())));
            }
        }
    }
    for (name, job) in jobs {
        if !claim(d, &name) {
            continue;
        }
        // Mark it run now, so a slow job is not started a second time.
        // (A followed CI run is looked at again and again: that is not the schedule running.)
        if let Ok(Some(mut row)) = d.store.schedule(&name) {
            if row.follow.is_empty() {
                row.last_run = now;
                let _ = d.store.put_schedule(&row);
            }
        }
        let d2 = d.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("job-{name}"))
            .spawn(move || {
                let _ = run_job(&d2, &name, &job);
                d2.intro
                    .running
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&name);
            });
        if spawned.is_err() {
            tracing::warn!("could not start a recurring job");
        }
    }
}

enum Job {
    Cycle,
    Analyst(String),
    Schedule(String, String),
}

fn claim(d: &Daemon, name: &str) -> bool {
    d.intro
        .running
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(name.to_string())
}

fn run_job(d: &Arc<Daemon>, name: &str, job: &Job) -> String {
    let said = match job {
        Job::Cycle => {
            let v = cycle(d);
            v["summary"].as_str().unwrap_or("ran").to_string()
        }
        Job::Analyst(p) => match d.project_id_by_name(p) {
            Some(pid) => match file_analyst_task(d, pid) {
                Ok(t) => format!("analyst task #{} filed", t.0),
                Err(e) => format!("analyst not started: {e}"),
            },
            None => "no such project".into(),
        },
        Job::Schedule(p, s) => {
            let spec = d
                .project_id_by_name(p)
                .and_then(|pid| d.project(pid))
                .and_then(|pc| {
                    pc.schedules()
                        .into_iter()
                        .find(|x| &x.name == s)
                        .map(|x| (pc.clone(), x))
                });
            match spec {
                Some((pc, spec)) => crate::schedule::run(d, &pc, &spec),
                None => "the schedule is no longer configured".into(),
            }
        }
    };
    if let Ok(Some(mut row)) = d.store.schedule(name) {
        if !matches!(job, Job::Schedule(..)) {
            row.last_status = said.clone();
            row.runs += 1;
            row.last_run = now_ms();
            let _ = d.store.put_schedule(&row);
        }
    }
    said
}

/// Run the job called `name` now (`introspect`, `analyst:<project>` or `<project>/<schedule>`).
pub fn run_now(d: &Arc<Daemon>, name: &str) -> Result<String, String> {
    let job = if name == "introspect" {
        Job::Cycle
    } else if let Some(p) = name.strip_prefix("analyst:") {
        Job::Analyst(p.to_string())
    } else if let Some((p, s)) = name.split_once('/') {
        Job::Schedule(p.to_string(), s.to_string())
    } else {
        return Err(format!("no job called {name:?}; see `mason schedule list`"));
    };
    if let Job::Schedule(p, s) = &job {
        let known = d
            .project_id_by_name(p)
            .and_then(|pid| d.project(pid))
            .is_some_and(|pc| pc.schedules().iter().any(|x| &x.name == s));
        if !known {
            return Err(format!("no schedule {name:?}; see `mason schedule list`"));
        }
    }
    if !claim(d, name) {
        return Err(format!("{name} is already running"));
    }
    let said = run_job(d, name, &job);
    d.intro
        .running
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(name);
    Ok(said)
}

// ---- the recorded data -------------------------------------------------------------------

/// What the event log says about time and landings, replayed from the start.
#[derive(Default)]
pub struct Replay {
    pub events: u64,
    /// Seconds of work in each state, per project name, clipped to the window.
    pub state_ms: BTreeMap<String, BTreeMap<String, u64>>,
    /// Seconds of waiting, per project name and wait class (see `mason_core::wait_class`).
    pub wait_ms: BTreeMap<String, BTreeMap<String, u64>>,
    /// How many waits fell (at least partly) in the window, and the longest, per project and class.
    pub wait_runs: BTreeMap<String, BTreeMap<String, WaitRuns>>,
    /// task -> (claimed at, route, project)
    pub claims: BTreeMap<u64, (u64, &'static str, String)>,
    /// task -> (landed at, project)
    pub lands: BTreeMap<u64, (u64, String)>,
    pub returned: BTreeSet<u64>,
    /// Send-backs in the window, per project and class (`send_back_class`).
    pub sent_back: BTreeMap<String, BTreeMap<&'static str, u64>>,
    /// Retries in the window, per project and the failure class of the task at the time
    /// (`code`, `outside`, `unclassified`): (by the owner, by the daemon).
    pub retries: BTreeMap<String, BTreeMap<&'static str, (u64, u64)>>,
    /// How long each test-step run took, per project, for runs that ended in the window.
    pub test_runs_ms: BTreeMap<String, Vec<u64>>,
}

/// The waits of one class, counted one by one.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct WaitRuns {
    pub count: u64,
    pub longest_ms: u64,
    pub longest_task: u64,
}

/// Note every wait that `before` had and `after` no longer has (it ended, or was replaced by a
/// newer one), clipped to `[from, to)`.
fn close_waits(before: &World, after: &World, from: u64, to: u64, end: u64, r: &mut Replay) {
    for (task, wait) in &before.waits {
        if after.waits.get(task).is_some_and(|n| n.since == wait.since) {
            continue;
        }
        note_wait(before, wait, from, to, end, r);
    }
}

fn note_wait(w: &World, wait: &mason_core::Wait, from: u64, to: u64, end: u64, r: &mut Replay) {
    let (lo, hi) = (wait.since.0.max(from), end.min(to));
    if hi <= lo {
        return;
    }
    let Some(p) = w
        .tasks
        .get(&wait.task)
        .and_then(|t| w.projects.get(&t.project))
    else {
        return;
    };
    let e = r
        .wait_runs
        .entry(p.name.clone())
        .or_default()
        .entry(wait.class_name())
        .or_default();
    e.count += 1;
    if hi - lo > e.longest_ms {
        e.longest_ms = hi - lo;
        e.longest_task = wait.task.0;
    }
}

fn charge(w: &World, from: u64, to: u64, a: u64, b: u64, r: &mut Replay) {
    let (lo, hi) = (a.max(from), b.min(to));
    if hi <= lo {
        return;
    }
    let dt = hi - lo;
    for t in w.tasks.values() {
        let Some(p) = w.projects.get(&t.project) else {
            continue;
        };
        let (state, wait): (&str, Option<String>) = match t.state {
            State::Proposed | State::Landed | State::Failed | State::Canceled => continue,
            _ if t.paused => continue,
            State::Queued if !t.blockers.is_empty() => continue,
            State::Queued => ("queued", Some("queue".into())),
            State::NeedsAttention => ("needs_attention", Some("owner".into())),
            s => (state_name(s), w.waits.get(&t.id).map(|x| x.class_name())),
        };
        *r.state_ms
            .entry(p.name.clone())
            .or_default()
            .entry(state.into())
            .or_default() += dt;
        if let Some(on) = wait {
            *r.wait_ms
                .entry(p.name.clone())
                .or_default()
                .entry(on)
                .or_default() += dt;
        }
    }
}

/// Replay the whole log, charging time that falls in `[from, to)`.
pub fn replay(d: &Daemon, from: u64, to: u64) -> Replay {
    let mut rp = Replayer::new(from, to);
    let mut after = 0u64;
    loop {
        let Ok(batch) = d.store.read_since(after, 5000) else {
            break;
        };
        if batch.is_empty() {
            break;
        }
        for se in batch {
            after = se.seq;
            rp.feed(&se.event);
        }
    }
    rp.finish()
}

/// The replay, one event at a time (so a test can feed it events without a daemon).
pub struct Replayer {
    r: Replay,
    w: World,
    prev: Option<u64>,
    from: u64,
    to: u64,
}

impl Replayer {
    pub fn new(from: u64, to: u64) -> Self {
        Replayer {
            r: Replay::default(),
            w: World::new(),
            prev: None,
            from,
            to,
        }
    }

    pub fn feed(&mut self, event: &mason_core::Event) {
        let (from, to) = (self.from, self.to);
        let (r, w) = (&mut self.r, &self.w);
        r.events += 1;
        let at = event.at.0;
        if let Some(p) = self.prev {
            charge(w, from, to, p, at, r);
        }
        self.prev = Some(at);
        let project_of = |w: &World, t: &TaskId| {
            w.tasks
                .get(t)
                .and_then(|x| w.projects.get(&x.project))
                .map(|p| p.name.clone())
                .unwrap_or_default()
        };
        match &event.kind {
            EventKind::Admitted {
                task,
                entry: Some(e),
            } => {
                r.claims
                    .entry(task.0)
                    .or_insert((at, e.route.name(), project_of(w, task)));
            }
            EventKind::StepSucceeded {
                task,
                next:
                    Some(Next {
                        state: State::Landed,
                        ..
                    }),
            }
            | EventKind::ClosedNoChange { task, .. } => {
                r.lands.insert(task.0, (at, project_of(w, task)));
            }
            EventKind::ExplorationRequested { task, .. } => {
                r.returned.insert(task.0);
            }
            _ => {}
        }
        let in_window = at >= from && at < to;
        match &event.kind {
            EventKind::SentBack { task, reason, .. } if in_window => {
                *r.sent_back
                    .entry(project_of(w, task))
                    .or_default()
                    .entry(send_back_class(reason))
                    .or_default() += 1;
            }
            EventKind::Retried { task, auto, .. } if in_window => {
                let class = match w.tasks.get(task).and_then(|t| t.failure_class) {
                    Some(FailClass::Code) => "code",
                    Some(FailClass::Outside) => "outside",
                    None => "unclassified",
                };
                let e = r
                    .retries
                    .entry(project_of(w, task))
                    .or_default()
                    .entry(class)
                    .or_default();
                if *auto {
                    e.1 += 1;
                } else {
                    e.0 += 1;
                }
            }
            _ => {}
        }
        // A test-step run ends with one of these: its length is its start and this event.
        if let (
            true,
            EventKind::StepSucceeded { task, .. }
            | EventKind::SentBack { task, .. }
            | EventKind::StepFailed { task, .. },
        ) = (in_window, &event.kind)
        {
            if w.tasks.get(task).is_some_and(|t| t.state == State::Testing) {
                if let Some(run) = w.open_run(*task).and_then(|id| w.step_runs.get(&id)) {
                    r.test_runs_ms
                        .entry(project_of(w, task))
                        .or_default()
                        .push(at.saturating_sub(run.started_at.0));
                }
            }
        }
        if let Ok((next, _)) = reduce(w, event) {
            close_waits(w, &next, from, to, at, r);
            self.w = next;
        }
    }

    pub fn finish(mut self) -> Replay {
        if let Some(p) = self.prev {
            charge(&self.w, self.from, self.to, p, self.to, &mut self.r);
        }
        // Waits still going count up to the end of the window.
        for wait in self.w.waits.values() {
            note_wait(&self.w, wait, self.from, self.to, self.to, &mut self.r);
        }
        self.r
    }
}

fn sum(m: Option<&BTreeMap<String, u64>>) -> u64 {
    m.map_or(0, |m| m.values().sum())
}

/// Everything recorded in `[from, to)`, for one project (or all).
pub fn stats_for(d: &Daemon, project: Option<&str>, from: u64, to: u64) -> Stats {
    let rep = replay(d, from, to);
    stats_from(d, &rep, project, from, to)
}

fn stats_from(d: &Daemon, rep: &Replay, project: Option<&str>, from: u64, to: u64) -> Stats {
    let in_p = |p: &str| project.is_none_or(|x| x == p);
    let sumo = |m: &BTreeMap<String, BTreeMap<String, u64>>| -> u64 {
        m.iter()
            .filter(|(p, _)| in_p(p))
            .map(|(_, v)| sum(Some(v)))
            .sum()
    };
    Stats {
        attempts: d
            .store
            .attempts_between(project, from, to)
            .unwrap_or_default(),
        landed: rep
            .lands
            .values()
            .filter(|(at, p)| *at >= from && *at < to && in_p(p))
            .count() as u64,
        force_skips: d
            .store
            .debts_between(from, to)
            .unwrap_or_default()
            .iter()
            .filter(|(p, _, scope, _)| in_p(p) && scope.starts_with("suite:"))
            .count() as u64,
        interventions: d
            .store
            .interventions(from)
            .unwrap_or_default()
            .iter()
            .filter(|r| {
                r.at < to && r.who != "pipeline" && r.kind != "refused_answer" && in_p(&r.project)
            })
            .count() as u64,
        wait_ms: sumo(&rep.wait_ms),
        wait_by: {
            let mut by = std::collections::BTreeMap::new();
            for (p, m) in &rep.wait_ms {
                if in_p(p) {
                    for (on, ms) in m {
                        *by.entry(on.clone()).or_insert(0u64) += ms;
                    }
                }
            }
            by
        },
        active_ms: sumo(&rep.state_ms),
    }
}

/// One row per project and wait class: count, total, longest (with its task) and the share of
/// the time tasks spent in a state. Longest first.
fn wait_rows(rep: &Replay, in_p: &dyn Fn(&str) -> bool, active_ms: u64) -> Vec<Value> {
    let mut rows: Vec<(u64, Value)> = vec![];
    for (p, classes) in rep.wait_ms.iter().filter(|(p, _)| in_p(p)) {
        for (class, total) in classes {
            let runs = rep.wait_runs.get(p).and_then(|m| m.get(class));
            rows.push((
                *total,
                json!({
                    "project": p, "class": class, "total_ms": total,
                    "count": runs.map(|r| r.count),
                    "longest_ms": runs.map(|r| r.longest_ms),
                    "longest_task": runs.map(|r| r.longest_task),
                    "share": (active_ms > 0).then(|| *total as f64 / active_ms as f64),
                }),
            ));
        }
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    rows.into_iter().map(|(_, v)| v).collect()
}

// ---- the detector cycle ------------------------------------------------------------------

/// Look at the recorded history, file what the rules find, and close the loop on fixes that
/// landed. Returns a summary.
pub fn cycle(d: &Arc<Daemon>) -> Value {
    let now = now_ms();
    let mut filed = vec![];
    let mut refused = vec![];
    let mut skipped = 0u64;
    let mut found_total = 0u64;
    let mut names: Vec<(ProjectId, &ProjectCfg)> = d.projects().into_iter().collect();
    names.sort_by(|a, b| a.1.id.cmp(&b.1.id));
    let switches = d.store.expired_switches(now).unwrap_or_default();
    let closed = d.store.findings().unwrap_or_default();
    for (i, (_, p)) in names.iter().enumerate() {
        let cfg = p.introspect();
        if !cfg.enabled {
            continue;
        }
        let from = now.saturating_sub(cfg.window_ms);
        let stats = stats_for(d, Some(&p.id), from, now);
        let pid = d.project_id_by_name(&p.id);
        let facts = Facts {
            landing_ready: pid.map_or(0, |pid| d.landing().queued(pid).len() as u64),
            landing_idle_ms: pid.map_or(0, |pid| d.landing().idle_ms(pid)),
            force_skips: d
                .store
                .force_skips_since(&p.id, from)
                .unwrap_or_default()
                .into_iter()
                .collect(),
            // Switches belong to no one project: the first one reports them.
            expired_switches: if i == 0 { switches.clone() } else { vec![] },
        };
        let found = detect(&p.id, &stats, &facts, &cfg.th);
        found_total += found.len() as u64;
        for f in found {
            // A fix that did not help is reported, not filed again: reverting it, or trying
            // another, is for the owner to decide from the digest.
            let latest = closed
                .iter()
                .filter(|r| r.project == f.project && r.rule == f.rule && r.subject == f.subject)
                .max_by_key(|r| r.id);
            if latest.is_some_and(|r| r.verdict == "didnt_help") {
                skipped += 1;
                continue;
            }
            // A fix for this finding landed before: only history since then counts.
            let after_fix = closed
                .iter()
                .filter(|r| {
                    r.project == f.project
                        && r.rule == f.rule
                        && r.subject == f.subject
                        && r.landed_at > 0
                })
                .map(|r| r.landed_at)
                .max();
            if let Some(landed) = after_fix {
                let mut later = stats.clone();
                later.attempts.retain(|a| a.at >= landed);
                let again = detect(&p.id, &later, &facts, &cfg.th)
                    .iter()
                    .any(|g| g.rule == f.rule && g.subject == f.subject);
                if !again {
                    skipped += 1;
                    continue;
                }
            }
            match file_finding(d, f, &stats) {
                Ok(Some(t)) => filed.push(t.0),
                Ok(None) => skipped += 1,
                Err(why) => refused.push(why),
            }
        }
    }
    let closed_now = close_loops(d);
    let summary = format!(
        "{found_total} found, {} filed, {skipped} already known, {} refused; {} verdict(s) reached",
        filed.len(),
        refused.len(),
        closed_now.len()
    );
    let v = json!({
        "at": now, "found": found_total, "filed": filed, "skipped": skipped,
        "refused": refused, "verdicts": closed_now, "summary": summary,
    });
    *d.intro.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(v.clone());
    tracing::info!(%summary, "introspection cycle");
    v
}

/// File one finding as a proposal and remember the metric it expects to move. `Ok(None)`:
/// the same finding is already open.
pub fn file_finding(
    d: &Arc<Daemon>,
    mut f: Finding,
    window: &Stats,
) -> Result<Option<TaskId>, String> {
    f.check()?;
    let open = d
        .store
        .findings()
        .map_err(|e| e.to_string())?
        .into_iter()
        .any(|r| {
            r.project == f.project
                && r.rule == f.rule
                && r.subject == f.subject
                && r.verdict.is_empty()
        });
    if open {
        return Ok(None);
    }
    if f.before.is_none() {
        f.before = measure(&f.metric, window);
    }
    let pid = d
        .project_id_by_name(&f.project)
        .ok_or_else(|| format!("no project {}", f.project))?;
    let metric_line = match f.before {
        Some(m) => format!(
            "{} was {:.3} ({} of {}, {} samples) when this was found",
            f.metric,
            m.rate(),
            m.num,
            m.den,
            m.n
        ),
        None => format!("{} is a snapshot with no window to compare", f.metric),
    };
    let body = format!(
        "{}\n\n**Metric expected to move:** `{}`. {metric_line}.\n\n_Found by the {} introspection {}; project `{}`._",
        f.body.trim(),
        f.metric,
        if f.rule == "analyst" { "weekly" } else { "daily" },
        if f.rule == "analyst" { "analyst" } else { "detector" },
        f.project
    );
    let filed = crate::proposals::file(
        d,
        &crate::proposals::Filing {
            from_task: None,
            project: pid,
            title: f.title.clone(),
            body,
            evidence: f.evidence.clone(),
            blocks_task: false,
            explored: true,
        },
    )?;
    let b = f.before.unwrap_or(Measure {
        num: 0.0,
        den: 0.0,
        n: 0,
    });
    d.store
        .add_finding(&FindingRow {
            rule: f.rule.clone(),
            subject: f.subject.clone(),
            project: f.project.clone(),
            metric: f.metric.clone(),
            title: f.title.clone(),
            task: filed.task.0,
            before_num: b.num,
            before_den: b.den,
            before_n: b.n,
            at: now_ms(),
            ..Default::default()
        })
        .map_err(|e| e.to_string())?;
    Ok(Some(filed.task))
}

/// For each open finding whose proposal landed, compare the metric since the landing with the
/// metric when it was found. Returns the verdicts reached this time.
pub fn close_loops(d: &Arc<Daemon>) -> Vec<Value> {
    let now = now_ms();
    let open: Vec<FindingRow> = d
        .store
        .findings()
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f.verdict.is_empty())
        .collect();
    if open.is_empty() {
        return vec![];
    }
    let Ok(w) = d.world() else { return vec![] };
    let rep = replay(d, 0, now);
    let mut out = vec![];
    for f in open {
        let Some(t) = w.tasks.get(&TaskId(f.task)) else {
            continue;
        };
        let min = d
            .project_id_by_name(&f.project)
            .and_then(|p| d.project(p))
            .map_or(Thresholds::default().min_samples, |p| {
                p.introspect().th.min_samples
            });
        match t.state {
            State::Canceled | State::Failed => {
                if d.store
                    .close_finding(
                        f.id,
                        "abandoned",
                        "the proposal was not carried out",
                        (0.0, 0.0, 0),
                        0,
                        now,
                    )
                    .unwrap_or(false)
                {
                    out.push(json!({"finding": f.id, "verdict": "abandoned"}));
                }
            }
            State::Landed => {
                let Some((landed_at, _)) = rep.lands.get(&f.task).cloned() else {
                    continue;
                };
                let after_stats = stats_from(d, &rep, Some(&f.project), landed_at, now);
                let before = Some(Measure {
                    num: f.before_num,
                    den: f.before_den,
                    n: f.before_n,
                })
                .filter(|_| f.before_n > 0 || f.before_den > 0.0);
                let after = measure(&f.metric, &after_stats);
                let v = verdict(before, after, min);
                if matches!(v, Verdict::Pending { .. }) {
                    continue;
                }
                let a = after.unwrap_or(Measure {
                    num: 0.0,
                    den: 0.0,
                    n: 0,
                });
                let text = v.sentence(&f.metric);
                if d.store
                    .close_finding(f.id, v.name(), &text, (a.num, a.den, a.n), landed_at, now)
                    .unwrap_or(false)
                {
                    out.push(
                        json!({"finding": f.id, "task": f.task, "verdict": v.name(), "text": text}),
                    );
                }
            }
            _ => {}
        }
    }
    if !out.is_empty() {
        d.poke();
    }
    out
}

// ---- the analyst -------------------------------------------------------------------------

/// File the analyst's task: Low priority, with the digest in its body. Not filed while the
/// last one is still open.
pub fn file_analyst_task(d: &Arc<Daemon>, pid: ProjectId) -> Result<TaskId, String> {
    let p = d.project(pid).ok_or("no such project")?;
    let key = format!("analyst:{}", p.id);
    let mut row = d
        .store
        .schedule(&key)
        .ok()
        .flatten()
        .unwrap_or_else(|| ScheduleRow {
            name: key.clone(),
            project: p.id.clone(),
            kind: "analyst".into(),
            every_ms: d.cfg.analyst_interval.as_millis() as u64,
            ..Default::default()
        });
    if let Some(prev) = row.last_task {
        let live = d
            .world()
            .ok()
            .and_then(|w| w.tasks.get(&TaskId(prev)).map(|t| t.state))
            .is_some_and(|s| !matches!(s, State::Landed | State::Canceled | State::Failed));
        if live {
            return Err(format!("the last analyst task, #{prev}, is still open"));
        }
    }
    let now = now_ms();
    let dg = digest(d, Some(&p.id), now.saturating_sub(7 * DAY_MS), now);
    let text = serde_json::to_string_pretty(&dg).unwrap_or_default();
    let digest_text: String = text.chars().take(8000).collect();
    let body = format!(
        "You are the analyst. Read the digest below and the event history (`mason digest`, `mason metrics`) and find what the daily rules cannot: a pattern in how this pipeline spends its time, fails, or makes the owner step in.\n\n\
         Write your findings as JSON in a file called `{ANALYST_FILE}` in the root of your worktree, and do not commit it:\n\n\
         ```json\n{{\"findings\": [{{\"title\": \"...\", \"body\": \"what is wrong and why it matters\", \"evidence\": \"a file and line or recorded output that shows it, e.g. crates/daemon/src/sched.rs:120\", \"metric\": \"one of: {}\"}}]}}\n```\n\n\
         A finding with no evidence naming a file, or naming a metric that is not in that list, is refused. Do not change any code. Then finish with a summary.\n\n## Digest (last 7 days, computed from recorded data)\n```json\n{digest_text}\n```",
        METRICS.join(", ")
    );
    let t = crate::schedule::file_low(d, p, ANALYST_TITLE, &body)?;
    row.last_task = Some(t.0);
    row.last_run = now;
    let _ = d.store.put_schedule(&row);
    Ok(t)
}

/// After an agent step of an analyst task: read the file it wrote, check every finding, and
/// file the good ones. Refusals are written beside the step's logs.
pub fn ingest_analyst(
    d: &Arc<Daemon>,
    task: TaskId,
    project: &str,
    title: &str,
    wt: &Path,
    dir: &Path,
) {
    if title != ANALYST_TITLE {
        return;
    }
    let file = wt.join(ANALYST_FILE);
    let Ok(text) = std::fs::read_to_string(&file) else {
        return;
    };
    let _ = std::fs::rename(&file, wt.join(format!("{ANALYST_FILE}.read")));
    let drafts = match parse_drafts(&text) {
        Ok(x) => x,
        Err(e) => {
            let _ = std::fs::write(
                dir.join("analyst-refused.txt"),
                format!("unreadable: {e}\n"),
            );
            note_analyst(
                d,
                project,
                format!("task #{}: result unreadable: {e}", task.0),
            );
            return;
        }
    };
    let (ok, refused) = accept_drafts(project, &drafts);
    let cfg = d
        .project_id_by_name(project)
        .and_then(|p| d.project(p))
        .map(|p| p.introspect())
        .unwrap_or_default();
    let now = now_ms();
    let stats = stats_for(d, Some(project), now.saturating_sub(cfg.window_ms), now);
    let (mut filed, mut refused_n) = (0, refused.len());
    let mut why = refused
        .iter()
        .map(|(t, w)| format!("{t}: {w}"))
        .collect::<Vec<_>>();
    for f in ok {
        match file_finding(d, f.clone(), &stats) {
            Ok(Some(_)) => filed += 1,
            Ok(None) => {}
            Err(e) => {
                refused_n += 1;
                why.push(format!("{}: {e}", f.title));
            }
        }
    }
    if !why.is_empty() {
        let _ = std::fs::write(dir.join("analyst-refused.txt"), why.join("\n") + "\n");
    }
    note_analyst(
        d,
        project,
        format!("task #{}: {filed} filed, {refused_n} refused", task.0),
    );
}

fn note_analyst(d: &Daemon, project: &str, say: String) {
    let key = format!("analyst:{project}");
    if let Ok(Some(mut row)) = d.store.schedule(&key) {
        row.last_status = say;
        let _ = d.store.put_schedule(&row);
    }
    d.poke();
}

fn parse_drafts(text: &str) -> Result<Vec<Draft>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let arr = v["findings"]
        .as_array()
        .ok_or("the file needs a `findings` array")?;
    let s = |x: &Value, k: &str| x[k].as_str().unwrap_or_default().to_string();
    Ok(arr
        .iter()
        .map(|x| Draft {
            title: s(x, "title"),
            body: s(x, "body"),
            evidence: s(x, "evidence"),
            metric: s(x, "metric"),
        })
        .collect())
}

// ---- triage ------------------------------------------------------------------------------

/// A question was just asked of the owner. If triage is on, ask the cheap agent what it would
/// do and keep that for review. The question still goes to the owner, always.
pub fn triage_question(d: &Arc<Daemon>, project: &ProjectCfg, task: TaskId, question: &str) {
    let cfg = project.introspect().triage;
    let (true, Some(cmd)) = (cfg.enabled, cfg.command.clone()) else {
        return;
    };
    let (d, question) = (d.clone(), question.to_string());
    let _ = std::thread::Builder::new()
        .name("triage".into())
        .spawn(move || {
            use std::io::Write;
            let input = json!({"task": task.0, "question": question}).to_string();
            let Ok(mut child) = std::process::Command::new("sh")
                .arg("-c")
                .arg(&cmd)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
            else {
                return;
            };
            if let Some(mut si) = child.stdin.take() {
                let _ = si.write_all(input.as_bytes());
            }
            let Ok(out) = child.wait_with_output() else {
                return;
            };
            let Ok(v) = serde_json::from_slice::<Value>(&out.stdout) else {
                return;
            };
            let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
            let action = s("action");
            if !["answer", "close", "escalate"].contains(&action.as_str()) {
                return;
            }
            // Unsure: it would answer without citing anything.
            let unsure = action != "escalate" && evidence_ok_text(&s("evidence")).is_err();
            let n = d.store.triage().map_or(0, |t| t.len() as u64) + 1;
            let queued = sample_for_review(n ^ task.0.rotate_left(17), cfg.percent, unsure);
            let _ = d.store.add_triage(&TriageRow {
                task: task.0,
                question,
                action,
                answer: s("answer"),
                evidence: s("evidence"),
                status: if queued { "queued" } else { "unsampled" }.into(),
                at: now_ms(),
                ..Default::default()
            });
            d.poke();
        });
}

fn evidence_ok_text(e: &str) -> Result<(), String> {
    mason_core::proposal::evidence_ok(e)
}

// ---- the digest and the Health report ----------------------------------------------------

/// The digest of `[from, to)`: landed tasks and their time, interventions, time by state and
/// wait reason, exploring skips by route and how many came back, forced landings with their
/// reasons, flaky suites, debts and the verdicts on fixes. Every number is a count or a sum of
/// recorded rows and events; what is not recorded is `null` with the reason.
pub fn digest(d: &Daemon, project: Option<&str>, from: u64, to: u64) -> Value {
    let rep = replay(d, from, to);
    let stats = stats_from(d, &rep, project, from, to);
    let in_p = |p: &str| project.is_none_or(|x| x == p);

    let times: Vec<u64> = rep
        .lands
        .iter()
        .filter(|(_, (at, p))| *at >= from && *at < to && in_p(p))
        .filter_map(|(t, (at, _))| rep.claims.get(t).map(|(c, _, _)| at.saturating_sub(*c)))
        .collect();
    let merged = |m: &BTreeMap<String, BTreeMap<String, u64>>| {
        let mut out: BTreeMap<String, u64> = BTreeMap::new();
        for (p, v) in m.iter().filter(|(p, _)| in_p(p)) {
            let _ = p;
            for (k, n) in v {
                *out.entry(k.clone()).or_default() += n;
            }
        }
        out
    };

    // Exploring skipped, by the route that skipped it, and how many came back.
    let mut routes: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    for (task, (at, route, p)) in &rep.claims {
        if *at < from || *at >= to || !in_p(p) {
            continue;
        }
        let e = routes.entry(route).or_default();
        e.0 += 1;
        if rep.returned.contains(task) {
            e.1 += 1;
        }
    }
    let skip_routes = ["flag", "clarity", "classifier"];
    let mut by_route = serde_json::Map::new();
    for (route, (n, back)) in &routes {
        by_route.insert(
            route.to_string(),
            json!({"tasks": n, "skipped_exploring": skip_routes.contains(route), "came_back": back}),
        );
    }
    let skipped: u64 = routes
        .iter()
        .filter(|(r, _)| skip_routes.contains(r))
        .map(|(_, v)| v.0)
        .sum();
    let came_back: u64 = routes
        .iter()
        .filter(|(r, _)| skip_routes.contains(r))
        .map(|(_, v)| v.1)
        .sum();

    let debts = d.store.debts_between(from, to).unwrap_or_default();
    let forced: Vec<Value> = debts
        .iter()
        .filter(|(p, _, scope, _)| in_p(p) && scope.starts_with("landing:"))
        .map(|(p, t, scope, why)| {
            json!({"project": p, "task": t, "kind": scope.trim_start_matches("landing:"),
                   "reason": if why.is_empty() { "no reason given" } else { why }})
        })
        .collect();
    let open_debts: Vec<Value> = d
        .store
        .debts(None, true)
        .unwrap_or_default()
        .iter()
        .filter(|x| in_p(&x.project))
        .map(|x| json!({"id": x.id, "task": x.task, "scope": x.scope, "reason": x.reason}))
        .collect();
    let mut flaky: BTreeMap<String, u64> = BTreeMap::new();
    for (suite, test, at) in d.store.flaky_since(from).unwrap_or_default() {
        if at < to {
            *flaky.entry(format!("{suite}::{test}")).or_default() += 1;
        }
    }
    let triage = d.store.triage().unwrap_or_default();
    let tri: Vec<_> = triage
        .iter()
        .filter(|t| t.at >= from && t.at < to)
        .collect();
    let findings: Vec<Value> = d
        .store
        .findings()
        .unwrap_or_default()
        .iter()
        .filter(|f| in_p(&f.project))
        .map(finding_json)
        .collect();
    let per_project = per_project_numbers(&rep, &in_p, from, to);
    json!({
        "from": from, "to": to, "project": project,
        "per_project": per_project,
        "landed": stats.landed,
        "median_claim_to_land_ms": median(times.clone()),
        "claim_to_land_samples": times.len(),
        "interventions": stats.interventions,
        "interventions_per_landed": per(stats.interventions, stats.landed),
        "time_by_state_ms": merged(&rep.state_ms),
        "wait_by_class": wait_rows(&rep, &in_p, stats.active_ms),
        "tokens_per_landed": Value::Null,
        "tokens_note": "token counts are not recorded per task, so none is shown",
        "exploring": {
            "by_route": by_route,
            "skipped": skipped,
            "came_back": came_back,
        },
        "forced_landings": forced,
        "force_skipped_suites": stats.force_skips,
        "flaky": flaky,
        "debts_outstanding": open_debts,
        "triage": {
            "suggested": tri.len(),
            "answered_by_triage": 0,
            "queued_for_review": tri.iter().filter(|t| t.status == "queued").count(),
            "reviewed": tri.iter().filter(|t| t.status == "reviewed").count(),
            "agreed": tri.iter().filter(|t| t.verdict == "agree").count(),
            "disagreed": tri.iter().filter(|t| t.verdict == "disagree").count(),
        },
        "findings": findings,
        "attempts": stats.attempts.len(),
        "trace": {
            "events_replayed": rep.events,
            "step_attempts_rows": stats.attempts.len(),
            "debt_rows": debts.len(),
            "intervention_rows": stats.interventions,
            "note": "each number above is a count or sum of these rows and events; nothing is estimated",
        },
    })
}

/// The numbers the reviews counted by hand, per project (s255): the claim-to-land median and
/// p90, send-backs per landed task by class, retries by failure class (the owner's and the
/// daemon's), how long test-step runs took, and what is not recorded.
fn per_project_numbers(rep: &Replay, in_p: &dyn Fn(&str) -> bool, from: u64, to: u64) -> Value {
    let mut names: BTreeSet<&String> = BTreeSet::new();
    names.extend(rep.lands.values().map(|(_, p)| p));
    names.extend(rep.sent_back.keys());
    names.extend(rep.retries.keys());
    names.extend(rep.test_runs_ms.keys());
    names.extend(rep.wait_runs.keys());
    let mut out = serde_json::Map::new();
    for p in names.into_iter().filter(|p| !p.is_empty() && in_p(p)) {
        let times: Vec<u64> = rep
            .lands
            .iter()
            .filter(|(_, (at, lp))| *at >= from && *at < to && lp == p)
            .filter_map(|(t, (at, _))| rep.claims.get(t).map(|(c, _, _)| at.saturating_sub(*c)))
            .collect();
        let landed = rep
            .lands
            .values()
            .filter(|(at, lp)| *at >= from && *at < to && lp == p)
            .count() as u64;
        let mut by_class = serde_json::Map::new();
        let mut per_landed = serde_json::Map::new();
        let sb = rep.sent_back.get(p.as_str());
        for class in ["test_red", "build_red", "conflict", "weakening", "other"] {
            let n = sb.and_then(|m| m.get(class)).copied().unwrap_or(0);
            by_class.insert(class.into(), json!(n));
            per_landed.insert(class.into(), json!(per(n, landed)));
        }
        // A red the base has too does not send a task back: its waits are counted instead.
        let base_red = rep
            .wait_runs
            .get(p.as_str())
            .and_then(|m| m.get("base-red"))
            .map_or(0, |w| w.count);
        by_class.insert("base_red".into(), json!(base_red));
        per_landed.insert("base_red".into(), json!(per(base_red, landed)));
        let mut retries = serde_json::Map::new();
        for (class, (manual, auto)) in rep.retries.get(p.as_str()).into_iter().flatten() {
            retries.insert(class.to_string(), json!({"manual": manual, "auto": auto}));
        }
        let runs = rep
            .test_runs_ms
            .get(p.as_str())
            .cloned()
            .unwrap_or_default();
        out.insert(
            p.to_string(),
            json!({
                "landed": landed,
                "claim_to_land": {
                    "median_ms": median(times.clone()),
                    "p90_ms": p90(times.clone()),
                    "samples": times.len(),
                },
                "send_backs": {
                    "by_class": by_class,
                    "per_landed": per_landed,
                    "note": "base_red counts waits (a task waits, it is not sent back)",
                },
                "retries_by_failure_class": retries,
                "test_step_run_ms": {
                    "median": median(runs.clone()),
                    "p90": p90(runs.clone()),
                    "runs": runs.len(),
                },
                "suite_run_ms": null_not_recorded("the run time of each suite is not recorded; only the test step's whole run is (test_step_run_ms)"),
                "run_check_ms": null_not_recorded("how long run_check calls took is not recorded"),
                "run_check_calls_per_task": null_not_recorded("run_check calls are not counted in the store or the event log"),
            }),
        );
    }
    Value::Object(out)
}

fn null_not_recorded(why: &str) -> Value {
    json!({"value": Value::Null, "not_recorded": why})
}

pub fn finding_json(f: &FindingRow) -> Value {
    let rate = |n: f64, dn: f64| if dn > 0.0 { json!(n / dn) } else { Value::Null };
    json!({
        "id": f.id, "rule": f.rule, "project": f.project, "metric": f.metric, "title": f.title,
        "task": f.task, "at": f.at,
        "verdict": if f.verdict.is_empty() { "open" } else { &f.verdict },
        "text": f.verdict_text,
        "before": {"num": f.before_num, "den": f.before_den, "n": f.before_n, "rate": rate(f.before_num, f.before_den)},
        "after": if f.verdict.is_empty() || f.after_n == 0 { Value::Null }
                 else { json!({"num": f.after_num, "den": f.after_den, "n": f.after_n, "rate": rate(f.after_num, f.after_den)}) },
        "landed_at": f.landed_at, "closed_at": f.closed_at,
    })
}

/// What the Health page and `mason health` show.
pub fn health(d: &Daemon) -> Value {
    let now = now_ms();
    let rows = d.store.schedules().unwrap_or_default();
    let sched: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "name": r.name, "project": r.project, "kind": r.kind, "every_ms": r.every_ms,
                "last_run": r.last_run, "next_run": r.last_run + r.every_ms, "runs": r.runs,
                "last_task": r.last_task, "last_status": r.last_status,
                "baseline": r.baseline.as_ref().map(|b| b.len()),
                "follow": crate::ci_schedule::follow_view(r),
            })
        })
        .collect();
    let findings: Vec<Value> = d
        .store
        .findings()
        .unwrap_or_default()
        .iter()
        .rev()
        .map(finding_json)
        .collect();
    let triage = d.store.triage().unwrap_or_default();
    let queue: Vec<Value> = triage
        .iter()
        .filter(|t| t.status == "queued")
        .map(|t| json!({"id": t.id, "task": t.task, "question": t.question, "action": t.action, "answer": t.answer, "evidence": t.evidence}))
        .collect();
    let triage_on = d.projects().values().any(|p| p.introspect().triage.enabled);
    // Tests that flaked in the last 7 days (the suites path and a test command's reruns
    // both record them), with how many times.
    let mut flaky: BTreeMap<String, u64> = BTreeMap::new();
    for (suite, test, _) in d
        .store
        .flaky_since(now.saturating_sub(7 * DAY_MS))
        .unwrap_or_default()
    {
        *flaky.entry(format!("{suite}::{test}")).or_default() += 1;
    }
    json!({
        "at": now,
        "stall_watch": {
            "sigconts": d.stall_conts.load(std::sync::atomic::Ordering::SeqCst),
            "kills": d.stall_kills.load(std::sync::atomic::Ordering::SeqCst),
        },
        "ws_sessions_closed_for_not_reading": d.ws_slow_closed.load(std::sync::atomic::Ordering::SeqCst),
        "flaky": flaky,
        "last_cycle": d.intro.last.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        "schedules": sched,
        "findings": findings,
        "triage": {
            "enabled": triage_on,
            "queue": queue,
            "reviewed": triage.iter().filter(|t| t.status == "reviewed").count(),
            "agreed": triage.iter().filter(|t| t.verdict == "agree").count(),
            "disagreed": triage.iter().filter(|t| t.verdict == "disagree").count(),
        },
        "digest": digest(d, None, now.saturating_sub(7 * DAY_MS), now),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(text: &str) -> Result<Cfg, String> {
        parse_cfg("web", &text.parse::<DocumentMut>().unwrap())
    }

    #[test]
    fn defaults_run_the_detectors_and_analyst_and_leave_triage_off() {
        let c = cfg("").unwrap();
        assert!(c.enabled && c.analyst);
        assert!(!c.triage.enabled, "triage is off unless asked for");
        assert_eq!(c.window_ms, DAY_MS);
        assert_eq!(c.th, Thresholds::default());
    }

    #[test]
    fn settings_are_read_and_bad_ones_name_the_key() {
        let c = cfg("[introspect]\nanalyst = false\nwindow = \"3d\"\nmin_samples = 9\nno_result_rate = 0.5\n").unwrap();
        assert!(!c.analyst);
        assert_eq!((c.window_ms, c.th.min_samples), (3 * DAY_MS, 9));
        assert_eq!(c.th.no_result_rate, 0.5);
        for (bad, key) in [
            ("[introspect]\nwindow = \"soon\"\n", "window"),
            ("[introspect]\nno_result_rate = 4\n", "no_result_rate"),
            ("[introspect]\nfast = true\n", "fast"),
            ("[triage]\nenabled = true\n", "command"),
            ("[triage]\nsample_percent = 120\n", "sample_percent"),
        ] {
            let e = cfg(bad).unwrap_err();
            assert!(e.contains(key), "{bad}: {e}");
        }
        let t =
            cfg("[triage]\nenabled = true\ncommand = \"sh t.sh\"\nsample_percent = 50\n").unwrap();
        assert_eq!((t.triage.enabled, t.triage.percent), (true, 50));
    }

    #[test]
    fn the_analyst_file_is_read_as_drafts() {
        let d = parse_drafts(r#"{"findings":[{"title":"t","body":"b","evidence":"e","metric":"wait_share"},{"title":"u"}]}"#).unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!(d[1].metric, "");
        assert!(parse_drafts("{}").is_err());
        assert!(parse_drafts("not json").is_err());
    }

    fn ev(at: u64, kind: EventKind) -> mason_core::Event {
        mason_core::Event {
            at: mason_core::Timestamp(at),
            kind,
        }
    }

    fn wait(at: u64, on: &str, class: Option<&str>) -> mason_core::Event {
        ev(
            at,
            EventKind::WaitStarted {
                task: TaskId(0),
                on: on.into(),
                class: class.map(String::from),
            },
        )
    }

    /// One task, waiting three times: twice for a compile slot (old events, no class, texts that
    /// differ in position and holder) and once for the test suite (new event, with a class).
    fn waits_log() -> Replay {
        let mut rp = Replayer::new(0, 100_000);
        for e in [
            ev(
                0,
                EventKind::ProjectRegistered {
                    project: mason_core::ProjectId(1),
                    name: "p".into(),
                },
            ),
            ev(
                0,
                EventKind::TaskFiled {
                    project: mason_core::ProjectId(1),
                    title: "t".into(),
                    body: String::new(),
                    priority: mason_core::Priority::Normal,
                    trust: mason_core::TrustSource::Owner,
                    explored: false,
                    proposed: false,
                    blockers: vec![],
                    not_before: None,
                },
            ),
            ev(
                0,
                EventKind::Admitted {
                    task: TaskId(0),
                    entry: None,
                },
            ),
            wait(
                1_000,
                "Waiting for compile slot: 10th in line, held by #94 for 0 s and 2 more",
                None,
            ),
            ev(11_000, EventKind::WaitEnded { task: TaskId(0) }),
            wait(
                20_000,
                "Waiting for compile slot: 2nd in line, held by #7 for 4 min",
                None,
            ),
            ev(25_000, EventKind::WaitEnded { task: TaskId(0) }),
            wait(
                30_000,
                "Waiting for test-suite slot: 1st in line",
                Some("test-suite"),
            ),
            ev(60_000, EventKind::WaitEnded { task: TaskId(0) }),
        ] {
            rp.feed(&e);
        }
        rp.finish()
    }

    #[test]
    fn old_waits_are_grouped_by_the_class_read_from_their_text() {
        let r = waits_log();
        let m = &r.wait_ms["p"];
        assert_eq!(m.get("compile"), Some(&15_000), "{m:?}");
        assert_eq!(m.get("test-suite"), Some(&30_000), "{m:?}");
        assert!(m.keys().all(|k| !k.contains("in line")), "{m:?}");
        let runs = &r.wait_runs["p"];
        assert_eq!(runs["compile"].count, 2);
        assert_eq!(
            (runs["compile"].longest_ms, runs["compile"].longest_task),
            (10_000, 0)
        );
        assert_eq!(runs["test-suite"].count, 1);
    }

    #[test]
    fn a_digest_row_per_class_totals_the_individual_waits() {
        let r = waits_log();
        let rows = wait_rows(&r, &|_| true, 100_000);
        let classes: Vec<&str> = rows.iter().map(|v| v["class"].as_str().unwrap()).collect();
        let waiting: Vec<&&str> = classes
            .iter()
            .filter(|c| **c != "queue" && **c != "owner")
            .collect();
        assert_eq!(waiting.len(), 2, "{classes:?}");
        let total: u64 = rows
            .iter()
            .filter(|v| v["count"].as_u64().is_some_and(|c| c > 0))
            .map(|v| v["total_ms"].as_u64().unwrap())
            .sum();
        assert_eq!(total, 10_000 + 5_000 + 30_000);
        assert_eq!(rows[0]["class"], "test-suite");
        assert_eq!(rows[0]["count"], 1);
    }

    /// One project, one task that is sent back once for a red test, fails outside the code, is
    /// retried by the daemon, and lands.
    fn numbers_log() -> Replay {
        use mason_core::{FailClass, Next, Route};
        let mut rp = Replayer::new(0, 100_000);
        let step = |at, to: State| {
            ev(
                at,
                EventKind::StepSucceeded {
                    task: TaskId(0),
                    next: Some(Next {
                        step: String::new(),
                        state: to,
                    }),
                },
            )
        };
        for e in [
            ev(
                0,
                EventKind::ProjectRegistered {
                    project: mason_core::ProjectId(1),
                    name: "p".into(),
                },
            ),
            ev(
                0,
                EventKind::TaskFiled {
                    project: mason_core::ProjectId(1),
                    title: "t".into(),
                    body: String::new(),
                    priority: mason_core::Priority::Normal,
                    trust: mason_core::TrustSource::Owner,
                    explored: true,
                    proposed: false,
                    blockers: vec![],
                    not_before: None,
                },
            ),
            ev(
                0,
                EventKind::Admitted {
                    task: TaskId(0),
                    entry: Some(mason_core::Entry {
                        def: "d".into(),
                        step: "develop".into(),
                        state: State::Developing,
                        route: Route::Explore,
                    }),
                },
            ),
            step(1_000, State::Testing),
            ev(
                5_000,
                EventKind::SentBack {
                    task: TaskId(0),
                    reason: "suite `cargo-workspace` is red".into(),
                    to_step: "develop".into(),
                },
            ),
            ev(
                6_000,
                EventKind::StepFailed {
                    task: TaskId(0),
                    reason: "the step host was lost 3 times".into(),
                    class: Some(FailClass::Outside),
                },
            ),
            ev(
                7_000,
                EventKind::Retried {
                    task: TaskId(0),
                    to: State::Developing,
                    step: "develop".into(),
                    auto: true,
                },
            ),
            step(8_000, State::Testing),
            step(20_000, State::Landed),
        ] {
            rp.feed(&e);
        }
        rp.finish()
    }

    #[test]
    fn the_digest_counts_send_backs_retries_and_run_times_per_project_from_the_events() {
        let r = numbers_log();
        let v = per_project_numbers(&r, &|_| true, 0, 100_000);
        let p = &v["p"];
        assert_eq!(p["landed"], 1);
        assert_eq!(p["claim_to_land"]["median_ms"], 20_000);
        assert_eq!(p["claim_to_land"]["p90_ms"], 20_000);
        assert_eq!(p["send_backs"]["by_class"]["test_red"], 1);
        assert_eq!(p["send_backs"]["by_class"]["conflict"], 0);
        assert_eq!(p["send_backs"]["per_landed"]["test_red"], 1.0);
        assert_eq!(p["retries_by_failure_class"]["outside"]["auto"], 1);
        assert_eq!(p["retries_by_failure_class"]["outside"]["manual"], 0);
        // The test step ran 1 to 5 s, then 8 to 20 s.
        assert_eq!(p["test_step_run_ms"]["runs"], 2);
        assert_eq!(p["test_step_run_ms"]["median"], 8_000);
        assert_eq!(p["test_step_run_ms"]["p90"], 12_000);
        // What is not recorded says so.
        assert!(p["run_check_calls_per_task"]["value"].is_null());
        assert!(p["run_check_calls_per_task"]["not_recorded"]
            .as_str()
            .unwrap()
            .contains("not"));
        // Outside the window nothing is counted.
        let none = per_project_numbers(&r, &|_| true, 50_000, 60_000);
        assert!(none["p"].is_null() || none["p"]["send_backs"]["by_class"]["test_red"] == 0);
    }
}
