// SPDX-License-Identifier: GPL-3.0-or-later

//! The orchestrator. Events go in the log; the effects the reducer asks for
//! are carried out here on worker threads, and what they find comes back as
//! new events. The same code path runs at startup (to resume what was in
//! flight) and after every event.

use crate::config::{load_projects, DaemonConfig, ProjectCfg};
use crate::crash::Crash;
use crate::pool::Pool;
use mason_core::*;
use mason_store::{Appended, Store, StoreError};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// An agent past its silence alert limit.
#[derive(Clone, Debug)]
pub struct SilentAgent {
    pub task: u64,
    pub step: String,
    pub silent_ms: u64,
}

pub struct Daemon {
    pub cfg: DaemonConfig,
    pub store: Store,
    /// Millisecond time of the last store write refused for want of disk; 0 if none since.
    store_full: AtomicU64,
    pub disk_sizes: Arc<crate::disk::Sizes>,
    /// By numeric id; the project's string id is `ProjectCfg::id`.
    pub projects: BTreeMap<ProjectId, ProjectCfg>,
    /// The LLM provider accounts, in providers.toml order.
    pub accounts: Vec<crate::providers::Account>,
    /// Blocking work for the async layer (the store, git).
    pub pool: Pool,
    pub crash: Crash,
    pub started: Instant,
    pub started_ms: u64,
    jobs: Mutex<HashMap<StepRunId, TaskId>>,
    emit_lock: Mutex<()>,
    admitting: AtomicBool,
    admit_again: AtomicBool,
    repo_locks: Mutex<HashMap<PathBuf, RepoLock>>,
    /// When the definitions thread last finished a pass (ms; the start time before the first).
    pub(crate) defs_pass_ended_ms: AtomicU64,
    stalls: AtomicU64,
    /// Agents that are silent past their alert limit: run -> (task, step, silent since, ms).
    pub(crate) silent: Mutex<HashMap<u64, SilentAgent>>,
    worst_stall_ms: AtomicU64,
    pub(crate) sweep: Mutex<crate::sweep::SweepState>,
    pub(crate) defs: crate::defs::Defs,
    pub broker: Arc<crate::sched::Broker>,
    /// Appended event sequence numbers, for the clients' sockets.
    pub bus: tokio::sync::broadcast::Sender<u64>,
    /// The live progress records of running steps, and their changes for the sockets.
    pub(crate) progress: crate::progress::Book,
    pub progress_bus: tokio::sync::broadcast::Sender<crate::progress::Update>,
    /// `--demo`: plays a scripted scenario; runs no work.
    pub demo: bool,
    pub(crate) demo_pos: Mutex<usize>,
    pub(crate) landing: crate::landing::Landing,
    pub(crate) tracker: crate::tracker::Hub,
    pub update_status: Mutex<crate::update::Status>,
    /// What the last look at the update channel found.
    pub update_check: Mutex<crate::update::Check>,
    /// One fetch into the source mirror at a time.
    pub update_fetch: Mutex<()>,
    /// Monitor builds, by kind.
    pub monitor_builds: Mutex<std::collections::HashMap<String, crate::update::MonitorState>>,
    pub(crate) drafts: crate::editor::Drafts,
    /// The one probe session per held account (see `holds`).
    pub probes: crate::holds::Probes,
    pub(crate) intro: crate::introspect::Runtime,
    pub(crate) notify: crate::notify::Runtime,
    pub lan: crate::lan::LanState,
    /// Simulators, emulators and phones, leased by capability (Stage 25).
    pub targets: crate::targets::Targets,
}

fn kind_name(k: &EventKind) -> String {
    format!("{k:?}")
        .split([' ', '{', '('])
        .next()
        .unwrap_or("?")
        .to_string()
}

impl Daemon {
    /// Open the store, register the configured projects, resume whatever was
    /// in flight when the last daemon died, and start the reconcile sweep.
    pub fn start(cfg: DaemonConfig) -> Result<Arc<Daemon>, String> {
        Self::start_inner(cfg, false)
    }

    /// A daemon that plays a scripted scenario (`--demo`): no projects, no
    /// agents, no git. The events are the real kinds, so the views, the
    /// protocol and the commands are the real ones.
    pub fn start_demo(cfg: DaemonConfig) -> Result<Arc<Daemon>, String> {
        Self::start_inner(cfg, true)
    }

    fn start_inner(cfg: DaemonConfig, demo: bool) -> Result<Arc<Daemon>, String> {
        for d in [
            cfg.hosts_dir(),
            cfg.sockets_dir(),
            cfg.worktrees_dir(),
            cfg.landing_dir(),
            cfg.agent_state_dir(),
            cfg.data_dir.join("t"),
        ] {
            std::fs::create_dir_all(&d)
                .map_err(|e| format!("cannot create {}: {e}", d.display()))?;
        }
        let configured = if demo {
            vec![]
        } else {
            load_projects(&cfg.config_dir).map_err(|e| e.join("\n"))?
        };
        let store =
            Store::open(cfg.store_path()).map_err(|e| format!("cannot open the store: {e}"))?;

        // Give each configured project a stable numeric id, registering new ones.
        let world = store.world().map_err(|e| e.to_string())?;
        let mut next = world
            .projects
            .keys()
            .map(|p| p.0)
            .max()
            .map_or(1, |m| m + 1);
        let mut projects = BTreeMap::new();
        for p in configured {
            let existing = world
                .projects
                .values()
                .find(|x| x.name == p.id)
                .map(|x| x.id);
            let id = match existing {
                Some(id) => id,
                None => {
                    let id = ProjectId(next);
                    next += 1;
                    store
                        .append(Event {
                            at: Timestamp(now_ms()),
                            kind: EventKind::ProjectRegistered {
                                project: id,
                                name: p.id.clone(),
                            },
                        })
                        .map_err(|e| e.to_string())?;
                    id
                }
            };
            projects.insert(id, p);
        }

        let rcfg = crate::sched::load_resource_cfg(&cfg.config_dir)?;
        // `MASON_CPU_FILE` names a file holding the percentage: how tests and the
        // exit scripts drive the compile gate without load.
        let cpu: Arc<dyn crate::cpu::CpuSource> =
            match (&cfg.cpu, std::env::var_os("MASON_CPU_FILE")) {
                (Some(c), _) => c.clone(),
                (None, Some(f)) => Arc::new(crate::cpu::FileCpu(f.into())),
                (None, None) => crate::cpu::Sampler::start(),
            };
        let accounts = if demo {
            crate::demo::accounts()
        } else {
            crate::providers::load(&cfg.config_dir)?
        };
        crate::providers::check_llm_conflict(&accounts, &rcfg.llm)?;
        // `MASON_DISK_FILE` names a file holding the free gigabytes: how tests and the exit
        // scripts drive the disk floor without touching the real disk.
        let disk: Arc<dyn crate::disk::DiskSource> =
            match (&cfg.disk, std::env::var_os("MASON_DISK_FILE")) {
                (Some(s), _) => s.clone(),
                (None, Some(f)) => Arc::new(crate::disk::FileDisk(f.into())),
                (None, None) => Arc::new(crate::disk::Statvfs),
            };
        let disk_every = std::env::var("MASON_DISK_EVERY_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map_or(cfg.disk_every, Duration::from_millis);
        let broker = crate::sched::Broker::new(
            cfg.data_dir.join("locks"),
            cfg.data_dir.join("limits.kv"),
            &rcfg,
            cfg.max_concurrent as u32,
            cpu,
            &accounts,
        )?;
        // The volumes that matter: the data directory (the store) and each project's build
        // directories.
        let mut disk_paths = vec![cfg.data_dir.clone()];
        {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default();
            for p in projects.values() {
                if let Some(t) = &p.task_cache {
                    let e = crate::taskcache::expand(t, 0, &home);
                    if !e.trim().is_empty() {
                        disk_paths.push(PathBuf::from(e));
                    }
                }
            }
        }
        broker.set_disk(disk, disk_paths, rcfg.disk_floor_gb, disk_every);
        let targets = crate::targets::Targets::new(
            crate::config::load_targets(&cfg.config_dir)?,
            &cfg.data_dir.join("locks"),
        )?;
        let d = Arc::new(Daemon {
            store,
            projects,
            pool: Pool::new("blocking", 4, 64),
            crash: Crash::from_env(),
            started: Instant::now(),
            started_ms: now_ms(),
            jobs: Mutex::default(),
            emit_lock: Mutex::default(),
            admitting: AtomicBool::new(false),
            admit_again: AtomicBool::new(false),
            repo_locks: Mutex::default(),
            defs_pass_ended_ms: AtomicU64::new(now_ms()),
            stalls: AtomicU64::new(0),
            silent: Mutex::default(),
            store_full: AtomicU64::new(0),
            disk_sizes: Default::default(),
            worst_stall_ms: AtomicU64::new(0),
            sweep: Mutex::default(),
            defs: crate::defs::Defs::default(),
            broker,
            accounts,
            bus: tokio::sync::broadcast::channel(1024).0,
            progress: Default::default(),
            progress_bus: tokio::sync::broadcast::channel(1024).0,
            demo,
            demo_pos: Mutex::new(0),
            landing: Default::default(),
            tracker: Default::default(),
            drafts: Default::default(),
            probes: Default::default(),
            intro: Default::default(),
            notify: Default::default(),
            lan: crate::lan::LanState::new(),
            targets,
            update_status: Mutex::new(crate::update::Status {
                phase: "idle".into(),
                ..Default::default()
            }),
            update_check: Default::default(),
            update_fetch: Mutex::new(()),
            monitor_builds: Default::default(),
            cfg,
        });
        if demo {
            crate::demo::seed_usage(&d);
        } else {
            d.load_definitions();
            crate::targets::spawn(&d);
            crate::update::spawn_checker(&d);
            d.progress_cleanup();
            d.recover();
            d.spawn_progress_ticker();
            d.spawn_sweeper();
            crate::usage::spawn_probe(&d);
            d.spawn_watcher();
            crate::tracker::spawn(&d);
            crate::introspect::spawn(&d);
            crate::selfguard::on_start(&d);
        }
        Ok(d)
    }

    /// Something other than an event changed what a client would see (a pause,
    /// a debt, a limit): wake the sockets.
    pub fn poke(&self) {
        let _ = self.bus.send(self.store.last_seq().unwrap_or(0));
    }

    /// Wait for an event being appended to finish, then hold off none: used just
    /// before this process is replaced.
    pub fn quiesce(&self) {
        drop(self.emit_lock.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// Why a project's tracker poll stopped, if it did.
    pub fn tracker_stopped(&self, p: ProjectId) -> Option<String> {
        self.tracker.stopped(p)
    }

    /// What the last look at a project's tracker found.
    pub fn tracker_record(&self, p: ProjectId) -> Option<crate::tracker::PollRecord> {
        self.tracker.record(p)
    }

    /// Open issues the tracker has not imported (or whose task was canceled), with why.
    pub fn tracker_pending(&self, p: ProjectId) -> Vec<crate::tracker::Waiting> {
        self.tracker.pending(p)
    }

    /// Ask the project's tracker again at once.
    pub fn tracker_refresh(&self, p: ProjectId) {
        self.tracker.refresh(p);
    }

    /// The landing queues (for the views, and for tests).
    pub fn landing(&self) -> &crate::landing::Landing {
        &self.landing
    }

    pub fn world(&self) -> Result<std::sync::Arc<mason_engine::World>, StoreError> {
        self.store.world()
    }

    pub fn project(&self, id: ProjectId) -> Option<&ProjectCfg> {
        self.projects.get(&id)
    }

    pub fn project_id_by_name(&self, name: &str) -> Option<ProjectId> {
        self.projects
            .iter()
            .find(|(_, p)| p.id == name)
            .map(|(id, _)| *id)
    }

    /// Serializes git work that touches a repository's shared state. The lock
    /// belongs to the *repository*, not the project: two projects may live in one
    /// repository (a branch each), and two `git fetch`es there collide on its refs.
    pub fn repo_lock(&self, p: ProjectId) -> RepoLock {
        let key = self.project(p).map_or_else(PathBuf::new, |c| {
            std::fs::canonicalize(&c.path).unwrap_or_else(|_| c.path.clone())
        });
        self.repo_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(key)
            .or_default()
            .clone()
    }

    /// Who holds a repository lock now and since when (label, ms). Never takes a repository lock
    /// itself, so the attention thread may call it while git is stuck.
    pub fn repo_lock_holders(&self) -> Vec<(String, u64)> {
        let locks: Vec<RepoLock> = self
            .repo_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        locks.iter().filter_map(RepoLock::holder).collect()
    }

    pub fn record_bug(&self, kind: &str, detail: &str) {
        tracing::warn!(kind, detail, "bug filed by the daemon");
        let _ = self.store.record_bug(now_ms(), kind, detail);
    }

    pub fn record_stall(&self, late: Duration) {
        self.stalls.fetch_add(1, Ordering::SeqCst);
        self.worst_stall_ms
            .fetch_max(late.as_millis() as u64, Ordering::SeqCst);
        tracing::warn!(late_ms = late.as_millis() as u64, "the event loop stalled");
    }

    pub fn stall_stats(&self) -> (u64, u64) {
        (
            self.stalls.load(Ordering::SeqCst),
            self.worst_stall_ms.load(Ordering::SeqCst),
        )
    }

    pub fn job_count(&self) -> usize {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Is any step run of `task` being worked on?
    pub fn has_job_for_task(&self, task: TaskId) -> bool {
        self.jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .any(|t| *t == task)
    }

    pub fn has_job(&self, run: StepRunId) -> bool {
        self.jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&run)
    }

    /// Append an event, then carry out what the reducer asked for.
    pub fn emit(self: &Arc<Self>, kind: EventKind) -> Result<Appended, StoreError> {
        let name = kind_name(&kind);
        let kind_for_release = kind.clone();
        let appended = {
            let _g = self.emit_lock.lock().unwrap_or_else(|e| e.into_inner());
            match self.store.append(Event {
                at: Timestamp(now_ms()),
                kind,
            }) {
                Ok(a) => {
                    self.store_full.store(0, Ordering::SeqCst);
                    a
                }
                Err(e) => {
                    self.note_store_error(&e);
                    return Err(e);
                }
            }
        };
        self.crash.point(&format!("after_append:{name}"));
        if let EventKind::Canceled { task, .. } = &kind_for_release {
            // Its numbers are released, never handed out again.
            let _ = self.store.release_numbers(task.0);
        }
        let _ = self.bus.send(appended.seq);
        self.handle_effects(&appended.effects);
        self.admit();
        Ok(appended)
    }

    /// A store write failed. If the disk is full, say so in the alerts, not only in the log; the
    /// alert clears when a write succeeds again.
    fn note_store_error(&self, e: &StoreError) {
        if crate::disk::is_disk_full(e) {
            self.store_full.store(now_ms().max(1), Ordering::SeqCst);
        }
    }

    /// When a store write last failed for want of disk, if none has succeeded since.
    pub fn store_full_since(&self) -> Option<u64> {
        Some(self.store_full.load(Ordering::SeqCst)).filter(|t| *t > 0)
    }

    /// Emit a job's result, but only if its step run is still the task's open
    /// one. A cancelled or retried task's old job must not advance the new run.
    /// Returns whether the event was appended.
    pub fn emit_for_run(self: &Arc<Self>, task: TaskId, run: StepRunId, kind: EventKind) -> bool {
        let name = kind_name(&kind);
        let appended = {
            let _g = self.emit_lock.lock().unwrap_or_else(|e| e.into_inner());
            match self.store.world() {
                Ok(w) if w.open_run(task) == Some(run) => {}
                _ => return false,
            }
            match self.store.append(Event {
                at: Timestamp(now_ms()),
                kind,
            }) {
                Ok(a) => {
                    self.store_full.store(0, Ordering::SeqCst);
                    a
                }
                Err(e) => {
                    self.note_store_error(&e);
                    tracing::warn!(%task, error = %e, "a job's event was refused");
                    return false;
                }
            }
        };
        self.crash.point(&format!("after_append:{name}"));
        let _ = self.bus.send(appended.seq);
        self.handle_effects(&appended.effects);
        self.admit();
        true
    }

    fn handle_effects(self: &Arc<Self>, effects: &[Effect]) {
        if self.demo {
            return;
        }
        for e in effects {
            match e {
                Effect::StartStep {
                    task, step, run, ..
                } => self.start_job(*task, *step, *run),
                Effect::StopStepHost { task } => self.stop_hosts(&format!("t{}r", task.0)),
                Effect::DeleteBranch { task } => {
                    let d = self.clone();
                    let task = *task;
                    let _ = self
                        .pool
                        .spawn(move || crate::jobs::cleanup(&d, task, true));
                }
                // Leases and the scheduler arrive in Stage 8; the rest need no work.
                _ => {}
            }
        }
    }

    pub fn start_job(self: &Arc<Self>, task: TaskId, step: State, run: StepRunId) {
        {
            let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            if jobs.contains_key(&run) {
                return;
            }
            jobs.insert(run, task);
        }
        let d = self.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("job-t{}r{}", task.0, run.0))
            .spawn(move || {
                d.progress_begin(task, run);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::jobs::run(&d, task, step, run)
                }));
                if result.is_err() {
                    d.record_bug("job_panic", &format!("task {task}, step run {run}"));
                    d.emit_for_run(
                        task,
                        run,
                        EventKind::StepFailed {
                            task,
                            reason: "internal error: the step's job panicked".into(),
                        },
                    );
                }
                d.progress_finish(run);
                d.jobs
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&run);
                d.admit();
            });
        if spawned.is_err() {
            self.jobs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&run);
            self.record_bug(
                "job_spawn",
                &format!("could not start a thread for task {task}"),
            );
        }
    }

    /// Stop every running step host whose id starts with `prefix`.
    pub fn stop_hosts(&self, prefix: &str) {
        let reg = mason_step_host::registry::Registry::new(self.cfg.hosts_dir());
        for s in reg.scan().unwrap_or_default() {
            if s.info.id.starts_with(prefix)
                && s.state == mason_step_host::registry::HostState::Running
            {
                tracing::warn!(host = %s.info.id, prefix, "stopping a step host");
                if let Ok(mut c) = mason_step_host::client::HostClient::connect(
                    &s.info.sock,
                    mason_step_host::protocol::PREVIOUS,
                    mason_step_host::protocol::CURRENT,
                    "daemon",
                ) {
                    let _ = c.stop(1000);
                }
            }
        }
    }

    /// Start queued tasks while an LLM session is free for them, best rank
    /// first across every project (Part 7).
    pub fn admit(self: &Arc<Self>) {
        if self.demo {
            return;
        }
        if self.admitting.swap(true, Ordering::SeqCst) {
            self.admit_again.store(true, Ordering::SeqCst);
            return;
        }
        // Whatever way this ends, a panic included, the next admit must be able to run.
        let _reset = AdmitReset(&self.admitting);
        self.crash.panic_point("admit");
        loop {
            self.admit_again.store(false, Ordering::SeqCst);
            while let Ok(w) = self.store.world() {
                let Some(id) = self.next_admission(&w) else {
                    break;
                };
                let Some(t) = w.tasks.get(&id) else { break };
                let entry = self.entry_for(t);
                // The task's numbers are handed out when it is first claimed.
                if let Err(why) = self.ensure_numbers(t.id) {
                    self.record_bug(
                        "numbers",
                        &format!("no numbers could be reserved for task {}: {why}", t.id),
                    );
                }
                if self
                    .emit(EventKind::Admitted {
                        task: t.id,
                        entry: Some(entry),
                    })
                    .is_err()
                {
                    break;
                }
            }
            if !self.admit_again.load(Ordering::SeqCst) {
                break;
            }
        }
    }

    /// How many LLM sessions are free to promise to a newly claimed task. A
    /// task that was claimed but has not yet asked for its step's resources
    /// still counts as holding one, so a full pool never produces worktrees
    /// with nothing happening in them.
    pub fn llm_free(&self, w: &mason_engine::World, account: &str) -> u32 {
        let name = crate::sched::llm_resource(account);
        // A task waiting for the owner's approval holds nothing.
        let approving = Activity::Waiting(WaitReason::On(APPROVAL_WAIT.into()));
        let unclaimed = w
            .tasks
            .values()
            .filter(|t| {
                t.state.is_step()
                    && t.activity != approving
                    && !self.broker.has_lease(t.id)
                    && self.account_of(t.project) == account
                    && self.step_needs_session(t)
            })
            .count() as u32;
        self.broker
            .limit(&name)
            .unwrap_or(0)
            .saturating_sub(self.broker.used(&name))
            .saturating_sub(unclaimed)
    }

    /// The provider account a project's agent steps use.
    pub fn account_of(&self, project: ProjectId) -> &str {
        self.project(project)
            .map_or(crate::sched::DEFAULT_ACCOUNT, |p| p.account.as_str())
    }

    /// Will this task's current step want an LLM session? Only an agent step
    /// does; a test or landing step that has not yet asked for its resources
    /// must not hold a session back from a task that is waiting for one.
    fn step_needs_session(&self, t: &Task) -> bool {
        let def = self
            .pinned_def(&t.def)
            .filter(|_| !t.def.is_empty())
            .unwrap_or_else(|| self.current_def(t.project));
        match def.step(&t.step) {
            Some(s) => s.kind == mason_definition::StepType::Agent,
            None => matches!(
                t.state,
                State::Exploring | State::Developing | State::Review
            ),
        }
    }

    /// The queued task that goes next, if an LLM session is free for it.
    pub fn next_admission(&self, w: &mason_engine::World) -> Option<TaskId> {
        // Accounts are separate: a full one holds back only its own projects.
        let free: std::collections::HashMap<&str, u32> = self
            .accounts
            .iter()
            .map(|a| (a.name.as_str(), self.llm_free(w, &a.name)))
            .collect();
        w.tasks
            .values()
            .filter(|t| {
                t.state == State::Queued && t.activity == Activity::Waiting(WaitReason::Slot)
            })
            .filter(|t| free.get(self.account_of(t.project)).copied().unwrap_or(0) > 0)
            // A pause on claims (or on everything) holds a project's queue back.
            .filter(|t| {
                let name = self.project(t.project).map_or("", |p| p.id.as_str());
                crate::attention::paused(self, name, crate::attention::What::Claim).is_none()
            })
            .min_by_key(|t| self.queued_rank(t))
            .map(|t| t.id)
    }

    /// A queued task's rank, from where it would start. (The clarity check
    /// is left out: it reads git, and ranking runs on every admission.)
    pub(crate) fn queued_rank(&self, t: &Task) -> mason_scheduler::Rank {
        let def = self.current_def(t.project);
        let skip = def.explored_entry.is_some() && t.explored_counts() && !t.force_explore;
        let start = def
            .entry(skip)
            .map_or_else(|| def.start.clone(), |s| s.name.clone());
        self.rank_for(t, &def, &start)
    }

    /// What a `mason run` made on behalf of `task` is ranked by: the steps
    /// remaining from the step the task is in now, and its priority. `None`
    /// for a task that does not exist or is finished or not running a step.
    pub fn lease_standing(&self, task: TaskId) -> Option<crate::sched::Standing> {
        let w = self.world().ok()?;
        let t = w.tasks.get(&task)?;
        if t.state.is_terminal() || !t.state.is_step() {
            return None;
        }
        let def = self.current_def(t.project);
        Some(crate::sched::Standing {
            steps_remaining: mason_definition::graph::steps_remaining(&def, &t.step),
            priority: t.priority as u8,
        })
    }

    /// The scheduling rank of a task about to run `step` (Part 7): not
    /// interactive, steps remaining on the shortest path to landed, tracker
    /// priority, enqueue time, task id.
    pub fn rank_for(
        &self,
        t: &Task,
        def: &mason_definition::Definition,
        step: &str,
    ) -> mason_scheduler::Rank {
        let jump = self.project(t.project).is_some_and(|p| p.priority_jump);
        mason_scheduler::Rank::new(
            false,
            mason_definition::graph::steps_remaining(def, step),
            t.priority as u8,
            t.enqueued_at.0,
            t.id.0,
            jump,
        )
    }

    /// Where a task starts and why, from the definition in force now (Part 4,
    /// "Skipping exploration"). The definition's `explored_entry` is what makes
    /// skipping possible at all.
    pub fn entry_for(&self, t: &Task) -> Entry {
        let def = self.current_def(t.project);
        let route = self.choose_route(t, &def);
        let step = def
            .entry(route.skipped_exploring())
            .or_else(|| def.step(&def.start))
            .expect("a validated definition has its start step");
        Entry {
            def: def.hash.clone(),
            step: step.name.clone(),
            state: step.category,
            route,
        }
    }

    fn choose_route(&self, t: &Task, def: &mason_definition::Definition) -> Route {
        // Without an explored_entry a project always explores; so does "Explore anyway".
        if def.explored_entry.is_none() || t.force_explore {
            return Route::Explore;
        }
        // The flag counts only from the owner and the owner's agents (or once approved).
        if t.explored_counts() {
            return Route::Flag;
        }
        // The deterministic check: no model, and anything uncertain explores.
        if let Some(idx) = self.file_index(t.project) {
            if mason_definition::clarity::check(&t.body, &|p| idx.exists(p)).clear {
                return Route::Clarity;
            }
        }
        // The project may add a classifier for tasks the rules cannot call.
        if let Some(cmd) = self
            .project(t.project)
            .and_then(|p| p.clarity_classifier.as_deref())
        {
            if crate::jobs::classify(cmd, &t.body) {
                return Route::Classifier;
            }
        }
        Route::Explore
    }

    /// Resume what was in flight: a job for every open step run. This is the
    /// normal path after a restart, not a repair, so it files no bug.
    pub fn recover(self: &Arc<Self>) {
        if let Ok(w) = self.store.world() {
            for r in w.step_runs.values().filter(|r| r.outcome.is_none()) {
                self.start_job(r.task, r.step, r.id);
            }
        }
        // Cleanup after landing is an effect too, and it can be lost to a crash.
        let d = self.clone();
        let _ = self.pool.spawn(move || d.recover_cleanup());
        self.admit();
    }

    /// Remove the worktree and branch of any task that is finished with them
    /// (landed, or cancelled with its branch to be deleted) but still has them.
    /// Safe to repeat; part of normal recovery, so it files no bug.
    fn recover_cleanup(self: &Arc<Self>) {
        use mason_landing::git;
        let Ok(w) = self.world() else { return };
        let done = |t: &Task| {
            t.state == State::Landed || (t.state == State::Canceled && !t.cancel_keeps_branch)
        };
        let mut stale: Vec<TaskId> = vec![];
        for (pid, p) in &self.projects {
            let refs = git::run(
                &p.path,
                &[
                    "for-each-ref",
                    "--format=%(refname:short)",
                    &format!("refs/heads/mason/{}/", p.id),
                ],
            )
            .unwrap_or_default();
            for b in refs.lines() {
                if let Some(n) = b.rsplit("/t").next().and_then(|x| x.parse::<u64>().ok()) {
                    if w.tasks
                        .get(&TaskId(n))
                        .is_some_and(|t| t.project == *pid && done(t))
                    {
                        stale.push(TaskId(n));
                    }
                }
            }
            for e in std::fs::read_dir(self.cfg.worktrees_dir().join(&p.id))
                .into_iter()
                .flatten()
                .flatten()
            {
                if let Ok(n) = e.file_name().to_string_lossy().parse::<u64>() {
                    if w.tasks
                        .get(&TaskId(n))
                        .is_some_and(|t| t.project == *pid && done(t))
                    {
                        stale.push(TaskId(n));
                    }
                }
            }
        }
        stale.sort();
        stale.dedup();
        for t in stale {
            crate::jobs::cleanup(self, t, true);
        }
    }

    pub fn task_dir(&self, task: TaskId) -> PathBuf {
        self.cfg.task_dir(task.0)
    }

    fn spawn_sweeper(self: &Arc<Self>) {
        let every = self.cfg.sweep_interval;
        crate::bg::spawn_loop(self, "sweeper", move |weak| {
            std::thread::sleep(every);
            let Some(d) = weak.upgrade() else {
                return false;
            };
            crate::sweep::sweep_once(&d);
            true
        });
    }
}

/// Clears `admitting` on every way out of `admit`.
struct AdmitReset<'a>(&'a AtomicBool);

impl Drop for AdmitReset<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// A repository's lock that remembers who holds it: the place in the code that took it and when.
/// Only for saying what git work is stuck on; it adds no way to take the lock out of turn.
#[derive(Clone, Default)]
pub struct RepoLock {
    m: Arc<Mutex<()>>,
    who: Arc<Mutex<Option<(String, u64)>>>,
}

pub struct RepoGuard<'a> {
    _g: std::sync::MutexGuard<'a, ()>,
    who: &'a Mutex<Option<(String, u64)>>,
}

impl Drop for RepoGuard<'_> {
    fn drop(&mut self) {
        *self.who.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

impl RepoLock {
    /// Take the lock, labelled with the caller's file and line.
    #[track_caller]
    pub fn lock(&self) -> std::sync::LockResult<RepoGuard<'_>> {
        let loc = std::panic::Location::caller();
        let label = format!(
            "{}:{}",
            loc.file().rsplit('/').next().unwrap_or(loc.file()),
            loc.line()
        );
        let (g, poisoned) = match self.m.lock() {
            Ok(g) => (g, false),
            Err(e) => (e.into_inner(), true),
        };
        *self.who.lock().unwrap_or_else(|e| e.into_inner()) = Some((label, now_ms()));
        let guard = RepoGuard {
            _g: g,
            who: &self.who,
        };
        if poisoned {
            Err(std::sync::PoisonError::new(guard))
        } else {
            Ok(guard)
        }
    }

    /// Take the lock only if it is free right now, labelled like `lock`.
    #[track_caller]
    pub fn try_lock(&self) -> std::sync::TryLockResult<RepoGuard<'_>> {
        use std::sync::TryLockError;
        let loc = std::panic::Location::caller();
        let label = format!(
            "{}:{}",
            loc.file().rsplit('/').next().unwrap_or(loc.file()),
            loc.line()
        );
        let (g, poisoned) = match self.m.try_lock() {
            Ok(g) => (g, false),
            Err(TryLockError::Poisoned(e)) => (e.into_inner(), true),
            Err(TryLockError::WouldBlock) => return Err(TryLockError::WouldBlock),
        };
        *self.who.lock().unwrap_or_else(|e| e.into_inner()) = Some((label, now_ms()));
        let guard = RepoGuard {
            _g: g,
            who: &self.who,
        };
        if poisoned {
            Err(TryLockError::Poisoned(std::sync::PoisonError::new(guard)))
        } else {
            Ok(guard)
        }
    }

    pub fn holder(&self) -> Option<(String, u64)> {
        self.who.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}
