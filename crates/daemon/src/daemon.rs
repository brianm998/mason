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

pub struct Daemon {
    pub cfg: DaemonConfig,
    pub store: Store,
    /// By numeric id; the project's string id is `ProjectCfg::id`.
    pub projects: BTreeMap<ProjectId, ProjectCfg>,
    /// Blocking work for the async layer (the store, git).
    pub pool: Pool,
    pub crash: Crash,
    pub started: Instant,
    pub started_ms: u64,
    jobs: Mutex<HashMap<StepRunId, TaskId>>,
    emit_lock: Mutex<()>,
    admitting: AtomicBool,
    admit_again: AtomicBool,
    repo_locks: Mutex<HashMap<ProjectId, Arc<Mutex<()>>>>,
    stalls: AtomicU64,
    worst_stall_ms: AtomicU64,
    pub(crate) sweep: Mutex<crate::sweep::SweepState>,
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
        let configured = load_projects(&cfg.config_dir).map_err(|e| e.join("\n"))?;
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
            stalls: AtomicU64::new(0),
            worst_stall_ms: AtomicU64::new(0),
            sweep: Mutex::default(),
            cfg,
        });
        d.recover();
        d.spawn_sweeper();
        Ok(d)
    }

    pub fn world(&self) -> Result<mason_engine::World, StoreError> {
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

    /// Serializes git work that touches a project's shared repository state.
    pub fn repo_lock(&self, p: ProjectId) -> Arc<Mutex<()>> {
        self.repo_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(p)
            .or_default()
            .clone()
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

    pub fn has_job(&self, run: StepRunId) -> bool {
        self.jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&run)
    }

    /// Append an event, then carry out what the reducer asked for.
    pub fn emit(self: &Arc<Self>, kind: EventKind) -> Result<Appended, StoreError> {
        let name = kind_name(&kind);
        let appended = {
            let _g = self.emit_lock.lock().unwrap_or_else(|e| e.into_inner());
            self.store.append(Event {
                at: Timestamp(now_ms()),
                kind,
            })?
        };
        self.crash.point(&format!("after_append:{name}"));
        self.handle_effects(&appended.effects);
        self.admit();
        Ok(appended)
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
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!(%task, error = %e, "a job's event was refused");
                    return false;
                }
            }
        };
        self.crash.point(&format!("after_append:{name}"));
        self.handle_effects(&appended.effects);
        self.admit();
        true
    }

    fn handle_effects(self: &Arc<Self>, effects: &[Effect]) {
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

    /// Start as many queued tasks as there are free slots, oldest first.
    /// (Ranking across projects and resources is Stage 8.)
    pub fn admit(self: &Arc<Self>) {
        if self.admitting.swap(true, Ordering::SeqCst) {
            self.admit_again.store(true, Ordering::SeqCst);
            return;
        }
        loop {
            self.admit_again.store(false, Ordering::SeqCst);
            while let Ok(w) = self.store.world() {
                let active = w.tasks.values().filter(|t| t.state.is_step()).count();
                if active >= self.cfg.max_concurrent {
                    break;
                }
                let next = w
                    .tasks
                    .values()
                    .filter(|t| {
                        t.state == State::Queued
                            && t.activity == Activity::Waiting(WaitReason::Slot)
                    })
                    .min_by_key(|t| (t.enqueued_at, t.id));
                let Some(t) = next else { break };
                if self
                    .emit(EventKind::Admitted {
                        task: t.id,
                        entry: None,
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
        self.admitting.store(false, Ordering::SeqCst);
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
        let weak = Arc::downgrade(self);
        let every = self.cfg.sweep_interval;
        let _ = std::thread::Builder::new()
            .name("sweeper".into())
            .spawn(move || loop {
                std::thread::sleep(every);
                let Some(d) = weak.upgrade() else { break };
                crate::sweep::sweep_once(&d);
            });
    }
}
