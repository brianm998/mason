// SPDX-License-Identifier: GPL-3.0-or-later

//! The landing engine's coordinator (Part 9). Tasks in the landing step join a
//! per-project queue; a coordinator thread forms batches from it by rank,
//! window and size; each batch merges its tasks into a disposable integration
//! branch in a persistent worktree, runs the suites once over the union diff,
//! walks the red ladder if they fail (rerun the failures, blame by impact,
//! bisect by halves, then ask), and promotes by fast-forward under the
//! repository lock. Only the tasks to blame are parked; the rest land.
//!
//! Nothing here is the only copy of anything: a task's landing job waits for
//! its verdict and a restarted daemon re-queues every task still in the
//! landing step. Assembling a batch resets the integration branch and merges
//! again, and a task already in the base is reported as landed, so a crash at
//! any phase resumes by doing the same steps over.

use crate::daemon::{now_ms, Daemon};
use crate::jobs::{run_shell, Ctx};
use mason_adapter_api::service::{
    CheckCounts, Checks, CiState, GitHostAdapter, Observed, PollEvent, Poller, PrState, Review,
};
use mason_core::progress::{Change, Figure};
use mason_core::*;
use mason_landing::batch::{Assembled, Batch, BatchTask, Finish};
use mason_landing::config::{self, LandingConfig, Rung};
use mason_landing::git;
use mason_landing::plan::{self, Bisect, Candidate, Footprint, Formation};
use mason_testing::Selection;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

/// How a task's landing ended, as its landing job sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// In the base (or was already).
    Landed,
    /// Back to development, with the reason and what to tell the agent.
    SentBack { reason: String, feedback: String },
    /// Back to development because the change conflicts with the base or with the
    /// rest of its batch: the change is not wrong, the base moved. Not a red round.
    Conflict { reason: String, feedback: String },
    /// Could not land and retrying will not help.
    Failed(String),
}

/// The batch's phases: forming, integrating, running suites, landing.
const PHASES: u64 = 4;

#[derive(Clone, Debug)]
struct Wait {
    task: TaskId,
    /// The landing step run this task waits in (its progress record).
    run: StepRunId,
    branch: String,
    rank: u64,
    ready_at: u64,
    footprint: Footprint,
}

#[derive(Clone, Debug)]
struct Running {
    slot: usize,
    footprint: Footprint,
}

#[derive(Default)]
struct State {
    waiting: HashMap<ProjectId, Vec<Wait>>,
    verdicts: HashMap<TaskId, Verdict>,
    running: HashMap<ProjectId, Vec<Running>>,
    coordinators: HashSet<ProjectId>,
    /// Tasks in a running batch that have not been given a verdict yet: what a batch that
    /// panics owes an answer to.
    owed: HashSet<TaskId>,
    /// Milliseconds tasks waited with nothing running, per project.
    idle_ms: HashMap<ProjectId, u64>,
    /// Bumped by every wake, so a wake between a look and a wait is not lost.
    gen: u64,
    /// Things the owner should know while they are true: `(project, key) -> (kind, text)`.
    attention: HashMap<(ProjectId, String), (String, String)>,
}

/// The landing queues of every project.
#[derive(Default)]
pub struct Landing {
    m: Mutex<State>,
    cv: Condvar,
}

impl Landing {
    /// Something changed (a task flagged, a freeze lifted): look at the queue again.
    pub fn wake(&self) {
        self.lock().gen += 1;
        self.cv.notify_all();
    }

    pub fn raise(&self, p: ProjectId, key: &str, kind: &str, text: &str) {
        self.lock()
            .attention
            .insert((p, key.to_string()), (kind.to_string(), text.to_string()));
    }

    pub fn clear(&self, p: ProjectId, key: &str) {
        self.lock().attention.remove(&(p, key.to_string()));
    }

    /// `(key, kind, text)` of what is true now, for the alerts.
    pub fn attention(&self) -> Vec<(String, String, String)> {
        self.lock()
            .attention
            .iter()
            .map(|((p, k), (kind, text))| {
                (format!("landing:{}:{k}", p.0), kind.clone(), text.clone())
            })
            .collect()
    }

    pub fn idle_ms(&self, p: ProjectId) -> u64 {
        self.lock().idle_ms.get(&p).copied().unwrap_or(0)
    }

    pub fn queued(&self, p: ProjectId) -> Vec<TaskId> {
        self.lock()
            .waiting
            .get(&p)
            .map(|v| v.iter().map(|w| w.task).collect())
            .unwrap_or_default()
    }

    pub fn running_batches(&self, p: ProjectId) -> usize {
        self.lock().running.get(&p).map_or(0, Vec::len)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.m.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn rank_of(p: Priority, task: TaskId) -> u64 {
    let p = match p {
        Priority::Urgent => 0,
        Priority::High => 1,
        Priority::Normal => 2,
        Priority::Low => 3,
    };
    p * 1_000_000_000 + task.0
}

// ---- configuration ------------------------------------------------------------

/// `.mason/landing.toml` on the base branch; absent means the defaults.
pub fn load_config(d: &Daemon, pid: ProjectId) -> Result<LandingConfig, String> {
    let p = d.project(pid).ok_or("no such project")?;
    let spec = format!("{}:.mason/landing.toml", p.base_ref());
    let text = git::run(&p.path, &["show", &spec]).unwrap_or_default();
    let c = config::parse(&text)
        .map_err(|e| format!("{} (.mason/landing.toml): {}", p.id, e.join("; ")))?;
    let problems = config::service_problems(
        &c,
        p.remote.is_some(),
        p.services.host.is_some(),
        p.services.ci.is_some(),
    );
    if !problems.is_empty() {
        return Err(format!("{}: {}", p.id, problems.join("; ")));
    }
    if let Some(b) = &c.base {
        if *b != p.base {
            return Err(format!(
                "{}: landing.toml says base `{b}` but the project's base is `{}`",
                p.id, p.base
            ));
        }
    }
    Ok(c)
}

// ---- the landing job's side ------------------------------------------------------

/// Join the project's queue and wait for the verdict. `None`: the task left the
/// step (cancelled, retried) while waiting.
pub fn submit_and_wait(c: &Ctx) -> Option<Verdict> {
    let d = &c.d;
    let pid = c.project_id;
    // Numbered files must carry the task's own reserved numbers. Checked before the task
    // joins the queue, so a wrong number is sent back with the number to use, instead of
    // surfacing later as a fold conflict.
    if let Some(v) = check_numbers(c) {
        return Some(v);
    }
    let paths = crate::suites::changed_paths(&c.project.path, &c.project.base_ref(), &c.branch)
        .unwrap_or_default();
    let def = d.current_def(pid);
    let all: Vec<String> = def.testing.suites.iter().map(|s| s.name.clone()).collect();
    let suites: BTreeSet<String> = match def.testing.impact.select(&paths, &all) {
        Selection::All => all.into_iter().collect(),
        Selection::Some(v) => v.into_iter().collect(),
    };
    let priority = d
        .world()
        .ok()
        .and_then(|w| w.tasks.get(&c.task).map(|t| t.priority))
        .unwrap_or(Priority::Normal);
    let wait = Wait {
        task: c.task,
        run: c.run,
        branch: c.branch.clone(),
        rank: rank_of(priority, c.task),
        ready_at: d.started.elapsed().as_secs(),
        footprint: Footprint {
            files: paths.into_iter().collect(),
            suites,
        },
    };
    {
        let mut g = d.landing.lock();
        g.verdicts.remove(&c.task);
        let q = g.waiting.entry(pid).or_default();
        q.retain(|w| w.task != c.task);
        q.push(wait);
        if g.coordinators.insert(pid) {
            spawn_coordinator(d, pid);
        }
    }
    d.landing.wake();
    loop {
        let mut g = d.landing.lock();
        if let Some(v) = g.verdicts.remove(&c.task) {
            return Some(v);
        }
        let (g2, _) = d
            .landing
            .cv
            .wait_timeout(g, Duration::from_millis(400))
            .unwrap_or_else(|e| e.into_inner());
        g = g2;
        if let Some(v) = g.verdicts.remove(&c.task) {
            return Some(v);
        }
        let in_step = matches!(d.world(), Ok(w) if w.open_run(c.task) == Some(c.run));
        if !in_step {
            if let Some(q) = g.waiting.get_mut(&pid) {
                q.retain(|w| w.task != c.task);
            }
            return None;
        }
    }
}

/// A task that adds a stage note, exit script or decision record under a number that is
/// not one of its reserved numbers, or under a number the base already uses, goes back.
fn check_numbers(c: &Ctx) -> Option<Verdict> {
    let w = c.d.world().ok()?;
    let t = w.tasks.get(&c.task)?;
    let mine = c.d.numbers_of(t);
    if mine.exit.is_none() && mine.decision.is_none() {
        return None; // numbers were never reserved (a demo or an old task): nothing to hold it to
    }
    let added = crate::numbers::added_files(&c.project.path, &c.project.base_ref(), &c.branch);
    let bad = crate::numbers::check_added(&added, &crate::numbers::base_paths(&c.project), &mine);
    if bad.is_empty() {
        return None;
    }
    let reason = format!(
        "uses numbers that are not its own: {}",
        bad.iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join("; ")
    );
    Some(Verdict::SentBack {
        reason,
        feedback: crate::numbers::feedback(&bad),
    })
}

fn deliver(d: &Daemon, task: TaskId, v: Verdict) {
    {
        let mut g = d.landing.lock();
        g.owed.remove(&task);
        g.verdicts.insert(task, v);
    }
    d.landing.wake();
}

// ---- the coordinator ----------------------------------------------------------------

fn spawn_coordinator(d: &Arc<Daemon>, pid: ProjectId) {
    let name = d.project(pid).map_or("?".into(), |p| p.id.clone());
    let mut first = true;
    let mut last = Instant::now();
    // A panic in a pass is caught by the loop and the next pass goes on; if the thread
    // could end, `coordinators` would still name it and no coordinator would ever run
    // again for this project.
    crate::bg::spawn_loop(d, format!("landing-{name}"), move |weak| {
        coordinate(weak, pid, &mut first, &mut last)
    });
}

/// One pass of the coordinator. `false` when the daemon is gone.
fn coordinate(weak: &Weak<Daemon>, pid: ProjectId, first: &mut bool, last: &mut Instant) -> bool {
    let Some(d) = weak.upgrade() else {
        return false;
    };
    if std::mem::take(first) {
        // Batches from a daemon that died did not finish; their tasks are back in the queue.
        if let Some(p) = d.project(pid) {
            let _ = d.store.abandon_batches(&p.id, now_ms());
            sweep_integration(&d, pid);
            remove_refs(&d, pid, None);
        }
    }
    d.crash.panic_point("coordinator");
    let seen = d.landing.lock().gen;
    let (queue_len, wait) = form_and_start(&d, pid, last);
    let timeout = if queue_len == 0 {
        Duration::from_secs(5)
    } else {
        wait.unwrap_or(Duration::from_secs(1))
            .clamp(Duration::from_millis(50), Duration::from_secs(5))
    };
    let g = d.landing.lock();
    if g.gen == seen {
        let _ = d
            .landing
            .cv
            .wait_timeout(g, timeout)
            .unwrap_or_else(|e| e.into_inner());
    }
    true
}

/// A dead daemon's batches left integration branches behind (and a worktree
/// standing on one). They are disposable: detach the worktrees and delete the
/// branches. Nothing else lives under `mason/int/<project>/`.
fn sweep_integration(d: &Arc<Daemon>, pid: ProjectId) {
    let Some(p) = d.project(pid) else { return };
    let lock = d.repo_lock(pid);
    let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
    let prefix = format!("mason/int/{}/", p.id);
    let Ok(wts) = git::worktrees(&p.path) else {
        return;
    };
    for (path, branch) in wts {
        if branch.as_deref().is_some_and(|b| b.starts_with(&prefix)) {
            let _ = git::run(&path, &["checkout", "--detach", "--quiet"]);
        }
    }
    let list = git::run(
        &p.path,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            &format!("refs/heads/{prefix}"),
        ],
    )
    .unwrap_or_default();
    for b in list.lines().filter(|b| !b.is_empty()) {
        let _ = git::run(&p.path, &["branch", "-D", b]);
    }
}

/// One look at the queue: start whatever may start. Returns how many tasks
/// wait and how long until the next batch is due.
fn form_and_start(
    d: &Arc<Daemon>,
    pid: ProjectId,
    last: &mut Instant,
) -> (usize, Option<Duration>) {
    let waiting: Vec<Wait> = d
        .landing
        .lock()
        .waiting
        .get(&pid)
        .cloned()
        .unwrap_or_default();
    let elapsed = last.elapsed();
    *last = Instant::now();
    {
        let mut g = d.landing.lock();
        if !waiting.is_empty() && g.running.get(&pid).is_none_or(Vec::is_empty) {
            *g.idle_ms.entry(pid).or_default() += elapsed.as_millis() as u64;
        }
    }
    if waiting.is_empty() {
        return (0, None);
    }
    let cfg = match load_config(d, pid) {
        Ok(c) => c,
        Err(e) => {
            // Nothing can land with a configuration that is refused: say so, once per look.
            for w in &waiting {
                take(d, pid, w.task);
                deliver(
                    d,
                    w.task,
                    Verdict::Failed(format!("landing is not configured: {e}")),
                );
            }
            return (0, None);
        }
    };
    let pname = d.project(pid).map(|p| p.id.clone()).unwrap_or_default();
    let freeze = d
        .store
        .freezes()
        .ok()
        .and_then(|v| v.into_iter().find(|f| f.project == pname));
    let flag = |t: TaskId| d.store.landing_flag(t.0).ok().flatten();

    let mut normal: Vec<Candidate> = vec![];
    let mut singles: Vec<(Wait, String)> = vec![];
    for w in &waiting {
        match flag(w.task).as_deref() {
            Some(k @ ("force" | "now")) => singles.push((w.clone(), k.to_string())),
            _ => {
                // Frozen: only the fix passes.
                if freeze
                    .as_ref()
                    .is_some_and(|f| f.fix_task != Some(w.task.0))
                {
                    continue;
                }
                normal.push(Candidate {
                    id: w.task.0,
                    rank: w.rank,
                    ready_at: w.ready_at,
                    footprint: w.footprint.clone(),
                });
            }
        }
    }
    for (w, kind) in singles {
        take(d, pid, w.task);
        start(d, pid, vec![w], cfg.clone(), &kind);
    }
    let now = d.started.elapsed().as_secs();
    let mut next_wait = None;
    let mut guard = 0;
    loop {
        guard += 1;
        let max_size = if cfg.mode == config::Mode::PrEach {
            1
        } else {
            cfg.max_size
        };
        match plan::form_batch(&normal, now, cfg.window, max_size) {
            Formation::Empty => break,
            Formation::Wait(t) => {
                next_wait = Some(t);
                break;
            }
            Formation::Form(ids) => {
                let members: Vec<Wait> = ids
                    .iter()
                    .filter_map(|i| waiting.iter().find(|w| w.task.0 == *i).cloned())
                    .collect();
                let fp = members
                    .iter()
                    .fold(Footprint::default(), |a, w| a.union(&w.footprint));
                let running: Vec<Footprint> = d
                    .landing
                    .lock()
                    .running
                    .get(&pid)
                    .map(|v| v.iter().map(|r| r.footprint.clone()).collect())
                    .unwrap_or_default();
                if !plan::may_run_alongside(&running, &fp, cfg.parallel) {
                    break; // a running batch overlaps or the cap is reached: wait for it
                }
                for w in &members {
                    take(d, pid, w.task);
                }
                normal.retain(|c| !ids.contains(&c.id));
                start(d, pid, members, cfg.clone(), "batch");
            }
        }
        if guard > 64 {
            break;
        }
    }
    let left = d.landing.lock().waiting.get(&pid).map_or(0, Vec::len);
    (left, next_wait)
}

fn take(d: &Daemon, pid: ProjectId, task: TaskId) {
    if let Some(q) = d.landing.lock().waiting.get_mut(&pid) {
        q.retain(|w| w.task != task);
    }
}

/// Start a batch on its own thread, in the lowest free integration slot.
fn start(d: &Arc<Daemon>, pid: ProjectId, members: Vec<Wait>, cfg: LandingConfig, kind: &str) {
    let fp = members
        .iter()
        .fold(Footprint::default(), |a, w| a.union(&w.footprint));
    let slot = {
        let mut g = d.landing.lock();
        let r = g.running.entry(pid).or_default();
        let slot = (0..).find(|s| r.iter().all(|x| x.slot != *s)).unwrap_or(0);
        r.push(Running {
            slot,
            footprint: fp,
        });
        g.owed.extend(members.iter().map(|w| w.task));
        slot
    };
    let d2 = d.clone();
    let kind = kind.to_string();
    let spawned = std::thread::Builder::new()
        .name(format!("batch-{slot}"))
        .spawn(move || {
            // The slot is given back on every way out of this thread.
            let _slot = SlotGuard {
                d: d2.clone(),
                pid,
                slot,
            };
            let owed: Vec<TaskId> = members.iter().map(|w| w.task).collect();
            let d3 = d2.clone();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                run_batch(d2, pid, slot, members, cfg, kind)
            }));
            if let Err(p) = r {
                // A batch that panics lands nothing and says why.
                crate::bg::report(&d3, &format!("landing batch {slot}"), p.as_ref());
                let open: Vec<TaskId> = {
                    let g = d3.landing.lock();
                    owed.into_iter().filter(|t| g.owed.contains(t)).collect()
                };
                for t in open {
                    deliver(
                        &d3,
                        t,
                        Verdict::Failed("internal error: the landing batch panicked".into()),
                    );
                }
            }
        });
    if spawned.is_err() {
        if let Some(r) = d.landing.lock().running.get_mut(&pid) {
            r.retain(|x| x.slot != slot);
        }
    }
}

/// Clears a batch's slot in `running` when its thread ends, however it ends.
struct SlotGuard {
    d: Arc<Daemon>,
    pid: ProjectId,
    slot: usize,
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        if let Some(r) = self.d.landing.lock().running.get_mut(&self.pid) {
            r.retain(|x| x.slot != self.slot);
        }
        self.d.landing.wake();
        self.d.poke();
    }
}

fn run_batch(
    d2: Arc<Daemon>,
    pid: ProjectId,
    slot: usize,
    members: Vec<Wait>,
    cfg: LandingConfig,
    kind: String,
) {
    {
        {
            d2.crash.panic_point("batch");
            let tasks: Vec<(TaskId, String)> =
                members.iter().map(|w| (w.task, w.branch.clone())).collect();
            let member_runs: Vec<StepRunId> = members.iter().map(|w| w.run).collect();
            let fps: HashMap<u64, Footprint> = members
                .iter()
                .map(|w| (w.task.0, w.footprint.clone()))
                .collect();
            let p = d2.project(pid).expect("project exists");
            let built = (|| -> Result<_, String> {
                Ok((crate::services::ci(p)?, crate::services::host(p)?))
            })();
            match built {
                Ok((ci, host)) => Runner {
                    d: d2.clone(),
                    pid,
                    cfg,
                    slot,
                    kind,
                    fps,
                    member_runs,
                    runs: Default::default(),
                    batch_id: Default::default(),
                    ci,
                    host,
                    pr: Mutex::new(None),
                    service_error: Mutex::new(None),
                }
                .run(tasks),
                Err(e) => {
                    for (t, _) in &tasks {
                        deliver(
                            &d2,
                            *t,
                            Verdict::Failed(format!("the project's services are not usable: {e}")),
                        );
                    }
                }
            }
        }
    }
}

fn end_reason(e: &Result<PrEnd, String>) -> Option<String> {
    match e {
        Ok(PrEnd::Closed(w)) => Some(format!("the pull request was {w}")),
        _ => None,
    }
}

enum PrEnd {
    Merged(String),
    Red,
    ChangesRequested,
    Closed(String),
    Conflict,
    BaseKeptMoving,
}

/// Close the pull requests and delete the candidate refs a batch (or, with
/// `None`, every batch a dead daemon left) put on the remote.
fn remove_refs(d: &Arc<Daemon>, pid: ProjectId, batch: Option<u64>) {
    let Some(p) = d.project(pid) else { return };
    let host = crate::services::host(p).ok().flatten();
    for r in d.store.open_refs(&p.id).unwrap_or_default() {
        if batch.is_some_and(|b| b != r.batch) {
            continue;
        }
        if let (Some(h), Some(n), "pr") = (&host, r.number, r.kind.as_str()) {
            let _ = h.close_pr(n);
        }
        if let Some(remote) = &p.remote {
            let _ = git::run(
                &p.path,
                &[
                    "push",
                    remote,
                    "--delete",
                    &format!("refs/heads/{}", r.name),
                ],
            );
        }
        let _ = d.store.remove_ref(r.id);
    }
}

// ---- one batch ----------------------------------------------------------------------

struct Runner {
    d: Arc<Daemon>,
    pid: ProjectId,
    cfg: LandingConfig,
    slot: usize,
    /// `batch`, `now` or `force`.
    kind: String,
    fps: HashMap<u64, Footprint>,
    /// The landing step runs of the batch's tasks: where its progress is shown.
    member_runs: Vec<StepRunId>,
    /// Makes each suite run's host id its own: a host id names one run.
    runs: std::sync::atomic::AtomicU64,
    batch_id: std::sync::atomic::AtomicU64,
    ci: Option<crate::services::Ci>,
    host: Option<Arc<dyn GitHostAdapter>>,
    /// The pull request the batch is about to merge (pr modes).
    pr: Mutex<Option<OpenPr>>,
    /// Set when a service could not answer: the batch ends with this reason.
    service_error: Mutex<Option<String>>,
}

#[derive(Clone, Debug)]
struct OpenPr {
    number: u64,
    branch: String,
    ref_id: u64,
}

struct Parked {
    task: u64,
    reason: String,
    feedback: String,
    /// Parked for a conflict with the base or the batch, not for anything red.
    conflict: bool,
}

impl Parked {
    fn verdict(&self) -> Verdict {
        let (reason, feedback) = (self.reason.clone(), self.feedback.clone());
        if self.conflict {
            Verdict::Conflict { reason, feedback }
        } else {
            Verdict::SentBack { reason, feedback }
        }
    }
}

impl Runner {
    fn project(&self) -> &crate::config::ProjectCfg {
        self.d.project(self.pid).expect("project exists")
    }

    fn int_wt(&self) -> PathBuf {
        self.d
            .cfg
            .landing_dir()
            .join(format!("{}-b{}", self.project().id, self.slot))
    }

    /// The batch's phases, for each member's landing run: `done` of [`PHASES`] with the
    /// name of the one it is in. (Suite counts and check counts are sub-counts.)
    fn phase(&self, done: u64, name: &str) {
        let msg = format!("batch: {name}");
        for run in &self.member_runs {
            self.d.progress_report(*run, |p, now| {
                let before = (p.done, p.total, p.unit.clone());
                p.unit = "phases".into();
                p.done = Figure::counted(done);
                p.total = Figure::reported(PHASES);
                let mut c = if before == (p.done, p.total, p.unit.clone()) {
                    Change::None
                } else {
                    Change::Urgent
                };
                if p.set_message(&msg, now) == Change::Urgent {
                    c = Change::Urgent;
                }
                if c != Change::None {
                    p.updated_at_ms = now;
                }
                c
            });
        }
    }

    /// What the batch is waiting on, as each member's message.
    fn say(&self, msg: &str) {
        for run in &self.member_runs {
            self.d
                .progress_report(*run, |p, now| p.set_message(msg, now));
        }
    }

    /// The host's checks, finished out of listed, as a sub-count of each member.
    fn checks(&self, c: &CheckCounts) {
        let msg = c.running.as_ref().map(|r| format!("check running: {r}"));
        for run in &self.member_runs {
            self.d.progress_sub(*run, "checks", msg.clone(), |s| {
                s.unit = "checks".into();
                s.done = Figure::reported(c.done);
                s.total = Figure::reported(c.total);
            });
        }
    }

    /// The run that shows the batch's suites (the first member's); the others are mirrored.
    fn sink(&self) -> Option<StepRunId> {
        self.member_runs.first().copied()
    }

    /// Copy the suite sub-counts of the sink run to the other members until `stop`.
    fn mirror(&self, stop: &std::sync::atomic::AtomicBool) {
        use std::sync::atomic::Ordering::SeqCst;
        let Some(lead) = self.sink() else { return };
        loop {
            let last = stop.load(SeqCst);
            if let Some(p) = self.d.progress_of(lead.0) {
                for run in self.member_runs.iter().filter(|r| **r != lead) {
                    for s in p.subs.iter().filter(|s| s.name != "checks") {
                        self.d.progress_sub(*run, &s.name, None, |x| *x = s.clone());
                    }
                }
            }
            if last {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn point(&self, p: &str) {
        let c = &self.d.crash;
        c.point(p);
        // The names of the single-task lander's boundaries still mean something.
        match p {
            "batch_merged" => {
                c.point("land_after_fetch");
                c.point("land_after_ff");
            }
            "batch_after_promote" => c.point("land_after_push"),
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record(
        &self,
        id: u64,
        phase: &str,
        landed: &[u64],
        parked: &[Parked],
        note: &str,
        head: &str,
        done: bool,
    ) {
        let l = landed
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let p = parked
            .iter()
            .map(|x| x.task.to_string())
            .collect::<Vec<_>>()
            .join(",");
        match phase {
            "merged" => self.phase(1, "integrating"),
            "verifying" => self.phase(2, "running suites"),
            "promoting" => self.phase(3, "landing"),
            "landed" => self.phase(PHASES, "landed"),
            _ => {}
        }
        let _ = self
            .d
            .store
            .set_batch(id, phase, &l, &p, note, head, done, now_ms());
        self.d.poke();
    }

    fn run(&self, tasks: Vec<(TaskId, String)>) {
        let d = &self.d;
        let p = self.project().clone();
        let ids: Vec<u64> = tasks.iter().map(|t| t.0 .0).collect();
        let listed = ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
        let Ok(batch_id) = d.store.add_batch(&p.id, &listed, now_ms()) else {
            for (t, _) in &tasks {
                deliver(
                    d,
                    *t,
                    Verdict::Failed("the batch could not be recorded".into()),
                );
            }
            return;
        };
        self.batch_id
            .store(batch_id, std::sync::atomic::Ordering::SeqCst);
        self.point("batch_formed");
        self.phase(0, "forming");
        let def = d.current_def(self.pid);
        let remote = if self.cfg.mode.local_only() {
            None
        } else {
            p.remote.as_deref()
        };
        let int_wt = self.int_wt();
        let int_branch = format!("mason/int/{}/b{batch_id}", p.id);
        let batch = Batch {
            repo: &p.path,
            int_wt: &int_wt,
            int_branch: &int_branch,
            base: &p.base,
            remote,
            resolver: self.cfg.resolver.as_deref(),
            max_fold: self.cfg.max_fold,
        };
        let lock = d.repo_lock(self.pid);
        let log_dir = d
            .cfg
            .task_dir(0)
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
            .join("batches")
            .join(batch_id.to_string());
        let point = |s: &str| self.point(s);

        let mut active: Vec<BatchTask> = tasks
            .iter()
            .map(|(t, b)| BatchTask {
                id: t.0,
                branch: b.clone(),
            })
            .collect();
        let mut parked: Vec<Parked> = vec![];
        // A parked task goes back to development at once, not when the batch ends: a
        // batch may wait hours for a review it needs.
        let mut delivered: HashSet<u64> = HashSet::new();
        let mut landed: Vec<u64> = vec![];
        let mut rounds = 0usize;
        let limit = self.cfg.max_fold as usize + ids.len() + 3;
        let mut note = String::new();
        let mut commit = String::new();
        let force = self.kind == "force";

        let outcome: Result<(), String> = 'outer: loop {
            for pk in parked.iter().filter(|x| delivered.insert(x.task)) {
                deliver(d, TaskId(pk.task), pk.verdict());
            }
            rounds += 1;
            if rounds > limit {
                break Err(format!("gave up after {rounds} rounds: the base kept moving or the batch kept changing"));
            }
            // -- assemble
            let asm = {
                let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
                batch.assemble(&active, &point)
            };
            let mut asm = match asm {
                Ok(a) => a,
                Err(e) => break Err(format!("could not assemble the batch: {e}")),
            };
            // MASON, not the tasks, writes the README status block, from the stage notes
            // the batch brings. It is committed here so the union that is verified is the
            // union that lands.
            if !asm.merged.is_empty() && batch.regenerate_status() {
                if let Ok(h) = git::rev_parse(&int_wt, "HEAD") {
                    asm.head = h;
                }
            }
            for t in &asm.already {
                landed.push(*t);
            }
            for e in &asm.ejected {
                parked.push(Parked {
                    task: e.id,
                    reason: format!("conflicts with the rest of its batch in {}", e.files),
                    feedback: format!(
                        "Your change conflicts with other changes landing at the same time (batch #{batch_id}) in: {}. Bring in the latest base branch and resolve it.",
                        e.files
                    ),
                    conflict: true,
                });
            }
            for pk in parked.iter().filter(|x| delivered.insert(x.task)) {
                deliver(d, TaskId(pk.task), pk.verdict());
            }
            active.retain(|t| asm.merged.contains(&t.id));
            if !asm.resolved.is_empty() {
                note = format!("conflicts in tasks {:?} were resolved", asm.resolved);
            }
            self.record(
                batch_id, "merged", &landed, &parked, &note, &asm.head, false,
            );
            if active.is_empty() {
                break Ok(());
            }

            // -- verify the union once
            let suites = if force {
                vec![]
            } else {
                self.suites_for(&def, &batch, &asm)
            };
            self.record(
                batch_id,
                "verifying",
                &landed,
                &parked,
                &note,
                &asm.head,
                false,
            );
            let mut vr = self.verify(
                &def,
                &suites,
                &int_wt,
                &asm.base_tip,
                &log_dir,
                &format!("b{batch_id}-r{rounds}"),
            );
            if let Some(e) = self.take_service_error() {
                break Err(e);
            }
            if vr.green && !vr.notes.is_empty() {
                note = vr.notes.join("; ");
            }
            if !vr.green {
                self.point("batch_red");
                self.record(
                    batch_id,
                    "red",
                    &landed,
                    &parked,
                    &vr.failed.iter().cloned().collect::<Vec<_>>().join(", "),
                    &asm.head,
                    false,
                );
                // The red ladder.
                let mut resolved = false;
                for rung in &self.cfg.ladder {
                    match rung {
                        Rung::RerunFailed => {
                            let again: Vec<String> = vr.failed.iter().cloned().collect();
                            let r2 = self.verify(
                                &def,
                                &again,
                                &int_wt,
                                &asm.base_tip,
                                &log_dir,
                                &format!("b{batch_id}-r{rounds}x"),
                            );
                            if let Some(e) = self.take_service_error() {
                                break 'outer Err(e);
                            }
                            if r2.green {
                                note = format!(
                                    "{} was red once and green on a rerun",
                                    again.join(", ")
                                );
                                vr.green = true;
                                resolved = true;
                                break;
                            }
                        }
                        Rung::BlameByImpact => {
                            let fps: Vec<(u64, &Footprint)> = active
                                .iter()
                                .filter_map(|t| self.fps.get(&t.id).map(|f| (t.id, f)))
                                .collect();
                            let blamed = plan::blame_by_impact(&vr.failed, &fps);
                            if !blamed.is_empty()
                                && (blamed.len() < active.len() || active.len() == 1)
                            {
                                self.park(&mut parked, &mut active, &blamed, batch_id, &vr);
                                continue 'outer;
                            }
                        }
                        Rung::Bisect => {
                            if active.len() == 1 {
                                let id = active[0].id;
                                self.park(&mut parked, &mut active, &[id], batch_id, &vr);
                                continue 'outer;
                            }
                            let ids: Vec<u64> = active.iter().map(|t| t.id).collect();
                            let mut red = |subset: &[u64]| -> bool {
                                let sub: Vec<BatchTask> = active
                                    .iter()
                                    .filter(|t| subset.contains(&t.id))
                                    .cloned()
                                    .collect();
                                let a = {
                                    let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
                                    batch.assemble(&sub, &|_| {})
                                };
                                let Ok(a) = a else { return false };
                                let s = self.suites_for(&def, &batch, &a);
                                !self
                                    .verify(
                                        &def,
                                        &s,
                                        &int_wt,
                                        &a.base_tip,
                                        &log_dir,
                                        &format!("b{batch_id}-r{rounds}-bis"),
                                    )
                                    .green
                            };
                            let found = plan::bisect(&ids, &mut red);
                            if let Some(e) = self.take_service_error() {
                                break 'outer Err(e);
                            }
                            if let Bisect::Culprit(id) = found {
                                self.park(&mut parked, &mut active, &[id], batch_id, &vr);
                                continue 'outer;
                            }
                        }
                        Rung::Ask => break,
                    }
                }
                if !resolved {
                    // Asking: the owner is told by an alert; nothing lands, and the batch's
                    // tasks go back with the evidence rather than waiting on a guess.
                    let all: Vec<u64> = active.iter().map(|t| t.id).collect();
                    let why = format!(
                        "batch #{batch_id} is red in {} and the red cannot be pinned on one task",
                        vr.failed.iter().cloned().collect::<Vec<_>>().join(", ")
                    );
                    let _ = d.store.set_batch(
                        batch_id,
                        "red",
                        &landed
                            .iter()
                            .map(u64::to_string)
                            .collect::<Vec<_>>()
                            .join(","),
                        &listed,
                        &why,
                        &asm.head,
                        false,
                        now_ms(),
                    );
                    for t in &all {
                        parked.push(Parked {
                            task: *t,
                            reason: why.clone(),
                            feedback: format!("{why}.\n{}", vr.notes.join("\n")),
                            conflict: false,
                        });
                    }
                    active.clear();
                    break Err(why);
                }
            }
            self.point("batch_verified");

            // -- promote
            self.record(
                batch_id,
                "promoting",
                &landed,
                &parked,
                &note,
                &asm.head,
                false,
            );
            let union_fp = Footprint {
                files: batch.union_diff(&asm).into_iter().collect(),
                suites: suites.iter().cloned().collect(),
            };
            let impacts = |path: &str| -> Vec<String> {
                match def.testing.impact.select(&[path.to_string()], &suites) {
                    Selection::All => suites.clone(),
                    Selection::Some(v) => v,
                }
            };
            let legacy = self.legacy_cmd(&def);
            let wt_for_cmd = int_wt.clone();
            let mut reverify = |moved: &[String]| -> bool {
                if force {
                    return true;
                }
                let outside = self.cfg.mode.uses_candidate_ref();
                if suites.is_empty() && !outside {
                    return legacy
                        .as_ref()
                        .is_none_or(|cmd| run_shell(cmd, &wt_for_cmd, p.test_timeout).0);
                }
                // A moved base means CI runs again on the new tip, whatever moved.
                if !outside && !plan::needs_reverify(moved, &union_fp, &impacts) {
                    return true;
                }
                self.verify(
                    &def,
                    &suites,
                    &wt_for_cmd,
                    &asm.base_tip,
                    &log_dir,
                    &format!("b{batch_id}-r{rounds}-fold"),
                )
                .green
            };
            if self.cfg.mode.uses_pr() && !force {
                let end = self.finish_pr(&batch, &int_wt);
                match end {
                    Err(e) => break Err(e),
                    Ok(PrEnd::Merged(c)) => {
                        commit = c;
                        *self.pr.lock().unwrap_or_else(|e| e.into_inner()) = None;
                        for t in &active {
                            landed.push(t.id);
                        }
                        break Ok(());
                    }
                    // Red after it was green, or a conflict with the moved base: assemble again.
                    Ok(PrEnd::Red) | Ok(PrEnd::Conflict) => continue 'outer,
                    Ok(PrEnd::BaseKeptMoving) => {
                        break Err(format!(
                            "the base moved more than {} times while landing",
                            self.cfg.max_fold
                        ))
                    }
                    Ok(PrEnd::ChangesRequested) | Ok(PrEnd::Closed(_)) => {
                        let why = match end_reason(&end) {
                            Some(w) => w,
                            None => "a reviewer asked for changes on the pull request".to_string(),
                        };
                        for t in active.drain(..) {
                            parked.push(Parked {
                                task: t.id,
                                reason: format!("batch #{batch_id}: {why}"),
                                feedback: format!("Your change was not merged: {why}."),
                                conflict: false,
                            });
                        }
                        break Err(why);
                    }
                }
            }
            let fin = {
                let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
                batch.finish(&asm, &mut reverify, &point)
            };
            match fin {
                Err(e) => break Err(format!("landing failed: {e}")),
                Ok(Err(_red)) => continue 'outer, // red after folding: assemble on the new base, blame properly
                Ok(Ok(Finish::Conflict(_))) => continue 'outer, // the conflict shows up as an ejection
                Ok(Ok(Finish::BaseKeptMoving)) => {
                    break Err(format!(
                        "the base moved more than {} times while landing",
                        self.cfg.max_fold
                    ))
                }
                Ok(Ok(Finish::LocalBaseCheckedOut(path))) => {
                    break Err(format!(
                    "the base branch is checked out at {}; use a remote, or the local-only mode",
                    path.display()
                ))
                }
                Ok(Ok(Finish::Promoted { commit: c, .. })) => {
                    commit = c;
                    for t in &active {
                        landed.push(t.id);
                    }
                    break Ok(());
                }
            }
        };

        let phase = match (&outcome, landed.is_empty(), parked.is_empty()) {
            (Ok(()), _, true) => "landed",
            (Ok(()), false, false) => "landed",
            (Ok(()), true, false) => "parked",
            (Err(_), _, false) => "parked",
            (Err(_), _, true) => "failed",
        };
        let final_note = match &outcome {
            Err(w) => w.clone(),
            Ok(()) => note.clone(),
        };
        self.record(
            batch_id,
            phase,
            &landed,
            &parked,
            &final_note,
            &commit,
            true,
        );
        self.clean_outside();
        // The integration branch was disposable.
        {
            let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
            if int_wt.join(".git").exists() {
                let _ = git::run(&int_wt, &["checkout", "--detach", "--quiet"]);
            }
            let _ = git::run(&p.path, &["branch", "-D", &int_branch]);
        }
        // -- the verdicts
        let was_ok = outcome.is_ok();
        let all_ids: Vec<u64> = ids.clone();
        match &outcome {
            Ok(()) => {
                for id in &all_ids {
                    if delivered.contains(id) {
                        continue;
                    }
                    if landed.contains(id) {
                        let _ = d.store.clear_landing_flag(*id);
                        deliver(d, TaskId(*id), Verdict::Landed);
                    } else if let Some(pk) = parked.iter().find(|x| x.task == *id) {
                        deliver(d, TaskId(*id), pk.verdict());
                    }
                }
            }
            Err(why) => {
                for id in &all_ids {
                    if delivered.contains(id) {
                        continue;
                    }
                    if let Some(pk) = parked.iter().find(|x| x.task == *id) {
                        deliver(d, TaskId(*id), pk.verdict());
                    } else if landed.contains(id) {
                        deliver(d, TaskId(*id), Verdict::Landed);
                    } else {
                        deliver(d, TaskId(*id), Verdict::Failed(why.clone()));
                    }
                }
            }
        }
        if was_ok && !commit.is_empty() {
            self.post_land(batch_id, &commit, &def, &log_dir);
        }
    }

    /// The suites to run over this assembly's union diff: the batch tier's,
    /// selected by what changed.
    fn suites_for(
        &self,
        def: &mason_definition::Definition,
        batch: &Batch,
        a: &Assembled,
    ) -> Vec<String> {
        let Some(tier) = self.cfg.batch_suites() else {
            return vec![];
        };
        let paths = batch.union_diff(a);
        match def.testing.impact.select(&paths, tier) {
            Selection::All => tier.to_vec(),
            Selection::Some(v) => v,
        }
    }

    fn legacy_cmd(&self, def: &mason_definition::Definition) -> Option<String> {
        crate::jobs::test_command(&self.d, def, self.pid, None)
    }

    fn verify(
        &self,
        def: &mason_definition::Definition,
        suites: &[String],
        wt: &Path,
        base: &str,
        log_dir: &Path,
        prefix: &str,
    ) -> crate::suites::OverRun {
        let green = || crate::suites::OverRun {
            green: true,
            failed: BTreeSet::new(),
            notes: vec![],
        };
        // Force lands past verification, CI included.
        if self.kind == "force" {
            return green();
        }
        // Names that are not suites (CI job names) have nothing to run locally.
        let local: Vec<String> = suites
            .iter()
            .filter(|s| {
                def.testing.suite(s).is_some()
                    || self.cfg.batch_suites().is_some_and(|t| t.contains(s))
            })
            .cloned()
            .collect();
        let mut r = if local.is_empty() {
            green()
        } else {
            let n = self.runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let stop = std::sync::atomic::AtomicBool::new(false);
            std::thread::scope(|sc| {
                sc.spawn(|| self.mirror(&stop));
                let r = crate::suites::run_over(
                    &self.d,
                    self.project(),
                    def,
                    &local,
                    wt,
                    base,
                    log_dir,
                    &format!("{prefix}-{n}"),
                    self.sink(),
                );
                stop.store(true, std::sync::atomic::Ordering::SeqCst);
                r
            })
        };
        if !r.green {
            return r;
        }
        let outside = if self.cfg.mode.uses_candidate_ref() {
            self.ci_verify(wt)
        } else if self.cfg.mode.uses_pr() {
            self.pr_verify(wt)
        } else {
            return r;
        };
        match outside {
            Ok((true, notes, _)) => r.notes.extend(notes),
            Ok((false, notes, failed)) => {
                r.green = false;
                r.failed.extend(failed);
                r.notes.extend(notes);
            }
            Err(e) => {
                // The service could not say: not the task's fault, and not red.
                // The batch cannot go on; it ends with this reason.
                self.service_error
                    .lock()
                    .unwrap_or_else(|x| x.into_inner())
                    .get_or_insert(e.clone());
                r.green = false;
                r.notes.push(e);
            }
        }
        r
    }

    fn take_service_error(&self) -> Option<String> {
        self.service_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    // ---- the outside: candidate refs, CI, pull requests ---------------------------------

    fn remote(&self) -> Result<String, String> {
        self.project()
            .remote
            .clone()
            .ok_or_else(|| "the project has no remote".to_string())
    }

    /// Push the worktree's HEAD to a branch of its own on the remote (never
    /// forced) and remember it, so that a crash cannot leave it behind unseen.
    fn push_candidate(&self, wt: &Path, kind: &str, name: &str) -> Result<(String, u64), String> {
        let remote = self.remote()?;
        let batch = self.batch_id.load(std::sync::atomic::Ordering::SeqCst);
        let sha = git::rev_parse(wt, "HEAD").map_err(|e| e.to_string())?;
        let id = self
            .d
            .store
            .add_ref(batch, &self.project().id, kind, name, None, now_ms())
            .map_err(|e| e.to_string())?;
        git::run(
            wt,
            &[
                "push",
                "--porcelain",
                &remote,
                &format!("{sha}:refs/heads/{name}"),
            ],
        )
        .map_err(|e| format!("could not push {name}: {e}"))?;
        self.point(&format!("{kind}_pushed"));
        Ok((sha, id))
    }

    fn drop_candidate(&self, name: &str, ref_id: u64) {
        if let Ok(remote) = self.remote() {
            let _ = git::run(
                &self.project().path,
                &["push", &remote, "--delete", &format!("refs/heads/{name}")],
            );
        }
        let _ = self.d.store.remove_ref(ref_id);
    }

    fn next_n(&self) -> u64 {
        self.runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    fn alert_key(&self, what: &str) -> String {
        format!(
            "{what}:b{}",
            self.batch_id.load(std::sync::atomic::Ordering::SeqCst)
        )
    }

    /// Sleep, but not through a daemon that is gone.
    fn nap(&self, secs: u64) {
        std::thread::sleep(Duration::from_secs(secs.max(1)));
    }

    /// Ask until a service answers with something final. A transient failure backs
    /// off; a permanent one raises attention at once and ends the wait.
    fn ask<T>(
        &self,
        what: &str,
        started: Instant,
        timeout_s: u64,
        poll_s: u64,
        mut f: impl FnMut() -> Result<Option<T>, mason_adapter_api::service::ApiError>,
    ) -> Result<T, String> {
        let mut poller = Poller::new(poll_s);
        let key = self.alert_key("service");
        loop {
            if started.elapsed().as_secs() > timeout_s {
                return Err(format!("{what}: still not finished after {timeout_s} s"));
            }
            let now = started.elapsed().as_secs();
            match f() {
                Ok(Some(v)) => {
                    self.d.landing.clear(self.pid, &key);
                    return Ok(v);
                }
                Ok(None) => {
                    self.d.landing.clear(self.pid, &key);
                    poller.observe(now, Observed::Unchanged);
                    self.nap(poll_s);
                }
                Err(e) => match poller.observe(now, Observed::Failed(e)) {
                    PollEvent::Stopped { why } => {
                        let text = format!("{}: {what}: {why}", self.project().id);
                        self.d.landing.raise(self.pid, &key, "landing", &text);
                        self.d.poke();
                        return Err(text);
                    }
                    PollEvent::Backoff { until, why } => {
                        self.d.landing.raise(
                            self.pid,
                            &key,
                            "landing",
                            &format!("{}: {what}: {why}; trying again", self.project().id),
                        );
                        self.nap(until.saturating_sub(now));
                    }
                    _ => {}
                },
            }
        }
    }

    /// `direct-ci` and `local-only-ci`: push `mason/ci/<project>-b<batch>-<n>`, run CI
    /// on it, take the answer, delete the ref. `(green, notes, failed job names)`.
    fn ci_verify(&self, wt: &Path) -> Result<(bool, Vec<String>, Vec<String>), String> {
        let (ci, cfg) = self.ci.clone().ok_or("the project has no [ci] section")?;
        let batch = self.batch_id.load(std::sync::atomic::Ordering::SeqCst);
        let name = format!("mason/ci/{}-b{batch}-{}", self.project().id, self.next_n());
        let (sha, ref_id) = self.push_candidate(wt, "ci", &name)?;
        let started = Instant::now();
        self.say(&format!("waiting for CI on {name}"));
        let r = (|| {
            let run = self.ask("CI trigger", started, cfg.timeout_s, cfg.poll_s, || {
                ci.trigger(&name, &sha).map(Some)
            })?;
            self.ask("CI", started, cfg.timeout_s, cfg.poll_s, || {
                Ok(match ci.status(&run)? {
                    CiState::Pending => None,
                    done => Some(done),
                })
            })
        })();
        self.point("ci_finished");
        self.drop_candidate(&name, ref_id);
        Ok(match r? {
            CiState::Green => (true, vec![format!("CI on {name}: green")], vec![]),
            CiState::Red { failed, summary } => {
                (false, vec![format!("CI on {name}: {summary}")], failed)
            }
            CiState::Pending => unreachable!("pending is waited out"),
        })
    }

    /// The pull-request modes: push a branch, open a PR from it, wait for its checks.
    /// Red closes the PR; green keeps it (in `self.pr`) for the merge.
    fn pr_verify(&self, wt: &Path) -> Result<(bool, Vec<String>, Vec<String>), String> {
        let host = self
            .host
            .clone()
            .ok_or("the project has no [host] section")?;
        let timeout = self
            .project()
            .services
            .ci
            .as_ref()
            .map_or(7200, |c| c.timeout_s);
        let poll = self.project().services.ci.as_ref().map_or(20, |c| c.poll_s);
        let batch = self.batch_id.load(std::sync::atomic::Ordering::SeqCst);
        let name = format!("mason/pr/{}-b{batch}-{}", self.project().id, self.next_n());
        let (_sha, ref_id) = self.push_candidate(wt, "pr", &name)?;
        let started = Instant::now();
        let ids = self
            .fps
            .keys()
            .map(|k| format!("#{k}"))
            .collect::<Vec<_>>()
            .join(" ");
        let opened = self.ask("opening the pull request", started, timeout, poll, || {
            host.open_pr(
                &self.project().base,
                &name,
                &format!("MASON batch {batch}: tasks {ids}"),
                &format!("Landed by MASON (batch {batch}): {ids}."),
            )
            .map(Some)
        });
        let pr = match opened {
            Ok(pr) => pr,
            Err(e) => {
                self.drop_candidate(&name, ref_id);
                return Err(e);
            }
        };
        let _ = self.d.store.set_ref_number(ref_id, pr.number);
        self.point("pr_opened");
        // A newer green PR replaces an older one the batch was holding.
        if let Some(old) = self.pr.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = host.close_pr(old.number);
            self.drop_candidate(&old.branch, old.ref_id);
        }
        self.say(&format!(
            "waiting for the pull request's checks (#{})",
            pr.number
        ));
        let st = self.ask("the pull request's checks", started, timeout, poll, || {
            let st = host.pr_status(pr.number)?;
            if let Some(c) = &st.counts {
                self.checks(c);
            }
            Ok(match (st.state, st.checks) {
                (PrState::Closed, _) => Some(st),
                (_, Checks::Pending) => None,
                _ => Some(st),
            })
        });
        let st = match st {
            Ok(st) => st,
            Err(e) => {
                let _ = host.close_pr(pr.number);
                self.drop_candidate(&name, ref_id);
                return Err(e);
            }
        };
        self.point("pr_checked");
        if st.checks == Checks::Red || st.state == PrState::Closed {
            let _ = host.close_pr(pr.number);
            self.drop_candidate(&name, ref_id);
            let why = if st.state == PrState::Closed && st.checks != Checks::Red {
                "the PR was closed"
            } else {
                "checks failed"
            };
            return Ok((
                false,
                vec![format!(
                    "PR #{} ({name}): {why}: {}",
                    pr.number,
                    st.failed.join(", ")
                )],
                st.failed,
            ));
        }
        *self.pr.lock().unwrap_or_else(|e| e.into_inner()) = Some(OpenPr {
            number: pr.number,
            branch: name.clone(),
            ref_id,
        });
        Ok((
            true,
            vec![format!("PR #{}: checks green", pr.number)],
            vec![],
        ))
    }

    /// Merge the verified PR (or hand it to the host's queue), waiting for any
    /// review it needs. If the base moved so the host refuses, fold the new tip
    /// in and push (a plain push: the PR's own branch only grows), and the
    /// checks run again.
    fn finish_pr(&self, batch: &Batch, int_wt: &Path) -> Result<PrEnd, String> {
        let host = self
            .host
            .clone()
            .ok_or("the project has no [host] section")?;
        let open = self
            .pr
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or("no pull request is open for the batch")?;
        let timeout = self
            .project()
            .services
            .ci
            .as_ref()
            .map_or(7200, |c| c.timeout_s);
        let poll = self.project().services.ci.as_ref().map_or(20, |c| c.poll_s);
        let started = Instant::now();
        let base_ref = batch.base_ref();
        let waiting = self.alert_key("review");
        let queue = self.cfg.mode == config::Mode::HostQueue;
        let mut queued = false;
        let mut folds = 0;
        let mut last_head = git::rev_parse(int_wt, "HEAD").map_err(|e| e.to_string())?;
        self.point("batch_before_promote");
        loop {
            let st = self.ask("the pull request", started, timeout, poll, || {
                host.pr_status(open.number).map(Some)
            })?;
            match st.state {
                PrState::Merged => {
                    self.d.landing.clear(self.pid, &waiting);
                    let _ = git::run(&self.project().path, &["fetch", "--prune", &self.remote()?]);
                    let commit = git::rev_parse(&self.project().path, &base_ref)
                        .map_err(|e| e.to_string())?;
                    self.point("batch_after_promote");
                    return Ok(PrEnd::Merged(commit));
                }
                PrState::Closed => {
                    self.d.landing.clear(self.pid, &waiting);
                    return Ok(PrEnd::Closed(if queue {
                        "dropped from the host's merge queue".into()
                    } else {
                        "closed without merging".into()
                    }));
                }
                PrState::Open => {}
            }
            if st.checks == Checks::Red {
                self.d.landing.clear(self.pid, &waiting);
                return Ok(PrEnd::Red);
            }
            if st.review == Review::ChangesRequested {
                self.d.landing.clear(self.pid, &waiting);
                return Ok(PrEnd::ChangesRequested);
            }
            if st.checks == Checks::Pending {
                self.nap(poll);
                continue;
            }
            if st.review == Review::Required {
                self.d.landing.raise(
                    self.pid,
                    &waiting,
                    "needs_you",
                    &format!(
                        "{}: pull request #{} is waiting for an approval",
                        self.project().id,
                        open.number
                    ),
                );
                self.d.poke();
                self.nap(poll);
                continue;
            }
            self.d.landing.clear(self.pid, &waiting);
            if queue {
                if !queued {
                    self.ask("the merge queue", started, timeout, poll, || {
                        host.enqueue(open.number).map(Some)
                    })?;
                    queued = true;
                }
                self.nap(poll);
                continue;
            }
            match host.merge_pr(open.number) {
                Ok(()) => continue, // read Merged on the next look
                Err(e) if e.permanent() => {
                    // Refused. Did the base move under us? Then fold it in and let checks run again.
                    let remote = self.remote()?;
                    let _ = git::run(&self.project().path, &["fetch", "--prune", &remote]);
                    let tip = git::rev_parse(&self.project().path, &base_ref)
                        .map_err(|e| e.to_string())?;
                    if git::is_ancestor(int_wt, &tip, "HEAD") {
                        return Err(format!(
                            "the host refused to merge pull request #{}: {}",
                            open.number, e.detail
                        ));
                    }
                    folds += 1;
                    if folds > self.cfg.max_fold {
                        return Ok(PrEnd::BaseKeptMoving);
                    }
                    if let git::CatchUp::Conflict(_) =
                        git::catch_up(int_wt, &base_ref).map_err(|e| e.to_string())?
                    {
                        return Ok(PrEnd::Conflict);
                    }
                    let head = git::rev_parse(int_wt, "HEAD").map_err(|e| e.to_string())?;
                    if head != last_head {
                        git::run(
                            int_wt,
                            &[
                                "push",
                                "--porcelain",
                                &remote,
                                &format!("{head}:refs/heads/{}", open.branch),
                            ],
                        )
                        .map_err(|e| format!("could not update {}: {e}", open.branch))?;
                        last_head = head;
                    }
                    self.nap(poll);
                }
                Err(e) => {
                    // Transient: look again.
                    self.d.landing.raise(
                        self.pid,
                        &self.alert_key("service"),
                        "landing",
                        &format!("{}: merging: {}", self.project().id, e.detail),
                    );
                    self.nap(poll);
                }
            }
        }
    }

    /// Take down whatever the batch put on the outside: close its PR, delete its refs.
    fn clean_outside(&self) {
        let batch = self.batch_id.load(std::sync::atomic::Ordering::SeqCst);
        remove_refs(&self.d, self.pid, Some(batch));
        self.d.landing.clear(self.pid, &self.alert_key("review"));
        self.d.landing.clear(self.pid, &self.alert_key("service"));
    }

    fn park(
        &self,
        parked: &mut Vec<Parked>,
        active: &mut Vec<BatchTask>,
        blamed: &[u64],
        batch_id: u64,
        vr: &crate::suites::OverRun,
    ) {
        for id in blamed {
            parked.push(Parked {
                task: *id,
                reason: format!("red in batch #{batch_id}: {}", vr.failed.iter().cloned().collect::<Vec<_>>().join(", ")),
                feedback: format!(
                    "Your change made {} red when it landed with the others (batch #{batch_id}).\n{}",
                    vr.failed.iter().cloned().collect::<Vec<_>>().join(", "),
                    vr.notes.join("\n")
                ),
                conflict: false,
            });
        }
        active.retain(|t| !blamed.contains(&t.id));
    }

    /// The post-land tier: suites on the base after it moved. Red freezes
    /// landing (only a fix passes) and files a top-priority fix task.
    fn post_land(
        &self,
        batch_id: u64,
        commit: &str,
        def: &mason_definition::Definition,
        log_dir: &Path,
    ) {
        let d = &self.d;
        let p = self.project().clone();
        let Some(tier) = self.cfg.post_land().cloned() else {
            return;
        };
        if tier.suites.is_empty() {
            return;
        }
        let wt = self
            .int_wt()
            .with_file_name(format!("{}-post{}", p.id, self.slot));
        let lock = d.repo_lock(self.pid);
        {
            let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
            git::remove_worktree(&p.path, &wt);
            if git::run(
                &p.path,
                &["worktree", "add", "--detach", &wt.to_string_lossy(), commit],
            )
            .is_err()
            {
                return;
            }
        }
        let r = crate::suites::run_over(
            d,
            &p,
            def,
            &tier.suites,
            &wt,
            commit,
            log_dir,
            &format!("b{batch_id}-post"),
            None,
        );
        {
            let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
            git::remove_worktree(&p.path, &wt);
        }
        let frozen = d
            .store
            .freezes()
            .ok()
            .and_then(|v| v.into_iter().find(|f| f.project == p.id));
        if r.green {
            if frozen.is_some() {
                let _ = d.store.clear_freeze(&p.id);
            }
        } else if tier.on_red.as_deref() == Some("freeze") && frozen.is_none() {
            let reason = format!(
                "after batch #{batch_id} landed, {} went red on the base",
                r.failed.iter().cloned().collect::<Vec<_>>().join(", ")
            );
            let fix = d
                .emit(EventKind::TaskFiled {
                    project: self.pid,
                    title: format!(
                        "Fix the base: {} red after batch #{batch_id}",
                        r.failed.iter().cloned().collect::<Vec<_>>().join(", ")
                    ),
                    body: format!("{reason}.\n\n{}", r.notes.join("\n")),
                    priority: Priority::Urgent,
                    trust: TrustSource::Owner,
                    explored: false,
                    proposed: false,
                    blockers: vec![],
                    not_before: None,
                })
                .ok()
                .and_then(|a| {
                    a.effects.iter().find_map(|e| match e {
                        Effect::TaskCreated { task } => Some(task.0),
                        _ => None,
                    })
                });
            let _ = d.store.set_freeze(&p.id, &reason, fix, now_ms());
        }
        d.landing.wake();
        d.poke();
    }
}
