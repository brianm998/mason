// SPDX-License-Identifier: GPL-3.0-or-later

//! The broker: one place that decides who gets a contended resource, for every
//! project at once (Part 7). The decisions come from `mason-scheduler`; the
//! kernel file locks come from `mason-resources`. This module joins them to
//! the daemon: step jobs wait here for their resources, interactive callers
//! (`mason run --needs`) queue here ahead of pipeline work, and every wait can
//! say why.

use crate::cpu::CpuSource;
use crate::disk::DiskSource;
use crate::policyd::PolicySource;
use crate::stall::StallSource;
use mason_core::TaskId;
use mason_resources::{Held, Locks, Take, TakeError};
use mason_scheduler::{
    explain, since, CpuGate, DiskGate, Kind, Ledger, Needs, PolicyGate, Rank, StallGate, WaitCause,
    COMPILE,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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
/// compile_idle_kill_s = 1800  # stop a task's compile that used no CPU this long (0: never)
/// [disk]     # below this much free disk, no compile and no LLM session starts
/// floor_gb = 20
/// stall_ms = 2000        # no further compile while the build disk is this slow (0: off)
/// [order]    # who goes first when work waits for a resource
/// priority_first = false # true: tracker priority before steps remaining
/// max_wait_s = 1800      # a request waiting this long goes ahead of other work as far along (0: never)
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
    /// Seconds a task's command may hold a compile slot using almost no CPU
    /// before it is stopped (0: never; it still shows as idle).
    pub compile_idle_kill_s: u32,
    /// Free space, in GB, below which nothing new starts (0: no floor).
    pub disk_floor_gb: u64,
    /// How slow an answer from the build disk, in ms, holds further compiles (0: never).
    pub disk_stall_ms: u64,
    /// How busy macOS's `syspolicyd` may be, in percent of one core, before further compiles
    /// wait (0: never). It scans each new executable's first launch.
    pub syspolicyd_max_pct: u32,
    /// Order waiting work by tracker priority before steps remaining. It applies to
    /// every project at once, since one queue serves them all.
    pub priority_first: bool,
    /// The gates (see `GATES`) that resources.toml sets, for `mason resources`.
    pub from_file: std::collections::BTreeSet<String>,
    /// Seconds after which a waiting request goes ahead of all other pipeline
    /// work that is not a landing, oldest first (0: requests do not age).
    pub max_wait_s: u64,
    /// Seconds a waiting exclusive request freezes ordinary work before it gives that up, and
    /// work runs again until the machine drains (0: no bound), `[order] exclusive_max_wait_s`.
    pub exclusive_max_wait_s: u64,
    /// Origins (`[order] exclusive_origins`) that may take the whole machine with `--exclusive`
    /// when they are not a person. Any other non-person request is bounded to all `compile`
    /// units instead.
    pub exclusive_origins: Vec<String>,
}

/// The default `order.exclusive_max_wait_s`: ten minutes.
pub const DEFAULT_EXCLUSIVE_MAX_WAIT_S: u64 = 600;

/// The default `order.max_wait_s`: half an hour.
pub const DEFAULT_MAX_WAIT_S: u64 = 1800;

/// The default `compile_cpu_max`: half the machine.
pub const DEFAULT_COMPILE_CPU_MAX: u32 = 50;

/// The default `compile_settle_s`: twice the time the CPU reading averages over.
pub const DEFAULT_COMPILE_SETTLE_S: u32 = 30;

/// The default `compile_idle_kill_s`: half an hour without CPU is a hang, not a
/// slow test (the slowest of MASON's own tests waits seconds, not minutes).
pub const DEFAULT_COMPILE_IDLE_KILL_S: u32 = 1800;

/// The default `compile_jobs`.
pub const DEFAULT_COMPILE_JOBS: u32 = 3;

/// The default `disk_stall_ms`. Healthy disks answered in under 250 ms even with
/// the machine loaded; the stalls this exists for lasted 20 seconds and more.
pub const DEFAULT_DISK_STALL_MS: u64 = 2000;

/// The default `syspolicyd_max_pct`: three cores. A quiet machine reads under 50%; the
/// 2026-11-16 session loss came at 640 to 770%, and a single cold workspace build's test run
/// reaches about 100 to 200%.
pub const DEFAULT_SYSPOLICYD_MAX_PCT: u32 = 300;

/// How often an open `mason run` wait is added to its task's deadline while it waits (s417).
/// Anything longer than this of a wait in progress is lost if the daemon restarts.
pub const CREDIT_EVERY: Duration = Duration::from_secs(30);

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
            compile_idle_kill_s: DEFAULT_COMPILE_IDLE_KILL_S,
            disk_floor_gb: crate::disk::DEFAULT_FLOOR_GB,
            disk_stall_ms: DEFAULT_DISK_STALL_MS,
            syspolicyd_max_pct: DEFAULT_SYSPOLICYD_MAX_PCT,
            priority_first: false,
            from_file: Default::default(),
            max_wait_s: DEFAULT_MAX_WAIT_S,
            exclusive_max_wait_s: DEFAULT_EXCLUSIVE_MAX_WAIT_S,
            exclusive_origins: vec![],
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
            if key == "order" && name == "priority_first" {
                c.priority_first = v.as_bool().ok_or_else(|| {
                    "resources.toml: order.priority_first must be true or false".to_string()
                })?;
                continue;
            }
            if key == "order" && name == "max_wait_s" {
                c.max_wait_s = v.as_integer().filter(|n| *n >= 0).ok_or_else(|| {
                    "resources.toml: order.max_wait_s must be a whole number".to_string()
                })? as u64;
                continue;
            }
            if key == "order" && name == "exclusive_max_wait_s" {
                c.exclusive_max_wait_s = v.as_integer().filter(|n| *n >= 0).ok_or_else(|| {
                    "resources.toml: order.exclusive_max_wait_s must be a whole number".to_string()
                })? as u64;
                continue;
            }
            if key == "order" && name == "exclusive_origins" {
                c.exclusive_origins = v
                    .as_array()
                    .and_then(|a| {
                        a.iter()
                            .map(|x| x.as_str().map(str::to_string))
                            .collect::<Option<Vec<_>>>()
                    })
                    .ok_or_else(|| {
                        "resources.toml: order.exclusive_origins must be a list of names"
                            .to_string()
                    })?;
                continue;
            }
            let n = v
                .as_integer()
                .filter(|n| *n >= 0)
                .ok_or_else(|| format!("resources.toml: {key}.{name} must be a whole number"))?;
            if let Some(g) = GATES.iter().find(|g| gate_key(g) == (key, name)) {
                c.from_file.insert(g.to_string());
            }
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
                "cpu" if name == "compile_idle_kill_s" => c.compile_idle_kill_s = n as u32,
                "cpu" if name == "syspolicyd_max_pct" => c.syspolicyd_max_pct = n as u32,
                "disk" if name == "floor_gb" => c.disk_floor_gb = n as u64,
                "disk" if name == "stall_ms" => c.disk_stall_ms = n as u64,
                other => return Err(format!("resources.toml: unknown setting {other}.{name}")),
            }
        }
    }
    Ok(c)
}

/// The compile gates the owner can change while masond runs (`Broker::set_gate`).
/// Each is `[cpu]`, `[memory]` or `[disk]` in resources.toml; the name is the key,
/// with its table for the two floors.
pub const GATES: &[&str] = &[
    "compile_cpu_max",
    "compile_settle_s",
    "compile_jobs",
    "compile_idle_kill_s",
    "syspolicyd_max_pct",
    "stall_ms",
    "memory.floor_mb",
    "disk.floor_gb",
];

/// The table and key in resources.toml of a gate.
fn gate_key(name: &str) -> (&str, &str) {
    match name {
        "memory.floor_mb" => ("memory", "floor_mb"),
        "disk.floor_gb" => ("disk", "floor_gb"),
        "stall_ms" => ("disk", "stall_ms"),
        n => ("cpu", n),
    }
}

/// The same bounds `parse_resource_cfg` holds, and none looser.
fn check_gate(name: &str, v: i64) -> Result<(), String> {
    if !GATES.contains(&name) {
        return Err(format!(
            "unknown gate {name:?}; the gates are {}",
            GATES.join(", ")
        ));
    }
    if name == "compile_cpu_max" {
        if !(1..=100).contains(&v) {
            return Err("compile_cpu_max must be from 1 to 100".into());
        }
    } else if v < 0 {
        return Err(format!("{name} must be a whole number, 0 or more"));
    } else if matches!(
        name,
        "compile_settle_s" | "compile_jobs" | "compile_idle_kill_s" | "syspolicyd_max_pct"
    ) && v > i64::from(u32::MAX)
    {
        return Err(format!("{name} is too large"));
    }
    Ok(())
}

/// What resources.toml (or the default) says for a gate. `None`: no memory floor.
fn base_gate(c: &ResourceCfg, name: &str) -> Option<i64> {
    Some(match name {
        "compile_cpu_max" => i64::from(c.compile_cpu_max),
        "compile_settle_s" => i64::from(c.compile_settle_s),
        "compile_jobs" => i64::from(c.compile_jobs),
        "compile_idle_kill_s" => i64::from(c.compile_idle_kill_s),
        "syspolicyd_max_pct" => i64::from(c.syspolicyd_max_pct),
        "stall_ms" => c.disk_stall_ms as i64,
        "memory.floor_mb" => c.memory_floor_mb? as i64,
        _ => c.disk_floor_gb as i64,
    })
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
        /// Another pipeline's name, when this is its work.
        pipeline: Option<String>,
        opened: Instant,
        /// How far this wait has been added to its task's deadline (s417): open waits are
        /// credited as they go, so a daemon restart loses at most [`CREDIT_EVERY`] of one.
        credited: Instant,
        last_poll: Instant,
        granted_at: Option<Instant>,
        /// Seen holding its marker lock at least once.
        locked: bool,
        /// The requester's process id (0: not told), so a watcher can tell a
        /// process tree that is waiting its turn from one that is stuck.
        pid: u32,
    },
    /// A slot of a machine class the kernel says is held, by a process the
    /// broker has no lease for: one that took it before this daemon started
    /// (a `mason run` that outlived the daemon's restart, say). It counts
    /// against the class until the kernel says the slot is free.
    Outside {
        class: String,
    },
    /// Slots held by a command that runs under a step host which outlived the daemon that
    /// granted them (s589). The old daemon took the kernel locks, so they went with it; the
    /// ledger counts them until the host's command ends (`dir` says when). Never compared
    /// with the kernel: there is no lock to find.
    Host {
        dir: PathBuf,
    },
}

struct State {
    ledger: Ledger,
    meta: HashMap<u64, Meta>,
    by_task: HashMap<TaskId, u64>,
    /// The task a `mason run` lease was opened for, when it names one.
    lease_tasks: HashMap<u64, TaskId>,
    /// Where a `mason run` lease came from (`--origin`, else the caller's working directory),
    /// by lease id (s527).
    origins: HashMap<u64, String>,
    /// An agent step's deadline, by task, while it runs: a `mason run` of that task's
    /// waits in the queue are added to it when granted (s363).
    wait_credit: HashMap<TaskId, mason_adapter_api::DeadlineExtra>,
    /// Resources whose limit was set by the owner (config or live) and so are
    /// not changed by a definition that happens to declare the same name.
    fixed: HashSet<String>,
    pools: HashSet<String>,
    floor_mb: Option<u64>,
    /// resources.toml as read, and the gates changed since (`gates.kv`).
    base: ResourceCfg,
    overrides: BTreeMap<String, i64>,
    /// Whether `compile_jobs` and `compile_settle_s` apply (a CPU reading replaces the count).
    compile_by_cpu: bool,
    stall_ms: u64,
    mem: Option<(Instant, Option<u64>)>,
    rank_seq: u64,
    cpu_max: u32,
    cpu: Arc<dyn CpuSource>,
    /// The latest reading, kept so the explanation matches the decision.
    cpu_busy: Option<u32>,
    /// Seconds between compile starts, for the report (0: not paced).
    compile_settle_s: u32,
    /// Tracker priority before steps remaining (`[order] priority_first`).
    priority_first: bool,
    /// How long each compile holder has used almost no CPU, by lease (from `hung`).
    idle: HashMap<u64, u64>,
    compile_idle_kill_s: u32,
    /// What was done about the queue lately: (when, text), newest last.
    notes: Vec<(u64, String)>,
    disk: Option<DiskCfg>,
    /// The latest reading: when, and the gate if the source could be read.
    disk_read: Option<(Instant, Option<DiskGate>)>,
    /// Grants waiting on a kernel lock that something outside the broker holds, by lease.
    kernel_waits: HashMap<u64, KernelWait>,
    /// The class of a wait being announced, by task (see `wait_class`).
    wait_class: HashMap<TaskId, String>,
    /// The build disk's responsiveness and the limit on it. `None`: no stall gate.
    stall: Option<Arc<dyn StallSource>>,
    /// How busy `syspolicyd` is, and the limit on it. `None`: no syspolicyd gate.
    policy: Option<Arc<dyn PolicySource>>,
    policy_max: u32,
    /// The latest `syspolicyd` reading: (busy %, limit %), `None` with no gate or no reading.
    policy_read: Option<(u32, u32)>,
    /// When the reading first went to the limit or over it and has stayed there since.
    policy_since: Option<u64>,
    /// When the last scheduling pass ran. A waiter skips its own pass while this is
    /// younger than the broker's tick (see `acquire`).
    last_pass: Option<Instant>,
    /// A pass is reading lock files right now; waiters leave it to finish.
    pass_running: bool,
    /// Scheduling passes run since start, for tests and diagnosis.
    passes: u64,
}

/// The pacing the tests of `pass_due` check against. The waiters themselves pace by the
/// broker's tick (`tick_ms`, see `acquire`); releases and submits still pass at once.
#[cfg(test)]
const PASS_EVERY: Duration = Duration::from_millis(100);

/// Is a waiter's periodic pass due? Not while one ran less than `every` ago: that pass
/// already granted whatever could be granted, and on 2026-11-13 forty waiters each
/// passing ten times a second held the broker lock almost all the time,
/// so monitor views waited 20 s for it.
fn pass_due_in(last: Option<Instant>, every: Duration) -> bool {
    last.is_none_or(|t| t.elapsed() >= every)
}

#[cfg(test)]
fn pass_due(last: Option<Instant>) -> bool {
    pass_due_in(last, PASS_EVERY)
}

/// What the kernel said about lock files, read with the broker lock released.
#[derive(Default)]
struct Probes {
    /// Leases whose marker was probed (holders, not outside holders).
    alive: HashMap<u64, bool>,
    slots: HashMap<String, Vec<Option<mason_resources::HolderInfo>>>,
    /// Ids of every ledger holder when the probe began.
    holders: BTreeSet<u64>,
    /// External leases that were holding when the probe began.
    holding: HashSet<u64>,
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

/// Per waiting task: its holders' grant times and the waiting count of its class.
type WaitHolders = BTreeMap<TaskId, (Vec<u64>, u32)>;

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
    /// Wait explanation per waiting task, republished by every `pass`. Readers
    /// clone the `Arc` and never take `st`.
    waits_snap: std::sync::RwLock<Arc<BTreeMap<TaskId, String>>>,
    /// The same waits as data, with the chain behind each, the leases that belong to no task
    /// and the queue-held summary (s527). Written with `waits_snap`, so the two agree.
    waits_view: std::sync::RwLock<Arc<crate::waitview::WaitsView>>,
    /// Per waiting task: its holders' grant times and the waiting count of its class.
    waits_holders: std::sync::RwLock<Arc<WaitHolders>>,
    /// Time each task has spent under a CPU cause, updated by every `pass`.
    cpu_hold: Mutex<mason_scheduler::CpuHold>,
    /// The largest of those, for readers that must not take `st`.
    cpu_held_ms: std::sync::atomic::AtomicU64,
    /// Scheduling passes run so far (a test counter).
    pass_count: std::sync::atomic::AtomicUsize,
    /// A waiter's pass interval in ms.
    tick_ms: std::sync::atomic::AtomicU64,
    /// Test seam: called after the broker lock is released, before lock files are probed.
    #[cfg(test)]
    probe_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

/// One holder of `compile`, for the idle check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileHolderInfo {
    pub lease: u64,
    pub label: String,
    pub task: Option<u64>,
    pub interactive: bool,
    /// The process that took the kernel slot (0: unknown).
    pub pid: u32,
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
    /// Steps on the shortest start-to-landed path of the task's definition.
    pub span: u32,
    pub priority: u8,
}

/// The origin of a request made by a person at a terminal (s552).
pub const PERSON: &str = "person";

/// A request just queued by [`Broker::lease_open_from`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    pub id: u64,
    /// The rank chosen: `person` or `pipeline`.
    pub rank: &'static str,
    /// What was changed on the request, when its `--exclusive` was bounded.
    pub converted: Option<String>,
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

/// Put a gate's effective value (the runtime one, else resources.toml's) into `State`.
fn apply_gate(st: &mut State, name: &str) {
    let v = st
        .overrides
        .get(name)
        .copied()
        .or_else(|| base_gate(&st.base, name));
    match name {
        "compile_cpu_max" => st.cpu_max = v.unwrap_or(0) as u32,
        "compile_settle_s" if st.compile_by_cpu => {
            let s = v.unwrap_or(0) as u32;
            st.compile_settle_s = s;
            st.ledger.pace_compiles(u64::from(s) * 1000);
        }
        "compile_jobs" if st.compile_by_cpu && !st.fixed.contains(COMPILE) => {
            let units = match v.unwrap_or(0) {
                0 => std::thread::available_parallelism().map_or(8, |n| n.get()) as u32,
                n => n as u32,
            };
            st.ledger.set_capacity(COMPILE, units, Kind::Machine);
        }
        "compile_idle_kill_s" => st.compile_idle_kill_s = v.unwrap_or(0) as u32,
        "stall_ms" => st.stall_ms = v.unwrap_or(0) as u64,
        "syspolicyd_max_pct" => st.policy_max = v.unwrap_or(0) as u32,
        "memory.floor_mb" => {
            st.floor_mb = v.map(|n| n as u64);
            st.mem = None;
        }
        "disk.floor_gb" => {
            if let Some(d) = st.disk.as_mut() {
                d.floor_mb = v.unwrap_or(0) as u64 * 1024;
            }
            st.disk_read = None;
        }
        _ => {}
    }
}

thread_local! {
    static LOCKS_TAKEN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many times the calling thread has taken the broker lock (a test counter).
pub fn broker_locks_taken_by_this_thread() -> usize {
    LOCKS_TAKEN.with(|c| c.get())
}

fn lock<'a>(m: &'a Mutex<State>) -> MutexGuard<'a, State> {
    LOCKS_TAKEN.with(|c| c.set(c.get() + 1));
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// How long a waiting request that never told us its pid may go without a poll
/// before it is dropped. Generous: polls come every 150 ms, but they queue
/// behind the broker's state lock, which a stalled disk can hold for a minute.
pub const UNPOLLED_WAITER_REAP: Duration = Duration::from_secs(120);

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
        // Reading the machine replaces a count set per project, but CPU use alone
        // lets too many in: a test run is mostly disk and process launches, and
        // everything blocked on a stalled disk reads as idle. So `compile_jobs`
        // caps them, until the owner sets a count live (limits.kv, read below).
        // 0 is one slot per hardware thread, which never holds a compile back,
        // since the CPU gate stops them long before that.
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
        if cfg.exclusive_max_wait_s > 0 {
            ledger.set_exclusive_max_wait(Some(cfg.exclusive_max_wait_s.saturating_mul(1000)));
        }
        if cfg.max_wait_s > 0 {
            ledger.set_max_wait(Some(cfg.max_wait_s.saturating_mul(1000)));
        }
        let b = Broker {
            st: Mutex::new(State {
                ledger,
                meta: HashMap::new(),
                by_task: HashMap::new(),
                fixed,
                pools,
                floor_mb: cfg.memory_floor_mb,
                base: cfg.clone(),
                overrides: BTreeMap::new(),
                compile_by_cpu,
                stall_ms: cfg.disk_stall_ms,
                mem: None,
                rank_seq: 0,
                cpu_max: cfg.compile_cpu_max,
                cpu,
                cpu_busy: None,
                priority_first: cfg.priority_first,
                lease_tasks: HashMap::new(),
                origins: HashMap::new(),
                wait_credit: HashMap::new(),
                idle: HashMap::new(),
                compile_idle_kill_s: cfg.compile_idle_kill_s,
                notes: vec![],
                compile_settle_s: if compile_by_cpu {
                    cfg.compile_settle_s
                } else {
                    0
                },
                disk: None,
                disk_read: None,
                kernel_waits: HashMap::new(),
                wait_class: HashMap::new(),
                stall: None,
                policy: None,
                policy_max: cfg.syspolicyd_max_pct,
                policy_read: None,
                policy_since: None,
                last_pass: None,
                pass_running: false,
                passes: 0,
            }),
            cv: Condvar::new(),
            locks,
            limits_file,
            waits_snap: Default::default(),
            waits_view: Default::default(),
            waits_holders: Default::default(),

            cpu_hold: Default::default(),
            cpu_held_ms: Default::default(),
            pass_count: Default::default(),
            tick_ms: std::sync::atomic::AtomicU64::new(100),
            #[cfg(test)]
            probe_hook: Mutex::new(None),
            kernel_says_after: Mutex::new(KERNEL_WAIT_SAYS_AFTER),
        };
        // Gates changed live survive a restart and win over resources.toml.
        if let Ok(text) = std::fs::read_to_string(b.gates_file()) {
            let mut st = lock(&b.st);
            for line in text.lines() {
                if let Some((name, v)) = line.split_once('\t') {
                    if let Ok(v) = v.trim().parse::<i64>() {
                        if check_gate(name, v).is_ok() {
                            st.overrides.insert(name.to_string(), v);
                            apply_gate(&mut st, name);
                        }
                    }
                }
            }
        }
        // Limits changed live survive a restart.
        if let Ok(text) = std::fs::read_to_string(&b.limits_file) {
            for line in text.lines() {
                if let Some((name, n)) = line.split_once('\t') {
                    if let Ok(n) = n.trim().parse::<u32>() {
                        let mut st = lock(&b.st);
                        // A saved `compile` count is the owner's word and replaces
                        // cpu.compile_jobs; the CPU and disk gates still apply.
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
        st = self.pass(st);
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
        if name == COMPILE && units == 0 {
            return Err("`compile` needs at least 1: 0 would stop every build and test".into());
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
        st = self.pass(st);
        drop(st);
        self.cv.notify_all();
        Ok(())
    }

    fn gates_file(&self) -> PathBuf {
        self.limits_file.with_file_name("gates.kv")
    }

    /// Change a compile gate while things run, or (`None`) go back to what
    /// resources.toml says. Checked with the bounds `parse_resource_cfg` holds,
    /// applied at once and kept across restarts in `gates.kv`.
    pub fn set_gate(&self, name: &str, value: Option<i64>) -> Result<(), String> {
        if let Some(v) = value {
            check_gate(name, v)?;
        } else if !GATES.contains(&name) {
            check_gate(name, 0)?;
        }
        let mut st = lock(&self.st);
        match value {
            Some(v) => drop(st.overrides.insert(name.to_string(), v)),
            None => drop(st.overrides.remove(name)),
        }
        apply_gate(&mut st, name);
        let text: String = st
            .overrides
            .iter()
            .map(|(n, v)| format!("{n}\t{v}\n"))
            .collect();
        let saved = if text.is_empty() {
            match std::fs::remove_file(self.gates_file()) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        } else {
            std::fs::write(self.gates_file(), text)
        };
        st = self.pass(st);
        drop(st);
        self.cv.notify_all();
        saved.map_err(|e| format!("{name} is changed but could not be kept across a restart: {e}"))
    }

    /// Every gate: its value now, where it came from, and what each other source says.
    pub fn gates(&self) -> Vec<serde_json::Value> {
        self.gates_locked(&lock(&self.st))
    }

    fn gates_locked(&self, st: &State) -> Vec<serde_json::Value> {
        let dflt = ResourceCfg::default();
        GATES
            .iter()
            .map(|name| {
                let over = st.overrides.get(*name).copied();
                let file = base_gate(&st.base, name);
                let source = if over.is_some() {
                    "runtime"
                } else if st.base.from_file.contains(*name) {
                    "resources.toml"
                } else {
                    "default"
                };
                let (lo, hi) = if *name == "compile_cpu_max" {
                    (1, 100)
                } else {
                    (0, i64::MAX)
                };
                serde_json::json!({
                    "name": name,
                    "value": over.or(file),
                    "source": source,
                    "default": base_gate(&dflt, name),
                    "file": if st.base.from_file.contains(*name) { file } else { None },
                    "min": lo,
                    "max": if hi == i64::MAX { serde_json::Value::Null } else { hi.into() },
                })
            })
            .collect()
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
            floor_mb: st
                .overrides
                .get("disk.floor_gb")
                .map_or(floor_gb, |v| *v as u64)
                * 1024,
            every,
        });
        st.disk_read = None;
        st = self.pass(st);
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
        st.stall = Some(source);
        st.stall_ms = st.overrides.get("stall_ms").map_or(max_ms, |v| *v as u64);
        st = self.pass(st);
        drop(st);
        self.cv.notify_all();
    }

    /// Turn the syspolicyd gate on: no further compile while `source` reads at or above `max_pct`.
    pub fn set_policy(&self, source: Arc<dyn PolicySource>, max_pct: u32) {
        let mut st = lock(&self.st);
        st.policy = Some(source);
        st.policy_max = st
            .overrides
            .get("syspolicyd_max_pct")
            .map_or(max_pct, |v| *v as u32);
        st = self.pass(st);
        drop(st);
        self.cv.notify_all();
    }

    /// Read the CPU and stall sources afresh. Every plan is made right after
    /// this, so the ledger's stall reading is always current.
    fn cpu_gate(&self, st: &mut State) -> Option<CpuGate> {
        let max_ms = st.stall_ms;
        let stall = st.stall.as_ref().filter(|_| max_ms > 0).and_then(|source| {
            source
                .slow_ms()
                .map(|slow_ms| StallGate { slow_ms, max_ms })
        });
        st.ledger.set_stall(stall);
        let max_pct = st.policy_max;
        let policy = st
            .policy
            .as_ref()
            .filter(|_| max_pct > 0)
            .and_then(|source| {
                source
                    .busy_pct()
                    .map(|busy_pct| PolicyGate { busy_pct, max_pct })
            });
        st.ledger.set_policy(policy);
        st.policy_read = policy.map(|p| (p.busy_pct, p.max_pct));
        match policy {
            Some(p) if p.busy_pct >= p.max_pct => {
                if st.policy_since.is_none() {
                    st.policy_since = Some(crate::daemon::now_ms());
                }
            }
            _ => st.policy_since = None,
        }
        st.cpu_busy = st.cpu.busy_pct();
        st.cpu_busy.map(|busy_pct| CpuGate {
            busy_pct,
            max_pct: st.cpu_max,
        })
    }

    /// The `syspolicyd` reading, read afresh: (busy %, limit %, since when it has been at or
    /// over the limit). `None` while there is no gate or no reading.
    pub fn policy_hold(&self) -> Option<(u32, u32, Option<u64>)> {
        let mut st = lock(&self.st);
        self.cpu_gate(&mut st);
        st.policy_read.map(|(b, m)| (b, m, st.policy_since))
    }

    /// The overall CPU reading the compile gate uses, to read from outside the broker.
    pub fn cpu_source(&self) -> Arc<dyn CpuSource> {
        lock(&self.st).cpu.clone()
    }

    /// How long the primary CPU reading has been missing (ms), once that is long
    /// enough to alert on. The gate is on its fallback meanwhile.
    pub fn cpu_missing_ms(&self) -> Option<(u64, &'static str)> {
        let st = lock(&self.st);
        st.cpu
            .primary_missing_ms()
            .filter(|ms| *ms > crate::cpu::MISSING_ALERT_MS)
            .map(|ms| (ms, st.cpu.source()))
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
    ///
    /// The lock files are probed with the broker lock released (a stalled disk
    /// must not block every reader of the broker); the guard comes back
    /// re-taken, and what changed meanwhile is ignored.
    fn pass<'a>(&'a self, mut st: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        use std::sync::atomic::Ordering::Relaxed;
        self.pass_count.fetch_add(1, Relaxed);
        st.passes += 1;
        // Claimed now, so a waiter that wakes while the files are read does not
        // start a second pass.
        st.last_pass = Some(Instant::now());
        st.pass_running = true;
        self.refresh_disk(&mut st);
        let mut probes = Probes::default();
        let snap = st.ledger.snapshot();
        probes.holders = snap.holders.iter().map(|h| h.id).collect();
        for (id, m) in &st.meta {
            if matches!(m, Meta::External { .. }) && st.ledger.holds(*id) {
                probes.holding.insert(*id);
            }
        }
        let alive_ids: Vec<u64> = snap
            .holders
            .iter()
            .map(|h| h.id)
            .filter(|id| {
                !matches!(
                    st.meta.get(id),
                    Some(Meta::Outside { .. } | Meta::Host { .. })
                )
            })
            .collect();
        let classes: Vec<(String, u32)> = snap
            .capacity
            .iter()
            .filter(|(_, c)| c.kind == Kind::Machine)
            .map(|(k, c)| (k.clone(), c.units))
            .collect();
        drop(st);
        #[cfg(test)]
        {
            let hook = self
                .probe_hook
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if let Some(h) = hook {
                h();
            }
        }
        for id in alive_ids {
            probes.alive.insert(id, self.locks.alive(id));
        }
        for (class, units) in classes {
            let v = self.locks.slots(&class, units);
            probes.slots.insert(class, v);
        }
        let mut st = lock(&self.st);
        self.apply_pass(&mut st, Some(&probes));
        st.pass_running = false;
        st
    }

    /// A pass for a release: grant what the freed units allow, at once, with no
    /// lock-file I/O. The next full pass does the reaping.
    fn quick_pass(&self, st: &mut State) {
        self.pass_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        st.passes += 1;
        self.refresh_disk(st);
        self.apply_pass(st, None);
    }

    /// The part of a pass that needs the lock and does no lock-file I/O.
    /// With no `probes` it only grants (and publishes); reaping dead holders and
    /// counting outside holders wait for the next full pass.
    fn apply_pass(&self, st: &mut State, probes: Option<&Probes>) {
        let now = crate::daemon::now_ms();
        let mut changed = false;
        let ids: Vec<u64> = if probes.is_some() {
            st.meta.keys().copied().collect()
        } else {
            vec![]
        };
        for id in ids {
            let Some(Meta::External {
                pipeline: _,
                opened,
                last_poll,
                granted_at,
                locked,
                pid,
                ..
            }) = st.meta.get_mut(&id)
            else {
                continue;
            };
            let mut silent_for = None;
            let dead = if st.ledger.holds(id) {
                // Granted since the probe began: nothing was read for it.
                let probes = probes.expect("ids is empty without probes");
                let Some(&alive) = probes
                    .alive
                    .get(&id)
                    .filter(|_| probes.holding.contains(&id))
                else {
                    continue;
                };
                if alive {
                    *locked = true;
                    false
                } else {
                    // Not yet locked is a grace period; locked once and now
                    // free is the kernel saying the holder is gone.
                    *locked || granted_at.is_none_or(|g| g.elapsed() > Duration::from_secs(5))
                }
            } else {
                let _ = opened;
                if *pid > 0 {
                    // The requester is known: it is gone only when its process
                    // is. A late poll (a stalled disk holding the state lock)
                    // costs it nothing.
                    !mason_step_host::registry::pid_alive(*pid)
                } else {
                    let silent = last_poll.elapsed();
                    if silent > UNPOLLED_WAITER_REAP {
                        silent_for = Some(silent);
                        true
                    } else {
                        false
                    }
                }
            };
            if let Some(silent) = silent_for {
                let label = st
                    .ledger
                    .snapshot()
                    .waiting
                    .iter()
                    .find(|r| r.id == id)
                    .map(|r| r.label.clone())
                    .unwrap_or_default();
                tracing::warn!(
                    lease = id,
                    label = %label,
                    silent_s = silent.as_secs(),
                    "dropping a waiting request that has not polled and has no pid"
                );
            }
            if dead {
                st.ledger.release(id);
                st.meta.remove(&id);
                changed = true;
            }
        }
        if probes.is_some() {
            changed |= Self::release_ended_hosts(st);
        }
        if let Some(probes) = probes {
            changed |= self.count_outside_holders(st, now, probes);
        }
        let (floor, free) = (st.floor_mb, self.free_memory(st));
        let gate = self.cpu_gate(st);
        let p = st.ledger.schedule_gated(now, free, floor, gate);
        // An agent waiting for its own `mason run` is not working: its deadline grows by the
        // wait, as it does for a `run_check` (s363). Open waits are added as they go, every
        // [`CREDIT_EVERY`], and the rest when granted (s417).
        let granted: HashSet<u64> = p.grants.iter().copied().collect();
        let State {
            meta,
            lease_tasks,
            wait_credit,
            ..
        } = &mut *st;
        for (id, m) in meta.iter_mut() {
            let Meta::External {
                credited,
                granted_at,
                ..
            } = m
            else {
                continue;
            };
            let grant = granted.contains(id);
            if granted_at.is_some() || !(grant || credited.elapsed() >= CREDIT_EVERY) {
                continue;
            }
            if grant {
                *granted_at = Some(Instant::now());
            }
            let w = credited.elapsed();
            *credited = Instant::now();
            if let Some(extra) = lease_tasks.get(id).and_then(|t| wait_credit.get(t)) {
                extra.add(w);
            }
        }
        if changed || !p.grants.is_empty() {
            self.cv.notify_all();
        }
        self.publish_waits(st, now, free, floor, gate);
    }

    /// Store the wait explanation of every waiting task, for readers that must
    /// not take the broker lock. The one place the snapshot is written.
    fn publish_waits(
        &self,
        st: &mut State,
        now: u64,
        free: Option<u64>,
        floor: Option<u64>,
        gate: Option<CpuGate>,
    ) {
        let waits = st.ledger.waits_gated(now, free, floor, gate);
        let by_id: HashMap<u64, TaskId> = st.by_task.iter().map(|(t, id)| (*id, *t)).collect();
        let snap: BTreeMap<TaskId, String> = waits
            .waits
            .iter()
            .filter_map(|(id, w)| by_id.get(id).map(|t| (*t, explain(w, now))))
            .collect();
        *self.waits_snap.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(snap);
        let view = Self::wait_view_of(st, &waits.waits, now);
        *self.waits_view.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(view);
        let mut depth: HashMap<String, u32> = HashMap::new();
        for (id, w) in waits.waits.iter() {
            if by_id.contains_key(id) {
                *depth.entry(w.class()).or_default() += 1;
            }
        }
        let holders: BTreeMap<TaskId, (Vec<u64>, u32)> = waits
            .waits
            .iter()
            .filter_map(|(id, w)| {
                by_id.get(id).map(|t| {
                    (
                        *t,
                        (
                            w.held_by.iter().map(|h| h.since).collect(),
                            depth.get(&w.class()).copied().unwrap_or(0),
                        ),
                    )
                })
            })
            .collect();
        *self
            .waits_holders
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Arc::new(holders);

        let mut hold = self.cpu_hold.lock().unwrap_or_else(|e| e.into_inner());
        hold.observe(
            now,
            waits
                .waits
                .iter()
                .filter_map(|(id, w)| by_id.get(id).map(|t| (t.0, w.is_cpu()))),
        );
        self.cpu_held_ms
            .store(hold.max_held_ms(), std::sync::atomic::Ordering::Relaxed);
    }

    /// The most time one waiting task has spent under a CPU cause (at most one pass old).
    pub fn cpu_gate_held_ms(&self) -> u64 {
        self.cpu_held_ms.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Build the structured view of one plan from the broker's state.
    fn wait_view_of(
        st: &mut State,
        waits: &BTreeMap<u64, mason_scheduler::Wait>,
        now: u64,
    ) -> crate::waitview::WaitsView {
        let State {
            ledger,
            meta,
            by_task,
            lease_tasks,
            origins,
            ..
        } = st;
        origins.retain(|id, _| meta.contains_key(id));
        let snap = ledger.snapshot_at(now);
        let step_tasks: HashMap<u64, u64> = by_task.iter().map(|(t, id)| (*id, t.0)).collect();
        let task_of: HashMap<u64, u64> = step_tasks
            .iter()
            .map(|(id, t)| (*id, *t))
            .chain(lease_tasks.iter().map(|(id, t)| (*id, t.0)))
            .collect();
        let leases: HashMap<u64, String> = meta
            .iter()
            .filter_map(|(id, m)| match m {
                Meta::External { pipeline, .. } => Some((
                    *id,
                    origins
                        .get(id)
                        .cloned()
                        .or_else(|| pipeline.clone())
                        .unwrap_or_else(|| "cli".into()),
                )),
                Meta::Outside { .. } => Some((*id, "held outside the queue".into())),
                Meta::Task => None,
            })
            .collect();
        crate::waitview::build(&crate::waitview::Inputs {
            now,
            snap: &snap,
            waits,
            task_of: &task_of,
            step_tasks: &step_tasks,
            leases: &leases,
        })
    }

    /// The waits as data (at most one pass old): what each waiting task is held back by, the
    /// leases that belong to no task, and what most of the queue is held by.
    pub fn wait_view(&self) -> Arc<crate::waitview::WaitsView> {
        self.waits_view
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Holder grant times and class queue depth per waiting task (at most one pass old).
    pub fn waits_holders(&self) -> Arc<BTreeMap<TaskId, (Vec<u64>, u32)>> {
        self.waits_holders
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Say where lease `id` came from.
    pub fn lease_set_origin(&self, id: u64, origin: &str) {
        let mut st = lock(&self.st);
        if st.meta.contains_key(&id) && !origin.trim().is_empty() {
            st.origins.insert(id, origin.trim().to_string());
        }
    }

    /// The published wait explanations (at most one pass old).
    pub fn waits_snapshot(&self) -> Arc<BTreeMap<TaskId, String>> {
        self.waits_snap
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Make the ledger count every machine slot the kernel says is held. A slot
    /// held by a process the broker has no lease for (it took it before this
    /// daemon started) would otherwise look free: the broker granted it, and the
    /// grantee could not take the lock ("a slot is held by another process"),
    /// again and again until the holder ended. Each such slot is adopted as an
    /// outside holder, and released once the kernel says the slot is free.
    /// Only leases holding their locks are compared with the kernel, so a grant
    /// whose holder has not taken them yet never hides an outside holder.
    fn count_outside_holders(&self, st: &mut State, now: u64, probes: &Probes) -> bool {
        let snap = st.ledger.snapshot();
        // A holder came or went while the files were read: the reading and the
        // ledger no longer describe the same moment. The next pass redoes it.
        if snap.holders.iter().map(|h| h.id).collect::<BTreeSet<_>>() != probes.holders {
            return false;
        }
        let mut changed = false;
        for (class, cap) in snap
            .capacity
            .iter()
            .filter(|(_, c)| c.kind == Kind::Machine)
        {
            let Some(slots) = probes.slots.get(class) else {
                continue;
            };
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
                .filter(|h| {
                    !outside.contains(&h.id) && probes.alive.get(&h.id).copied().unwrap_or(false)
                })
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

    /// Count, as held, the slots of every command that still runs under a step host the
    /// previous daemon started (s589). Called once, before the first grant: until it is,
    /// the machine classes would look free. Each is released when its host's command ends.
    /// Only machine classes the broker knows are counted; the rest are not slots.
    pub fn adopt_hosts(&self, holds: &[crate::hostedcheck::HostHold]) {
        let mut st = lock(&self.st);
        let now = crate::daemon::now_ms();
        let caps = st.ledger.snapshot().capacity;
        for h in holds {
            let amounts: BTreeMap<String, u32> = h
                .uses
                .iter()
                .filter(|(k, n)| **n > 0 && caps.get(*k).is_some_and(|c| c.kind == Kind::Machine))
                .map(|(k, n)| (k.clone(), *n))
                .collect();
            if amounts.is_empty() {
                continue;
            }
            let needs = Needs {
                exclusive: false,
                amounts,
            };
            let id = st.ledger.adopt(
                needs,
                &format!("{} (from before the restart)", h.label),
                now,
            );
            st.meta.insert(id, Meta::Host { dir: h.dir.clone() });
        }
    }

    /// Give back the slots of a host whose command ended (or whose host is gone).
    fn release_ended_hosts(st: &mut State) -> bool {
        use mason_step_host::registry::{status_of, HostState};
        let ended: Vec<u64> = st
            .meta
            .iter()
            .filter_map(|(id, m)| match m {
                Meta::Host { dir } => (!matches!(
                    status_of(dir),
                    Ok(s) if s.state == HostState::Running
                ))
                .then_some(*id),
                _ => None,
            })
            .collect();
        for id in &ended {
            st.ledger.release(*id);
            st.meta.remove(id);
        }
        !ended.is_empty()
    }

    /// Record, for the length of one `on_wait` call, what the wait is for. The callback reads
    /// it with `wait_class`.
    fn say_class(&self, task: TaskId, class: Option<String>) {
        let mut st = lock(&self.st);
        match class {
            Some(c) => {
                st.wait_class.insert(task, c);
            }
            None => {
                st.wait_class.remove(&task);
            }
        }
    }

    /// What the wait `task` has just announced is for. Only meaningful inside `on_wait`.
    pub fn wait_class(&self, task: TaskId) -> Option<String> {
        lock(&self.st).wait_class.get(&task).cloned()
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
        self.acquire_as(task, label, rank, needs, alive, on_wait, false, None)
    }

    /// [`Broker::acquire`] with how long the work is expected to hold it (s385), so it may
    /// run while the machine drains for an exclusive request if it will be done in time.
    /// `beside` as in [`Broker::acquire_beside`].
    #[allow(clippy::too_many_arguments)]
    pub fn acquire_expecting(
        self: &Arc<Self>,
        task: TaskId,
        label: &str,
        rank: Rank,
        needs: Needs,
        alive: &dyn Fn() -> bool,
        on_wait: &mut dyn FnMut(&str),
        beside: bool,
        expected_ms: Option<u64>,
    ) -> Result<Lease, Cancelled> {
        self.acquire_as(
            task,
            label,
            rank,
            needs,
            alive,
            on_wait,
            beside,
            expected_ms,
        )
    }

    /// [`Broker::acquire`] for one of several requests a step makes at once (s345: suites run
    /// side by side). It is recorded like a `mason run` command's request, beside the step's
    /// own, so it never takes the step's place in `by_task`.
    pub fn acquire_beside(
        self: &Arc<Self>,
        task: TaskId,
        label: &str,
        rank: Rank,
        needs: Needs,
        alive: &dyn Fn() -> bool,
        on_wait: &mut dyn FnMut(&str),
    ) -> Result<Lease, Cancelled> {
        self.acquire_as(task, label, rank, needs, alive, on_wait, true, None)
    }

    #[allow(clippy::too_many_arguments)]
    fn acquire_as(
        self: &Arc<Self>,
        task: TaskId,
        label: &str,
        rank: Rank,
        needs: Needs,
        alive: &dyn Fn() -> bool,
        on_wait: &mut dyn FnMut(&str),
        beside: bool,
        expected_ms: Option<u64>,
    ) -> Result<Lease, Cancelled> {
        let id = {
            let mut st = lock(&self.st);
            let rank = order(&st, rank);
            let id = st
                .ledger
                .submit(rank, needs.clone(), label, crate::daemon::now_ms());
            st.ledger.set_expected(id, expected_ms);
            st.meta.insert(id, Meta::Task);
            if beside {
                st.lease_tasks.insert(id, task);
            } else {
                st.by_task.insert(task, id);
            }
            // Granted in the same hold as the submit: admission reads the ledger
            // and must never see a lease that is neither held nor waiting.
            self.quick_pass(&mut st);
            id
        };
        let mut announced = false;
        let mut last_check = Instant::now();
        loop {
            // The check and the wait share one hold of the lock, so a release
            // that happens in between cannot be missed (it needs the lock). A pass
            // that another waiter ran moments ago already granted what it could,
            // and grants wake every waiter, so this one only checks its own lease.
            let mut st = lock(&self.st);
            // One pass serves every waiter: run it only if none ran lately.
            let tick =
                Duration::from_millis(self.tick_ms.load(std::sync::atomic::Ordering::Relaxed));
            if !st.pass_running && pass_due_in(st.last_pass, tick) {
                st = self.pass(st);
            }
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
                let class = waits.waits.get(&id).map(|w| w.class());
                drop(st);
                announced = true;
                self.say_class(task, class);
                on_wait(&text);
                self.say_class(task, None);
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
                self.say_class(task, Some("outside-pipeline".into()));
                on_wait(&text);
                self.say_class(task, None);
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
        st.lease_tasks.remove(&id);
        if let Some(t) = task {
            if st.by_task.get(&t) == Some(&id) {
                st.by_task.remove(&t);
            }
        }
        self.quick_pass(&mut st);
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

    /// Test seams for the shared pass.
    #[cfg(test)]
    fn set_tick_for_test(&self, d: Duration) {
        self.tick_ms
            .store(d.as_millis() as u64, std::sync::atomic::Ordering::Relaxed);
    }

    /// Make the last pass look a tick old and wake every waiter.
    #[cfg(test)]
    fn expire_pass_for_test(&self) {
        lock(&self.st).last_pass = None;
        self.cv.notify_all();
    }

    /// The tasks holding an `llm:` lease right now, under one hold of the lock: the
    /// home view used to call `llm_held_by` once per task, each queuing for the lock.
    pub fn llm_holders(&self) -> HashSet<TaskId> {
        let st = lock(&self.st);
        st.by_task
            .iter()
            .filter(|(_, id)| {
                st.ledger.holder(**id).is_some_and(|h| {
                    h.needs
                        .amounts
                        .iter()
                        .any(|(k, n)| *n > 0 && k.starts_with("llm:"))
                })
            })
            .map(|(t, _)| *t)
            .collect()
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

    /// Why a task's step is waiting, from the last published pass (never
    /// takes the broker lock), or `None` if it is not waiting.
    pub fn explain_task(&self, task: TaskId) -> Option<String> {
        self.waits_snapshot().get(&task).cloned()
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
        self.lease_open_expecting(label, uses, standing, None)
    }

    /// [`Broker::lease_open_as`] with how long the command is expected to hold what it takes
    /// (s400), from the times of its earlier runs, so a drain it holds can be passed by short
    /// work. `None`: unknown, as before.
    pub fn lease_open_expecting(
        &self,
        label: &str,
        uses: &BTreeMap<String, u32>,
        standing: Option<Standing>,
        expected_ms: Option<u64>,
    ) -> u64 {
        self.lease_open_from(label, uses, standing, expected_ms, None)
            .id
    }

    /// [`Broker::lease_open_expecting`] with who is really waiting (s552). `origin` is `None` or
    /// `person` for a person at a terminal: ranked first, and `--exclusive` takes the whole
    /// machine. Any other origin (an agent, a hook, no terminal, a task's environment) is
    /// ranked as pipeline work, next after landings (the task's own standing when it has one),
    /// and its `--exclusive` becomes all `compile` units unless `[order] exclusive_origins`
    /// names the origin. What was chosen is returned and shown on the request's label.
    pub fn lease_open_from(
        &self,
        label: &str,
        uses: &BTreeMap<String, u32>,
        standing: Option<Standing>,
        expected_ms: Option<u64>,
        origin: Option<&str>,
    ) -> Opened {
        let person = standing.is_none() && origin.is_none_or(|o| o == PERSON);
        let (uses, converted) = match origin {
            Some(o) if o != PERSON => self.bound_exclusive(uses, o),
            _ => (uses.clone(), None),
        };
        let shown = match &converted {
            Some(c) => format!("{label} [{c}]"),
            None => label.to_string(),
        };
        let id = self.open_lease(&shown, &uses, None, expected_ms, |st, now| {
            if person {
                return Rank::new(true, 0, 0, now, st.rank_seq, false);
            }
            let (steps, priority, span) =
                standing.map_or((0, 0, 1), |s| (s.steps_remaining, s.priority, s.span));
            order(
                st,
                Rank::new(false, steps, priority, now, st.rank_seq, false).with_span(span),
            )
        });
        Opened {
            id,
            rank: if person { "person" } else { "pipeline" },
            converted,
        }
    }

    /// A non-person request for the whole machine, unless the owner allows `origin`, asks for
    /// every `compile` unit instead (s552). Returns the uses to queue and what was changed.
    pub fn bound_exclusive(
        &self,
        uses: &BTreeMap<String, u32>,
        origin: &str,
    ) -> (BTreeMap<String, u32>, Option<String>) {
        let st = lock(&self.st);
        if uses.get(EXCLUSIVE).copied().unwrap_or(0) == 0
            || st.base.exclusive_origins.iter().any(|o| o == origin)
        {
            return (uses.clone(), None);
        }
        let all = st.ledger.capacity(COMPILE).map_or(1, |c| c.units.max(1));
        let mut out = uses.clone();
        out.remove(EXCLUSIVE);
        out.insert(COMPILE.to_string(), all);
        (
            out,
            Some(format!(
                "exclusive from {origin} is all {all} compile units, not the machine"
            )),
        )
    }

    /// Queue a request from another pipeline. It is ranked as pipeline work,
    /// never as a person's: `steps` and `priority` from the caller, this
    /// request's own arrival, and the machine's ordering rule. A person's
    /// request still sorts ahead of it.
    pub fn lease_open_outside(
        &self,
        label: &str,
        uses: &BTreeMap<String, u32>,
        o: &crate::outside::Outside,
    ) -> u64 {
        let (uses, converted) = self.bound_exclusive(uses, &o.pipeline);
        let uses = &uses;
        let label = match &converted {
            Some(c) => format!("{label} [{c}]"),
            None => label.to_string(),
        };
        let shown = crate::outside::display_label(&o.pipeline, &label);
        self.open_lease(&shown, uses, Some(o.pipeline.clone()), None, |st, now| {
            order(
                st,
                Rank::new(false, o.steps, o.priority, now, st.rank_seq, false),
            )
        })
    }

    fn open_lease(
        &self,
        label: &str,
        uses: &BTreeMap<String, u32>,
        pipeline: Option<String>,
        expected_ms: Option<u64>,
        rank_of: impl FnOnce(&State, u64) -> Rank,
    ) -> u64 {
        let mut st = lock(&self.st);
        let now = crate::daemon::now_ms();
        st.rank_seq += 1;
        let rank = rank_of(&st, now);
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
        st.ledger.set_expected(id, expected_ms);
        st.meta.insert(
            id,
            Meta::External {
                pipeline,
                opened: Instant::now(),
                credited: Instant::now(),
                last_poll: Instant::now(),
                granted_at: None,
                locked: false,
                pid: 0,
            },
        );
        self.quick_pass(&mut st);
        drop(st);
        id
    }

    /// Note which task a `mason run` lease was opened for, so that a change of
    /// the task's priority reaches it.
    /// While an agent step of `task` runs, add the time its `mason run` commands wait in
    /// the queue to its deadline (s363). [`Broker::stop_crediting_waits`] ends it.
    pub fn credit_waits_to(&self, task: TaskId, extra: mason_adapter_api::DeadlineExtra) {
        lock(&self.st).wait_credit.insert(task, extra);
    }

    /// The agent step of `task` ended: its `mason run` waits no longer count for it.
    pub fn stop_crediting_waits(&self, task: TaskId) {
        lock(&self.st).wait_credit.remove(&task);
    }

    pub fn lease_set_task(&self, id: u64, task: TaskId) {
        let mut st = lock(&self.st);
        if st.meta.contains_key(&id) {
            st.lease_tasks.insert(id, task);
        }
    }

    /// Give a task's waiting requests (its step's and those of its `mason run`
    /// commands) a new tracker priority, in place: each keeps its arrival time,
    /// progress and landing status. Returns how many were re-ranked.
    pub fn set_task_priority(&self, task: TaskId, priority: u8) -> usize {
        let mut st = lock(&self.st);
        let mut ids: Vec<u64> = st.by_task.get(&task).copied().into_iter().collect();
        ids.extend(
            st.lease_tasks
                .iter()
                .filter(|(_, t)| **t == task)
                .map(|(id, _)| *id),
        );
        let mut n = 0;
        for id in ids {
            if let Some(rank) = st.ledger.rank_of(id) {
                st.ledger.rerank(id, rank.with_priority(priority));
                n += 1;
            }
        }
        if n > 0 {
            st = self.pass(st);
            drop(st);
            self.cv.notify_all();
        }
        n
    }

    /// Note which process asked for lease `id`.
    pub fn lease_set_pid(&self, id: u64, pid: u32) {
        if let Some(Meta::External { pid: p, .. }) = lock(&self.st).meta.get_mut(&id) {
            *p = pid;
        }
    }

    /// The processes of requests that are still waiting for their turn. A tree
    /// with one of these in it is waiting, not stuck.
    pub fn waiting_pids(&self) -> std::collections::HashSet<u32> {
        let st = lock(&self.st);
        st.meta
            .iter()
            .filter_map(|(id, m)| match m {
                Meta::External { pid, .. } if *pid > 0 && !st.ledger.holds(*id) => Some(*pid),
                _ => None,
            })
            .collect()
    }

    /// Where an interactive request stands. Polling is its heartbeat.
    pub fn lease_poll(&self, id: u64) -> LeaseState {
        let mut st = lock(&self.st);
        if let Some(Meta::External { last_poll, .. }) = st.meta.get_mut(&id) {
            *last_poll = Instant::now();
        }
        st = self.pass(st);
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
        let outside = matches!(
            st.meta.get(&id),
            Some(Meta::External {
                pipeline: Some(_),
                ..
            })
        );
        let snap = st.ledger.snapshot();
        LeaseState::Waiting {
            explain: waits
                .waits
                .get(&id)
                .map(|w| {
                    let first = snap
                        .waiting
                        .iter()
                        .find(|r| r.id == id)
                        .and_then(|r| r.needs.amounts.keys().next());
                    match (outside, first) {
                        (true, Some(r)) => crate::outside::outside_explain(r, w.position - 1),
                        _ => explain(w, now),
                    }
                })
                .unwrap_or_else(|| "Waiting".into()),
        }
    }

    pub fn lease_close(&self, id: u64) -> bool {
        let mut st = lock(&self.st);
        let had = st.ledger.release(id);
        st.meta.remove(&id);
        st = self.pass(st);
        drop(st);
        self.cv.notify_all();
        had
    }

    /// Everything holding `compile` now, for the idle check: the lease, the task it
    /// is for, whether it is a person's command, and the process that took the slot.
    pub fn compile_holders(&self) -> Vec<CompileHolderInfo> {
        let st = lock(&self.st);
        let snap = st.ledger.snapshot();
        let task_of: HashMap<u64, u64> = st.by_task.iter().map(|(t, id)| (*id, t.0)).collect();
        let cap = snap.capacity.get(COMPILE).map_or(0, |c| c.units);
        let pid_of: HashMap<u64, u32> = self
            .locks
            .slots(COMPILE, cap)
            .into_iter()
            .flatten()
            .filter(|h| h.lease > 0)
            .map(|h| (h.lease, h.pid))
            .collect();
        snap.holders
            .iter()
            .filter(|h| h.needs.exclusive || h.needs.amounts.contains_key(COMPILE))
            .map(|h| CompileHolderInfo {
                lease: h.id,
                label: h.label.clone(),
                task: task_of.get(&h.id).copied(),
                interactive: is_person(&st, h.id),
                pid: pid_of.get(&h.id).copied().unwrap_or(0),
            })
            .collect()
    }

    /// The pipeline tasks holding something now: how many, and how long the oldest has held
    /// it, in ms. The self-update's window (s473) looks at this; a person's `mason run`
    /// and another pipeline's work do not count.
    pub fn task_work(&self, now_ms: u64) -> (usize, u64) {
        let st = lock(&self.st);
        let snap = st.ledger.snapshot();
        let mut n = 0;
        let mut oldest = 0;
        for h in snap.holders.iter() {
            if matches!(st.meta.get(&h.id), Some(Meta::Task)) {
                n += 1;
                oldest = oldest.max(now_ms.saturating_sub(h.since));
            }
        }
        (n, oldest)
    }

    /// The `compile` count in force now.
    pub fn compile_capacity(&self) -> Option<u64> {
        let st = lock(&self.st);
        st.ledger
            .snapshot()
            .capacity
            .get(COMPILE)
            .map(|c| u64::from(c.units))
    }

    /// The idle check's latest reading, by lease.
    pub fn set_idle(&self, idle: HashMap<u64, u64>) {
        lock(&self.st).idle = idle;
    }

    pub fn compile_idle_kill_s(&self) -> u32 {
        lock(&self.st).compile_idle_kill_s
    }

    /// Something done about the queue, for the home page: the last five of the
    /// last day are kept.
    pub fn note(&self, now: u64, text: String) {
        let mut st = lock(&self.st);
        st.notes.push((now, text));
        st.notes
            .retain(|(at, _)| now.saturating_sub(*at) < 24 * 3600 * 1000);
        let n = st.notes.len();
        if n > 5 {
            st.notes.drain(..n - 5);
        }
    }

    /// The `compile` resource for the monitor's home page: who holds it, who
    /// waits and why (in line order), the limit, and the gates on it.
    pub fn compilation(&self) -> mason_protocol::Compilation {
        use mason_protocol::{Compilation, CompileHolder, CompileWaiter};
        let mut st = lock(&self.st);
        st = self.pass(st);
        let (floor, free) = (st.floor_mb, self.free_memory(&mut st));
        let now = crate::daemon::now_ms();
        let gate = self.cpu_gate(&mut st);
        let waits = st.ledger.waits_gated(now, free, floor, gate);
        let snap = st.ledger.snapshot_at(now);
        let task_of: HashMap<u64, u64> = st.by_task.iter().map(|(t, id)| (*id, t.0)).collect();
        let compiles = |n: &Needs| n.exclusive || n.amounts.contains_key(COMPILE);
        let limit = snap.capacity.get(COMPILE).map_or(0, |c| c.units);
        let running = snap
            .holders
            .iter()
            .filter(|h| compiles(&h.needs))
            .map(|h| CompileHolder {
                label: mason_scheduler::label_parts(&h.label).0,
                command: mason_scheduler::label_parts(&h.label).1,
                task: task_of.get(&h.id).copied().unwrap_or(0),
                since_ms: h.since,
                interactive: is_person(&st, h.id),
                idle_ms: st.idle.get(&h.id).copied().unwrap_or(0),
                project: String::new(),
                title: String::new(),
            })
            .collect();
        let pending = snap
            .waiting
            .iter()
            .filter(|r| compiles(&r.needs))
            .enumerate()
            .map(|(i, r)| {
                let (cause, reason) = match waits.waits.get(&r.id) {
                    Some(w) => compile_reason(w, now, limit),
                    None => ("", "starting".to_string()),
                };
                CompileWaiter {
                    label: mason_scheduler::label_parts(&r.label).0,
                    command: mason_scheduler::label_parts(&r.label).1,
                    task: task_of.get(&r.id).copied().unwrap_or(0),
                    since_ms: r.since,
                    position: i as u32 + 1,
                    reason,
                    cause: cause.into(),
                    interactive: r.rank.interactive(),
                    steps_remaining: if r.rank.interactive() {
                        0
                    } else {
                        r.rank.steps()
                    },
                    landing: r.rank.is_landing(),
                }
            })
            .collect();
        let leases = Self::wait_view_of(&mut st, &waits.waits, now).leases;
        let settle = st.compile_settle_s;
        let mut rules = format!(
            "At most {limit} at once. The first compile always starts; another starts only \
             while CPU use is below {}%",
            st.cpu_max
        );
        if settle > 0 {
            rules.push_str(&format!(", {settle} s after the last one started"));
        }
        if st.stall.is_some() && st.stall_ms > 0 {
            rules.push_str(", and not while the build disk is slow");
        }
        if st.policy.is_some() && st.policy_max > 0 {
            rules.push_str(&format!(
                ", and not while macOS's syspolicyd uses {}% CPU or more checking new programs",
                st.policy_max
            ));
        }
        if st.compile_idle_kill_s > 0 {
            rules.push_str(&format!(
                ". A task's command that uses no CPU for {} while holding a slot is stopped",
                since(st.compile_idle_kill_s as u64 * 1000, 0)
            ));
        }
        rules.push_str(if st.priority_first {
            ". A landing starts whatever the CPU reads. Line order: a person's command \
             first, then a landing, then a task that has waited past order.max_wait_s (oldest first), then the most urgent task, then the one fewest steps \
             from landing, then the oldest."
        } else {
            ". A landing starts whatever the CPU reads. Line order: a person's command \
             first, then a landing, then a task that has waited past order.max_wait_s (oldest first), then the task fewest steps from landing, then \
             priority, then the oldest."
        });
        Compilation {
            limit,
            used: snap.used.get(COMPILE).copied().unwrap_or(0),
            running,
            pending,
            cpu_busy_pct: st.cpu_busy.map_or(-1, |b| b as i32),
            cpu_max_pct: st.cpu_max,
            settle_s: settle,
            rules,
            cpu_source: st.cpu.source().to_string(),
            syspolicyd_pct: st.policy_read.map_or(-1, |(b, _)| b as i32),
            syspolicyd_max_pct: st.policy_read.map_or(0, |(_, m)| m),
            notes: st
                .notes
                .iter()
                .rev()
                .map(|(at, t)| format!("{} ago: {t}", since(now, *at)))
                .collect(),
            leases,
        }
    }

    /// Everything, for `mason resources` and the monitors.
    pub fn snapshot(&self) -> serde_json::Value {
        use serde_json::json;
        let mut st = lock(&self.st);
        st = self.pass(st);
        let (floor, free) = (st.floor_mb, self.free_memory(&mut st));
        let now = crate::daemon::now_ms();
        let gate = self.cpu_gate(&mut st);
        let waits = st.ledger.waits_gated(now, free, floor, gate);
        let snap = st.ledger.snapshot_at(now);
        let task_of: HashMap<u64, u64> = st
            .by_task
            .iter()
            .map(|(t, id)| (*id, t.0))
            .chain(st.lease_tasks.iter().map(|(id, t)| (*id, t.0)))
            .collect();
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
                let (label, command) = mason_scheduler::label_parts(&h.label);
                json!({
                    "label": label,
                    "command": command,
                    "pipeline": pipeline_of(&st, h.id),
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
                let pipeline = pipeline_of(&st, r.id);
                let explain = match (&pipeline, waits.waits.get(&r.id)) {
                    (Some(_), Some(w)) => Some(match r.needs.amounts.keys().next() {
                        Some(res) => crate::outside::outside_explain(res, w.position - 1),
                        None => explain(w, now),
                    }),
                    (_, w) => w.map(|w| explain(w, now)),
                };
                let (label, command) = mason_scheduler::label_parts(&r.label);
                json!({
                    "label": label,
                    "command": command,
                    "pipeline": pipeline,
                    "since_ms": r.since,
                    "interactive": r.rank.interactive(),
                    "steps_remaining": r.rank.steps(),
                    "task": task_of.get(&r.id),
                    "priority": (!r.rank.interactive()).then(|| r.rank.priority()),
                    "aged": r.rank.is_aged() || waits.waits.get(&r.id).is_some_and(|w| w.aged_ms.is_some()),
                    "uses": r.needs.amounts,
                    "exclusive": r.needs.exclusive,
                    "explain": explain,
                })
            })
            .collect();
        let gates = self.gates_locked(&st);
        json!({
            "resources": resources,
            "holders": holders,
            "waiting": waiting,
            "memory": { "floor_mb": st.floor_mb, "free_mb": free },
            "gates": gates,
            "cpu": {
                "busy_pct": st.cpu_busy,
                "compile_max_pct": st.cpu_max,
                "source": st.cpu.source(),
                "failed_reads": st.cpu.failed_reads(),
                "syspolicyd_pct": st.policy_read.map(|(b, _)| b),
                "syspolicyd_max_pct": st.policy_read.map(|(_, m)| m),
            },
            "now_ms": now,
        })
    }
}

/// Why a compile waits, as a code and a short phrase for the home page.
fn compile_reason(w: &mason_scheduler::Wait, now: u64, limit: u32) -> (&'static str, String) {
    match &w.cause {
        WaitCause::Full { .. } => ("full", format!("every slot is held ({limit} of {limit})")),
        WaitCause::Behind { .. } => (
            "behind",
            match w.behind.first() {
                Some(a) => {
                    // "behind " + " in line" take 15 of the 50 characters a reason may use.
                    let d = mason_scheduler::label_parts(&a.label).0;
                    let d = if d.chars().count() > 35 {
                        let cut: String = d.chars().take(34).collect();
                        format!("{}…", cut.trim_end())
                    } else {
                        d
                    };
                    format!("behind {d} in line")
                }
                None => "behind an earlier request".into(),
            },
        ),
        WaitCause::Exclusive => ("exclusive", "something needs the whole machine".into()),
        WaitCause::Cpu { busy_pct, max_pct } => (
            "cpu",
            format!("CPU {busy_pct}% busy; another compile starts below {max_pct}%"),
        ),
        WaitCause::Settling { until } => (
            "settling",
            format!(
                "the last compile started moments ago; the next may start in {} s",
                until.saturating_sub(now).div_ceil(1000)
            ),
        ),
        WaitCause::DiskStall { slow_ms, max_ms } => (
            "disk-stall",
            format!(
                "the build disk is slow ({:.1} s to answer, limit {:.1} s)",
                *slow_ms as f64 / 1000.0,
                *max_ms as f64 / 1000.0
            ),
        ),
        WaitCause::Policy { busy_pct, max_pct } => (
            "syspolicyd",
            format!(
                "macOS's syspolicyd is at {busy_pct}% CPU checking new programs (limit {max_pct}%)"
            ),
        ),
        WaitCause::Disk { free_mb, floor_mb } => (
            "disk",
            format!(
                "only {} GB of disk free (floor {} GB)",
                free_mb / 1024,
                floor_mb / 1024
            ),
        ),
        WaitCause::Memory { free_mb, floor_mb } => (
            "memory",
            format!("{free_mb} MB memory free, {floor_mb} MB needed"),
        ),
        WaitCause::TooBig { .. } => ("too-big", explain(w, now)),
        WaitCause::Unknown { .. } => ("unknown", explain(w, now)),
    }
}

/// The machine's ordering rule applied to a pipeline rank.
fn pipeline_of(st: &State, id: u64) -> Option<String> {
    match st.meta.get(&id) {
        Some(Meta::External { pipeline, .. }) => pipeline.clone(),
        _ => None,
    }
}

/// A person's `mason run` lease: external, and not another pipeline's.
fn is_person(st: &State, id: u64) -> bool {
    matches!(
        st.meta.get(&id),
        Some(Meta::External { pipeline: None, .. })
    )
}

fn order(st: &State, rank: Rank) -> Rank {
    if st.priority_first {
        rank.priority_first()
    } else {
        rank
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
    free_memory_mb_here()
}

#[cfg(windows)]
fn free_memory_mb_here() -> Option<u64> {
    windows_free_memory_mb()
}

#[cfg(not(windows))]
fn free_memory_mb_here() -> Option<u64> {
    unix_free_memory_mb()
}

/// `GlobalMemoryStatusEx`: available physical memory.
#[cfg(windows)]
#[allow(unsafe_code)]
fn windows_free_memory_mb() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: an all-zero MEMORYSTATUSEX is valid; dwLength is set as the API requires.
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: `m` is live and sized as dwLength says.
    if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 {
        return None;
    }
    Some(crate::cpu::windows_free_mb(m.ullAvailPhys))
}

#[cfg(not(windows))]
fn unix_free_memory_mb() -> Option<u64> {
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

/// Test helper: a broker with capacity 1 and `n` tasks (ids 1..=n) queued for it.
#[cfg(test)]
pub(crate) fn broker_with_waiting(dir: &Path, n: u64) -> Arc<Broker> {
    let cfg = ResourceCfg {
        machine: BTreeMap::from([("compile".to_string(), 1)]),
        ..Default::default()
    };
    let b = Broker::new(
        dir.join("locks"),
        dir.join("limits.kv"),
        &cfg,
        2,
        crate::cpu::FakeCpu::new(0),
        &[],
    )
    .unwrap();
    let one = BTreeMap::from([("compile".to_string(), 1)]);
    {
        let mut st = lock(&b.st);
        for t in 1..=n {
            st.rank_seq += 1;
            let rank = order(&st, Rank::new(false, 3, 0, t, st.rank_seq, false));
            let id = st.ledger.submit(rank, needs_of(&one, None), "t", t);
            st.meta.insert(id, Meta::Task);
            st.by_task.insert(TaskId(t), id);
        }
        drop(b.pass(st));
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_waiters_pass_is_due_only_when_none_ran_lately() {
        assert!(pass_due(None));
        assert!(!pass_due(Some(Instant::now() + Duration::from_secs(60))));
        if let Some(old) = Instant::now().checked_sub(PASS_EVERY * 2) {
            assert!(pass_due(Some(old)));
        }
    }

    #[test]
    fn waiting_jobs_share_one_pass_and_all_get_their_turn() {
        // On 2026-11-13 forty waiters each ran the whole pass ten times a second, so
        // the broker lock was almost never free and the home view waited 20 s for it.
        // Measured against the waiters' own liveness checks (each makes one per four
        // or more wake-ups), not against the clock: the old shape ran at least four
        // passes per check, the shared pass far fewer than one.
        use std::sync::atomic::{AtomicU64, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let cfg = ResourceCfg {
            machine: BTreeMap::from([("compile".to_string(), 1)]),
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
        let rank = |t: u64| Rank::new(false, 1, 2, t, t, false);
        let first = b
            .acquire(
                TaskId(1),
                "#1",
                rank(1),
                Needs::of(&[("compile", 1)]),
                &|| true,
                &mut |_| {},
            )
            .unwrap();
        let checks = Arc::new(AtomicU64::new(0));
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let waiters: Vec<_> = (2..=31u64)
            .map(|t| {
                let (b, checks, tx) = (b.clone(), checks.clone(), tx.clone());
                std::thread::spawn(move || {
                    let alive = || {
                        checks.fetch_add(1, Ordering::SeqCst);
                        true
                    };
                    let lease = b.acquire(
                        TaskId(t),
                        &format!("#{t}"),
                        rank(t),
                        Needs::of(&[("compile", 1)]),
                        &alive,
                        &mut |_| {
                            let _ = tx.send(());
                        },
                    );
                    lease.is_ok()
                })
            })
            .collect();
        for _ in 0..30 {
            rx.recv_timeout(Duration::from_secs(60))
                .expect("every waiter says it waits");
        }
        let (p0, c0) = (lock(&b.st).passes, checks.load(Ordering::SeqCst));
        // Bounded by attempts, not asserted on time.
        for _ in 0..12_000 {
            if checks.load(Ordering::SeqCst) - c0 >= 120 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let (p1, c1) = (lock(&b.st).passes, checks.load(Ordering::SeqCst));
        assert!(c1 - c0 >= 120, "the waiters kept checking ({})", c1 - c0);
        assert!(
            p1 - p0 < c1 - c0,
            "{} passes for {} liveness checks: each waiter is running its own pass",
            p1 - p0,
            c1 - c0
        );
        // Every waiter still gets the slot in turn once it is released.
        drop(first);
        for w in waiters {
            assert!(w.join().unwrap(), "a waiter was never granted");
        }
    }

    #[test]
    fn explain_task_reads_the_snapshot_without_the_broker_lock() {
        let dir = tempfile::tempdir().unwrap();
        let b = broker_with_waiting(dir.path(), 20);
        let snap = b.waits_snapshot();
        // One holder, nineteen waiting.
        assert_eq!(snap.len(), 19);
        let st = lock(&b.st);
        let (tx, rx) = std::sync::mpsc::channel();
        let b2 = b.clone();
        let h = std::thread::spawn(move || {
            let all: Vec<_> = (1..=20).map(|t| b2.explain_task(TaskId(t))).collect();
            tx.send(all).unwrap();
        });
        let got = rx.recv_timeout(Duration::from_secs(30));
        drop(st);
        h.join().unwrap();
        let got = got.expect("explain_task blocked on the broker lock");
        assert_eq!(got.iter().flatten().count(), 19);
        for (t, g) in (1..=20u64).zip(&got) {
            assert_eq!(g.as_ref(), snap.get(&TaskId(t)));
        }
        // The text is what the ledger says.
        let mut st = lock(&b.st);
        let now = crate::daemon::now_ms();
        let w = st.ledger.waits_gated(now, None, None, None);
        let (id, wait) = w.waits.iter().next().unwrap();
        let task = *st.by_task.iter().find(|(_, i)| *i == id).unwrap().0;
        assert_eq!(snap.get(&task).unwrap(), &explain(wait, now));
        let _ = &mut st;
    }

    /// A capacity-1 broker with one holder and `n` waiters in `acquire`, waiter `i`
    /// (task i+2) with `i + 1` steps remaining. Returns the broker, the holder's
    /// lease and a channel that gets each waiter's task as it is granted.
    fn full_broker_with_waiters(
        dir: &Path,
        n: u64,
    ) -> (
        Arc<Broker>,
        Lease,
        std::sync::mpsc::Receiver<u64>,
        Vec<std::thread::JoinHandle<()>>,
    ) {
        let cfg = ResourceCfg {
            machine: BTreeMap::from([("compile".to_string(), 1)]),
            ..Default::default()
        };
        let b = Broker::new(
            dir.join("locks"),
            dir.join("limits.kv"),
            &cfg,
            2,
            crate::cpu::FakeCpu::new(0),
            &[],
        )
        .unwrap();
        b.set_tick_for_test(Duration::from_secs(3600));
        let held = b
            .acquire(
                TaskId(1),
                "#1",
                Rank::new(false, 1, 2, 1, 1, false),
                Needs::of(&[("compile", 1)]),
                &|| true,
                &mut |_| {},
            )
            .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut hs = vec![];
        for i in 0..n {
            let (b2, tx) = (b.clone(), tx.clone());
            hs.push(std::thread::spawn(move || {
                let t = i + 2;
                let l = b2.acquire(
                    TaskId(t),
                    "w",
                    Rank::new(false, i as u32 + 1, 2, t, t, false),
                    Needs::of(&[("compile", 1)]),
                    &|| true,
                    &mut |_| {},
                );
                let _l = l.unwrap();
                let _ = tx.send(t);
            }));
        }
        // Everyone is queued once the ledger shows them.
        for _ in 0..6000 {
            if b.waits_snapshot().len() as u64 == n {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(b.waits_snapshot().len() as u64, n);
        (b, held, rx, hs)
    }

    #[test]
    fn one_pass_serves_every_waiter_per_tick() {
        let dir = tempfile::tempdir().unwrap();
        let (b, held, rx, hs) = full_broker_with_waiters(dir.path(), 30);
        let base = b.pass_count.load(std::sync::atomic::Ordering::SeqCst);
        let ticks = 5;
        for k in 1..=ticks {
            b.expire_pass_for_test();
            // Wait (bounded; only to report failure) until this tick's pass ran.
            let mut ran = false;
            for _ in 0..6000 {
                if b.pass_count.load(std::sync::atomic::Ordering::SeqCst) > base + k - 1 {
                    ran = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(ran, "tick {k} ran no pass");
        }
        let n = b.pass_count.load(std::sync::atomic::Ordering::SeqCst) - base;
        assert!(n <= ticks, "{n} passes for {ticks} ticks with 30 waiters");
        // The first waiter in rank order is granted on release.
        drop(held);
        assert_eq!(rx.recv_timeout(Duration::from_secs(60)).unwrap(), 2);
        for want in 3..=31 {
            assert_eq!(rx.recv_timeout(Duration::from_secs(60)).unwrap(), want);
        }
        for h in hs {
            h.join().unwrap();
        }
    }

    #[test]
    fn readers_and_release_do_not_wait_for_a_blocked_lock_file_probe() {
        let dir = tempfile::tempdir().unwrap();
        let (b, held, rx, hs) = full_broker_with_waiters(dir.path(), 30);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let (g, e) = (gate.clone(), entered.clone());
            *b.probe_hook.lock().unwrap() = Some(Arc::new(move || {
                e.store(true, std::sync::atomic::Ordering::SeqCst);
                let (m, cv) = &*g;
                let mut open = m.lock().unwrap();
                while !*open {
                    open = cv.wait(open).unwrap();
                }
            }));
        }
        b.expire_pass_for_test();
        for _ in 0..6000 {
            if entered.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(entered.load(std::sync::atomic::Ordering::SeqCst));
        // A probe is blocked now. Readers and the release must still complete.
        let (tx, done) = std::sync::mpsc::channel();
        let b2 = b.clone();
        let reader = std::thread::spawn(move || {
            let _ = b2.explain_task(TaskId(5));
            let _ = b2.used("compile");
            let _ = tx.send(());
        });
        let ok = done.recv_timeout(Duration::from_secs(60)).is_ok();
        // Release on this thread with the probe still blocked, in a helper so a
        // failure to return cannot hang the test forever.
        let (tx2, rel) = std::sync::mpsc::channel();
        let releaser = std::thread::spawn(move || {
            drop(held);
            let _ = tx2.send(());
        });
        let rel_ok = rel.recv_timeout(Duration::from_secs(60)).is_ok();
        // Let the probes go (the hook stays open for every later pass).
        {
            let (m, cv) = &*gate;
            *m.lock().unwrap() = true;
            cv.notify_all();
        }
        assert!(ok, "explain_task/used blocked behind a lock-file probe");
        assert!(rel_ok, "release blocked behind a lock-file probe");
        reader.join().unwrap();
        releaser.join().unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(60)).unwrap(), 2);
        for want in 3..=31 {
            assert_eq!(rx.recv_timeout(Duration::from_secs(60)).unwrap(), want);
        }
        for h in hs {
            h.join().unwrap();
        }
    }

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

    fn waiting_broker(dir: &std::path::Path) -> (Arc<Broker>, u64, u64) {
        let cfg = ResourceCfg {
            machine: BTreeMap::from([("compile".to_string(), 1)]),
            ..Default::default()
        };
        let b = Broker::new(
            dir.join("locks"),
            dir.join("limits.kv"),
            &cfg,
            2,
            crate::cpu::FakeCpu::new(0),
            &[],
        )
        .unwrap();
        let one = BTreeMap::from([("compile".to_string(), 1)]);
        let first = b.lease_open("first", &one);
        assert!(matches!(b.lease_poll(first), LeaseState::Granted { .. }));
        let second = b.lease_open("second", &one);
        assert!(matches!(b.lease_poll(second), LeaseState::Waiting { .. }));
        (b, first, second)
    }

    fn age_poll(b: &Broker, id: u64, by: Duration) {
        if let Some(Meta::External { last_poll, .. }) = lock(&b.st).meta.get_mut(&id) {
            *last_poll = Instant::now().checked_sub(by).expect("uptime");
        }
    }

    #[test]
    fn an_agents_mason_run_wait_is_added_to_its_deadline_when_granted() {
        // 2026-11-14: star #242's develop agent ran out of its 4 h budget while its own
        // `mason run` jobs (a release build, a 19-frame run needing the whole machine) waited
        // in the queue; only a `run_check`'s wait was added to the deadline (s363).
        let dir = tempfile::tempdir().unwrap();
        let (b, first, second) = waiting_broker(dir.path());
        b.lease_set_task(second, TaskId(242));
        let extra = mason_adapter_api::DeadlineExtra::default();
        let other = mason_adapter_api::DeadlineExtra::default();
        b.credit_waits_to(TaskId(242), extra.clone());
        b.credit_waits_to(TaskId(7), other.clone());
        if let Some(Meta::External {
            opened, credited, ..
        }) = lock(&b.st).meta.get_mut(&second)
        {
            *opened = Instant::now()
                .checked_sub(Duration::from_secs(600))
                .expect("uptime");
            *credited = *opened;
        }
        b.lease_close(first);
        assert!(matches!(b.lease_poll(second), LeaseState::Granted { .. }));
        assert!(extra.get() >= Duration::from_secs(600), "{:?}", extra.get());
        assert_eq!(
            other.get(),
            Duration::ZERO,
            "another task's deadline is untouched"
        );
        // Once the agent step ends, nothing more is added.
        b.stop_crediting_waits(TaskId(242));
        let before = extra.get();
        b.lease_close(second);
        let third = b.lease_open("third", &BTreeMap::from([("compile".to_string(), 1)]));
        b.lease_set_task(third, TaskId(242));
        assert!(matches!(b.lease_poll(third), LeaseState::Granted { .. }));
        assert_eq!(extra.get(), before);
    }

    #[test]
    fn an_open_mason_run_wait_is_added_to_the_deadline_before_it_is_granted() {
        // 2026-11-15: a daemon takeover dropped every open wait's credit, and 29 develop
        // agents that had waited up to 2 h for the machine timed out on re-attach (s417).
        let dir = tempfile::tempdir().unwrap();
        let (b, _first, second) = waiting_broker(dir.path());
        b.lease_set_task(second, TaskId(345));
        let extra = mason_adapter_api::DeadlineExtra::default();
        b.credit_waits_to(TaskId(345), extra.clone());
        if let Some(Meta::External { credited, .. }) = lock(&b.st).meta.get_mut(&second) {
            *credited = Instant::now()
                .checked_sub(Duration::from_secs(300))
                .expect("uptime");
        }
        assert!(!matches!(b.lease_poll(second), LeaseState::Granted { .. }));
        let after = extra.get();
        assert!(after >= Duration::from_secs(300), "{after:?}");
        // Credited once: another pass right away adds nothing more of it.
        assert!(!matches!(b.lease_poll(second), LeaseState::Granted { .. }));
        assert!(extra.get() < after + CREDIT_EVERY, "{:?}", extra.get());
    }

    #[test]
    fn a_waiter_with_a_live_pid_survives_a_long_gap_between_polls() {
        let dir = tempfile::tempdir().unwrap();
        let (b, _first, second) = waiting_broker(dir.path());
        b.lease_set_pid(second, std::process::id());
        age_poll(&b, second, Duration::from_secs(600));
        assert!(matches!(b.lease_poll(second), LeaseState::Waiting { .. }));
    }

    #[test]
    fn a_waiter_whose_pid_has_exited_is_reaped_at_the_next_pass() {
        let dir = tempfile::tempdir().unwrap();
        let (b, _first, second) = waiting_broker(dir.path());
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        b.lease_set_pid(second, pid);
        assert!(matches!(b.lease_poll(second), LeaseState::Gone));
    }

    #[test]
    fn a_waiter_without_a_pid_is_reaped_only_after_the_long_silence() {
        let dir = tempfile::tempdir().unwrap();
        let (b, _first, second) = waiting_broker(dir.path());
        age_poll(&b, second, Duration::from_secs(60));
        {
            drop(b.pass(lock(&b.st)));
        }
        assert!(lock(&b.st).meta.contains_key(&second), "60 s is not enough");
        age_poll(&b, second, UNPOLLED_WAITER_REAP + Duration::from_secs(1));
        {
            drop(b.pass(lock(&b.st)));
        }
        assert!(!lock(&b.st).meta.contains_key(&second));
        assert!(matches!(b.lease_poll(second), LeaseState::Gone));
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
        // The pass that grants `first` changes the holder set while the lock files
        // are read, so it leaves the adoption of the outside holder to the next
        // pass. Poll (each poll runs a pass) until the ledger counts it: wait on
        // the state, bounded by attempts, not by time.
        for _ in 0..2000 {
            if b.used("compile") == 2 {
                break;
            }
            assert!(matches!(b.lease_poll(first), LeaseState::Granted { .. }));
            std::thread::yield_now();
        }
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
        // The kernel frees a flock only when every fd copy is closed, and a child
        // forked by a parallel test can hold one briefly. So the release is not
        // synchronous with the drop: retry, bounded by attempts, not by time.
        drop(held);
        let mut granted = false;
        for _ in 0..2000 {
            if matches!(b.lease_poll(second), LeaseState::Granted { .. }) {
                granted = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(granted, "{}", b.snapshot());
    }

    /// A step host dir whose "host process" is this test process, so it reads as running.
    fn fake_host(root: &Path, id: &str, label: &str, uses: &str) -> PathBuf {
        use mason_step_host::registry::write_kv;
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        write_kv(
            &dir.join("host.kv"),
            &[
                ("id", id.to_string()),
                ("host_pid", std::process::id().to_string()),
                ("child_pid", std::process::id().to_string()),
                ("sock", dir.join("sock").display().to_string()),
                ("started_ms", "1".to_string()),
                ("heartbeat_ms", "1".to_string()),
            ],
        )
        .unwrap();
        write_kv(
            &dir.join("holds.kv"),
            &[("label", label.to_string()), ("uses", uses.to_string())],
        )
        .unwrap();
        dir
    }

    #[test]
    fn a_command_still_running_under_its_host_keeps_its_slot_across_a_restart() {
        // s589: the old daemon took the kernel locks, so they went with it. The new daemon
        // counts the host's recorded slots until the host's command ends.
        let dir = tempfile::tempdir().unwrap();
        let cfg = ResourceCfg {
            machine: BTreeMap::from([("test-suite".to_string(), 1)]),
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
        let hosts = dir.path().join("h");
        let hd = fake_host(&hosts, "t567r1-check0", "#567 check", "test-suite=1");
        let found = crate::hostedcheck::running_holds(&hosts);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label, "#567 check");
        b.adopt_hosts(&found);
        assert_eq!(b.used("test-suite"), 1);
        assert!(
            b.snapshot()
                .to_string()
                .contains("#567 check (from before the restart)"),
            "{}",
            b.snapshot()
        );
        let one = BTreeMap::from([("test-suite".to_string(), 1)]);
        let second = b.lease_open("second", &one);
        for _ in 0..50 {
            assert!(
                matches!(b.lease_poll(second), LeaseState::Waiting { .. }),
                "the limit is 1 and the first command still runs: {}",
                b.snapshot()
            );
            assert_eq!(b.used("test-suite"), 1);
        }
        // The command ends: its slot is given back and the second request is granted.
        mason_step_host::registry::ExitRecord {
            code: 0,
            signal: 0,
            output_len: 0,
            ended_ms: 2,
        }
        .write(&hd)
        .unwrap();
        let mut granted = false;
        for _ in 0..2000 {
            if matches!(b.lease_poll(second), LeaseState::Granted { .. }) {
                granted = true;
                break;
            }
            std::thread::yield_now();
        }
        assert!(granted, "{}", b.snapshot());
        assert_eq!(b.used("test-suite"), 1, "only the second holds it now");
    }

    #[test]
    fn an_ended_host_or_an_unknown_class_is_not_adopted() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ResourceCfg {
            machine: BTreeMap::from([("test-suite".to_string(), 1)]),
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
        let hosts = dir.path().join("h");
        let done = fake_host(&hosts, "a-check0", "#1 check", "test-suite=1");
        mason_step_host::registry::ExitRecord {
            code: 0,
            signal: 0,
            output_len: 0,
            ended_ms: 2,
        }
        .write(&done)
        .unwrap();
        fake_host(&hosts, "b-check0", "#2 check", "nonsense=1");
        let found = crate::hostedcheck::running_holds(&hosts);
        assert_eq!(found.len(), 1, "the ended host holds nothing");
        b.adopt_hosts(&found);
        assert_eq!(b.used("test-suite"), 0);
        assert_eq!(b.used("nonsense"), 0);
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

    /// A task's step asking for `uses`, queued directly (as the pipeline does).
    fn queue_task(b: &Broker, t: u64, uses: &BTreeMap<String, u32>) -> u64 {
        let mut st = lock(&b.st);
        st.rank_seq += 1;
        let rank = order(&st, Rank::new(false, 3, 0, t, st.rank_seq, false));
        let id = st
            .ledger
            .submit(rank, needs_of(uses, None), &format!("#{t}"), t);
        st.meta.insert(id, Meta::Task);
        st.by_task.insert(TaskId(t), id);
        drop(b.pass(st));
        id
    }

    fn compile_broker(dir: &Path, units: u32) -> Arc<Broker> {
        compile_broker_at(dir, units, 0)
    }

    fn compile_broker_at(dir: &Path, units: u32, cpu_pct: u32) -> Arc<Broker> {
        let cfg = ResourceCfg {
            machine: BTreeMap::from([("compile".to_string(), units)]),
            ..Default::default()
        };
        Broker::new(
            dir.join("locks"),
            dir.join("limits.kv"),
            &cfg,
            2,
            crate::cpu::FakeCpu::new(cpu_pct),
            &[],
        )
        .unwrap()
    }

    #[test]
    fn ten_compiles_behind_a_waiting_exclusive_lease_all_name_it() {
        // 2026-11-16: an exclusive `mason run` with no task waited for #510's compile slot, and
        // every task behind it read "waiting for compile slot" with slots free.
        let dir = tempfile::tempdir().unwrap();
        let b = compile_broker(dir.path(), 6);
        let one = BTreeMap::from([("compile".to_string(), 1)]);
        let holder = queue_task(&b, 510, &one);
        let excl = b.lease_open(
            "interactive `starpipe heavy`",
            &BTreeMap::from([("exclusive".to_string(), 1)]),
        );
        b.lease_set_origin(excl, "starpipe in /work/star");
        assert!(matches!(b.lease_poll(excl), LeaseState::Waiting { .. }));
        for t in 1..=10 {
            queue_task(&b, t, &one);
        }
        let v = b.wait_view();
        assert_eq!(v.tasks.len(), 10, "{:?}", v.tasks.keys());
        for (t, w) in &v.tasks {
            assert_eq!(w.cause, "behind-exclusive", "#{t}: {}", w.sentence);
            let blocker = w.blocker.as_ref().expect("names the lease");
            assert!(blocker.lease && blocker.exclusive && !blocker.holding);
            assert_eq!(blocker.label, "starpipe heavy");
            assert_eq!(blocker.origin, "starpipe in /work/star");
            assert!(w.sentence.contains("exclusive request"), "{}", w.sentence);
            assert!(w.sentence.contains("starpipe heavy"), "{}", w.sentence);
            assert_eq!(w.holders.len(), 1);
            assert_eq!(w.holders[0].task, 510);
            assert_eq!(w.chain[0].party.as_ref().unwrap().label, "starpipe heavy");
            // The chain goes on: the lease waits for #510, which is running.
            assert_eq!(w.chain[1].party.as_ref().unwrap().task, 510);
            assert!(w.chain[1].sentence.is_empty());
        }
        let hold = v.hold.clone().expect("one queue-blocked summary");
        assert_eq!(hold.kind, "exclusive-request");
        assert_eq!(hold.waiting, 10);
        assert_eq!(hold.party.as_ref().unwrap().label, "starpipe heavy");
        assert_eq!(hold.holders[0].task, 510);
        assert!(hold.summary.contains("starpipe heavy"), "{}", hold.summary);
        assert!(
            hold.summary.contains("#510's compile slot"),
            "{}",
            hold.summary
        );
        assert!(
            hold.summary.contains("10 tasks wait behind it"),
            "{}",
            hold.summary
        );
        // The lease belongs to no task, and is listed with where it came from.
        assert_eq!(v.leases.len(), 1);
        assert_eq!(v.leases[0].needs, "exclusive");
        assert!(v.leases[0].lease && v.leases[0].origin.starts_with("starpipe"));

        // The holder goes: no row, chain or summary names it on the next snapshot.
        b.release(holder, Some(TaskId(510)));
        let v = b.wait_view();
        let shown = format!("{:?}", v.tasks);
        assert!(!shown.contains("510"), "{shown}");
        assert!(!v.hold.as_ref().is_some_and(|h| h.summary.contains("#510")));
    }

    #[test]
    fn a_gate_holding_most_waiting_work_is_the_queue_summary() {
        let dir = tempfile::tempdir().unwrap();
        let b = compile_broker_at(dir.path(), 6, 90);
        let one = BTreeMap::from([("compile".to_string(), 1)]);
        // The first compile always starts; the CPU gate holds the ones after it.
        queue_task(&b, 500, &one);
        for t in 1..=5 {
            queue_task(&b, t, &one);
        }
        let v = b.wait_view();
        let hold = v.hold.clone().expect("the CPU gate holds the queue");
        assert_eq!(hold.kind, "gate");
        assert_eq!(hold.cause, "cpu");
        assert!(hold.summary.contains("CPU at 90%"), "{}", hold.summary);
        for w in v.tasks.values() {
            assert_eq!(w.cause, "cpu");
        }
    }
}
