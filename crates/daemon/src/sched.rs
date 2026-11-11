// SPDX-License-Identifier: GPL-3.0-or-later

//! The broker: one place that decides who gets a contended resource, for every
//! project at once (Part 7). The decisions come from `mason-scheduler`; the
//! kernel file locks come from `mason-resources`. This module joins them to
//! the daemon: step jobs wait here for their resources, interactive callers
//! (`mason run --needs`) queue here ahead of pipeline work, and every wait can
//! say why.

use crate::cpu::CpuSource;
use crate::disk::DiskSource;
use crate::stall::StallSource;
use mason_core::TaskId;
use mason_resources::{Held, Locks, Take, TakeError};
use mason_scheduler::{
    explain, since, CpuGate, DiskGate, Kind, Ledger, Needs, Rank, StallGate, WaitCause, COMPILE,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The account of an installation with no providers.toml. Accounts become `llm:<account>` resources.
pub const DEFAULT_ACCOUNT: &str = "claude";

pub fn llm_resource(account: &str) -> String {
    format!("llm:{account}")
}

/// The name that means the whole machine, in a step's `uses`.
pub const EXCLUSIVE: &str = "exclusive";

/// `resources.toml` in the config directory: machine-wide facts.
///
/// ```toml
/// [llm]      # sessions per provider account
/// claude = 2
/// [machine]  # classes held by kernel file locks; `compile` has no count unless set here
/// compile = 4
/// [pools]    # leased by capability: simulators, phones, staging
/// simulator = 2
/// [memory]
/// floor_mb = 2048
/// [cpu]      # a compile starts only while CPU use is below this percent
/// compile_cpu_max = 50
/// compile_settle_s = 30  # and this long after the last compile started
/// compile_jobs = 3       # and never more than this many at once (0: no count)
/// [disk]     # below this much free disk, no compile and no LLM session starts
/// floor_gb = 20
/// stall_ms = 2000        # no further compile while the build disk is this slow (0: off)
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceCfg {
    pub llm: BTreeMap<String, u32>,
    pub machine: BTreeMap<String, u32>,
    pub pools: BTreeMap<String, u32>,
    pub memory_floor_mb: Option<u64>,
    /// A compile starts only while overall CPU use is below this percent.
    pub compile_cpu_max: u32,
    /// Seconds between compile starts, so each shows in the CPU reading before
    /// the next is decided. 0 starts every waiter the reading allows at once.
    pub compile_settle_s: u32,
    /// The most compiles at once when no `[machine] compile` is set (0: no
    /// count). A test run is a compile that then spends most of its time on
    /// disk and process launches, which CPU use does not show.
    pub compile_jobs: u32,
    /// Free space, in GB, below which nothing new starts (0: no floor).
    pub disk_floor_gb: u64,
    /// How slow an answer from the build disk, in ms, holds further compiles (0: never).
    pub disk_stall_ms: u64,
}

/// The default `compile_cpu_max`: half the machine.
pub const DEFAULT_COMPILE_CPU_MAX: u32 = 50;

/// The default `compile_settle_s`: twice the time the CPU reading averages over.
pub const DEFAULT_COMPILE_SETTLE_S: u32 = 30;

/// The default `compile_jobs`.
pub const DEFAULT_COMPILE_JOBS: u32 = 3;

/// The default `disk_stall_ms`. Healthy disks answered in under 250 ms even with
/// the machine loaded; the stalls this exists for lasted 20 seconds and more.
pub const DEFAULT_DISK_STALL_MS: u64 = 2000;

impl Default for ResourceCfg {
    fn default() -> Self {
        ResourceCfg {
            llm: BTreeMap::new(),
            machine: BTreeMap::new(),
            pools: BTreeMap::new(),
            memory_floor_mb: None,
            compile_cpu_max: DEFAULT_COMPILE_CPU_MAX,
            compile_settle_s: DEFAULT_COMPILE_SETTLE_S,
            compile_jobs: DEFAULT_COMPILE_JOBS,
            disk_floor_gb: crate::disk::DEFAULT_FLOOR_GB,
            disk_stall_ms: DEFAULT_DISK_STALL_MS,
        }
    }
}

pub fn parse_resource_cfg(text: &str) -> Result<ResourceCfg, String> {
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("resources.toml: {e}"))?;
    let mut c = ResourceCfg::default();
    for (key, item) in doc.iter() {
        let table = item
            .as_table_like()
            .ok_or_else(|| format!("resources.toml: `{key}` must be a table"))?;
        for (name, v) in table.iter() {
            let n = v
                .as_integer()
                .filter(|n| *n >= 0)
                .ok_or_else(|| format!("resources.toml: {key}.{name} must be a whole number"))?;
            match key {
                "llm" => drop(c.llm.insert(name.to_string(), n as u32)),
                "machine" => drop(c.machine.insert(name.to_string(), n as u32)),
                "pools" => drop(c.pools.insert(name.to_string(), n as u32)),
                "memory" if name == "floor_mb" => c.memory_floor_mb = Some(n as u64),
                "cpu" if name == "compile_cpu_max" => {
                    if !(1..=100).contains(&n) {
                        return Err(
                            "resources.toml: cpu.compile_cpu_max must be from 1 to 100".into()
                        );
                    }
                    c.compile_cpu_max = n as u32;
                }
                "cpu" if name == "compile_settle_s" => c.compile_settle_s = n as u32,
                "cpu" if name == "compile_jobs" => c.compile_jobs = n as u32,
                "disk" if name == "floor_gb" => c.disk_floor_gb = n as u64,
                "disk" if name == "stall_ms" => c.disk_stall_ms = n as u64,
                other => return Err(format!("resources.toml: unknown setting {other}.{name}")),
            }
        }
    }
    Ok(c)
}

pub fn load_resource_cfg(config_dir: &Path) -> Result<ResourceCfg, String> {
    match std::fs::read_to_string(config_dir.join("resources.toml")) {
        Ok(t) => parse_resource_cfg(&t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ResourceCfg::default()),
        Err(e) => Err(format!("resources.toml: {e}")),
    }
}

/// Turn a step's `uses` (and its agent, if any) into what the scheduler needs.
pub fn needs_of(uses: &BTreeMap<String, u32>, account: Option<&str>) -> Needs {
    let mut n = Needs::default();
    for (name, count) in uses {
        if name == EXCLUSIVE {
            n.exclusive = *count > 0;
        } else if *count > 0 {
            n.amounts.insert(name.clone(), *count);
        }
    }
    if let Some(a) = account {
        n.amounts.insert(llm_resource(a), 1);
    }
    n
}

#[derive(Debug, PartialEq, Eq)]
pub struct Cancelled;

enum Meta {
    Task,
    External {
        opened: Instant,
        last_poll: Instant,
        granted_at: Option<Instant>,
        /// Seen holding its marker lock at least once.
        locked: bool,
    },
    /// A slot of a machine class the kernel says is held, by a process the
    /// broker has no lease for: one that took it before this daemon started
    /// (a `mason run` that outlived the daemon's restart, say). It counts
    /// against the class until the kernel says the slot is free.
    Outside {
        class: String,
    },
}

struct State {
    ledger: Ledger,
    meta: HashMap<u64, Meta>,
    by_task: HashMap<TaskId, u64>,
    /// Resources whose limit was set by the owner (config or live) and so are
    /// not changed by a definition that happens to declare the same name.
    fixed: HashSet<String>,
    pools: HashSet<String>,
    floor_mb: Option<u64>,
    mem: Option<(Instant, Option<u64>)>,
    rank_seq: u64,
    cpu_max: u32,
    cpu: Arc<dyn CpuSource>,
    /// The latest reading, kept so the explanation matches the decision.
    cpu_busy: Option<u32>,
    /// CPU use decides when compiles start, and no count of them can be set.
    compile_by_cpu: bool,
    disk: Option<DiskCfg>,
    /// The latest reading: when, and the gate if the source could be read.
    disk_read: Option<(Instant, Option<DiskGate>)>,
    /// Grants waiting on a kernel lock that something outside the broker holds, by lease.
    kernel_waits: HashMap<u64, KernelWait>,
    /// The build disk's responsiveness and the limit on it. `None`: no stall gate.
    stall: Option<(Arc<dyn StallSource>, u64)>,
}

struct KernelWait {
    /// What it waits for: the machine classes, or "the whole machine".
    resource: String,
    since_ms: u64,
    text: String,
}

/// A wait the owner should be told about, worked out from the broker as it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaitAlert {
    pub resource: String,
    /// The request can never fit; the alert is `never-fits:<resource>`. Otherwise
    /// it is `long-wait:<resource>`.
    pub never: bool,
    pub waiting: usize,
    pub idle_ms: u64,
    pub text: String,
}

/// After this long on a kernel lock the task's wait says what it waits for.
const KERNEL_WAIT_SAYS_AFTER: Duration = Duration::from_secs(60);

struct DiskCfg {
    source: Arc<dyn DiskSource>,
    /// The data directory and each project's `task_cache` directory.
    paths: Vec<PathBuf>,
    floor_mb: u64,
    every: Duration,
}

pub struct Broker {
    st: Mutex<State>,
    cv: Condvar,
    locks: Locks,
    limits_file: PathBuf,
    /// How long a grant waits for its kernel locks before the task's wait says so.
    kernel_says_after: Mutex<Duration>,
}

/// What a granted resource set looks like to the holder. Dropping it releases
/// the kernel locks and the lease, and wakes the next waiter.
pub struct Lease {
    b: Arc<Broker>,
    id: u64,
    task: Option<TaskId>,
    held: Option<Held>,
}

impl Lease {
    pub fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        drop(self.held.take());
        self.b.release(self.id, self.task);
    }
}

/// Where a task stands in the pipeline, for ranking a `mason run` made on its
/// behalf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Standing {
    pub steps_remaining: u32,
    pub priority: u8,
}

/// What an interactive caller sees of its request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeaseState {
    Waiting {
        explain: String,
    },
    /// Granted. The caller takes the kernel locks itself (so they die with it).
    Granted {
        classes: BTreeMap<String, u32>,
        exclusive: bool,
        capacity: BTreeMap<String, u32>,
        locks_dir: PathBuf,
    },
    Gone,
}

fn lock<'a>(m: &'a Mutex<State>) -> MutexGuard<'a, State> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Broker {
    pub fn new(
        locks_dir: PathBuf,
        limits_file: PathBuf,
        cfg: &ResourceCfg,
        default_llm: u32,
        cpu: Arc<dyn CpuSource>,
        accounts: &[crate::providers::Account],
    ) -> Result<Arc<Broker>, String> {
        let locks = Locks::new(&locks_dir).map_err(|e| format!("cannot create locks dir: {e}"))?;
        let mut ledger = Ledger::new();
        let mut fixed = HashSet::new();
        let mut pools = HashSet::new();
        // One capacity per account; `[llm]` below sets a name as it always did.
        for a in accounts {
            ledger.set_capacity(
                &llm_resource(&a.name),
                a.sessions.unwrap_or(default_llm),
                Kind::Llm,
            );
        }
        for (a, n) in &cfg.llm {
            ledger.set_capacity(&llm_resource(a), *n, Kind::Llm);
            fixed.insert(llm_resource(a));
        }
        for (c, n) in &cfg.machine {
            ledger.set_capacity(c, *n, Kind::Machine);
            fixed.insert(c.clone());
        }
        for (p, n) in &cfg.pools {
            ledger.set_capacity(p, *n, Kind::Pool);
            fixed.insert(p.clone());
            pools.insert(p.clone());
        }
        // Reading the machine replaces a count set per project or saved live, but
        // CPU use alone lets too many in: a test run is mostly disk and process
        // launches, and everything blocked on a stalled disk reads as idle. So
        // `compile_jobs` caps them. 0 is one slot per hardware thread, which never
        // holds a compile back, since the CPU gate stops them long before that.
        let compile_by_cpu = cpu.replaces_compile_count() && !cfg.machine.contains_key(COMPILE);
        if compile_by_cpu {
            let units = match cfg.compile_jobs {
                0 => std::thread::available_parallelism().map_or(8, |n| n.get()) as u32,
                n => n,
            };
            ledger.set_capacity(COMPILE, units, Kind::Machine);
            if cfg.compile_settle_s > 0 {
                ledger.pace_compiles(u64::from(cfg.compile_settle_s) * 1000);
            }
        }
        let b = Broker {
            st: Mutex::new(State {
                ledger,
                meta: HashMap::new(),
                by_task: HashMap::new(),
                fixed,
                pools,
                floor_mb: cfg.memory_floor_mb,
                mem: None,
                rank_seq: 0,
                cpu_max: cfg.compile_cpu_max,
                cpu,
                cpu_busy: None,
                compile_by_cpu,
                disk: None,
                disk_read: None,
                kernel_waits: HashMap::new(),
                stall: None,
            }),
            cv: Condvar::new(),
            locks,
            limits_file,
            kernel_says_after: Mutex::new(KERNEL_WAIT_SAYS_AFTER),
        };
        // Limits changed live survive a restart.
        if let Ok(text) = std::fs::read_to_string(&b.limits_file) {
            for line in text.lines() {
                if let Some((name, n)) = line.split_once('\t') {
                    if let Ok(n) = n.trim().parse::<u32>() {
                        let mut st = lock(&b.st);
                        // A count saved before CPU use decided this.
                        if name == COMPILE && st.compile_by_cpu {
                            continue;
                        }
                        let kind = kind_for(&st, name);
                        st.ledger.set_capacity(name, n, kind);
                        st.fixed.insert(name.to_string());
                    }
                }
            }
        }
        Ok(Arc::new(b))
    }

    pub fn locks(&self) -> &Locks {
        &self.locks
    }

    /// A definition names a resource it uses: make sure it exists. The first
    /// declaration wins, and an owner's limit is never overridden.
    pub fn declare(&self, name: &str, units: u32) {
        let mut st = lock(&self.st);
        if st.fixed.contains(name) || st.ledger.declared(name) {
            return;
        }
        let kind = kind_for(&st, name);
        st.ledger.set_capacity(name, units, kind);
    }

    /// A device inventory reading sets how many units a device pool has. Unlike
    /// `set_limit` it is not the owner's word: nothing is written to
    /// limits.txt, and an owner-fixed limit wins. Lowering takes nothing from a
    /// holder. Returns whether anything changed.
    pub fn set_pool_units(&self, name: &str, units: u32) -> bool {
        let mut st = lock(&self.st);
        if st.fixed.contains(name) {
            return false;
        }
        if st.ledger.capacity(name).map(|c| c.units) == Some(units) {
            return false;
        }
        st.ledger.set_capacity(name, units, Kind::Pool);
        st.pools.insert(name.to_string());
        self.pass(&mut st);
        drop(st);
        self.cv.notify_all();
        true
    }

    /// Change a limit while things run. Lowering it takes nothing away: it only
    /// stops new grants until the count falls. Survives a restart.
    pub fn set_limit(&self, name: &str, units: u32) -> Result<(), String> {
        let mut st = lock(&self.st);
        if name == EXCLUSIVE {
            return Err("`exclusive` is the whole machine and has no limit".into());
        }
        if name == COMPILE && st.compile_by_cpu {
            return Err(
                "`compile` has no live count: at most cpu.compile_jobs run at once, each \
                 starting while CPU use is below cpu.compile_cpu_max in resources.toml"
                    .into(),
            );
        }
        let kind = kind_for(&st, name);
        st.ledger.set_capacity(name, units, kind);
        st.fixed.insert(name.to_string());
        let mut lines: BTreeMap<String, u32> = std::fs::read_to_string(&self.limits_file)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let (n, u) = l.split_once('\t')?;
                Some((n.to_string(), u.trim().parse().ok()?))
            })
            .collect();
        lines.insert(name.to_string(), units);
        let text: String = lines.iter().map(|(n, u)| format!("{n}\t{u}\n")).collect();
        let _ = std::fs::write(&self.limits_file, text);
        self.pass(&mut st);
        drop(st);
        self.cv.notify_all();
        Ok(())
    }

    /// Turn the disk floor on: free space is read from `source` on the volume of each of
    /// `paths`, at most every `every`. Without this there is no disk gate.
    pub fn set_disk(
        &self,
        source: Arc<dyn DiskSource>,
        paths: Vec<PathBuf>,
        floor_gb: u64,
        every: Duration,
    ) {
        let mut st = lock(&self.st);
        st.disk = Some(DiskCfg {
            source,
            paths,
            floor_mb: floor_gb * 1024,
            every,
        });
        st.disk_read = None;
        self.pass(&mut st);
        drop(st);
        self.cv.notify_all();
    }

    /// The disk reading, at most once per `every`, handed to the ledger so the plan sees it.
    fn refresh_disk(&self, st: &mut State) -> Option<DiskGate> {
        let cfg = st.disk.as_ref()?;
        if let Some((at, g)) = st.disk_read {
            if at.elapsed() < cfg.every {
                return g;
            }
        }
        let g = crate::disk::least_free_mb(&*cfg.source, &cfg.paths).map(|free_mb| DiskGate {
            free_mb,
            floor_mb: cfg.floor_mb,
        });
        st.disk_read = Some((Instant::now(), g));
        st.ledger.set_disk(g);
        g
    }

    /// The disk gate if free space is below the floor right now.
    pub fn disk_low(&self) -> Option<DiskGate> {
        let mut st = lock(&self.st);
        self.refresh_disk(&mut st)
            .filter(|g| g.free_mb < g.floor_mb)
    }

    /// How many requests the disk gate is holding right now (zero while disk is fine).
    pub fn disk_held(&self) -> usize {
        let mut st = lock(&self.st);
        if self.refresh_disk(&mut st).is_none() {
            return 0;
        }
        let (floor, free) = (st.floor_mb, self.free_memory(&mut st));
        let gate = self.cpu_gate(&mut st);
        st.ledger
            .waits_gated(crate::daemon::now_ms(), free, floor, gate)
            .waits
            .values()
            .filter(|w| matches!(w.cause, WaitCause::Disk { .. }))
            .count()
    }

    /// Turn the stall gate on: no further compile while `source` reads at or above `max_ms`.
    pub fn set_stall(&self, source: Arc<dyn StallSource>, max_ms: u64) {
        let mut st = lock(&self.st);
        st.stall = Some((source, max_ms));
        self.pass(&mut st);
        drop(st);
        self.cv.notify_all();
    }

    /// Read the CPU and stall sources afresh. Every plan is made right after
    /// this, so the ledger's stall reading is always current.
    fn cpu_gate(&self, st: &mut State) -> Option<CpuGate> {
        let stall = st.stall.as_ref().and_then(|(source, max_ms)| {
            source.slow_ms().map(|slow_ms| StallGate {
                slow_ms,
                max_ms: *max_ms,
            })
        });
        st.ledger.set_stall(stall);
        st.cpu_busy = st.cpu.busy_pct();
        st.cpu_busy.map(|busy_pct| CpuGate {
            busy_pct,
            max_pct: st.cpu_max,
        })
    }

    fn free_memory(&self, st: &mut State) -> Option<u64> {
        st.floor_mb?;
        if let Some((at, v)) = st.mem {
            if at.elapsed() < Duration::from_secs(1) {
                return v;
            }
        }
        let v = measure_free_memory_mb();
        st.mem = Some((Instant::now(), v));
        v
    }

    /// Reap holders that died, then grant whatever can be granted. Called by
    /// everything that waits, and after everything that releases, so the next
    /// waiter starts at once.
    fn pass(&self, st: &mut State) {
        let now = crate::daemon::now_ms();
        self.refresh_disk(st);
        let mut changed = false;
        let ids: Vec<u64> = st.meta.keys().copied().collect();
        for id in ids {
            let Some(Meta::External {
                opened,
                last_poll,
                granted_at,
                locked,
            }) = st.meta.get_mut(&id)
            else {
                continue;
            };
            let dead = if st.ledger.holds(id) {
                let alive = self.locks.alive(id);
                if alive {
                    *locked = true;
                    false
                } else {
                    // Not yet locked is a grace period; locked once and now
                    // free is the kernel saying the holder is gone.
                    *locked || granted_at.is_none_or(|g| g.elapsed() > Duration::from_secs(5))
                }
            } else {
                last_poll.elapsed() > Duration::from_secs(10) && opened.elapsed() > Duration::ZERO
            };
            if dead {
                st.ledger.release(id);
                st.meta.remove(&id);
                changed = true;
            }
        }
        changed |= self.count_outside_holders(st, now);
        let (floor, free) = (st.floor_mb, self.free_memory(st));
        let gate = self.cpu_gate(st);
        let p = st.ledger.schedule_gated(now, free, floor, gate);
        for g in &p.grants {
            if let Some(Meta::External { granted_at, .. }) = st.meta.get_mut(g) {
                *granted_at = Some(Instant::now());
            }
        }
        if changed || !p.grants.is_empty() {
            self.cv.notify_all();
        }
    }

    /// Make the ledger count every machine slot the kernel says is held. A slot
    /// held by a process the broker has no lease for (it took it before this
    /// daemon started) would otherwise look free: the broker granted it, and the
    /// grantee could not take the lock ("a slot is held by another process"),
    /// again and again until the holder ended. Each such slot is adopted as an
    /// outside holder, and released once the kernel says the slot is free.
    /// Only leases holding their locks are compared with the kernel, so a grant
    /// whose holder has not taken them yet never hides an outside holder.
    fn count_outside_holders(&self, st: &mut State, now: u64) -> bool {
        let snap = st.ledger.snapshot();
        let mut changed = false;
        for (class, cap) in snap
            .capacity
            .iter()
            .filter(|(_, c)| c.kind == Kind::Machine)
        {
            let slots = self.locks.slots(class, cap.units);
            let kernel = slots.iter().filter(|s| s.is_some()).count() as u32;
            let mut outside: Vec<u64> = st
                .meta
                .iter()
                .filter(|(_, m)| matches!(m, Meta::Outside { class: c } if c == class))
                .map(|(id, _)| *id)
                .collect();
            outside.sort_unstable();
            // Only leases that hold their locks: a grant that has not taken them
            // yet is not one of the slots the kernel counts.
            let ours: u32 = snap
                .holders
                .iter()
                .filter(|h| !outside.contains(&h.id) && self.locks.alive(h.id))
                .map(|h| {
                    if h.needs.exclusive {
                        cap.units
                    } else {
                        h.needs.amounts.get(class).copied().unwrap_or(0)
                    }
                })
                .sum();
            let want = kernel.saturating_sub(ours) as usize;
            while outside.len() > want {
                let id = outside.pop().expect("not empty");
                st.ledger.release(id);
                st.meta.remove(&id);
                changed = true;
            }
            let mut labels = slots.iter().flatten().map(|h| h.label.clone());
            while outside.len() < want {
                let what = labels.next().unwrap_or_else(|| "unknown".into());
                let needs = Needs {
                    exclusive: false,
                    amounts: BTreeMap::from([(class.clone(), 1)]),
                };
                let id = st
                    .ledger
                    .adopt(needs, &format!("{what} (held outside the queue)"), now);
                st.meta.insert(
                    id,
                    Meta::Outside {
                        class: class.clone(),
                    },
                );
                outside.push(id);
                changed = true;
            }
        }
        changed
    }

    /// Wait for a step's resources, all at once. `alive` says whether the step
    /// is still wanted (a cancelled task stops waiting). `on_wait` is called
    /// once, with the explanation, if the step has to wait at all. Time spent
    /// here is outside every step timeout, which only start afterwards.
    pub fn acquire(
        self: &Arc<Self>,
        task: TaskId,
        label: &str,
        rank: Rank,
        needs: Needs,
        alive: &dyn Fn() -> bool,
        on_wait: &mut dyn FnMut(&str),
    ) -> Result<Lease, Cancelled> {
        let id = {
            let mut st = lock(&self.st);
            let id = st
                .ledger
                .submit(rank, needs.clone(), label, crate::daemon::now_ms());
            st.meta.insert(id, Meta::Task);
            st.by_task.insert(task, id);
            self.pass(&mut st);
            id
        };
        let mut announced = false;
        let mut last_check = Instant::now();
        loop {
            // The check and the wait share one hold of the lock, so a release
            // that happens in between cannot be missed (it needs the lock).
            let mut st = lock(&self.st);
            self.pass(&mut st);
            if st.ledger.holds(id) {
                break;
            }
            if !announced {
                let (floor, free) = (st.floor_mb, self.free_memory(&mut st));
                let now = crate::daemon::now_ms();
                let gate = self.cpu_gate(&mut st);
                let waits = st.ledger.waits_gated(now, free, floor, gate);
                let text = waits
                    .waits
                    .get(&id)
                    .map(|w| explain(w, now))
                    .unwrap_or_else(|| "Waiting for resources".into());
                drop(st);
                announced = true;
                on_wait(&text);
                continue;
            }
            if last_check.elapsed() > Duration::from_millis(400) {
                drop(st);
                last_check = Instant::now();
                if !alive() {
                    self.release(id, Some(task));
                    return Err(Cancelled);
                }
                continue;
            }
            let _ = self.cv.wait_timeout(st, Duration::from_millis(100));
        }
        // The grant is accounting; the kernel locks make it real.
        let (take, caps) = {
            let st = lock(&self.st);
            let snap = st.ledger.snapshot();
            let machine = |r: &String| {
                snap.capacity
                    .get(r)
                    .is_some_and(|c| c.kind == Kind::Machine)
            };
            (
                Take {
                    classes: needs
                        .amounts
                        .iter()
                        .filter(|(r, _)| machine(r))
                        .map(|(r, n)| (r.clone(), *n))
                        .collect(),
                    exclusive: needs.exclusive,
                },
                snap.capacity
                    .iter()
                    .filter(|(_, c)| c.kind == Kind::Machine)
                    .map(|(r, c)| (r.clone(), c.units))
                    .collect::<BTreeMap<_, _>>(),
            )
        };
        let began = Instant::now();
        let began_ms = crate::daemon::now_ms();
        let says_after = *self
            .kernel_says_after
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut said = false;
        let mut last_io: Option<String> = None;
        let held = loop {
            match self
                .locks
                .take(id, label, crate::daemon::now_ms(), &take, &caps)
            {
                Ok(h) => break h,
                Err(TakeError::Busy) => {}
                Err(TakeError::Io(e)) => {
                    // Once per distinct error, not every 25 ms.
                    let m = e.to_string();
                    if last_io.as_deref() != Some(m.as_str()) {
                        tracing::warn!(error = %m, "taking resource locks failed; retrying");
                        last_io = Some(m);
                    }
                }
            }
            if !said && began.elapsed() >= says_after {
                said = true;
                let (resource, text) = self.kernel_wait_text(&take, &caps, last_io.as_deref());
                lock(&self.st).kernel_waits.insert(
                    id,
                    KernelWait {
                        resource,
                        since_ms: began_ms,
                        text: text.clone(),
                    },
                );
                on_wait(&text);
            }
            std::thread::sleep(Duration::from_millis(25));
            if !alive() {
                lock(&self.st).kernel_waits.remove(&id);
                self.release(id, Some(task));
                return Err(Cancelled);
            }
        };
        lock(&self.st).kernel_waits.remove(&id);
        Ok(Lease {
            b: self.clone(),
            id,
            task: Some(task),
            held: Some(held),
        })
    }

    /// What a grant that cannot take its kernel locks is waiting for, and a name for it.
    fn kernel_wait_text(
        &self,
        take: &Take,
        caps: &BTreeMap<String, u32>,
        io_error: Option<&str>,
    ) -> (String, String) {
        let names: Vec<&str> = take.classes.keys().map(String::as_str).collect();
        let resource = if take.exclusive || names.is_empty() {
            "the whole machine".to_string()
        } else {
            names.join(", ")
        };
        let mut labels: Vec<String> = take
            .classes
            .keys()
            .flat_map(|c| self.locks.slots(c, caps.get(c).copied().unwrap_or(0)))
            .flatten()
            .map(|h| h.label)
            .collect();
        labels.sort();
        labels.dedup();
        let mut text = format!(
            "Granted, but waiting for the kernel lock on {resource}, which something outside the queue holds"
        );
        if !labels.is_empty() {
            text.push_str(&format!(" ({})", labels.join(", ")));
        }
        if let Some(e) = io_error {
            text.push_str(&format!("; taking it also failed: {e}"));
        }
        (resource, text)
    }

    /// Waits to tell the owner about: requests that can never fit (always), and waits on a
    /// resource that has granted nothing for `long_after_ms` (0: none of these), including
    /// grants stuck on a kernel lock. One per resource and cause. This only observes.
    pub fn wait_alerts(&self, long_after_ms: u64) -> Vec<WaitAlert> {
        let mut st = lock(&self.st);
        self.refresh_disk(&mut st);
        let (floor, free) = (st.floor_mb, self.free_memory(&mut st));
        let now = crate::daemon::now_ms();
        let gate = self.cpu_gate(&mut st);
        let mut out: Vec<WaitAlert> = st
            .ledger
            .stalls(now, free, floor, gate, long_after_ms)
            .into_iter()
            .map(|s| {
                let text = match &s.cause {
                    WaitCause::TooBig {
                        resource,
                        need,
                        limit,
                    } => format!(
                        "{} needs {need} of {resource} but its limit is {limit}, so it can never run. \
                         Raise it: mason resources limit {resource} {need}",
                        s.label
                    ),
                    WaitCause::Unknown { resource } => format!(
                        "{} needs `{resource}`, which nothing declares, so it can never run. \
                         Declare it with: mason resources limit {resource} <units>",
                        s.label
                    ),
                    _ => format!(
                        "{} waiting on {}, nothing granted on it for {}: {}",
                        s.waiting,
                        s.resource,
                        since(now, now.saturating_sub(s.idle_ms)),
                        s.text
                    ),
                };
                let text = if s.never && s.waiting > 1 {
                    format!("{text} (and {} more)", s.waiting - 1)
                } else {
                    text
                };
                WaitAlert {
                    resource: s.resource,
                    never: s.never,
                    waiting: s.waiting,
                    idle_ms: s.idle_ms,
                    text,
                }
            })
            .collect();
        if long_after_ms > 0 {
            let mut kernel: BTreeMap<String, WaitAlert> = BTreeMap::new();
            for k in st.kernel_waits.values() {
                let idle_ms = now.saturating_sub(k.since_ms);
                if idle_ms < long_after_ms {
                    continue;
                }
                let e = kernel.entry(k.resource.clone()).or_insert(WaitAlert {
                    resource: format!("kernel-lock:{}", k.resource),
                    never: false,
                    waiting: 0,
                    idle_ms: 0,
                    text: String::new(),
                });
                e.waiting += 1;
                e.idle_ms = e.idle_ms.max(idle_ms);
                if e.text.is_empty() {
                    e.text = k.text.clone();
                }
            }
            for mut e in kernel.into_values() {
                e.text = format!(
                    "{} waiting on {}, for {}: {}",
                    e.waiting,
                    e.resource,
                    since(now, now.saturating_sub(e.idle_ms)),
                    e.text
                );
                out.push(e);
            }
        }
        out
    }

    /// Test seam: how long a grant waits for kernel locks before it says so.
    pub fn set_kernel_says_after(&self, after: Duration) {
        *self
            .kernel_says_after
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = after;
    }

    fn release(&self, id: u64, task: Option<TaskId>) {
        let mut st = lock(&self.st);
        st.ledger.release(id);
        st.meta.remove(&id);
        if let Some(t) = task {
            if st.by_task.get(&t) == Some(&id) {
                st.by_task.remove(&t);
            }
        }
        self.pass(&mut st);
        drop(st);
        self.cv.notify_all();
    }

    /// Does this task have a lease, waiting or held?
    pub fn has_lease(&self, task: TaskId) -> bool {
        lock(&self.st).by_task.contains_key(&task)
    }

    /// Does this task hold (granted, not waiting) a lease that uses an
    /// `llm:` resource, i.e. is a model session running for it right now?
    pub fn llm_held_by(&self, task: TaskId) -> bool {
        let st = lock(&self.st);
        let Some(id) = st.by_task.get(&task) else {
            return false;
        };
        st.ledger.holder(*id).is_some_and(|h| {
            h.needs
                .amounts
                .iter()
                .any(|(k, n)| *n > 0 && k.starts_with("llm:"))
        })
    }

    /// Units of a resource held right now.
    pub fn used(&self, name: &str) -> u32 {
        lock(&self.st)
            .ledger
            .snapshot()
            .used
            .get(name)
            .copied()
            .unwrap_or(0)
    }

    pub fn limit(&self, name: &str) -> Option<u32> {
        lock(&self.st).ledger.capacity(name).map(|c| c.units)
    }

    /// Why a task's step is waiting, live, or `None` if it is not waiting.
    pub fn explain_task(&self, task: TaskId) -> Option<String> {
        let mut st = lock(&self.st);
        let id = *st.by_task.get(&task)?;
        if !st.ledger.is_waiting(id) {
            return None;
        }
        self.refresh_disk(&mut st);
        let (floor, free) = (st.floor_mb, self.free_memory(&mut st));
        let now = crate::daemon::now_ms();
        let gate = self.cpu_gate(&mut st);
        let waits = st.ledger.waits_gated(now, free, floor, gate);
        waits.waits.get(&id).map(|w| explain(w, now))
    }

    // ---- interactive callers: `mason run --needs` ----

    /// Queue an interactive request. It outranks pipeline work.
    pub fn lease_open(&self, label: &str, uses: &BTreeMap<String, u32>) -> u64 {
        self.lease_open_as(label, uses, None)
    }

    /// Queue a request from `mason run`. With no `standing` it is a person's
    /// and outranks pipeline work. With one, it was made on behalf of a task
    /// and is ranked as that task's pipeline work: the task's steps remaining
    /// and priority, but *this request's* arrival time and sequence number
    /// (never the task's own enqueue time, so a caller cannot borrow an old
    /// task's place in line) and never the project's priority jump. A standing
    /// can therefore only lower a request from the interactive rank, never
    /// raise it above what omitting the task would give.
    pub fn lease_open_as(
        &self,
        label: &str,
        uses: &BTreeMap<String, u32>,
        standing: Option<Standing>,
    ) -> u64 {
        let mut st = lock(&self.st);
        let now = crate::daemon::now_ms();
        st.rank_seq += 1;
        let rank = match standing {
            None => Rank::new(true, 0, 0, now, st.rank_seq, false),
            Some(s) => Rank::new(
                false,
                s.steps_remaining,
                s.priority,
                now,
                st.rank_seq,
                false,
            ),
        };
        let needs = needs_of(uses, None);
        for r in needs.amounts.keys() {
            if !st.ledger.declared(r) {
                // An interactive caller may name a class nobody declared yet;
                // give it one slot rather than make it wait forever.
                let kind = kind_for(&st, r);
                st.ledger.set_capacity(r, 1, kind);
            }
        }
        let id = st.ledger.submit(rank, needs, label, now);
        st.meta.insert(
            id,
            Meta::External {
                opened: Instant::now(),
                last_poll: Instant::now(),
                granted_at: None,
                locked: false,
            },
        );
        self.pass(&mut st);
        id
    }

    /// Where an interactive request stands. Polling is its heartbeat.
    pub fn lease_poll(&self, id: u64) -> LeaseState {
        let mut st = lock(&self.st);
        if let Some(Meta::External { last_poll, .. }) = st.meta.get_mut(&id) {
            *last_poll = Instant::now();
        }
        self.pass(&mut st);
        if st.ledger.holds(id) {
            let snap = st.ledger.snapshot();
            let h = snap.holders.iter().find(|h| h.id == id).expect("holds");
            let machine = |r: &String| {
                snap.capacity
                    .get(r)
                    .is_some_and(|c| c.kind == Kind::Machine)
            };
            return LeaseState::Granted {
                classes: h
                    .needs
                    .amounts
                    .iter()
                    .filter(|(r, _)| machine(r))
                    .map(|(r, n)| (r.clone(), *n))
                    .collect(),
                exclusive: h.needs.exclusive,
                capacity: snap
                    .capacity
                    .iter()
                    .filter(|(_, c)| c.kind == Kind::Machine)
                    .map(|(r, c)| (r.clone(), c.units))
                    .collect(),
                locks_dir: self.locks.dir().to_path_buf(),
            };
        }
        if !st.ledger.is_waiting(id) {
            return LeaseState::Gone;
        }
        let (floor, free) = (st.floor_mb, self.free_memory(&mut st));
        let now = crate::daemon::now_ms();
        let gate = self.cpu_gate(&mut st);
        let waits = st.ledger.waits_gated(now, free, floor, gate);
        LeaseState::Waiting {
            explain: waits
                .waits
                .get(&id)
                .map(|w| explain(w, now))
                .unwrap_or_else(|| "Waiting".into()),
        }
    }

    pub fn lease_close(&self, id: u64) -> bool {
        let mut st = lock(&self.st);
        let had = st.ledger.release(id);
        st.meta.remove(&id);
        self.pass(&mut st);
        drop(st);
        self.cv.notify_all();
        had
    }

    /// Everything, for `mason resources` and the monitors.
    pub fn snapshot(&self) -> serde_json::Value {
        use serde_json::json;
        let mut st = lock(&self.st);
        self.pass(&mut st);
        let (floor, free) = (st.floor_mb, self.free_memory(&mut st));
        let now = crate::daemon::now_ms();
        let gate = self.cpu_gate(&mut st);
        let waits = st.ledger.waits_gated(now, free, floor, gate);
        let snap = st.ledger.snapshot();
        let kind = |k: Kind| match k {
            Kind::Llm => "llm",
            Kind::Machine => "machine",
            Kind::Pool => "pool",
        };
        let resources: Vec<_> = snap
            .capacity
            .iter()
            .map(|(name, c)| {
                let slots = if c.kind == Kind::Machine {
                    self.locks
                        .slots(name, c.units)
                        .into_iter()
                        .map(|s| match s {
                            Some(h) => json!({"held_by": h.label, "since_ms": h.since_ms}),
                            None => json!(null),
                        })
                        .collect::<Vec<_>>()
                } else {
                    vec![]
                };
                json!({
                    "name": name,
                    "kind": kind(c.kind),
                    "limit": c.units,
                    "used": snap.used.get(name).copied().unwrap_or(0),
                    "slots": slots,
                })
            })
            .collect();
        let holders: Vec<_> = snap
            .holders
            .iter()
            .map(|h| {
                json!({
                    "label": h.label,
                    "since_ms": h.since,
                    "uses": h.needs.amounts,
                    "exclusive": h.needs.exclusive,
                })
            })
            .collect();
        let waiting: Vec<_> = snap
            .waiting
            .iter()
            .map(|r| {
                json!({
                    "label": r.label,
                    "since_ms": r.since,
                    "interactive": r.rank.interactive(),
                    "steps_remaining": r.rank.steps(),
                    "uses": r.needs.amounts,
                    "exclusive": r.needs.exclusive,
                    "explain": waits.waits.get(&r.id).map(|w| explain(w, now)),
                })
            })
            .collect();
        json!({
            "resources": resources,
            "holders": holders,
            "waiting": waiting,
            "memory": { "floor_mb": st.floor_mb, "free_mb": free },
            "cpu": { "busy_pct": st.cpu_busy, "compile_max_pct": st.cpu_max },
            "now_ms": now,
        })
    }
}

fn kind_for(st: &State, name: &str) -> Kind {
    if name.starts_with("llm:") {
        Kind::Llm
    } else if st.pools.contains(name) {
        Kind::Pool
    } else {
        Kind::Machine
    }
}

/// Free memory in MB, or `None` if it cannot be read. Never load average.
pub fn measure_free_memory_mb() -> Option<u64> {
    if let Ok(t) = std::fs::read_to_string("/proc/meminfo") {
        let kb: u64 = t
            .lines()
            .find(|l| l.starts_with("MemAvailable:"))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()?;
        return Some(kb / 1024);
    }
    let out = std::process::Command::new("vm_stat").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let page: u64 = text
        .lines()
        .next()?
        .split("page size of ")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let pages = |key: &str| -> u64 {
        text.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|v| v.trim().trim_end_matches('.').parse().ok())
            .unwrap_or(0)
    };
    let free = pages("Pages free") + pages("Pages inactive") + pages("Pages speculative");
    Some(free * page / (1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_config_parses_and_refuses_nonsense() {
        let c = parse_resource_cfg(
            "[llm]\nclaude = 2\n[machine]\ncompile = 4\n[pools]\nsimulator = 1\n[memory]\nfloor_mb = 512\n",
        )
        .unwrap();
        assert_eq!(c.llm["claude"], 2);
        assert_eq!(c.machine["compile"], 4);
        assert_eq!(c.pools["simulator"], 1);
        assert_eq!(c.memory_floor_mb, Some(512));
        assert_eq!(c.compile_cpu_max, 50);
        assert_eq!(
            parse_resource_cfg("[cpu]\ncompile_cpu_max = 80\n")
                .unwrap()
                .compile_cpu_max,
            80
        );
        assert!(parse_resource_cfg("[cpu]\ncompile_cpu_max = 0\n").is_err());
        assert!(parse_resource_cfg("[cpu]\ncompile_cpu_max = 101\n").is_err());
        assert!(parse_resource_cfg("[machine]\ncompile = -1\n").is_err());
        assert!(parse_resource_cfg("[machine]\ncompile = \"x\"\n").is_err());
        assert!(parse_resource_cfg("[wat]\nx = 1\n").is_err());
        assert_eq!(parse_resource_cfg("").unwrap(), ResourceCfg::default());
    }

    #[test]
    fn a_slot_held_outside_the_broker_is_not_granted_again() {
        // A `mason run` that outlived a daemon restart still holds its slot, and
        // the new daemon has no lease for it. It used to grant that slot: the
        // grantee could not take the lock ("a slot is held by another process")
        // and tried again, and again, for as long as the old run lasted.
        let dir = tempfile::tempdir().unwrap();
        let cfg = ResourceCfg {
            machine: BTreeMap::from([("compile".to_string(), 2)]),
            ..Default::default()
        };
        let b = Broker::new(
            dir.path().join("locks"),
            dir.path().join("limits.kv"),
            &cfg,
            2,
            crate::cpu::FakeCpu::new(0),
            &[],
        )
        .unwrap();
        let one = BTreeMap::from([("compile".to_string(), 1)]);
        let old = Locks::new(dir.path().join("locks")).unwrap();
        let held = old
            .take(
                999_999,
                "tests 37",
                0,
                &Take {
                    classes: one.clone(),
                    exclusive: false,
                },
                &BTreeMap::from([("compile".to_string(), 2)]),
            )
            .unwrap();
        let first = b.lease_open("first", &one);
        assert!(matches!(b.lease_poll(first), LeaseState::Granted { .. }));
        assert_eq!(b.used("compile"), 2, "the outside holder counts");
        let second = b.lease_open("second", &one);
        assert!(
            matches!(b.lease_poll(second), LeaseState::Waiting { .. }),
            "both slots are held: one by the first lease, one outside the broker"
        );
        let shown = b.snapshot().to_string();
        assert!(
            shown.contains("tests 37 (held outside the queue)"),
            "{shown}"
        );
        // The outside holder ends: the slot is free for the next waiter.
        drop(held);
        assert!(matches!(b.lease_poll(second), LeaseState::Granted { .. }));
    }

    #[test]
    fn exclusive_in_uses_means_the_whole_machine() {
        let uses: BTreeMap<String, u32> =
            [("exclusive".to_string(), 1), ("compile".to_string(), 2)].into();
        let n = needs_of(&uses, Some("claude"));
        assert!(n.exclusive);
        assert_eq!(n.amounts["compile"], 2);
        assert_eq!(n.amounts["llm:claude"], 1);
    }

    #[test]
    fn free_memory_reads_something_on_this_machine() {
        assert!(measure_free_memory_mb().is_some_and(|m| m > 0));
    }

    #[test]
    fn a_device_reading_sizes_a_pool_without_becoming_the_owners_limit() {
        let dir = tempfile::tempdir().unwrap();
        let limits = dir.path().join("limits.kv");
        let b = Broker::new(
            dir.path().join("locks"),
            limits.clone(),
            &ResourceCfg::default(),
            2,
            crate::cpu::FakeCpu::new(0),
            &[],
        )
        .unwrap();
        assert!(b.set_pool_units("device:ios-device", 0));
        assert_eq!(b.limit("device:ios-device"), Some(0));
        assert!(
            !b.set_pool_units("device:ios-device", 0),
            "no change, no news"
        );
        assert!(b.set_pool_units("device:ios-device", 2));
        assert!(
            b.set_pool_units("device:ios-device", 1),
            "lowering is allowed"
        );
        assert!(
            !limits.exists(),
            "a reading is not written as the owner's limit"
        );
        // The owner's word wins over a reading.
        b.set_limit("device:ios-device", 5).unwrap();
        assert!(!b.set_pool_units("device:ios-device", 1));
        assert_eq!(b.limit("device:ios-device"), Some(5));
    }

    #[test]
    fn a_kernel_lock_held_from_outside_makes_the_wait_say_so_and_count_as_long() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ResourceCfg {
            machine: BTreeMap::from([("compile".to_string(), 2)]),
            ..Default::default()
        };
        let b = Broker::new(
            dir.path().join("locks"),
            dir.path().join("limits.kv"),
            &cfg,
            2,
            crate::cpu::FakeCpu::new(0),
            &[],
        )
        .unwrap();
        // The threshold is injected; the real one is a minute.
        b.set_kernel_says_after(Duration::ZERO);
        // Something outside the broker holds the whole machine, which the ledger cannot see.
        let outside = Locks::new(dir.path().join("locks")).unwrap();
        let held = outside
            .take(
                999_999,
                "someone's exclusive run",
                0,
                &Take {
                    classes: BTreeMap::new(),
                    exclusive: true,
                },
                &BTreeMap::from([("compile".to_string(), 2)]),
            )
            .unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let b2 = b.clone();
        let t = std::thread::spawn(move || {
            let rank = Rank::new(false, 1, 2, 1, 1, false);
            let cancelled = std::sync::atomic::AtomicBool::new(false);
            let still_wanted = || !cancelled.load(std::sync::atomic::Ordering::SeqCst);
            let lease = b2.acquire(
                TaskId(7),
                "#7",
                rank,
                Needs::of(&[("compile", 1)]),
                &still_wanted,
                &mut |why| {
                    let _ = tx.send(why.to_string());
                },
            );
            lease.is_ok()
        });
        // The queue's own wait (if any) comes first; the kernel one is the one that says so.
        let said = loop {
            let m = rx.recv_timeout(Duration::from_secs(30)).expect("it said");
            if m.contains("kernel lock") {
                break m;
            }
        };
        assert!(said.contains("compile"), "{said}");
        // It counts as a long wait once it has gone on for the limit.
        let mut alerts = vec![];
        for _ in 0..400 {
            alerts = b.wait_alerts(1);
            if !alerts.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(alerts.len(), 1, "{alerts:?}");
        assert!(alerts[0].resource.starts_with("kernel-lock:"));
        assert!(alerts[0].text.contains("kernel lock"), "{:?}", alerts[0]);
        assert!(b.wait_alerts(0).is_empty(), "0 turns it off");
        drop(held);
        assert!(
            t.join().unwrap(),
            "it gets the lock once the outside holder ends"
        );
        assert!(b.wait_alerts(1).is_empty());
    }

    #[test]
    fn a_limit_of_zero_alerts_at_once_with_the_fix() {
        let dir = tempfile::tempdir().unwrap();
        let b = Broker::new(
            dir.path().join("locks"),
            dir.path().join("limits.kv"),
            &ResourceCfg::default(),
            2,
            crate::cpu::FakeCpu::new(0),
            &[],
        )
        .unwrap();
        b.set_limit("widget", 0).unwrap();
        let id = b.lease_open("run", &BTreeMap::from([("widget".to_string(), 1)]));
        assert!(matches!(b.lease_poll(id), LeaseState::Waiting { .. }));
        let a = b.wait_alerts(0);
        assert_eq!(a.len(), 1, "{a:?}");
        assert!(a[0].never && a[0].resource == "widget");
        assert!(
            a[0].text.contains("mason resources limit widget 1"),
            "{}",
            a[0].text
        );
    }
}
