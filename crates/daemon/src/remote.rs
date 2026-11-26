// SPDX-License-Identifier: GPL-3.0-or-later

//! A remote executor: run a suite or a check on GitHub Actions instead of on this machine and
//! hand back what a local run would (#697).
//!
//! Only *where* a command runs changes. The caller gets the exit code, the combined output and
//! the results file back exactly as a local step host would give them, so parsing, verdicts,
//! evidence, reruns and the rest are the same code. The pieces:
//!
//! - [`executor_for`]: the one function that says where a run goes. It answers `Local` for every
//!   run (#698 replaces it with the real decision), unless a test forces a project remote.
//! - [`snapshot`]: a commit of exactly what a local run would see (the worktree's uncommitted and
//!   untracked files included), built with a temporary index so the worktree, its index and its
//!   `HEAD` are untouched. **Trust overlay:** the workflow file in it is the base branch's copy,
//!   every other file under `.github/workflows/` is removed, and `.mason/run.json` is written
//!   from the pinned definition. The runner is told nothing about how it is judged by the task's
//!   own tree.
//! - [`execute`]: push the snapshot to `refs/heads/mason/run/<project>/<id>`, find the run it
//!   started by head SHA, poll it (ETag and backoff, [`Poller`]), download the artifact, delete
//!   the ref. A record next to the log lets a daemon that restarts collect the same run.
//!
//! The artifact is read with the system `unzip` (macOS and the Linux runners have it): no crate
//! is added and `deny.toml` stays as it is. Only three named members are extracted, and only
//! if they are regular files, because the zip is the runner's output and so untrusted.

use crate::config::{ProjectCfg, RemoteCfg};
use crate::daemon::{now_ms, Daemon};
use mason_adapter_api::github_budget::{self, Priority};
use mason_adapter_api::http::{Curl, Http};
use mason_adapter_api::service::{ErrorKind, Observed, PollEvent, Poller};
use mason_adapters::github::{ActionRun, GitHub, TokenRef};
use mason_landing::git;
use mason_step_host::registry::{read_kv, write_kv};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The artifact a run uploads.
pub const ARTIFACT: &str = "mason-run";
/// The workflow's job that runs the command (the one a runner has to pick up).
pub const RUN_JOB: &str = "run";

// ---- where a run goes -------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Executor {
    /// Under a step host on this machine: today's code, unchanged.
    Local,
    /// On GitHub Actions, on a runner of this kind.
    GitHub { runs_on: String },
    /// On another Mac running `masond` as a node: `nid` is its stable id, `name` what the owner
    /// calls it. Nothing produces it until placement can (the multi-machine plan, phase 2).
    Node { nid: String, name: String },
}

impl Executor {
    /// Does the run leave this machine, so that it needs none of this Mac's `compile` or
    /// `test-suite` slots and no CPU or heat gate?
    pub fn is_off_machine(&self) -> bool {
        !matches!(self, Executor::Local)
    }

    /// The executor a placement decision names.
    pub fn of_place(p: crate::place::Place) -> Executor {
        match p {
            crate::place::Place::GitHub { runs_on, .. } => Executor::GitHub { runs_on },
            crate::place::Place::Node { nid, name, .. } => Executor::Node { nid, name },
            crate::place::Place::Local { .. } => Executor::Local,
        }
    }
}

/// What is being run, for the decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Suite,
    Check,
}

/// Where this run goes: [`crate::place::decide`] for a suite (`place` names the suite and, for a
/// task's own run, the task). A run no task owns (a landing batch or its bisect halves, a base
/// run, a debt) has `None` for the task and follows the global switch and the suite's own rules:
/// a batch spans tasks, so no one task's choice can decide it (#702). Without a suite it stays
/// here. A test can force a project remote (`MASON_TEST_FORCE_REMOTE=1` in the daemon's own
/// environment, which a task cannot set, and only for a project that has a `[remote_runs]`).
pub fn executor_for(
    d: &Daemon,
    project: &str,
    kind: Kind,
    place: Option<(Option<mason_core::TaskId>, &mason_testing::Suite)>,
) -> Executor {
    if forced_for_tests() {
        if let Some(r) = remote_cfg(d, project) {
            return Executor::GitHub { runs_on: r.runs_on };
        }
    }
    let Some((task, suite)) = place else {
        return Executor::Local;
    };
    let kind = match kind {
        Kind::Suite => "suite",
        Kind::Check => "check",
    };
    Executor::of_place(crate::place::place_of(
        d,
        project,
        task.map(|t| t.0),
        suite,
        kind,
    ))
}

/// Where a check no task owns and no suite describes (a landing lint or re-test) goes: the global
/// switch alone, for a project with a `[remote_runs]` (#702).
pub fn executor_global(d: &Daemon, project: &str) -> Executor {
    if forced_for_tests() {
        if let Some(r) = remote_cfg(d, project) {
            return Executor::GitHub { runs_on: r.runs_on };
        }
    }
    Executor::of_place(crate::place::global_place(d, project))
}

/// Where a whole step of `task` goes (`pool_needs`): the task's mode, else the project's, else
/// the global one, with no suite's rules.
pub fn executor_task(d: &Daemon, project: &str, task: Option<u64>) -> Executor {
    if forced_for_tests() {
        if let Some(r) = remote_cfg(d, project) {
            return Executor::GitHub { runs_on: r.runs_on };
        }
    }
    Executor::of_place(crate::place::plain_place(d, project, task))
}

fn forced_for_tests() -> bool {
    std::env::var("MASON_TEST_FORCE_REMOTE").is_ok_and(|v| v == "1")
}

/// Does any project have a `[remote_runs]` section? (Cheap: lets a seam skip finding its project.)
pub fn any_remote(d: &Daemon) -> bool {
    d.projects().values().any(|p| p.services.remote.is_some())
}

/// The base branch of `project` as a revision (`origin/main`): what a snapshot is measured from.
pub fn base_ref_of(d: &Daemon, project: &str) -> Option<String> {
    let p = project_cfg(d, project)?;
    let remote = p.remote.clone().unwrap_or_else(|| "origin".into());
    Some(remote_base_ref(&p.path, &remote, &p.base))
}

fn project_cfg(d: &Daemon, project: &str) -> Option<ProjectCfg> {
    d.project_id_by_name(project)
        .and_then(|i| d.project(i))
        .cloned()
}

pub fn remote_cfg(d: &Daemon, project: &str) -> Option<RemoteCfg> {
    project_cfg(d, project).and_then(|p| p.services.remote)
}

/// The project whose repository `cwd` belongs to (a worktree of it), for a seam that is not
/// told the project.
pub fn project_of(d: &Daemon, cwd: &Path) -> Option<String> {
    let key = |p: &Path| -> Option<PathBuf> {
        let o = Command::new(git::program())
            .args(["rev-parse", "--git-common-dir"])
            .current_dir(p)
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
        let full = if Path::new(&t).is_absolute() {
            PathBuf::from(t)
        } else {
            p.join(t)
        };
        std::fs::canonicalize(&full).ok().or(Some(full))
    };
    let mine = key(cwd)?;
    d.projects()
        .values()
        .find(|p| key(&p.path).as_ref() == Some(&mine))
        .map(|p| p.id.clone())
}

// ---- what a caller sees -----------------------------------------------------------------------

/// Why a run was stopped before it ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopWhy {
    Skipped,
    Gone,
}

/// A run's result, as a step host would have given it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finished {
    pub code: i32,
    pub signal: i32,
    pub output: String,
    pub timed_out: bool,
    pub stop: Option<StopWhy>,
    /// Build plus run time on the runner, in ms.
    pub ran_ms: Option<u64>,
    /// Which part of a run with a build ended it when it did not pass: "build" or "run" (from
    /// `result.json`; `None` when the runner did not say, and for sharded runs).
    pub phase: Option<String>,
    /// "ran on GitHub (ubuntu-latest), run <url>, picked up after 6 s".
    pub ran_on: String,
    /// A sharded run's shards (#701), in shard order; `output` and `code` are their merge.
    /// Empty for a run that was not sharded.
    pub parts: Vec<Part>,
    /// Shards that got no result (no runner, or cancelled before they ran): the caller of
    /// [`execute`] runs only these again. Empty in what [`execute_sharded`] returns.
    pub missing: Vec<u32>,
}

pub use crate::shards::Part;

/// Why a run did not give a result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fail {
    /// An outage or a missing runner: a wait charged to no task. With `fallback = "local"` the
    /// run goes to the local path instead.
    Environmental(String),
    /// The run cannot be made at all (git failed on the snapshot).
    Error(String),
}

const ENV_MARK: &str = "\u{1}remote-environmental\u{1}";

/// An `Err(String)` of the local path's shape that says "environmental" (see [`environmental`]).
pub fn env_err(why: &str) -> String {
    format!("{ENV_MARK}{why}")
}

/// The reason, when an error text came from [`env_err`].
pub fn environmental(e: &str) -> Option<&str> {
    e.strip_prefix(ENV_MARK)
}

/// What a run needs. Everything here comes from MASON's pinned definition and its own state,
/// never from the task's tree.
#[derive(Clone, Debug)]
pub struct RunSpec {
    /// The step host id of the local path; also the ref's last component.
    pub id: String,
    pub project: String,
    pub suite: String,
    pub build: Option<String>,
    pub command: String,
    pub rerun: Option<String>,
    pub results_file: Option<String>,
    pub limit: Duration,
    pub env: Vec<(String, String)>,
    pub runs_on: String,
    /// The key the evidence is filed under (`hostedcheck::tree_key`), recorded beside the
    /// snapshot's SHA. Empty for a suite run, which is keyed by the tree it was given.
    pub tree_key: String,
    /// Split across this many runners (#701); 0 and 1 are one runner.
    pub shards: u32,
    /// The shards (1-based) this run makes; empty means all of them. A rerun of the shards that
    /// got no result names only those.
    pub only: Vec<u32>,
    /// Each shard's tests, indexed from 0, for the runner to take its own from `.mason/run.json`.
    pub shard_lists: Vec<Vec<String>>,
    /// What a shard's runner runs instead of `command`.
    pub shard_command: Option<String>,
    /// The lists name tests of the whole suite, not only of this narrowed run (#873): a shard
    /// that ran none of them is then no defect, so no shard is counted as listing any.
    pub partial_lists: bool,
    /// How many shards the suite asked for, when this run was split for a grant of fewer
    /// runner units ([`split_for_grant`]); 0 when it was not sized by a grant.
    pub shards_wanted: u32,
}

impl RunSpec {
    /// The runners this run is on: its shards, or one.
    pub fn runners(&self) -> u32 {
        self.shards.max(1)
    }

    /// Why this run must not start on GitHub: it was granted fewer runners than the fewest that
    /// can finish the suite inside its limit ([`min_shards`]). `None` when it may.
    pub fn too_few_runners(&self) -> Option<String> {
        let min = min_shards(self.shards_wanted);
        (self.shards_wanted > 1 && self.runners() < min).then(|| {
            format!(
                "granted {} of {} runner units; the suite needs at least {min} to finish inside its limit",
                self.runners(),
                self.shards_wanted
            )
        })
    }

    /// Did this run have fewer runners than the suite asked for?
    pub fn short_of_runners(&self) -> bool {
        self.shards_wanted > 1 && self.runners() < self.shards_wanted
    }

    fn sharded(&self) -> bool {
        self.shards > 1
    }

    /// The shards this run makes, 1-based.
    fn active(&self) -> Vec<u32> {
        if !self.sharded() {
            vec![1]
        } else if self.only.is_empty() {
            (1..=self.shards).collect()
        } else {
            self.only.clone()
        }
    }
}

/// The artifact of shard `k`: [`ARTIFACT`] when the run is not sharded.
pub fn artifact_name(sharded: bool, k: u32) -> String {
    if sharded {
        format!("{ARTIFACT}-{k}")
    } else {
        ARTIFACT.to_string()
    }
}

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
    fn sleep(&self, ms: u64);
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        now_ms()
    }
    fn sleep(&self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

/// Where and how a run is made.
pub struct Site {
    /// The worktree the command would run in: the snapshot is taken of it and pushed from it.
    pub cwd: PathBuf,
    /// The git remote (name or URL) the ref is pushed to.
    pub remote: String,
    /// The base branch as a revision (`origin/main`): the workflow file comes from it.
    pub base_ref: String,
    /// The workflow file's name under `.github/workflows/`.
    pub workflow: String,
    pub pickup_timeout: Duration,
    pub fallback_local: bool,
    /// `favor-remote` (#833): a run still unpicked after this long comes back here when
    /// `local_room` says this machine has room. `None`: it waits out `pickup_timeout`.
    pub spill_after: Option<Duration>,
    /// With `fallback_local`: a run that would come back here (at `spill_after` or when
    /// `pickup_timeout` has passed) does so only when this says there is room. Without one the
    /// run comes back at the timeout whatever this machine is doing.
    pub local_room: Option<RoomFn>,
    /// With `fallback_local`: why this machine cannot run the run at all now, whatever else it
    /// is doing (a hold is active, or a slot it needs has limit 0), or `None` when it can. A run
    /// is never sent here when this says why not: it waits on GitHub instead ([`wait_for_grant`]).
    pub local_barred: Option<RoomFn>,
    /// Splits a run again for a grant of the given number of runner units, as its caller split
    /// it the first time ([`split_for_grant`]). Without one a run cannot wait for a bigger grant.
    pub resplit: Option<ResplitFn>,
    /// `[remote_runs] test_env`, added to every run's environment.
    pub test_env: Vec<(String, String)>,
    pub gh: GitHub,
    pub clock: Arc<dyn Clock>,
    /// Seconds between looks at a run.
    pub poll_s: u64,
    /// Told what a run in progress is doing ("on GitHub: running, 4 min"), whenever that text
    /// changes. For display only.
    pub on_phase: Option<PhaseFn>,
}

/// Why this machine has no room for the run now (a hold is active, a slot it needs has no free
/// unit), or `None` when it has room. Polled by the wait loop: it must be cheap.
pub type RoomFn = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// The run made again for a grant of this many runner units.
pub type ResplitFn = Arc<dyn Fn(RunSpec, u32) -> RunSpec + Send + Sync>;

/// A run waiting for pickup for `waited_ms` comes back here (#833): the spill time is set, it
/// has passed, and `room` says this machine has room. Pure.
pub fn spill_due(waited_ms: u64, spill_after: Option<Duration>, room: bool) -> bool {
    room && spill_after.is_some_and(|a| waited_ms >= a.as_millis() as u64)
}

/// When the pickup clock starts (#1025): when GitHub reports the run queued (`created_at`), so a
/// slow push batch (or a restart that re-sent every run) does not use up the pickup timeout.
/// Only a time after the push and not in the future counts. Pure.
pub fn pickup_clock_from(pushed_ms: u64, created_ms: Option<u64>, now: u64) -> u64 {
    created_ms
        .filter(|c| *c > pushed_ms && *c <= now)
        .unwrap_or(pushed_ms)
}

/// The median of the last few pickup times (s), `None` without any. Pure.
pub fn median_pickup(v: &[u64]) -> Option<u64> {
    let mut v = v.to_vec();
    v.sort_unstable();
    (!v.is_empty()).then(|| v[v.len() / 2])
}

static RECENT_PICKUPS: Mutex<Vec<u64>> = Mutex::new(Vec::new());
const RECENT_PICKUPS_KEPT: usize = 5;

fn note_pickup(s: u64) {
    let mut v = RECENT_PICKUPS.lock().unwrap_or_else(|e| e.into_inner());
    v.push(s);
    if v.len() > RECENT_PICKUPS_KEPT {
        v.remove(0);
    }
}

/// The median pickup time of the last few runs this daemon saw picked up (what each run's
/// record keeps as `pickup_s`).
pub fn recent_pickup_s() -> Option<u64> {
    median_pickup(&RECENT_PICKUPS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// A sink for the text of a run's current phase.
pub type PhaseFn = Arc<dyn Fn(&str) + Send + Sync>;

/// What a project's `[remote_runs]` gives, for one worktree. `None`: the project has no `[remote_runs]`.
pub fn site_for(d: &Daemon, project: &str, cwd: &Path) -> Option<Site> {
    site_for_task(d, project, cwd, None)
}

/// [`site_for`] for a run of `task`, whose own `run_on` can make it `favor-remote`.
pub fn site_for_task(d: &Daemon, project: &str, cwd: &Path, task: Option<u64>) -> Option<Site> {
    let p = project_cfg(d, project)?;
    let r = p.services.remote.clone()?;
    let token = TokenRef::parse(&r.token).ok()?;
    let http: Arc<dyn Http> = Arc::new(Curl {
        timeout_s: 300,
        http2: false,
    });
    let remote = p.remote.clone().unwrap_or_else(|| "origin".into());
    let base_ref = remote_base_ref(&p.path, &remote, &p.base);
    Some(Site {
        cwd: cwd.to_path_buf(),
        remote,
        base_ref,
        workflow: r.workflow,
        pickup_timeout: Duration::from_secs(r.pickup_timeout_s),
        // `favor-remote` spills over to this machine after the pickup timeout (s812), whether the
        // task or project sets it or follows the global mode. Plain `remote` does not (#833).
        fallback_local: crate::place::task_choice(d, task)
            .unwrap_or_else(|| crate::place::project_effective(d, project).0)
            == crate::place::Mode::FavorRemote,
        spill_after: None,
        local_room: None,
        local_barred: None,
        resplit: None,
        test_env: r.test_env,
        gh: GitHub::new(http, &r.api, &r.repo, token),
        clock: Arc::new(SystemClock),
        poll_s: 10,
        on_phase: None,
    })
}

/// [`site_for_task`] with this machine's room wired in: a run that falls back to this machine
/// (`fallback_local`) does so only when [`crate::place::no_room_for`] finds none of `uses`
/// blocked here, so a run GitHub has not picked up is not sent to a machine that would hold it in
/// its own queue (2026-11-22: 16 runs fell back at once behind a heat hold). A `favor-remote` run
/// also comes back after `spill_after` instead of at the timeout, on the same condition.
/// `covered`: the caller already holds those slots for this run (a landing batch, s911), so
/// their use is not counted against it.
pub fn site_for_run(
    d: &Arc<Daemon>,
    project: &str,
    cwd: &Path,
    task: Option<u64>,
    uses: &std::collections::BTreeMap<String, u32>,
    covered: bool,
) -> Option<Site> {
    let mut site = site_for_task(d, project, cwd, task)?;
    if site.fallback_local {
        let after = project_cfg(d, project)?
            .services
            .remote
            .as_ref()?
            .spill_after_s;
        let uses = uses.clone();
        let d = d.clone();
        let project = project.to_string();
        site.spill_after = Some(Duration::from_secs(after));
        {
            let (d, uses) = (d.clone(), uses.clone());
            site.local_barred = Some(Arc::new(move || {
                crate::place::cannot_run_here(&d.broker.load_reading(now_ms()), &uses)
            }));
        }
        // `None` means room, and then this machine's lease is already taken (#1025): the run
        // gives up GitHub only with a grant in hand, so two runs cannot both take the last unit.
        // A covered run (a batch holds its slots) takes nothing.
        site.local_room = Some(Arc::new(move || {
            let here =
                match crate::place::no_room_for(&d.broker.load_reading(now_ms()), &uses, covered) {
                    Some(why) => Some(why),
                    None if covered => None,
                    None => {
                        try_take_fallback(&d, &project, task.map(mason_core::TaskId), &uses).err()
                    }
                };
            // A run that has waited too long for GitHub may go to a node instead (the caller
            // tries one before it takes this machine's slots): a node with room is room too,
            // and nothing of this machine is taken for it.
            if here.is_some() && crate::nodes::best_room(&d, &project, None).is_some() {
                return None;
            }
            here
        }));
    }
    Some(site)
}

/// The last answer of a [`RoomFn`], asked again only after [`ROOM_EVERY_MS`]: the wait loop looks
/// twice a second, and the answer reads the broker's state.
#[derive(Default)]
struct RoomCache {
    at: Option<u64>,
    why: Option<String>,
}

const ROOM_EVERY_MS: u64 = 2_000;

impl RoomCache {
    /// Why there is no room now; `None` with room, and when the site has no room function.
    fn no_room(&mut self, site: &Site, now: u64) -> Option<String> {
        let f = site.local_room.as_ref()?;
        if self
            .at
            .is_none_or(|a| now.saturating_sub(a) >= ROOM_EVERY_MS)
        {
            self.why = f();
            self.at = Some(now);
        }
        self.why.clone()
    }
}

/// `<remote>/<base>` when the repository has it, else the local branch.
fn remote_base_ref(repo: &Path, remote: &str, base: &str) -> String {
    let r = format!("{remote}/{base}");
    if git::ok(repo, &["rev-parse", "--verify", "--quiet", &r]) {
        r
    } else {
        base.to_string()
    }
}

// ---- other workflows' jobs (#766, #819) ----------------------------------------------------

/// A repository the poller reads: its API, name and token, and the workflow files that are
/// MASON's own (their runs are not counted: the pool counts them itself).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountRepo {
    pub api: String,
    pub repo: String,
    pub token: String,
    pub own: Vec<String>,
}

/// The repositories the registered projects name in `[remote_runs]`, `[ci]` (GitHub Actions) or
/// `[tracker]`, once each, with the token of the first section that names it.
pub fn account_repos(d: &Daemon) -> Vec<AccountRepo> {
    let mut out: Vec<AccountRepo> = vec![];
    let mut add = |api: &str, repo: &str, token: &str, own: Option<&str>| {
        if repo.is_empty() || token.is_empty() {
            return;
        }
        let at = out.iter().position(|r| r.repo == repo && r.api == api);
        let r = match at {
            Some(i) => &mut out[i],
            None => {
                out.push(AccountRepo {
                    api: api.to_string(),
                    repo: repo.to_string(),
                    token: token.to_string(),
                    own: vec![],
                });
                out.last_mut().unwrap()
            }
        };
        if let Some(o) = own {
            if !r.own.iter().any(|x| x == o) {
                r.own.push(o.to_string());
            }
        }
    };
    for p in d.projects().values() {
        if let Some(r) = &p.services.remote {
            add(&r.api, &r.repo, &r.token, Some(&r.workflow));
        }
        if let Some(c) = &p.services.ci {
            if let crate::config::CiKind::GithubActions {
                api, repo, token, ..
            } = &c.kind
            {
                add(api, repo, token, None);
            }
        }
        if let Some(t) = &p.services.tracker {
            if t.plugin.is_none() {
                add(&t.api, &t.repo, &t.token, None);
            }
        }
    }
    out
}

/// What one run of another workflow has on the runners.
#[derive(Clone, Debug, Default)]
struct RunJobs {
    etag: Option<String>,
    name: String,
    /// The list the run was in when its jobs were read (0 in progress, 1 queued).
    list: usize,
    /// When its jobs were read, ms.
    read_ms: u64,
    running: u32,
    queued: u32,
    macos_running: u32,
    macos_queued: u32,
}

/// One repository's state: the ETag and runs of each list (`in_progress`, `queued`), and what
/// each run has, so a 304 changes nothing and costs nothing.
#[derive(Default)]
struct RepoState {
    list_etag: [Option<String>; 2],
    lists: [Vec<(u64, String, String)>; 2],
    runs: std::collections::BTreeMap<u64, RunJobs>,
    /// Read in full at least once.
    read: bool,
    error: Option<String>,
}

/// What the account's other workflows have on GitHub's runners, running or queued, in every
/// repository given to [`AccountJobs::poll`]. Each repository's lists of runs are asked with
/// their last ETag; jobs are looked up only for runs of a list that changed, and with the run's
/// own ETag.
#[derive(Default)]
pub struct AccountJobs {
    repos: std::collections::BTreeMap<String, RepoState>,
    last: crate::sched::OtherJobs,
}

fn job_is_macos(j: &mason_adapters::github::ActionJob) -> bool {
    j.labels.iter().any(|l| l.starts_with("macos"))
}

impl AccountJobs {
    /// Look once at each repository. A repository that cannot be read keeps its last count (the
    /// result says it is stale) or, when never read, is named as not counted. `now_ms` stamps a
    /// complete read.
    pub fn poll(
        &mut self,
        sources: &[(AccountRepo, GitHub)],
        now_ms: u64,
    ) -> crate::sched::OtherJobs {
        self.repos
            .retain(|k, _| sources.iter().any(|(r, _)| &r.repo == k));
        for (r, gh) in sources {
            let st = self.repos.entry(r.repo.clone()).or_default();
            match Self::poll_repo(st, gh, &r.own, now_ms) {
                Ok(()) => {
                    st.read = true;
                    st.error = None;
                }
                Err(e) => st.error = Some(e),
            }
        }
        let mut out = crate::sched::OtherJobs::default();
        let mut by_name: std::collections::BTreeMap<String, (u32, u32)> = Default::default();
        for (repo, st) in &self.repos {
            if !st.read {
                out.not_counted.push(format!(
                    "{repo} ({})",
                    st.error.as_deref().unwrap_or("unread")
                ));
                continue;
            }
            out.stale |= st.error.is_some();
            let short = repo.rsplit('/').next().unwrap_or(repo);
            for run in st.runs.values() {
                out.gha_running += run.running;
                out.gha_queued += run.queued;
                out.macos_running += run.macos_running;
                out.macos_queued += run.macos_queued;
                let e = by_name.entry(format!("{short} {}", run.name)).or_default();
                e.0 += run.running;
                e.1 += run.queued;
            }
        }
        let mut parts = vec![];
        for (n, (run, q)) in &by_name {
            if *run > 0 {
                parts.push(format!("{n} {run} running"));
            }
            if *q > 0 {
                parts.push(format!("{n} {q} queued"));
            }
        }
        out.label = parts.join(", ");
        out.read_at_ms = if out.stale {
            self.last.read_at_ms
        } else {
            now_ms
        };
        self.last = out.clone();
        out
    }

    fn poll_repo(
        st: &mut RepoState,
        gh: &GitHub,
        own: &[String],
        now_ms: u64,
    ) -> Result<(), String> {
        let mut changed = false;
        for (i, status) in ["in_progress", "queued"].into_iter().enumerate() {
            let found = gh
                .runs_with_status(status, st.list_etag[i].as_deref())
                .map_err(|e| e.to_string())?;
            if let Some((runs, etag)) = found {
                st.lists[i] = runs;
                st.list_etag[i] = etag;
                changed = true;
            }
        }
        if !changed {
            return Ok(());
        }
        let mut live: std::collections::BTreeMap<u64, (String, usize)> = Default::default();
        for (id, name, path) in st.lists.iter().flatten() {
            if own
                .iter()
                .any(|o| path.rsplit('/').next() == Some(o.as_str()))
            {
                continue;
            }
            let list = st.lists[1].iter().any(|(i, _, _)| i == id) as usize;
            live.insert(*id, (name.clone(), list));
        }
        st.runs.retain(|id, _| live.contains_key(id));
        for (id, (name, list)) in live {
            let run = st.runs.entry(id).or_default();
            // A run whose status in the lists is what it was when its jobs were read, and read
            // not long ago, is not asked about again.
            if run.read_ms > 0
                && run.list == list
                && now_ms.saturating_sub(run.read_ms) < JOBS_AGAIN_MS
            {
                continue;
            }
            let Some((jobs, etag)) = gh
                .remote_run_jobs_changed(id, run.etag.as_deref())
                .map_err(|e| e.to_string())?
            else {
                run.name = name;
                run.list = list;
                run.read_ms = now_ms.max(1);
                continue;
            };
            *run = RunJobs {
                etag,
                name,
                list,
                read_ms: now_ms.max(1),
                ..Default::default()
            };
            for j in &jobs {
                let mac = job_is_macos(j) as u32;
                match j.status.as_str() {
                    "in_progress" => {
                        run.running += 1;
                        run.macos_running += mac;
                    }
                    "queued" => {
                        run.queued += 1;
                        run.macos_queued += mac;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

/// How long a run of another workflow keeps the jobs read for it when its status in the lists
/// has not changed.
const JOBS_AGAIN_MS: u64 = 180_000;

/// Seconds between looks at the account's other workflows.
const OTHER_WORKFLOWS_EVERY_S: u64 = 60;

/// Keep the broker told what the account's other workflows have on GitHub's runners, every
/// minute (longer when the GitHub quota is running short, and not at all while it is used up).
/// Nothing runs where no project names a GitHub repository.
pub fn spawn_other_workflows(d: &Arc<Daemon>) {
    if forced_for_tests() || account_repos(d).is_empty() {
        return;
    }
    let mut seen = AccountJobs::default();
    let mut every = OTHER_WORKFLOWS_EVERY_S;
    crate::bg::spawn_loop(d, "other-workflows", move |weak| {
        std::thread::sleep(Duration::from_secs(every));
        let Some(d) = weak.upgrade() else {
            return false;
        };
        if let Some(w) = github_budget::wait_s() {
            // The quota is used up: the count stands as it is until the reset.
            every = w.min(60);
            return true;
        }
        every = github_budget::paced(OTHER_WORKFLOWS_EVERY_S, Priority::Background);
        let http: Arc<dyn Http> = Arc::new(Curl {
            timeout_s: 30,
            http2: false,
        });
        let sources: Vec<(AccountRepo, GitHub)> = account_repos(&d)
            .into_iter()
            .filter_map(|r| {
                let token = TokenRef::parse(&r.token).ok()?;
                let gh = GitHub::new(http.clone(), &r.api, &r.repo, token);
                Some((r, gh))
            })
            .collect();
        d.broker
            .set_other_jobs(seen.poll(&sources, crate::daemon::now_ms()));
        true
    });
}

// ---- the snapshot -----------------------------------------------------------------------------

/// The toolchains a run of `suite` needs (`rust`, `java`, `swift`); the workflow sets up only
/// these. MASON's own suites: `swift*` needs Xcode and `compose*` needs Java and Gradle; every
/// other suite, and a check with no suite, is Rust.
pub fn toolchains(suite: &str) -> Vec<&'static str> {
    if suite.starts_with("swift") {
        vec!["swift"]
    } else if suite.starts_with("compose") {
        vec!["java"]
    } else {
        vec!["rust"]
    }
}

/// The Xcode version a command asks for (`DEVELOPER_DIR=/Applications/Xcode_16.4.app`): `16.4`.
/// The runner prefers it and falls back to the image's default Xcode when it is not installed.
pub fn xcode_requested(cmd: &str) -> Option<String> {
    let rest = &cmd[cmd.find("Xcode")? + "Xcode".len()..];
    let rest = rest.strip_prefix(['_', '-', ' '])?;
    let v: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let v = v.trim_end_matches('.').to_string();
    (!v.is_empty()).then_some(v)
}

/// How long to wait before the next try after `n` runs in a row failed for the environment:
/// the base doubled each time, up to 64 times the base.
pub fn env_backoff(base: Duration, n: u32) -> Duration {
    base * (1u32 << n.min(6))
}

/// `.mason/run.json`: what the runner is told, from the pinned definition.
pub fn run_json(spec: &RunSpec) -> String {
    let env: serde_json::Map<String, serde_json::Value> = spec
        .env
        .iter()
        // Only MASON's own facts go to a runner; the value is public in a public repository.
        .filter(|(k, _)| {
            k.starts_with("MASON_") && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();
    // Colourless, so the daemon's line parsers read cargo's `--> file:line` as written.
    let mut env = env;
    env.insert("CARGO_TERM_COLOR".into(), "never".into());
    env.insert("NO_COLOR".into(), "1".into());
    // ci.yml's exit-scripts job skips the Swift and Compose builds; a remote run does too.
    if spec.suite.starts_with("exit-scripts") {
        for k in ["MASON_SKIP_SWIFT", "MASON_SKIP_COMPOSE"] {
            env.entry(k.to_string())
                .or_insert_with(|| serde_json::Value::String("1".into()));
        }
    }
    // The `run` step stops a couple of minutes before MASON cancels the run, so the upload step
    // still runs and the output so far reaches us (#776).
    let timeout_min = step_timeout_min(spec.limit);
    serde_json::json!({
        "toolchains": toolchains(&spec.suite),
        "run": spec.id,
        "project": spec.project,
        "suite": spec.suite,
        "build": spec.build,
        "command": spec.command,
        "rerun": spec.rerun,
        "results_file": spec.results_file,
        "limit_s": spec.limit.as_secs(),
        "timeout_min": timeout_min,
        "runs_on": spec.runs_on,
        "xcode": xcode_requested(&spec.command),
        "env": env,
        "shards": spec.shards.max(1),
        "matrix": spec.active(),
        "shard_lists": spec.shard_lists,
        "shard_command": spec.shard_command,
    })
    .to_string()
}

fn git_in(cwd: &Path, index: &Path, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
    use std::io::Write;
    let mut cmd = Command::new(git::program());
    cmd.args(["-c", "user.name=MASON", "-c", "user.email=mason@localhost"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(cwd)
        .env("GIT_INDEX_FILE", index)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("cannot run git: {e}"))?;
    if let (Some(bytes), Some(mut si)) = (stdin, child.stdin.take()) {
        si.write_all(bytes).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A commit of what a local run in `wt` would see, plus the trust overlay (see the module
/// docs). The worktree, its index and `HEAD` are not touched: the files are added to a
/// temporary index. Returns the commit's SHA.
pub fn snapshot(
    wt: &Path,
    base_ref: &str,
    workflow: Option<&str>,
    run_json: &str,
    message: &str,
) -> Result<String, String> {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = std::env::temp_dir().join(format!(
        "mason-snapshot-{}-{}-{}",
        std::process::id(),
        now_ms(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let index = tmp.join("index");
    let r = (|| {
        let g = |args: &[&str], stdin: Option<&[u8]>| git_in(wt, &index, args, stdin);
        g(&["read-tree", "HEAD"], None)?;
        // Everything a local run would see: modified, deleted, new and untracked files (not the
        // ignored ones, which a local run's checkout does not carry either).
        g(&["add", "-A"], None)?;
        // A run for a node has no workflow: the node runs the command itself and nothing of
        // GitHub's is involved, so the tree is what a local run would see, as it is.
        if let Some(workflow) = workflow {
            // No workflow of the task's runs on these refs...
            g(
                &[
                    "rm",
                    "-r",
                    "--cached",
                    "-q",
                    "--ignore-unmatch",
                    "--",
                    ".github/workflows",
                ],
                None,
            )?;
            // ...except the base branch's own copy of ours.
            let path = format!(".github/workflows/{workflow}");
            let blob = g(&["rev-parse", &format!("{base_ref}:{path}")], None)
                .map_err(|e| format!("the base branch ({base_ref}) has no {path}: {e}"))?;
            // A run starts by dispatch; a workflow that also runs on the push would start it twice.
            let own = g(&["cat-file", "blob", &blob], None)?;
            if mason_adapters::runref::fires_on(
                &mason_adapters::runref::push_trigger(&own),
                "mason/run/x",
            ) {
                return Err(format!(
                    "the base branch ({base_ref}) has {path}, which runs on `mason/run/**`; a run is started by dispatch, so the push would start it twice (see `mason doctor`)"
                ));
            }
            g(
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("100644,{blob},{path}"),
                ],
                None,
            )?;
            // And the base branch's copy of every other workflow (tests read `ci.yml`), never the
            // task's. None of them may run on `mason/run/**`: it would run with our secrets.
            let listing = g(
                &["ls-tree", "-r", base_ref, "--", ".github/workflows"],
                None,
            )?;
            for line in listing.lines() {
                let Some((meta, name)) = line.split_once('\t') else {
                    continue;
                };
                let mut m = meta.split_whitespace();
                let (mode, kind, sha) = (m.next(), m.next(), m.next());
                if name == path || kind != Some("blob") {
                    continue;
                }
                let (Some(mode), Some(sha)) = (mode, sha) else {
                    continue;
                };
                if name.ends_with(".yml") || name.ends_with(".yaml") {
                    let text = g(&["cat-file", "blob", sha], None)?;
                    if mason_adapters::runref::fires_on(
                        &mason_adapters::runref::push_trigger(&text),
                        "mason/run/x",
                    ) {
                        return Err(format!(
                            "the base branch ({base_ref}) has {name}, which runs on `mason/run/**`; a remote run would start it with the repository's secrets (see `mason doctor`)"
                        ));
                    }
                }
                g(
                    &[
                        "update-index",
                        "--add",
                        "--cacheinfo",
                        &format!("{mode},{sha},{name}"),
                    ],
                    None,
                )?;
            }
        }
        let spec_blob = g(&["hash-object", "-w", "--stdin"], Some(run_json.as_bytes()))?;
        g(
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("100644,{spec_blob},.mason/run.json"),
            ],
            None,
        )?;
        // The runner's checkout is shallow and has no base, so the change's file list is made
        // here, where the history is (s759): the files that differ from the merge base with the
        // base branch, the dirty worktree included. Without a merge base there is no list, and
        // the scripts that read it fall back to git.
        if let Ok(mb) = g(&["merge-base", base_ref, "HEAD"], None) {
            let tree = g(&["write-tree"], None)?;
            let files = g(
                &[
                    "diff-tree",
                    "-r",
                    "--name-only",
                    "--no-renames",
                    "--diff-filter=ACDMRT",
                    &mb,
                    &tree,
                ],
                None,
            )?;
            let mut text = files;
            if !text.is_empty() {
                text.push('\n');
            }
            let blob = g(&["hash-object", "-w", "--stdin"], Some(text.as_bytes()))?;
            g(
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("100644,{blob},.mason/changed-files.txt"),
                ],
                None,
            )?;
            let base_blob = g(
                &["hash-object", "-w", "--stdin"],
                Some(format!("{mb}\n").as_bytes()),
            )?;
            g(
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("100644,{base_blob},.mason/base-sha"),
                ],
                None,
            )?;
        }
        let tree = g(&["write-tree"], None)?;
        g(&["commit-tree", &tree, "-p", "HEAD", "-m", message], None)
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    r
}

// ---- the record -------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
struct State {
    ref_name: String,
    sha: String,
    tree_key: String,
    pushed_ms: u64,
    run_id: Option<u64>,
    url: String,
    created_at: String,
    picked_ms: Option<u64>,
    pickup_s: Option<u64>,
    runs_on: String,
    collected: bool,
    /// GitHub finished the run while no thread of ours was collecting it, and its units were
    /// given back (#787). The record is still attachable: a later call for the same run
    /// clears this and collects the result from GitHub.
    released: bool,
    ref_deleted: bool,
    /// The workflow was dispatched on the ref (once; a restarted daemon finds the run by SHA).
    dispatched: bool,
    code: i32,
    signal: i32,
    timed_out: bool,
    ran_ms: Option<u64>,
    /// Where the run ended ("build" or "run"), empty when not said.
    phase: String,
    /// The shards of the run, 1-based; empty for a run that is not sharded.
    shards: Vec<u32>,
    /// What each collected shard sent besides its output (`<id>.shard-<k>.log`), so a daemon
    /// that restarts after the run was collected still has its parts: `k:listed:code:signal`.
    parts: Vec<(u32, u32, i32, i32, u64, u64)>,
}

impl State {
    fn save(&self, path: &Path) {
        let opt = |o: Option<u64>| o.map(|n| n.to_string()).unwrap_or_default();
        let _ = write_kv(
            path,
            &[
                ("ref", self.ref_name.clone()),
                ("sha", self.sha.clone()),
                ("tree_key", self.tree_key.clone()),
                ("pushed_ms", self.pushed_ms.to_string()),
                ("run_id", opt(self.run_id)),
                ("url", self.url.clone()),
                ("created_at", self.created_at.clone()),
                ("picked_ms", opt(self.picked_ms)),
                ("pickup_s", opt(self.pickup_s)),
                ("runs_on", self.runs_on.clone()),
                ("collected", (self.collected as u8).to_string()),
                ("released", (self.released as u8).to_string()),
                ("ref_deleted", (self.ref_deleted as u8).to_string()),
                ("dispatched", (self.dispatched as u8).to_string()),
                ("code", self.code.to_string()),
                ("signal", self.signal.to_string()),
                ("timed_out", (self.timed_out as u8).to_string()),
                ("ran_ms", opt(self.ran_ms)),
                ("phase", self.phase.clone()),
                (
                    "parts",
                    self.parts
                        .iter()
                        .map(|p| format!("{}:{}:{}:{}:{}:{}", p.0, p.1, p.2, p.3, p.4, p.5))
                        .collect::<Vec<_>>()
                        .join(","),
                ),
                (
                    "shards",
                    self.shards
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(","),
                ),
            ],
        );
    }

    fn load(path: &Path) -> Option<State> {
        let kv = read_kv(path).ok()?;
        let s = |k: &str| kv.get(k).cloned().unwrap_or_default();
        let n = |k: &str| kv.get(k).and_then(|v| v.parse::<u64>().ok());
        let flag = |k: &str| kv.get(k).is_some_and(|v| v == "1");
        let sha = s("sha");
        if sha.is_empty() {
            return None;
        }
        Some(State {
            ref_name: s("ref"),
            sha,
            tree_key: s("tree_key"),
            pushed_ms: n("pushed_ms").unwrap_or(0),
            run_id: n("run_id"),
            url: s("url"),
            created_at: s("created_at"),
            picked_ms: n("picked_ms"),
            pickup_s: n("pickup_s"),
            runs_on: s("runs_on"),
            collected: flag("collected"),
            released: flag("released"),
            ref_deleted: flag("ref_deleted"),
            dispatched: flag("dispatched"),
            code: s("code").parse().unwrap_or(0),
            signal: s("signal").parse().unwrap_or(0),
            timed_out: flag("timed_out"),
            ran_ms: n("ran_ms"),
            phase: s("phase"),
            shards: s("shards")
                .split(',')
                .filter_map(|v| v.parse().ok())
                .collect(),
            parts: s("parts")
                .split(',')
                .filter_map(|p| {
                    let f: Vec<&str> = p.split(':').collect();
                    let n = |i: usize| f.get(i).and_then(|v| v.parse::<i64>().ok());
                    Some((
                        n(0)? as u32,
                        n(1)? as u32,
                        n(2)? as i32,
                        n(3)? as i32,
                        n(4)? as u64,
                        n(5)? as u64,
                    ))
                })
                .collect(),
        })
    }

    fn where_text(&self) -> String {
        let mut t = format!("ran on GitHub ({})", self.runs_on);
        if !self.url.is_empty() {
            t.push_str(&format!(", run {}", self.url));
        }
        if let Some(s) = self.pickup_s {
            t.push_str(&format!(", picked up after {s} s"));
        }
        t
    }
}

/// The ref a run is pushed to: one per run, never reused.
pub fn ref_name(project: &str, id: &str) -> String {
    let clean = |s: &str| {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>()
    };
    format!("mason/run/{}/{}", clean(project), clean(id))
}

/// `2026-11-20T10:00:06Z` as seconds since the epoch.
pub fn parse_iso(s: &str) -> Option<u64> {
    let (date, time) = s.trim().trim_end_matches('Z').split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time
        .split(':')
        .map(|x| x.split('.').next()?.parse::<i64>().ok());
    let (h, mi, sec) = (t.next()??, t.next()??, t.next()??);
    // Days from civil (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days * 86400 + h * 3600 + mi * 60 + sec).ok()
}

// ---- stale run refs ---------------------------------------------------------------------------

/// A run ref older than this with no live record is a leftover of a crash or a killed run.
pub const STALE_REF_AGE_S: u64 = 24 * 3600;
/// The sweep looks at the remote this often, not on every pass.
const STALE_SWEEP_EVERY_MS: u64 = 30 * 60 * 1000;

/// The refs the run records under `roots` name as still in use: pushed, not collected, not
/// deleted. Records sit next to a run's log, in a task's directory or a landing directory.
pub fn live_run_refs(roots: &[PathBuf]) -> std::collections::HashSet<String> {
    fn walk(dir: &Path, depth: u32, out: &mut std::collections::HashSet<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if depth > 0 {
                    walk(&p, depth - 1, out);
                }
            } else if p.extension().is_some_and(|x| x == "remote") {
                if let Some(st) = State::load(&p) {
                    if !st.ref_deleted && !st.collected {
                        out.insert(st.ref_name);
                    }
                }
            }
        }
    }
    let mut out = std::collections::HashSet::new();
    for r in roots {
        walk(r, 3, &mut out);
    }
    out
}

/// Delete the stale run refs among `refs` (`(ref name, sha)` as the remote lists them): older
/// than [`STALE_REF_AGE_S`] by `time_of` (the commit's time, `None` when unknown, which keeps
/// the ref) and not in `live`. `delete` removes one and says whether it did. Each deletion is
/// logged. Returns the refs deleted.
pub fn sweep_run_refs(
    refs: &[(String, String)],
    live: &std::collections::HashSet<String>,
    now_s: u64,
    time_of: &dyn Fn(&str) -> Option<u64>,
    delete: &mut dyn FnMut(&str) -> bool,
) -> Vec<String> {
    let aged: Vec<(String, Option<u64>)> = refs
        .iter()
        .filter(|(r, _)| !live.contains(r))
        .map(|(r, sha)| (r.clone(), time_of(sha)))
        .collect();
    let mut gone = vec![];
    for r in mason_adapters::runref::stale_refs(&aged, live, now_s, STALE_REF_AGE_S) {
        if delete(&r) {
            tracing::info!(ref_name = %r, "deleted a stale run ref (older than 24 h, no live run record)");
            gone.push(r);
        }
    }
    gone
}

/// The safety sweep's part: for each project with `[remote_runs]`, delete its stale run refs. At most
/// every half hour; a remote that cannot be reached is tried again next time.
pub fn sweep_stale_refs(d: &Arc<Daemon>) {
    static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if !any_remote(d) {
        return;
    }
    let now = now_ms();
    let last = LAST.load(std::sync::atomic::Ordering::SeqCst);
    if last != 0 && now.saturating_sub(last) < STALE_SWEEP_EVERY_MS {
        return;
    }
    LAST.store(now, std::sync::atomic::Ordering::SeqCst);
    // Off the sweep's thread: it lists the remote and asks GitHub for each ref's age.
    let d = d.clone();
    std::thread::spawn(move || {
        let _caller = github_budget::as_caller("remote-sweep");
        sweep_stale_refs_now(&d, now)
    });
}

fn sweep_stale_refs_now(d: &Daemon, now: u64) {
    let mut roots = vec![d.cfg.landing_dir()];
    if let Ok(rd) = std::fs::read_dir(d.cfg.data_dir.join("t")) {
        roots.extend(rd.flatten().map(|e| e.path()));
    }
    let live = live_run_refs(&roots);
    let projects: Vec<String> = d
        .projects()
        .values()
        .filter(|p| p.services.remote.is_some())
        .map(|p| p.id.clone())
        .collect();
    for id in projects {
        let Some(p) = project_cfg(d, &id) else {
            continue;
        };
        let Some(site) = site_for(d, &id, &p.path) else {
            continue;
        };
        let Ok(listing) = git::run(
            &site.cwd,
            &["ls-remote", &site.remote, "refs/heads/mason/run/*"],
        ) else {
            continue;
        };
        let refs: Vec<(String, String)> = listing
            .lines()
            .filter_map(|l| {
                let (sha, r) = l.split_once('\t')?;
                Some((r.strip_prefix("refs/heads/")?.to_string(), sha.to_string()))
            })
            .collect();
        if refs.is_empty() {
            continue;
        }
        let gone = sweep_run_refs(
            &refs,
            &live,
            now / 1000,
            &|sha| site.gh.commit_time(sha),
            &mut |r| {
                let gone = git::run(
                    &site.cwd,
                    &["push", &site.remote, "--delete", &format!("refs/heads/{r}")],
                )
                .is_ok();
                if gone {
                    delete_ref_caches(&site, &format!("refs/heads/{r}"));
                }
                gone
            },
        );
        if !gone.is_empty() {
            tracing::info!(project = %id, count = gone.len(), "swept stale run refs");
        }
    }
}

// ---- running ----------------------------------------------------------------------------------

#[derive(Debug)]
pub enum Outcome {
    Done(Finished),
    /// The owner skipped or forced, or the task is gone: the run was cancelled.
    Stopped(StopWhy),
}

fn record_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.remote"))
}

/// Delete the Actions caches a deleted run ref saved (best effort: they would only crowd the
/// repository's 10 GB limit, and GitHub evicts them after 7 days anyway).
fn delete_ref_caches(site: &Site, git_ref: &str) {
    if let Err(e) = site.gh.delete_caches_for_ref(git_ref) {
        tracing::warn!(ref_name = %git_ref, "could not delete the run ref's caches: {e}");
    }
}

/// Delete the run's ref (best effort, remembered in the record).
fn delete_ref(site: &Site, st: &mut State, rec: &Path) {
    if st.ref_deleted {
        return;
    }
    let target = format!("refs/heads/{}", st.ref_name);
    let r = git::run(&site.cwd, &["push", &site.remote, "--delete", &target]);
    // A ref that is already gone is as good as deleted.
    if r.is_ok()
        || r.as_ref()
            .is_err_and(|e| e.stderr.contains("does not exist"))
    {
        st.ref_deleted = true;
        delete_ref_caches(site, &target);
    } else if let Err(e) = r {
        tracing::warn!(ref_name = %st.ref_name, "could not delete the run ref: {e}");
    }
    st.save(rec);
}

fn cancel(site: &Site, st: &State) {
    let runs = match st.run_id {
        Some(id) => vec![ActionRun {
            id,
            branch: st.ref_name.clone(),
            ..Default::default()
        }],
        None => site.gh.runs_for_sha(&st.sha).unwrap_or_default(),
    };
    for r in runs.iter().filter(|r| r.branch == st.ref_name) {
        if let Err(e) = site.gh.cancel_own_run(r) {
            tracing::warn!(run = r.id, "could not cancel the run: {e}");
        }
    }
}

/// How long a run cut off at its limit is given, after the cancel, to end and show its artifact.
const CANCEL_ARTIFACT_WAIT_MS: u64 = 180_000;

/// The output a cancelled run uploaded, or nothing when no artifact comes within
/// [`CANCEL_ARTIFACT_WAIT_MS`] (or the run is unknown).
fn output_after_cancel(site: &Site, st: &State, scratch: &Path) -> Vec<u8> {
    let Some(id) = st.run_id else {
        return vec![];
    };
    let deadline = site.clock.now_ms().saturating_add(CANCEL_ARTIFACT_WAIT_MS);
    let names: Vec<(u32, String)> = if st.shards.is_empty() {
        vec![(1, ARTIFACT.to_string())]
    } else {
        st.shards
            .iter()
            .map(|k| (*k, artifact_name(true, *k)))
            .collect()
    };
    loop {
        let done =
            matches!(site.gh.run_changed(id, None), Ok(Some((r, _))) if r.status == "completed");
        if done {
            let mut out = vec![];
            for (k, name) in &names {
                let _ = std::fs::remove_dir_all(scratch);
                if std::fs::create_dir_all(scratch).is_err() {
                    continue;
                }
                let zip = scratch.join(format!("cut-{k}.zip"));
                if !matches!(site.gh.download_artifact(id, name, &zip), Ok(true)) {
                    continue;
                }
                // Only `output.log`: a cut-off run may have no `result.json`.
                if let Ok(o) = Command::new(unzip_program())
                    .args(["-p"])
                    .arg(&zip)
                    .arg("output.log")
                    .output()
                {
                    if o.status.success() {
                        out.extend(o.stdout);
                    }
                }
            }
            if !out.is_empty() {
                return out;
            }
        }
        if site.clock.now_ms() >= deadline {
            return vec![];
        }
        site.clock.sleep(5_000);
    }
}

fn unzip_program() -> &'static str {
    if Path::new("/usr/bin/unzip").exists() {
        "/usr/bin/unzip"
    } else {
        "unzip"
    }
}

/// What `read_artifact` says of an artifact the runner made no `result.json` for.
const NO_RESULT_JSON: &str = "the artifact has no result.json";

/// The minutes the workflow's `run` step may take of a run limited to `limit`: a couple of
/// minutes less, so the upload step still runs (#776).
fn step_timeout_min(limit: Duration) -> u64 {
    limit.as_secs().div_ceil(60).saturating_sub(2).max(1)
}

/// Did a job of the run stay on its runner for the whole of the `run` step's allowance? Such a
/// job was stopped by GitHub at the step limit, before it could write `result.json` (and before
/// MASON's own limit, which is two minutes later). The job's own start and end say so; a job
/// with no end time is measured to `now` from when the run was picked up.
fn hit_step_limit(
    jobs: &[mason_adapters::github::ActionJob],
    picked_ms: Option<u64>,
    now_ms: u64,
    limit: Duration,
) -> bool {
    let allowed_s = step_timeout_min(limit) * 60;
    jobs.iter().filter(|j| is_run_job(&j.name)).any(|j| {
        let started = j.started_at.as_deref().and_then(parse_iso);
        let ended = j.completed_at.as_deref().and_then(parse_iso);
        match (started, ended) {
            (Some(a), Some(b)) => b.saturating_sub(a) >= allowed_s,
            _ => picked_ms.is_some_and(|p| now_ms.saturating_sub(p) / 1000 >= allowed_s),
        }
    })
}

/// What the artifact holds.
struct Artifact {
    /// 1-based; 1 for a run that is not sharded.
    shard: u32,
    output: Vec<u8>,
    code: i32,
    signal: i32,
    build_s: u64,
    run_s: u64,
    /// `"build"` or `"run"`: where the run ended; absent from older runners.
    phase: Option<String>,
    /// The results file(s) the suite names, as paths relative to the worktree.
    results: Vec<(PathBuf, Vec<u8>)>,
}

/// Every regular file under `root` (symlinks are skipped), as a path relative to it made of
/// plain components only.
fn regular_files(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = vec![];
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(root.join(&rel)) else {
            continue;
        };
        for e in rd.flatten() {
            let r = rel.join(e.file_name());
            let Ok(m) = std::fs::symlink_metadata(e.path()) else {
                continue;
            };
            if m.is_dir() {
                stack.push(r);
            } else if m.is_file()
                && r.components()
                    .all(|c| matches!(c, std::path::Component::Normal(_)))
            {
                if let Ok(b) = std::fs::read(e.path()) {
                    out.push((r, b));
                }
            }
        }
    }
    out.sort();
    out
}

fn read_artifact(zip: &Path, into: &Path) -> Result<Artifact, String> {
    let o = Command::new(unzip_program())
        .args(["-q", "-o"])
        .arg(zip)
        .args(["output.log", "result.json", "results/*", "-d"])
        .arg(into)
        .output()
        .map_err(|e| format!("cannot run unzip: {e}"))?;
    // unzip exits 11 when some named member is absent (`results/*` is optional).
    if !o.status.success() && o.status.code() != Some(11) {
        return Err(format!(
            "unzip: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ));
    }
    let regular = |name: &str| -> Option<Vec<u8>> {
        let p = into.join(name);
        let m = std::fs::symlink_metadata(&p).ok()?;
        if !m.is_file() {
            return None;
        }
        std::fs::read(p).ok()
    };
    let output = regular("output.log").ok_or("the artifact has no output.log")?;
    let meta = regular("result.json").ok_or(NO_RESULT_JSON)?;
    let v: serde_json::Value =
        serde_json::from_slice(&meta).map_err(|e| format!("result.json: {e}"))?;
    Ok(Artifact {
        shard: 1,
        output,
        code: v["code"].as_i64().ok_or("result.json has no code")? as i32,
        signal: v["signal"].as_i64().unwrap_or(0) as i32,
        build_s: v["build_s"].as_u64().unwrap_or(0),
        run_s: v["run_s"].as_u64().unwrap_or(0),
        phase: v["phase"]
            .as_str()
            .filter(|p| matches!(*p, "build" | "run"))
            .map(str::to_string),
        results: regular_files(&into.join("results")),
    })
}

/// The `output.log` of an artifact that was read into `into` (the one a cut-off run uploads
/// without a `result.json`).
fn cut_output(into: &Path) -> Vec<u8> {
    std::fs::read(into.join("output.log")).unwrap_or_default()
}

/// A run that was stopped at the limit (MASON's, or the workflow step's): recorded as timed out
/// with the output kept and a closing line naming the tests that never finished, so the
/// failing-names and still-running logic see it as it sees a cut-off run. `kept` is the output
/// so far; `note` is the cut-off line.
fn record_cut(
    site: &Site,
    st: &mut State,
    log: &Path,
    rec: &Path,
    kept: &[u8],
    note: &str,
) -> Outcome {
    st.timed_out = true;
    st.code = -1;
    st.signal = 9;
    st.collected = true;
    let mut text = String::from_utf8_lossy(kept).into_owned();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(note);
    text.push('\n');
    let hung = crate::suites::still_running(&text);
    if !hung.is_empty() {
        text.push_str(&format!(
            "Tests that started and never finished: {}\n",
            hung.join(", ")
        ));
    }
    let _ = std::fs::write(log, &text);
    st.save(rec);
    delete_ref(site, st, rec);
    Outcome::Done(finished_from(st, log))
}

fn finished_from(st: &State, log: &Path) -> Finished {
    let parts = st
        .parts
        .iter()
        .map(|p| Part {
            shard: p.0,
            listed: p.1,
            code: p.2,
            signal: p.3,
            run_s: p.4,
            build_s: p.5,
            output: crate::jobs::read_lossy(&part_log(log, p.0)).unwrap_or_default(),
        })
        .collect();
    Finished {
        code: st.code,
        signal: st.signal,
        output: crate::jobs::read_lossy(log).unwrap_or_default(),
        timed_out: st.timed_out,
        stop: None,
        ran_ms: st.ran_ms,
        phase: Some(st.phase.clone()).filter(|p| !p.is_empty()),
        ran_on: st.where_text(),
        parts,
        missing: vec![],
    }
}

/// The file a collected shard's output is kept in, beside the run's log.
fn part_log(log: &Path, shard: u32) -> PathBuf {
    log.with_extension(format!("shard-{shard}.log"))
}

/// Run `spec` on GitHub, or collect the run a record in `dir` says is already made. `stop` is
/// polled while the run waits and runs: `Some` cancels it. The log is written to
/// `<dir>/<id>.log`, and the results file (when the suite has one) to `site.cwd`, where a
/// local run would have left it.
pub fn execute(
    site: &Site,
    spec: &RunSpec,
    dir: &Path,
    stop: &dyn Fn() -> Option<StopWhy>,
    grant: &Grant,
) -> Result<Outcome, Fail> {
    let _caller = github_budget::as_caller("remote-run");
    let _ = std::fs::create_dir_all(dir);
    let rec = record_path(dir, &spec.id);
    let log = dir.join(format!("{}.log", spec.id));
    let mut loaded = State::load(&rec);
    // A run is reused only for the tree and command it ran. A record for another key (or an old
    // one with none) is superseded: its run is cancelled if live, its ref and record go, and the
    // run starts again.
    if let Some(old) = loaded.take() {
        if old.tree_key != spec.tree_key {
            tracing::info!(
                id = %spec.id,
                old_tree = %old.tree_key,
                new_tree = %spec.tree_key,
                "not reusing the earlier run: it was made on another tree"
            );
            let mut old = old;
            if !old.collected {
                cancel(site, &old);
            }
            delete_ref(site, &mut old, &rec);
            let _ = std::fs::remove_file(&rec);
            let _ = std::fs::remove_file(dir.join(format!("{}.log", spec.id)));
        } else {
            loaded = Some(old);
        }
    }
    let (mut st, resumed) = match loaded {
        Some(s) => (s, true),
        None => {
            let name = ref_name(&spec.project, &spec.id);
            let sha = snapshot(
                &site.cwd,
                &site.base_ref,
                Some(&site.workflow),
                &run_json(spec),
                &format!("mason run {} ({})", spec.id, spec.suite),
            )
            .map_err(|e| {
                if e.contains("the base branch") {
                    Fail::Environmental(e)
                } else {
                    Fail::Error(format!("cannot snapshot the worktree: {e}"))
                }
            })?;
            let st = State {
                ref_name: name,
                sha,
                tree_key: spec.tree_key.clone(),
                pushed_ms: site.clock.now_ms(),
                runs_on: spec.runs_on.clone(),
                shards: if spec.sharded() {
                    spec.active()
                } else {
                    vec![]
                },
                ..Default::default()
            };
            // The record first: a daemon that dies between here and the push finds the run.
            st.save(&rec);
            (st, false)
        }
    };
    if st.released {
        // Attached again after the reaper gave its units back: the run is collected as usual.
        st.released = false;
        st.save(&rec);
    }
    if st.collected {
        delete_ref(site, &mut st, &rec);
        return Ok(Outcome::Done(finished_from(&st, &log)));
    }
    let abandon = |st: &mut State| {
        delete_ref(site, st, &rec);
        let _ = std::fs::remove_file(&rec);
    };
    if !resumed {
        if let Err(e) = push(site, &st) {
            let _ = std::fs::remove_file(&rec);
            return Err(Fail::Environmental(format!(
                "could not push {}: {e}",
                st.ref_name
            )));
        }
        if let Err(e) = dispatch(site, spec, &mut st, &rec) {
            // The ref is there and no run was started: take it down again.
            delete_ref(site, &mut st, &rec);
            let _ = std::fs::remove_file(&rec);
            return Err(Fail::Environmental(format!(
                "could not start {} on GitHub: {e}",
                st.ref_name
            )));
        }
    } else if !st.dispatched && st.run_id.is_none() {
        // A daemon that died between the push and the dispatch: start the run, unless GitHub
        // already has one for this commit.
        let known = site
            .gh
            .runs_for_sha(&st.sha)
            .map(|v| v.iter().any(|r| r.branch == st.ref_name));
        match known {
            Ok(true) => {
                st.dispatched = true;
                st.save(&rec);
            }
            _ => {
                let _ = push(site, &st);
                if let Err(e) = dispatch(site, spec, &mut st, &rec) {
                    abandon(&mut st);
                    return Err(Fail::Environmental(format!(
                        "could not start {} on GitHub: {e}",
                        st.ref_name
                    )));
                }
            }
        }
    }
    let mut poller = Poller::new(site.poll_s);
    let mut outage_since: Option<u64> = None;
    let mut artifact_misses = 0u32;
    let mut live = Live::default();
    // The jobs whose runner unit has been given back (#820).
    let mut gave: std::collections::HashSet<String> = Default::default();
    let per_job = units_for(&spec.runs_on, 1);
    let mut room = RoomCache::default();
    let mut held_back = false;
    let scratch = dir.join(format!("{}.remote-artifact", spec.id));
    loop {
        if let Some(why) = stop() {
            cancel(site, &st);
            abandon(&mut st);
            return Ok(Outcome::Stopped(why));
        }
        let now = site.clock.now_ms();
        // The limit counts from when a runner took the job, not from the push.
        if let Some(p) = st.picked_ms {
            if now >= p.saturating_add(spec.limit.as_millis() as u64) {
                cancel(site, &st);
                // The output so far (the workflow uploads it even for a cut-off run), then the
                // cut-off line naming the tests that never finished.
                let kept = output_after_cancel(site, &st, &scratch);
                let _ = std::fs::remove_dir_all(&scratch);
                let note = format!(
                    "(the run was cancelled after its limit of {} s on the runner)",
                    spec.limit.as_secs()
                );
                return Ok(record_cut(site, &mut st, &log, &rec, &kept, &note));
            }
        } else if github_budget::wait_s().is_some() {
            // GitHub's quota is used up until its reset: whether a runner has the run cannot be
            // seen, so the wait for one is not counted (and nothing can be cancelled either).
        } else {
            // Not picked up yet. A run that is due to come back here (favor-remote's
            // `spill_after`, or `pickup_timeout`) does so only with room for it here when the site
            // can say (`local_room`); otherwise it stays on GitHub, where a runner may still take
            // it. A site without a room function falls back at the timeout as it always did.
            let waited = now.saturating_sub(pickup_clock_from(
                st.pushed_ms,
                parse_iso(&st.created_at),
                now,
            ));
            let timed_out = waited >= site.pickup_timeout.as_millis() as u64;
            let gated = site.fallback_local && site.local_room.is_some();
            let spill = gated && spill_due(waited, site.spill_after, true);
            if timed_out || spill {
                let why = if gated { room.no_room(site, now) } else { None };
                match why {
                    Some(why) => {
                        if !held_back {
                            held_back = true;
                            tracing::info!(
                                run = %spec.id,
                                "no runner has picked up {} after {} s; not running it here yet, still waiting on GitHub: {why}",
                                st.ref_name,
                                waited / 1000
                            );
                        }
                    }
                    None if !timed_out => {
                        // Favor-remote (#833): GitHub has not picked it up and this machine has
                        // room, so it comes back now instead of waiting out the pickup timeout.
                        // The run is cancelled, its ref deleted and its units freed, as for a
                        // timeout.
                        cancel(site, &st);
                        abandon(&mut st);
                        return Err(Fail::Environmental(format!(
                            "no runner picked up {} within {} s (favor-remote: this machine has room, so it runs here)",
                            st.ref_name,
                            waited / 1000
                        )));
                    }
                    None => {
                        cancel(site, &st);
                        abandon(&mut st);
                        return Err(Fail::Environmental(format!(
                            "no runner picked up {} within {} s",
                            st.ref_name,
                            if gated {
                                waited / 1000
                            } else {
                                site.pickup_timeout.as_secs()
                            }
                        )));
                    }
                }
            }
        }
        if !poller.due(now / 1000) {
            site.clock.sleep(500);
            continue;
        }
        let waited = now.saturating_sub(st.pushed_ms);
        let mut deadline = site.pickup_timeout.as_millis() as u64;
        if let Some(a) = site.spill_after {
            deadline = deadline.min(a.as_millis() as u64);
        }
        poller.interval_s = github_budget::paced(
            run_poll_interval_s(
                site.poll_s,
                st.run_id.is_some(),
                st.picked_ms.is_some(),
                artifact_misses > 0,
                waited,
                deadline,
            ),
            Priority::Collect,
        );
        let step = poll(
            site,
            &mut st,
            &mut poller,
            &mut artifact_misses,
            &mut live,
            &scratch,
            &rec,
            now,
            spec.limit,
        );
        // Each `run` job that has finished gives its runner unit back at once, so the next in
        // line need not wait for the slowest shard (#820). Until the run's jobs exist (the
        // `plan` job still runs) nothing is given.
        for j in &live.jobs {
            if is_run_job(&j.name) && j.status == "completed" && gave.insert(j.name.clone()) {
                grant.give(&per_job);
            }
        }
        match step {
            Ok(Step::Wait) => outage_since = None,
            Ok(Step::Fail(why)) => {
                cancel(site, &st);
                abandon(&mut st);
                let _ = std::fs::remove_dir_all(&scratch);
                return Err(Fail::Environmental(why));
            }
            Ok(Step::Cut(kept)) => {
                let _ = std::fs::remove_dir_all(&scratch);
                let note = format!(
                    "(the run was cancelled after its limit of {} s on the runner: GitHub stopped the run step at {} min)",
                    spec.limit.as_secs(),
                    step_timeout_min(spec.limit)
                );
                return Ok(record_cut(site, &mut st, &log, &rec, &kept, &note));
            }
            Ok(Step::Parts(got, missing)) => {
                let _ = std::fs::remove_dir_all(&scratch);
                let parts: Vec<Part> = got
                    .iter()
                    .map(|a| Part {
                        shard: a.shard,
                        listed: spec
                            .shard_lists
                            .get(a.shard as usize - 1)
                            .filter(|_| !spec.partial_lists)
                            .map_or(0, |l| l.len() as u32),
                        code: a.code,
                        signal: a.signal,
                        output: String::from_utf8_lossy(&a.output).into_owned(),
                        run_s: a.run_s,
                        build_s: a.build_s,
                    })
                    .collect();
                let (code, signal, output) = crate::shards::merge(&parts);
                // Shards run side by side: the run took as long as the slowest.
                let ran_ms = got.iter().map(|a| (a.build_s + a.run_s) * 1000).max();
                if spec.results_file.is_some() {
                    for a in &got {
                        for (rel, bytes) in &a.results {
                            let p = site.cwd.join(rel);
                            if let Some(parent) = p.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            let _ = std::fs::write(p, bytes);
                        }
                    }
                }
                if !missing.is_empty() {
                    // Not collected: the caller runs the missing shards again and keeps these.
                    let ran_on = st.where_text();
                    abandon(&mut st);
                    return Ok(Outcome::Done(Finished {
                        code,
                        signal,
                        output,
                        timed_out: false,
                        stop: None,
                        ran_ms,
                        phase: None,
                        ran_on,
                        parts,
                        missing,
                    }));
                }
                let _ = std::fs::write(&log, &output);
                for p in &parts {
                    let _ = std::fs::write(part_log(&log, p.shard), &p.output);
                }
                st.parts = parts
                    .iter()
                    .map(|p| (p.shard, p.listed, p.code, p.signal, p.run_s, p.build_s))
                    .collect();
                st.code = code;
                st.signal = signal;
                st.ran_ms = ran_ms;
                st.collected = true;
                st.save(&rec);
                delete_ref(site, &mut st, &rec);
                let _ = std::fs::write(
                    dir.join(format!("{}.where", spec.id)),
                    format!("{}\n", st.where_text()),
                );
                return Ok(Outcome::Done(finished_from(&st, &log)));
            }
            Ok(Step::Got(a)) => {
                let _ = std::fs::remove_dir_all(&scratch);
                let _ = std::fs::write(&log, &a.output);
                // The results file(s), where a local run would have left them.
                if spec.results_file.is_some() {
                    for (rel, bytes) in &a.results {
                        let p = site.cwd.join(rel);
                        if let Some(parent) = p.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::write(p, bytes);
                    }
                }
                st.code = a.code;
                st.signal = a.signal;
                st.ran_ms = Some((a.build_s + a.run_s) * 1000);
                st.phase = a.phase.clone().unwrap_or_default();
                st.collected = true;
                st.save(&rec);
                delete_ref(site, &mut st, &rec);
                let _ = std::fs::write(
                    dir.join(format!("{}.where", spec.id)),
                    format!("{}\n", st.where_text()),
                );
                return Ok(Outcome::Done(finished_from(&st, &log)));
            }
            Err(e) => {
                let ev = poller.observe(now / 1000, Observed::Failed(e.clone()));
                // The quota being used up is not an outage: the run is still there, and the
                // poller waits for the reset.
                if matches!(e.kind, ErrorKind::RateLimited { .. }) {
                    outage_since = None;
                    continue;
                }
                match ev {
                    PollEvent::Stopped { why } => {
                        // A token that is refused or a repository that is gone: nothing here
                        // will change by waiting.
                        abandon(&mut st);
                        return Err(Fail::Environmental(format!("GitHub refused: {why}")));
                    }
                    _ => {
                        let since = *outage_since.get_or_insert(now);
                        if now.saturating_sub(since) >= site.pickup_timeout.as_millis() as u64 {
                            abandon(&mut st);
                            return Err(Fail::Environmental(format!(
                                "GitHub did not answer for {} s: {e}",
                                site.pickup_timeout.as_secs()
                            )));
                        }
                    }
                }
            }
        }
    }
}

/// The inputs of the dispatch: the runner and the shard list the workflow's `run` job starts
/// with. They are read back from the same JSON as `.mason/run.json`, so they are what the
/// workflow's check compares them to.
pub fn dispatch_inputs(spec: &RunSpec) -> Vec<(String, String)> {
    let v: serde_json::Value = serde_json::from_str(&run_json(spec)).unwrap_or_default();
    let sharded = v["shards"].as_u64().unwrap_or(1) > 1;
    vec![
        ("runs_on".into(), v["runs_on"].to_string()),
        ("matrix".into(), v["matrix"].to_string()),
        ("timeout_min".into(), v["timeout_min"].to_string()),
        ("sharded".into(), sharded.to_string()),
        ("toolchains".into(), v["toolchains"].to_string()),
    ]
}

/// Start the workflow on the pushed ref, once, and record that it was.
fn dispatch(site: &Site, spec: &RunSpec, st: &mut State, rec: &Path) -> Result<(), String> {
    site.gh
        .dispatch_workflow(&site.workflow, &st.ref_name, &dispatch_inputs(spec))
        .map_err(|e| e.to_string())?;
    st.dispatched = true;
    st.save(rec);
    Ok(())
}

/// Waits before the second, third and fourth tries of a push (ms).
const PUSH_BACKOFF_MS: [u64; 3] = [10_000, 30_000, 90_000];

/// Whether a push error is the credential being refused: waiting will not change that.
fn push_refused(e: &str) -> bool {
    let l = e.to_ascii_lowercase();
    [
        "authentication failed",
        "permission denied",
        "could not read username",
        "invalid username or password",
        "bad credentials",
        "error: 401",
        "error: 403",
        "protected branch",
    ]
    .iter()
    .any(|m| l.contains(m))
}

/// Whether a push error came from GitHub's side (a rejection, a 5xx, a dropped connection):
/// the next try may well work (#772).
fn push_transient(e: &str) -> bool {
    if push_refused(e) {
        return false;
    }
    let l = e.to_ascii_lowercase();
    l.contains("remote rejected")
        || l.contains("connection reset")
        || l.contains("rpc failed")
        || l.contains("early eof")
        || l.contains("the remote end hung up")
        || l.contains("timed out")
        || ["500", "502", "503", "504"]
            .iter()
            .any(|c| l.contains(&format!("error: {c}")))
}

fn push_once(site: &Site, st: &State) -> Result<(), String> {
    git::run(
        &site.cwd,
        &[
            "push",
            "--porcelain",
            &site.remote,
            &format!("{}:refs/heads/{}", st.sha, st.ref_name),
        ],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// Push the snapshot, trying again (about 10 s, 30 s, 90 s apart) when the rejection is
/// GitHub's and not a refused credential.
fn push(site: &Site, st: &State) -> Result<(), String> {
    let mut tries = 0;
    loop {
        match push_once(site, st) {
            Ok(()) => return Ok(()),
            Err(e) if tries < PUSH_BACKOFF_MS.len() && push_transient(&e) => {
                tracing::warn!(ref_name = %st.ref_name, "push failed, trying again: {e}");
                site.clock.sleep(PUSH_BACKOFF_MS[tries]);
                tries += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Seconds between looks at a run, before the budget stretches them. Quick (`base_s`) while the
/// run is being found after the dispatch and while a finished run's artifact is awaited, and
/// when a run unpicked for long enough is near the time it would come back here (so a pickup is
/// seen before the timeout, which is wall time, not polls). A run still queued is looked at six
/// times, and a running one three times, as rarely. Pure.
pub fn run_poll_interval_s(
    base_s: u64,
    run_known: bool,
    picked: bool,
    awaiting_artifact: bool,
    waited_ms: u64,
    deadline_ms: u64,
) -> u64 {
    let base = base_s.max(1);
    if !run_known || awaiting_artifact {
        base
    } else if picked {
        base * 3
    } else {
        let slow = base * 6;
        if waited_ms.saturating_add(slow.saturating_mul(2000)) >= deadline_ms {
            base
        } else {
            slow
        }
    }
}

enum Step {
    Wait,
    Got(Artifact),
    /// A sharded run's artifacts, and the shards that sent none.
    Parts(Vec<Artifact>, Vec<u32>),
    Fail(String),
    /// The run ended at the workflow step's limit with no `result.json`: its output so far.
    Cut(Vec<u8>),
}

/// One look at the run: find it, see whether a runner has it, and when it is over take its
/// artifact. A transient failure is returned as the error, for the poller's backoff.
#[allow(clippy::too_many_arguments)]
fn poll(
    site: &Site,
    st: &mut State,
    poller: &mut Poller,
    misses: &mut u32,
    live: &mut Live,
    scratch: &Path,
    rec: &Path,
    now: u64,
    limit: Duration,
) -> Result<Step, mason_adapter_api::service::ApiError> {
    let secs = now / 1000;
    let mut run_changed = true;
    let run = match st.run_id {
        None => {
            let found = site
                .gh
                .runs_for_sha(&st.sha)?
                .into_iter()
                .find(|r| r.branch == st.ref_name);
            let Some(r) = found else {
                poller.observe(secs, Observed::Unchanged);
                return Ok(Step::Wait);
            };
            st.run_id = Some(r.id);
            st.url = r.url.clone();
            st.created_at = r.created_at.clone();
            st.save(rec);
            poller.observe(secs, Observed::Changed { etag: None });
            r
        }
        Some(id) => match site.gh.run_changed(id, poller.etag.as_deref())? {
            Some((r, etag)) => {
                poller.observe(secs, Observed::Changed { etag });
                r
            }
            None => {
                run_changed = false;
                poller.observe(secs, Observed::Unchanged);
                ActionRun {
                    id,
                    // Unchanged since the last look: a run that ended is looked at again only
                    // while its artifact is awaited.
                    status: if *misses > 0 {
                        "completed".into()
                    } else if st.picked_ms.is_some() {
                        "in_progress".into()
                    } else {
                        "queued".into()
                    },
                    branch: st.ref_name.clone(),
                    ..Default::default()
                }
            }
        },
    };
    // The jobs, for whether a runner has the job and for what it is doing. A 304 on the run says
    // nothing about its jobs, so they have their own ETag (a 304 does not count against the
    // rate limit); the last answer stands until they change. They are read when the run changed
    // (it goes in progress when a runner takes a job), when never read, and otherwise only when
    // the last reading is old: a minute for a run not yet picked up (its pickup is timed), two
    // minutes for the step shown of one running.
    if run.status != "completed" {
        let stale_ms = if st.picked_ms.is_none() || !st.shards.is_empty() {
            60_000
        } else {
            120_000
        };
        if run_changed || live.stale(now, stale_ms) {
            live.refresh(site, run.id, now)?;
        }
        let jobs = &live.jobs;
        // What the monitor shows of each job (no second read: the last answer stands).
        crate::remote_runs::set_jobs(rec, job_view(jobs));
        if st.picked_ms.is_none() {
            if let Some(j) = jobs.iter().find(|j| {
                is_run_job(&j.name) && matches!(j.status.as_str(), "in_progress" | "completed")
            }) {
                st.picked_ms = Some(now);
                let from_github = match (
                    j.started_at.as_deref().and_then(parse_iso),
                    parse_iso(&st.created_at),
                ) {
                    (Some(a), Some(b)) => Some(a.saturating_sub(b)),
                    _ => None,
                };
                st.pickup_s =
                    Some(from_github.unwrap_or_else(|| now.saturating_sub(st.pushed_ms) / 1000));
                note_pickup(st.pickup_s.unwrap_or(0));
                st.save(rec);
            }
        }
        if let Some(f) = &site.on_phase {
            let text = phase_text(jobs, st.shards.len(), now, st.pushed_ms);
            if live.said.as_deref() != Some(text.as_str()) {
                f(&text);
                live.said = Some(text);
            }
        }
    }
    if run.status != "completed" {
        // A shard that never got a runner while the others are done would hold the run until its
        // limit: end the run, and its missing artifact has that shard alone run again (#701).
        if let (false, Some(p)) = (st.shards.is_empty(), st.picked_ms) {
            if now >= p.saturating_add(site.pickup_timeout.as_millis() as u64) {
                let of = |s: &str| {
                    live.jobs
                        .iter()
                        .any(|j| is_run_job(&j.name) && j.status == s)
                };
                if of("queued") && !of("in_progress") {
                    cancel(site, st);
                }
            }
        }
        return Ok(Step::Wait);
    }
    if st.picked_ms.is_none() {
        st.picked_ms = Some(now);
    }
    let conclusion = run.conclusion.clone().unwrap_or_default();
    if conclusion == "startup_failure" {
        return Ok(Step::Fail(format!(
            "run {} ended in startup_failure (the workflow did not start)",
            run.url
        )));
    }
    let _ = std::fs::remove_dir_all(scratch);
    std::fs::create_dir_all(scratch).map_err(|e| {
        mason_adapter_api::service::ApiError::new(ErrorKind::Unavailable, e.to_string())
    })?;
    if !st.shards.is_empty() {
        let (mut got, mut absent) = (vec![], vec![]);
        let mut cut: Vec<(u32, Vec<u8>)> = vec![];
        for k in st.shards.clone() {
            let zip = scratch.join(format!("artifact-{k}.zip"));
            if !site
                .gh
                .download_artifact(run.id, &artifact_name(true, k), &zip)?
            {
                absent.push(k);
                continue;
            }
            let into = scratch.join(format!("files-{k}"));
            std::fs::create_dir_all(&into).map_err(|e| {
                mason_adapter_api::service::ApiError::new(ErrorKind::Unavailable, e.to_string())
            })?;
            match read_artifact(&zip, &into) {
                Ok(mut a) => {
                    a.shard = k;
                    got.push(a);
                }
                Err(e) if e == NO_RESULT_JSON => cut.push((k, cut_output(&into))),
                Err(e) => return Ok(Step::Fail(format!("run {}: shard {k}: {e}", run.url))),
            }
        }
        if !cut.is_empty() {
            let jobs = fresh_jobs(site, run.id)?;
            if hit_step_limit(&jobs, st.picked_ms, now, limit) {
                let mut out = vec![];
                for (k, o) in cut {
                    out.extend(format!("--- shard {k} ---\n").into_bytes());
                    out.extend(o);
                }
                return Ok(Step::Cut(out));
            }
            return Ok(Step::Fail(format!(
                "run {}: shard {}: {NO_RESULT_JSON}",
                run.url, cut[0].0
            )));
        }
        if !absent.is_empty() && *misses < 2 {
            *misses += 1;
            poller.observe(secs, Observed::Changed { etag: None });
            return Ok(Step::Wait);
        }
        if got.is_empty() {
            return Ok(Step::Fail(format!(
                "run {} ended ({conclusion}) without a result artifact (the job failed in setup, before the `run` step, so the runner is at fault, not the tests)",
                run.url
            )));
        }
        return Ok(Step::Parts(got, absent));
    }
    let zip = scratch.join("artifact.zip");
    if !site.gh.download_artifact(run.id, ARTIFACT, &zip)? {
        *misses += 1;
        // Artifacts can lag the run's end by a moment.
        if *misses >= 3 {
            return Ok(Step::Fail(format!(
                "run {} ended ({conclusion}) without a result artifact (the job failed in setup, before the `run` step, so the runner is at fault, not the tests)",
                run.url
            )));
        }
        poller.observe(secs, Observed::Changed { etag: None });
        return Ok(Step::Wait);
    }
    let into = scratch.join("files");
    std::fs::create_dir_all(&into).map_err(|e| {
        mason_adapter_api::service::ApiError::new(ErrorKind::Unavailable, e.to_string())
    })?;
    match read_artifact(&zip, &into) {
        Ok(a) => Ok(Step::Got(a)),
        Err(e) if e == NO_RESULT_JSON => {
            // GitHub stopped the `run` step at its limit: the output uploaded, no result. That is
            // a timeout, not a run without a result.
            let jobs = fresh_jobs(site, run.id)?;
            if hit_step_limit(&jobs, st.picked_ms, now, limit) {
                Ok(Step::Cut(cut_output(&into)))
            } else {
                Ok(Step::Fail(format!("run {}: {e}", run.url)))
            }
        }
        Err(e) => Ok(Step::Fail(format!("run {}: {e}", run.url))),
    }
}

/// The jobs of a run as GitHub has them now (not the last answer of the jobs' ETag).
fn fresh_jobs(
    site: &Site,
    id: u64,
) -> Result<Vec<mason_adapters::github::ActionJob>, mason_adapter_api::service::ApiError> {
    Ok(site
        .gh
        .remote_run_jobs_changed(id, None)?
        .map(|(jobs, _)| jobs)
        .unwrap_or_default())
}

/// The jobs of a run as the monitor lists them.
pub fn job_view(jobs: &[mason_adapters::github::ActionJob]) -> Vec<crate::remote_runs::Job> {
    jobs.iter()
        .map(|j| crate::remote_runs::Job {
            name: j.name.clone(),
            state: match j.status.as_str() {
                "completed" => "done",
                "in_progress" => "running",
                _ => "queued",
            }
            .into(),
            conclusion: j.conclusion.clone().unwrap_or_default(),
            step: if j.status == "in_progress" {
                j.steps
                    .iter()
                    .find(|s| s.status == "in_progress")
                    .or_else(|| j.steps.iter().find(|s| s.status == "queued"))
                    .map(|s| s.name.clone())
                    .unwrap_or_default()
            } else {
                String::new()
            },
        })
        .collect()
}

/// What a run still going has looked like at the last look at its jobs.
#[derive(Default)]
struct Live {
    etag: Option<String>,
    jobs: Vec<mason_adapters::github::ActionJob>,
    /// The phase text last handed to `Site::on_phase`.
    said: Option<String>,
    /// When the jobs were last asked for.
    read_ms: Option<u64>,
}

impl Live {
    /// Never read, or read at least `after_ms` ago.
    fn stale(&self, now: u64, after_ms: u64) -> bool {
        self.read_ms
            .is_none_or(|t| now.saturating_sub(t) >= after_ms)
    }

    fn refresh(
        &mut self,
        site: &Site,
        id: u64,
        now: u64,
    ) -> Result<(), mason_adapter_api::service::ApiError> {
        self.read_ms = Some(now);
        if let Some((jobs, etag)) = site.gh.remote_run_jobs_changed(id, self.etag.as_deref())? {
            self.jobs = jobs;
            self.etag = etag;
        }
        Ok(())
    }
}

/// What a run on GitHub is doing, in words, from its jobs: "on GitHub: waiting for a runner,
/// 2 min", "on GitHub: running, 4 min" (how long the runner has been in that step) or, for a run
/// split in `shards` jobs, "on GitHub: 3 of 8 shards done". Never a percentage: nothing here is
/// counted.
pub fn phase_text(
    jobs: &[mason_adapters::github::ActionJob],
    shards: usize,
    now_ms: u64,
    pushed_ms: u64,
) -> String {
    let mins = |from_s: u64| format!("{} min", (now_ms / 1000).saturating_sub(from_s) / 60);
    let runs: Vec<_> = jobs.iter().filter(|j| is_run_job(&j.name)).collect();
    let started = runs.iter().any(|j| j.status != "queued");
    if !started {
        return format!(
            "on GitHub: waiting for a runner, {}",
            mins(pushed_ms / 1000)
        );
    }
    if shards > 0 {
        let done = runs.iter().filter(|j| j.status == "completed").count();
        return format!("on GitHub: {done} of {shards} shards done");
    }
    let Some(j) = runs.first() else {
        return "on GitHub: starting".into();
    };
    match j.steps.iter().rev().find(|s| s.status == "in_progress") {
        Some(s) => {
            let what = match s.name.as_str() {
                "run" => "running",
                "Set up job" => "setting up",
                other => other,
            };
            match s.started_at.as_deref().and_then(parse_iso) {
                Some(t) => format!("on GitHub: {what}, {}", mins(t)),
                None => format!("on GitHub: {what}"),
            }
        }
        None if j.status == "completed" => "on GitHub: collecting the result".into(),
        None => "on GitHub: starting".into(),
    }
}

/// Is this job one that runs the command: `run`, or `run (3)` of a sharded run?
fn is_run_job(name: &str) -> bool {
    name == RUN_JOB || name.strip_prefix("run (").is_some_and(|r| r.ends_with(')'))
}

/// [`execute`] for a run split into shards (#701). The shards that got a result are kept; a
/// shard that never got a runner or was cancelled before it ran is made again by itself, up to
/// twice, and the merged result is returned. A run that is not sharded is just [`execute`].
pub fn execute_sharded(
    site: &Site,
    spec: &RunSpec,
    dir: &Path,
    stop: &dyn Fn() -> Option<StopWhy>,
    grant: &Grant,
) -> Result<Outcome, Fail> {
    // Granted too few runners to finish the suite inside its limit: nothing is pushed, and the
    // lease is not asked for again (the caller drops it whole).
    if let Some(why) = spec.too_few_runners() {
        return Err(Fail::Environmental(why));
    }
    // A lease emptied by an earlier run of the same suite or check asks again before this one
    // is pushed (#820).
    if !grant.regain(units_for(&spec.runs_on, spec.shards.max(1)), stop) {
        return Ok(Outcome::Stopped(stop().unwrap_or(StopWhy::Gone)));
    }
    if !spec.sharded() {
        return execute(site, spec, dir, stop, grant);
    }
    let mut kept: Vec<Part> = vec![];
    let mut attempt_spec = spec.clone();
    for attempt in 0..=SHARD_RERUNS {
        if attempt > 0 {
            attempt_spec.id = format!("{}-s{attempt}", spec.id);
            // The shards run again ask for their units again, at the run's own rank and from
            // when it began to wait, before anything is pushed (#820).
            if !grant.ask_again(
                units_for(&spec.runs_on, attempt_spec.only.len() as u32),
                stop,
            ) {
                return Ok(Outcome::Stopped(stop().unwrap_or(StopWhy::Gone)));
            }
        }
        let f = match execute(site, &attempt_spec, dir, stop, grant)? {
            Outcome::Done(f) => f,
            stopped => return Ok(stopped),
        };
        let missing = f.missing.clone();
        if attempt == 0 && missing.is_empty() {
            // The common case: the record of a run that was collected is the whole answer.
            return Ok(Outcome::Done(f));
        }
        if f.timed_out {
            return Ok(Outcome::Done(f));
        }
        kept.retain(|k| !f.parts.iter().any(|p| p.shard == k.shard));
        kept.extend(f.parts.iter().cloned());
        if missing.is_empty() {
            let (code, signal, output) = crate::shards::merge(&kept);
            return Ok(Outcome::Done(Finished {
                code,
                signal,
                output,
                parts: kept,
                missing: vec![],
                ..f
            }));
        }
        tracing::warn!(run = %spec.id, ?missing, "shards without a result: running only those again");
        attempt_spec.only = missing;
    }
    Err(Fail::Environmental(format!(
        "shards {:?} of {} got no result after {SHARD_RERUNS} reruns",
        attempt_spec.only, spec.id
    )))
}

/// How many times the shards that got no result are made again.
const SHARD_RERUNS: u32 = 2;

// ---- runner pools (#700) ----------------------------------------------------------------------

/// Is this a macOS runner? (`macos-14`, `macos-latest`, a `macos-*-xlarge`.) GitHub counts macOS
/// jobs separately: at most 5 at once on the free plan.
pub fn is_macos(runs_on: &str) -> bool {
    runs_on
        .trim()
        .trim_matches(['[', ']', '"'])
        .starts_with("macos")
}

/// What a remote run holds while it is out: one `gha` unit per job, and with a macOS runner one
/// `gha-macos` unit as well (which is how GitHub counts). A run sharded `shards` ways (#701)
/// has that many jobs. It holds nothing of this machine.
pub fn units_for(runs_on: &str, shards: u32) -> std::collections::BTreeMap<String, u32> {
    let n = shards.max(1);
    let mut u = std::collections::BTreeMap::from([(mason_scheduler::GHA.to_string(), n)]);
    if is_macos(runs_on) {
        u.insert(mason_scheduler::GHA_MACOS.to_string(), n);
    }
    u
}

/// The command a suite's whole run is split with across runners (#701), when `cmd` is that
/// whole run: a narrowed run, a run without the known-red tests and the like are not the whole
/// suite and run on one runner. [`suites`](crate::suites) splits by this rule and
/// [`run_place`] counts units by it, so what a run holds is how many jobs it starts.
pub fn shard_command<'a>(s: &'a mason_testing::Suite, cmd: &str) -> Option<&'a str> {
    if s.shards > 1 && !s.narrowed && cmd == s.command {
        return s.shard_command.as_deref();
    }
    // A narrowed run split the same way (#880): its `dev_shard_command` was filled in with the
    // same pick as `command`; one still holding a `{...}` was not, and runs whole.
    let sc = s.dev_shard_command.as_deref()?;
    (s.shards > 1 && s.narrowed && cmd == s.command && !sc.contains('{')).then_some(sc)
}

/// How many jobs `cmd` of `suite` starts on GitHub: its shards, else one.
pub fn jobs_of(suite: Option<&mason_testing::Suite>, cmd: &str) -> u32 {
    suite
        .filter(|s| shard_command(s, cmd).is_some())
        .map_or(1, |s| s.shards)
}

/// The units a run placed on node `nid` holds: one of that node's resource and nothing of this
/// machine. A nodes-only run in `remote` mode carries a [`crate::place::wait_nid`] id; it holds
/// the unit of the node chosen now, and nothing while none has room yet (it waits in the node
/// executor, not for a slot here).
pub fn node_units_of(nid: &str) -> std::collections::BTreeMap<String, u32> {
    let real = crate::place::wait_for(nid).unwrap_or(nid);
    if real.is_empty() {
        Default::default()
    } else {
        std::collections::BTreeMap::from([(crate::nodes::resource(real), 1)])
    }
}

/// Where a run goes and what it holds there (#818).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunPlace {
    /// On this machine: it takes the slots of `uses`, as ever.
    Local,
    /// On GitHub: it takes no slot of this machine, and one `gha` unit per job it starts (and as
    /// many `gha-macos` when the runner is macOS).
    GitHub {
        runs_on: String,
        units: std::collections::BTreeMap<String, u32>,
    },
    /// On a node: it takes no slot of this machine either, and one unit of that node's resource
    /// ([`crate::nodes::resource`]). It waits for nothing of this Mac, so neither the slot
    /// limits nor the CPU and heat holds apply to it. When the node cannot take it, it runs
    /// here and asks for this machine's slots then (the fallback lease).
    Node {
        nid: String,
        name: String,
        units: std::collections::BTreeMap<String, u32>,
    },
}

/// The one function that says, for a run, where it goes and what it holds. Placement is the
/// run's own (`place_of` with the suite and the task); only a run with no suite follows the
/// global switch. `cmd` decides the sharding, by the rule `run_remote` uses.
pub fn run_place(
    d: &Daemon,
    project: &str,
    task: Option<mason_core::TaskId>,
    suite: Option<&mason_testing::Suite>,
    cmd: &str,
    kind: Kind,
) -> RunPlace {
    let exec = match suite {
        Some(s) => executor_for(d, project, kind, Some((task, s))),
        None => executor_global(d, project),
    };
    match exec {
        Executor::GitHub { runs_on } => {
            let units = units_for(&runs_on, jobs_of(suite, cmd));
            RunPlace::GitHub { runs_on, units }
        }
        Executor::Local => RunPlace::Local,
        // A run placed on a node holds one unit of that node and nothing of this machine. It
        // was placed as a run here until now, so a check or step bound for a node queued for
        // this Mac's `test-suite` slot (and its heat hold) before it ever went out.
        Executor::Node { nid, name } => {
            let units = node_units_of(&nid);
            RunPlace::Node { nid, name, units }
        }
    }
}

/// `needs` of a run of `suite` placed by [`run_place`]: on GitHub the machine's `compile`,
/// `test-suite` and `test-small` (and the whole machine) become the run's runner units; here
/// nothing changes (`None`), whatever the global switch says (#766's lesson: decide per suite).
///
/// `flexible`: a sharded run asks for 1 to N runner units and is granted what is free (at least
/// 1), then splits into that many shards ([`shards_granted`]) instead of waiting for all N.
pub fn suite_pool_needs(
    d: &Daemon,
    project: &str,
    task: Option<mason_core::TaskId>,
    needs: &mason_scheduler::Needs,
    suite: &mason_testing::Suite,
    flexible: bool,
) -> Option<mason_scheduler::Needs> {
    match run_place(d, project, task, Some(suite), &suite.command, Kind::Suite) {
        RunPlace::GitHub { units, .. } => {
            let n = with_units(needs, units);
            Some(if flexible { n.flexible() } else { n })
        }
        RunPlace::Node { units, .. } => Some(with_units(needs, units)),
        RunPlace::Local => None,
    }
}

/// How many shards a run of `shards` makes when the broker granted it `granted` runner units
/// (`None`: not a flexible grant, so all of them). Never more than asked, never fewer than one.
pub fn shards_granted(shards: u32, granted: Option<u32>) -> u32 {
    granted.map_or(shards, |g| g.clamp(1, shards.max(1)))
}

/// The fewest runners a suite of `shards` shards can finish on inside its limit. The suite's
/// shard count was sized for its limit, so half as many runners is taken to be the most it can
/// be squeezed to: a workspace sized for 8 runners does not finish in an hour on one (the
/// 47 runs in 36 h that hit the step limit).
pub fn min_shards(shards: u32) -> u32 {
    shards.div_ceil(2).max(1)
}

/// `rs` split for a run of `shards` shards that was granted `granted` runner units: that many
/// ways (`lists` makes the tests of each), with `shard_command` on each runner; one unit runs
/// `rs`'s own command whole on one runner. Suite runs and agents' checks both split by this.
pub fn split_for_grant(
    mut rs: RunSpec,
    shards: u32,
    shard_command: &str,
    granted: Option<u32>,
    partial_lists: bool,
    lists: impl FnOnce(u32) -> Vec<Vec<String>>,
) -> RunSpec {
    let n = shards_granted(shards, granted);
    rs.shards_wanted = shards;
    if n > 1 {
        rs.shards = n;
        rs.shard_command = Some(shard_command.to_string());
        rs.shard_lists = lists(n);
        rs.partial_lists = partial_lists;
    }
    rs
}

/// `needs` without the machine's slots, plus `units`.
pub fn with_units(
    needs: &mason_scheduler::Needs,
    units: std::collections::BTreeMap<String, u32>,
) -> mason_scheduler::Needs {
    const MACHINE: [&str; 3] = ["compile", "test-suite", "test-small"];
    let mut out = mason_scheduler::Needs {
        exclusive: false,
        amounts: needs
            .amounts
            .iter()
            .filter(|(k, _)| !MACHINE.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), *v))
            .collect(),
        flex: false,
    };
    for (k, v) in units {
        let have = out.amounts.entry(k).or_insert(0);
        *have = (*have).max(v);
    }
    out
}

/// Proof that the caller holds the runner units of the run it is about to push, and the hold
/// itself. Only a broker lease makes one ([`Grant::of`]), so `execute` cannot be reached
/// without asking the broker first (#818). A run that attaches to a run already out, or that
/// goes to a local slot, holds nothing new ([`Grant::attached`]); a caller whose lease is not
/// passed down to the push yet says so by name ([`Grant::unplumbed`]) so each is findable.
/// Clones share one hold, which is released when the last is dropped or by [`Grant::give_back`].
#[derive(Clone, Default)]
pub struct Grant {
    inner: Arc<std::sync::Mutex<GrantInner>>,
}

#[derive(Default)]
struct GrantInner {
    /// The lease this grant keeps, if it owns one.
    owned: Option<crate::sched::Lease>,
    /// Who can give units back and ask again: the owned lease, or the enclosing step's.
    giver: Option<crate::sched::Giver>,
    /// The runner units still held.
    held: std::collections::BTreeMap<String, u32>,
    /// Where the run's hold is recorded, rewritten as units go back.
    hold: Option<(PathBuf, String)>,
}

impl std::fmt::Debug for Grant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Grant({:?})", self.units())
    }
}

impl Grant {
    fn lock(&self) -> std::sync::MutexGuard<'_, GrantInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// The grant of a lease the broker made; it keeps the lease until it is dropped.
    pub fn of(lease: crate::sched::Lease) -> Self {
        Grant {
            inner: Arc::new(std::sync::Mutex::new(GrantInner {
                held: lease.units(),
                giver: Some(lease.giver()),
                owned: Some(lease),
                hold: None,
            })),
        }
    }
    /// A run that is already out (its record exists) or is collected, or one that takes local
    /// slots: nothing new is pushed.
    pub fn attached() -> Self {
        Grant::default()
    }
    /// A caller whose lease is not passed down to the push yet: its units are held by an
    /// enclosing hold that this function cannot see. Each use is a remaining gap of #818.
    pub fn unplumbed(_why: &'static str) -> Self {
        Grant::default()
    }
    /// The units of a lease someone else keeps (the test step's): this grant can give them
    /// back and ask for them again, but does not own the lease (#820).
    pub fn borrowed(
        giver: Option<crate::sched::Giver>,
        units: std::collections::BTreeMap<String, u32>,
    ) -> Self {
        Grant {
            inner: Arc::new(std::sync::Mutex::new(GrantInner {
                giver,
                held: units,
                ..Default::default()
            })),
        }
    }
    /// Say where the run's hold is recorded, so units given back are written there.
    pub fn note_hold(&self, dir: &Path, id: &str) {
        self.lock().hold = Some((dir.to_path_buf(), id.to_string()));
    }
    /// The runner units this grant holds.
    pub fn units(&self) -> std::collections::BTreeMap<String, u32> {
        self.lock().held.clone()
    }
    /// A job of the run has finished: give back its units and grant the next in line at once.
    /// What is held is written to the run's hold, so a restart counts only jobs still out.
    pub fn give(&self, give: &std::collections::BTreeMap<String, u32>) {
        let mut g = self.lock();
        let Some(giver) = g.giver.clone() else {
            return;
        };
        let give: std::collections::BTreeMap<String, u32> = give
            .iter()
            .filter_map(|(k, n)| Some((k.clone(), (*n).min(*g.held.get(k)?))))
            .filter(|(_, n)| *n > 0)
            .collect();
        if give.is_empty() {
            return;
        }
        giver.shrink(&give);
        for (k, n) in &give {
            if let Some(h) = g.held.get_mut(k) {
                *h -= n;
            }
        }
        g.held.retain(|_, n| *n > 0);
        if let Some((dir, id)) = &g.hold {
            set_hold_units(dir, id, &g.held);
        }
    }
    /// Shards of the run are made again: ask for `units`, at the rank and from the wait start
    /// the run had (#820), and wait. What is still held is given back first. `false`: the
    /// caller was told to stop while it waited. A grant that holds nothing asks for nothing.
    pub fn ask_again(
        &self,
        units: std::collections::BTreeMap<String, u32>,
        stop: &dyn Fn() -> Option<StopWhy>,
    ) -> bool {
        let rest = self.units();
        self.give(&rest);
        let Some(giver) = self.lock().giver.clone() else {
            return true;
        };
        match giver.ask_again(units.clone(), &|| stop().is_none(), &mut |_| {}) {
            Ok(lease) => {
                let mut g = self.lock();
                g.giver = Some(lease.giver());
                g.held = lease.units();
                g.owned = Some(lease);
                if let Some((dir, id)) = &g.hold {
                    set_hold_units(dir, id, &g.held);
                }
                true
            }
            Err(_) => false,
        }
    }
    /// Ask for `units` again if every unit of the lease has already been given back (its jobs
    /// finished and the run is made again). `false`: told to stop while it waited.
    pub fn regain(
        &self,
        units: std::collections::BTreeMap<String, u32>,
        stop: &dyn Fn() -> Option<StopWhy>,
    ) -> bool {
        let released = self.lock().giver.as_ref().is_some_and(|g| !g.is_held());
        !released || self.ask_again(units, stop)
    }
    /// How many runner units the lease was given when it asked for 1 to N, if it was flexible.
    pub fn granted_runners(&self) -> Option<u32> {
        self.lock().owned.as_ref().and_then(|l| l.granted_runners())
    }
    /// Give the units back: the run is not going to GitHub after all (`fallback = "local"`).
    /// Returns whether it held any. A lease with no runner units, this machine's own slots, is
    /// kept: the run goes on under it (#1149).
    pub fn give_back(&self) -> bool {
        let mut g = self.lock();
        if g.owned
            .as_ref()
            .is_some_and(|l| l.units().is_empty() && l.holds_local_slot())
        {
            return false;
        }
        let held = g.owned.take();
        g.giver = None;
        g.held.clear();
        held.is_some()
    }
    /// What of `uses` the grant's own lease does not hold: all of it when it owns none.
    pub fn shortfall(
        &self,
        uses: &std::collections::BTreeMap<String, u32>,
    ) -> std::collections::BTreeMap<String, u32> {
        self.lock()
            .owned
            .as_ref()
            .map_or_else(|| uses.clone(), |l| l.shortfall(uses))
    }
    /// The id of the grant's own lease, when it holds a slot of this machine.
    pub fn local_slot_lease(&self) -> Option<u64> {
        self.lock()
            .owned
            .as_ref()
            .filter(|l| l.holds_local_slot())
            .map(|l| l.id())
    }
    /// For the tests of the push itself.
    #[cfg(test)]
    pub fn test() -> Self {
        Grant::attached()
    }
}

/// Rewrite the units of the hold of run `id` to `units`, keeping the rest of it.
fn set_hold_units(dir: &Path, id: &str, units: &std::collections::BTreeMap<String, u32>) {
    let Ok(kv) = read_kv(&hold_file(dir, id)) else {
        return;
    };
    let text: Vec<String> = units.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let _ = write_kv(
        &hold_file(dir, id),
        &[
            ("key", kv.get("key").cloned().unwrap_or_default()),
            ("label", kv.get("label").cloned().unwrap_or_default()),
            ("units", text.join(",")),
            ("record", kv.get("record").cloned().unwrap_or_default()),
            ("kind", kv.get("kind").cloned().unwrap_or_default()),
            ("task", kv.get("task").cloned().unwrap_or_default()),
        ],
    );
}

/// A lease request for `needs` of a project whose suites run on GitHub: the machine's
/// `compile`, `test-suite` and `test-small` (and the whole machine) are replaced by the runner
/// pool units, so the run waits for no machine gate and holds none of the machine. `None`: the
/// project's runs are local, or `needs` holds none of those.
pub fn pool_needs(
    d: &Daemon,
    project: &str,
    needs: &mason_scheduler::Needs,
    task: Option<mason_core::TaskId>,
) -> Option<mason_scheduler::Needs> {
    const MACHINE: [&str; 3] = ["compile", "test-suite", "test-small"];
    if !needs.exclusive && !MACHINE.iter().any(|m| needs.amounts.contains_key(*m)) {
        return None;
    }
    // The effective mode of the task (task, then project, then global; #761, #831): never
    // `executor_for(.., None)`, which is always local. A step whose suites are split between
    // here and GitHub by their own rules is not looked into: it follows this mode.
    let units = match executor_task(d, project, task.map(|t| t.0)) {
        Executor::GitHub { runs_on } => units_for(&runs_on, 1),
        // A step bound for a node holds one unit of that node's resource and nothing of this
        // machine, whichever node the run then finds room on.
        Executor::Node { nid, .. } => node_units_of(&nid),
        Executor::Local => return None,
    };
    Some(with_units(needs, units))
}

/// Does `suite`, run for `task` (`None`: no task owns the run, a landing batch's), go to GitHub?
/// This is [`executor_for`] asked of a suite, the very call a run makes when it starts
/// (`suites::run_hosted_resuming`), so what a landing batch decides before it holds slots and
/// what the run then does come from one function (s943).
pub fn suite_goes_remote(
    d: &Daemon,
    project: &str,
    task: Option<mason_core::TaskId>,
    suite: &mason_testing::Suite,
) -> bool {
    executor_for(d, project, Kind::Suite, Some((task, suite))).is_off_machine()
}

/// A landing batch as the owner of broker requests it makes during its run (s943): the first
/// task's rank, which is how `Landing::hold` ranks the batch's own lease, and what ends a wait
/// (every member has left the landing step).
#[derive(Clone, Debug)]
pub struct BatchOwner {
    /// The batch's first task: the owner of its requests.
    pub task: mason_core::TaskId,
    pub rank: mason_scheduler::Rank,
    pub tasks: Vec<mason_core::TaskId>,
    pub runs: Vec<mason_core::StepRunId>,
    /// A wait ends when no member has an open run among `runs` (not for the post-land tier,
    /// whose tasks have left landing already).
    pub watch: bool,
}

impl BatchOwner {
    /// Is anything still waiting for this batch's request?
    pub fn alive(&self, d: &Daemon) -> bool {
        if !self.watch || self.runs.is_empty() {
            return true;
        }
        d.world().map_or(true, |w| {
            self.tasks
                .iter()
                .any(|t| w.open_run(*t).is_some_and(|r| self.runs.contains(&r)))
        })
    }
}

/// `needs` of this machine, taken for a landing batch's own run that its lease does not cover
/// (s943): ranked as the batch's first task, beside the batch's own lease, under the same
/// limits and CPU and thermal holds as any local run. Nothing to take gives `Ok(None)`.
pub fn batch_lease(
    d: &Arc<Daemon>,
    project: &str,
    owner: &BatchOwner,
    needs: mason_scheduler::Needs,
    label: &str,
) -> Result<Option<crate::sched::Lease>, String> {
    if !needs.exclusive && needs.amounts.is_empty() {
        return Ok(None);
    }
    let def = d.project_id_by_name(project).map(|p| d.current_def(p));
    for name in needs.amounts.keys() {
        if !name.starts_with("llm:") && !name.starts_with("device:") {
            let units = def
                .as_ref()
                .and_then(|def| def.resources.get(name).copied())
                .unwrap_or(1);
            d.broker.declare(name, units);
        }
    }
    let alive = || owner.alive(d);
    tracing::info!(project, %label, "a batch's run takes this machine's slots as the batch");
    d.broker
        .acquire_beside(owner.task, label, owner.rank, needs, &alive, &mut |_| {})
        .map(Some)
        .map_err(|_| format!("{label}: the wait for this machine's slots was ended"))
}

/// [`fallback_lease`] for a suite of a landing batch (s943): the batch holds nothing for a suite
/// bound for GitHub, so when it comes back here it takes what it holds when placed here, ranked
/// as the batch's own work and not behind everything.
pub fn fallback_lease_for_batch(
    d: &Arc<Daemon>,
    project: &str,
    owner: &BatchOwner,
    uses: &std::collections::BTreeMap<String, u32>,
    label: &str,
) -> Result<Option<crate::sched::Lease>, String> {
    // A lease the run took before it gave up GitHub (#1025) stands in for the batch's own.
    if let Some(l) = take_pretaken(d, project, Some(owner.task), uses) {
        return Ok(Some(l));
    }
    tracing::info!(project, %label, ?uses, "no GitHub runner: queueing for this machine's slots as the batch (fallback = local)");
    batch_lease(d, project, owner, crate::sched::needs_of(uses, None), label)
}

/// The rank of a fallback run: its task's own when the task is in a step, else the debts band
/// (a landed, failed or canceled task, or none: owed work, never the landing band, #835).
pub fn fallback_rank(
    d: &Daemon,
    task: Option<mason_core::TaskId>,
    owner: Option<&mason_core::Task>,
    def: Option<&mason_definition::Definition>,
) -> mason_scheduler::Rank {
    match (owner, def) {
        (Some(t), Some(def)) if t.state.is_step() => d.rank_for(t, def, &t.step),
        (Some(t), _) => {
            mason_scheduler::Rank::new(false, 0, 0, t.enqueued_at.0, t.id.0, false).debt()
        }
        _ => {
            mason_scheduler::Rank::new(false, 0, 0, now_ms(), task.map_or(0, |t| t.0), false).debt()
        }
    }
}

/// The machine slots a run takes when it was meant for GitHub but `fallback = "local"` sends it
/// here (no runner picked it up, or GitHub was down). A run bound for GitHub skips this
/// machine's queue (#759, #761), so without this it would start here at once, beside everything
/// else, past the slot limits and the CPU and thermal holds. `uses` is what the same run holds
/// when it is placed here; it is asked of the broker like any local run's, so those limits and
/// holds apply. Ranked as `task`'s own work, or behind everything when no task owns the run.
/// Beside: the task's step may still hold its own request. Nothing to take gives `Ok(None)`.
pub fn fallback_lease(
    d: &Arc<Daemon>,
    project: &str,
    task: Option<mason_core::TaskId>,
    uses: &std::collections::BTreeMap<String, u32>,
    label: &str,
) -> Result<Option<crate::sched::Lease>, String> {
    fallback_lease_until(d, project, task, uses, label, &|| false)
}

/// [`fallback_lease`] whose wait also ends when `stopped` says so (the owner skipped the task's
/// testing while its check queued here, #873).
pub fn fallback_lease_until(
    d: &Arc<Daemon>,
    project: &str,
    task: Option<mason_core::TaskId>,
    uses: &std::collections::BTreeMap<String, u32>,
    label: &str,
    stopped: &dyn Fn() -> bool,
) -> Result<Option<crate::sched::Lease>, String> {
    fallback_lease_under(d, project, task, uses, label, &Grant::attached(), stopped)
}

/// [`fallback_lease_until`] for a run whose caller holds `grant` until the run ends (#1149): the
/// slots that lease holds are the run's, and only what it lacks is asked of the broker, beside
/// it. Asking for all of `uses` again made a check queue for the slot its own agent held, which
/// at a low limit deadlocked the whole queue. `Ok(None)`: nothing more to take.
pub fn fallback_lease_under(
    d: &Arc<Daemon>,
    project: &str,
    task: Option<mason_core::TaskId>,
    uses: &std::collections::BTreeMap<String, u32>,
    label: &str,
    grant: &Grant,
    stopped: &dyn Fn() -> bool,
) -> Result<Option<crate::sched::Lease>, String> {
    // A run that `Site::local_room` found room for took its lease before it gave up GitHub
    // (#1025): that one is used, not a second request.
    if let Some(l) = take_pretaken(d, project, task, uses) {
        tracing::info!(project, %label, ?uses, "no GitHub runner: running here on the lease taken before the GitHub run was cancelled (fallback = local)");
        return Ok(Some(l));
    }
    let short = grant.shortfall(uses);
    let Some((needs, rank, who)) = fallback_request(d, project, task, &short) else {
        if !uses.is_empty() {
            tracing::info!(project, %label, ?uses, "running here under the slots the run already holds");
        }
        return Ok(None);
    };
    let alive = || !stopped() && task_alive(d, task);
    tracing::info!(project, %label, uses = ?short, "no GitHub runner: queueing for this machine's slots (fallback = local)");
    match grant.local_slot_lease() {
        Some(held) => d
            .broker
            .acquire_for_check(who, label, rank, needs, held, &alive),
        None => d
            .broker
            .acquire_beside(who, label, rank, needs, &alive, &mut |_| {}),
    }
    .map(Some)
    .map_err(|_| format!("{label}: the wait for this machine's slots was ended"))
}

fn task_alive(d: &Daemon, task: Option<mason_core::TaskId>) -> bool {
    match task {
        Some(t) => d
            .world()
            .map(|w| w.tasks.get(&t).is_some_and(|x| x.state.is_step()))
            .unwrap_or(true),
        None => true,
    }
}

/// What a fallback asks the broker for: the needs, the rank and the owner, with the resources
/// declared. `None`: nothing to take.
fn fallback_request(
    d: &Arc<Daemon>,
    project: &str,
    task: Option<mason_core::TaskId>,
    uses: &std::collections::BTreeMap<String, u32>,
) -> Option<(
    mason_scheduler::Needs,
    mason_scheduler::Rank,
    mason_core::TaskId,
)> {
    let needs = crate::sched::needs_of(uses, None);
    if !needs.exclusive && needs.amounts.is_empty() {
        return None;
    }
    let def = d.project_id_by_name(project).map(|p| d.current_def(p));
    for name in needs.amounts.keys() {
        if !name.starts_with("llm:") && !name.starts_with("device:") {
            let units = def
                .as_ref()
                .and_then(|def| def.resources.get(name).copied())
                .unwrap_or(1);
            d.broker.declare(name, units);
        }
    }
    let owner = task.and_then(|t| d.world().ok().and_then(|w| w.tasks.get(&t).cloned()));
    let rank = fallback_rank(d, task, owner.as_ref(), def.as_deref());
    Some((needs, rank, task.unwrap_or(mason_core::TaskId(0))))
}

/// A lease taken ahead of the fallback (#1025), waiting for the run's caller to ask for it.
struct Pretaken {
    daemon: usize,
    project: String,
    task: Option<mason_core::TaskId>,
    uses: std::collections::BTreeMap<String, u32>,
    at: std::time::Instant,
    lease: crate::sched::Lease,
}

static PRETAKEN: std::sync::Mutex<Vec<Pretaken>> = std::sync::Mutex::new(Vec::new());

/// A lease nobody asked for within this long is given back.
const PRETAKEN_KEEP: Duration = Duration::from_secs(600);

fn take_pretaken(
    d: &Arc<Daemon>,
    project: &str,
    task: Option<mason_core::TaskId>,
    uses: &std::collections::BTreeMap<String, u32>,
) -> Option<crate::sched::Lease> {
    let me = Arc::as_ptr(d) as usize;
    let mut v = PRETAKEN.lock().unwrap_or_else(|e| e.into_inner());
    v.retain(|p| p.at.elapsed() < PRETAKEN_KEEP);
    let same = |p: &Pretaken| p.daemon == me && p.project == project && &p.uses == uses;
    let i = v
        .iter()
        .position(|p| same(p) && p.task == task)
        .or_else(|| v.iter().position(same))?;
    Some(v.remove(i).lease)
}

/// Take this machine's slots for a fallback now, without queueing (#1025), and keep the lease
/// for [`fallback_lease`]. `Err(why)` when it is not granted at once: the run then stays on
/// GitHub. Nothing to take is `Ok`.
fn try_take_fallback(
    d: &Arc<Daemon>,
    project: &str,
    task: Option<mason_core::TaskId>,
    uses: &std::collections::BTreeMap<String, u32>,
) -> Result<(), String> {
    let Some((needs, rank, who)) = fallback_request(d, project, task, uses) else {
        return Ok(());
    };
    // A lease nobody collected is given back now, not only when another run asks for one.
    PRETAKEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|p| p.at.elapsed() < PRETAKEN_KEEP);
    let lease = d
        .broker
        .try_acquire_beside(who, "fallback (no GitHub runner, run here)", rank, needs)
        .map_err(|_| "this machine could not grant its slots at once".to_string())?;
    PRETAKEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Pretaken {
            daemon: Arc::as_ptr(d) as usize,
            project: project.to_string(),
            task,
            uses: uses.clone(),
            at: std::time::Instant::now(),
            lease,
        });
    Ok(())
}

/// [`fallback_lease`], unless `covered`: the caller (a landing batch) already holds this
/// machine's slots for this very run, so the run goes on under that hold. Asking the broker
/// again would queue it behind everything, an exclusive request included, and that request waits
/// for the machine to drain, that is for the caller's own hold: a cycle that froze the local
/// queue on 2026-11-22 (s911, decision 0904).
pub fn fallback_lease_unless_covered(
    d: &Arc<Daemon>,
    project: &str,
    task: Option<mason_core::TaskId>,
    uses: &std::collections::BTreeMap<String, u32>,
    label: &str,
    covered: bool,
) -> Result<Option<crate::sched::Lease>, String> {
    if covered {
        tracing::info!(project, %label, "no GitHub runner: running here under the caller's hold of this machine's slots (fallback = local)");
        return Ok(None);
    }
    fallback_lease(d, project, task, uses, label)
}

/// What a remote run holds, written when it starts and removed when it ends, so a daemon that
/// restarts counts the units of runs still out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteHold {
    /// `task N` when a task's step holds them (its lease takes them over when it asks again),
    /// else the run's id.
    pub key: String,
    pub label: String,
    pub units: std::collections::BTreeMap<String, u32>,
    /// The run's record (`<id>.remote`): the run has ended when it is gone or collected.
    pub record: PathBuf,
    /// What the run is for: `check`, `test_step`, `batch` or `debt` (empty in a hold written
    /// before the monitor showed it).
    pub kind: String,
    /// The task the run is for; 0 for a run no task owns.
    pub task: u64,
}

/// The URL of the run behind `record` (empty until GitHub has made it) and its runner label.
pub fn record_view(record: &Path) -> (String, String) {
    State::load(record).map_or_else(Default::default, |s| (s.url, s.runs_on))
}

/// Where the holds are kept, beside the step hosts' directory.
pub fn holds_dir(hosts: &Path) -> PathBuf {
    hosts.with_file_name("remote-holds")
}

fn hold_file(dir: &Path, id: &str) -> PathBuf {
    let clean: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir.join(format!("{clean}.kv"))
}

/// Record `hold` for run `id`.
pub fn write_hold(dir: &Path, id: &str, h: &RemoteHold) {
    let _ = std::fs::create_dir_all(dir);
    let units: Vec<String> = h.units.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let _ = write_kv(
        &hold_file(dir, id),
        &[
            ("key", h.key.clone()),
            ("label", h.label.clone()),
            ("units", units.join(",")),
            ("record", h.record.display().to_string()),
            ("kind", h.kind.clone()),
            ("task", h.task.to_string()),
        ],
    );
}

/// Forget the hold of run `id`: it has ended.
pub fn remove_hold(dir: &Path, id: &str) {
    let _ = std::fs::remove_file(hold_file(dir, id));
}

/// Where a run stands, for the monitor: `queued` (pushed, or on GitHub with no runner yet),
/// `running`, or `done`; its GitHub URL, and when a runner took it.
pub fn run_stage(record: &Path) -> Option<(String, String, Option<u64>)> {
    let s = State::load(record)?;
    let stage = if s.collected {
        "done"
    } else if s.picked_ms.is_some() {
        "running"
    } else {
        "queued"
    };
    Some((stage.into(), s.url, s.picked_ms))
}

/// Has the run behind this record ended? A missing record is a run that was abandoned or
/// never began; a collected one is done.
pub fn record_ended(record: &Path) -> bool {
    State::load(record).is_none_or(|s| s.collected || s.released)
}

/// The holds of runs whose record says they are still out.
pub fn running_holds(dir: &Path) -> Vec<RemoteHold> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut out: Vec<RemoteHold> = rd
        .flatten()
        .filter_map(|e| {
            let kv = read_kv(&e.path()).ok()?;
            let record = PathBuf::from(kv.get("record")?);
            if record_ended(&record) {
                let _ = std::fs::remove_file(e.path());
                return None;
            }
            let units = kv
                .get("units")?
                .split(',')
                .filter_map(|p| {
                    let (k, v) = p.split_once('=')?;
                    Some((k.to_string(), v.parse().ok()?))
                })
                .collect();
            Some(RemoteHold {
                key: kv.get("key")?.clone(),
                label: kv.get("label").cloned().unwrap_or_default(),
                units,
                record,
                kind: kv.get("kind").cloned().unwrap_or_default(),
                task: kv.get("task").and_then(|t| t.parse().ok()).unwrap_or(0),
            })
        })
        .collect();
    out.sort_by(|a, b| a.record.cmp(&b.record));
    out
}

/// The id of the run a record belongs to (`<id>.remote`).
fn record_id(record: &Path) -> Option<String> {
    Some(record.file_stem()?.to_str()?.to_string())
}

/// The project in a run's ref (`mason/run/<project>/<id>`).
fn ref_project(ref_name: &str) -> Option<&str> {
    ref_name.strip_prefix("mason/run/")?.split('/').next()
}

/// Look at an adopted run on GitHub; if it has completed, delete its ref, mark its record
/// released and remove its hold. A run still going, or one GitHub cannot be asked about now, is
/// kept. A run that was cancelled has nothing to collect: its record goes. Returns whether the
/// hold was freed.
pub fn reap_hold(site: &Site, holds: &Path, record: &Path) -> bool {
    reap_hold_etag(site, holds, record, &mut None)
}

/// [`reap_hold`], asking GitHub with the ETag of the last look (`etag` is updated): a 304 costs
/// no quota and leaves the hold as it is.
pub fn reap_hold_etag(site: &Site, holds: &Path, record: &Path, etag: &mut Option<String>) -> bool {
    let Some(id) = record_id(record) else {
        return false;
    };
    let Some(mut st) = State::load(record) else {
        remove_hold(holds, &id);
        return true;
    };
    if st.collected || st.released {
        remove_hold(holds, &id);
        return true;
    }
    let run = match st.run_id {
        Some(rid) => match site.gh.run_changed(rid, etag.as_deref()) {
            Ok(Some((r, e))) => {
                *etag = e;
                Some(r)
            }
            Ok(None) | Err(_) => return false,
        },
        None => match site.gh.runs_for_sha(&st.sha) {
            Ok(v) => v.into_iter().find(|r| r.branch == st.ref_name),
            Err(_) => return false,
        },
    };
    let Some(run) = run else {
        // GitHub lists a run as soon as it exists, queued or not. A record that never learned
        // its run's id and still has none on GitHub long after its push never had one (the
        // dispatch was lost): nobody will collect it, and kept, it held a runner unit across
        // every restart (debt602, 36 h on 2026-11-24). Give it up.
        if st.run_id.is_none()
            && site.clock.now_ms().saturating_sub(st.pushed_ms) >= orphan_after_ms(site)
        {
            tracing::info!(
                run = %id,
                "no GitHub run was ever made for {}; freeing its runner unit",
                st.ref_name
            );
            st.released = true;
            delete_ref(site, &mut st, record);
            let _ = std::fs::remove_file(record);
            remove_hold(holds, &id);
            return true;
        }
        return false;
    };
    if run.status != "completed" {
        return false;
    }
    st.run_id = Some(run.id);
    if st.url.is_empty() {
        st.url = run.url.clone();
    }
    let cancelled = run.conclusion.as_deref() == Some("cancelled");
    st.released = true;
    delete_ref(site, &mut st, record);
    if cancelled {
        let _ = std::fs::remove_file(record);
    }
    remove_hold(holds, &id);
    true
}

/// How long after its push a record with no run on GitHub is given up: the pickup timeout, and
/// never under half an hour.
fn orphan_after_ms(site: &Site) -> u64 {
    (site.pickup_timeout.as_millis() as u64).max(30 * 60 * 1000)
}

/// Cancel an adopted run nobody will collect, delete its ref, end its record and remove its
/// hold.
pub fn cancel_hold(site: &Site, holds: &Path, record: &Path) {
    let Some(id) = record_id(record) else { return };
    if let Some(mut st) = State::load(record) {
        cancel(site, &st);
        delete_ref(site, &mut st, record);
    }
    let _ = std::fs::remove_file(record);
    remove_hold(holds, &id);
}

/// Does the check record beside run `record` (`<id>.check`) say nobody will collect the run: it
/// was superseded by a newer call, or its result was given? Checks have such a record; a step's
/// run has none.
fn check_record_ended(record: &Path) -> bool {
    let kv = read_kv(&record.with_extension("check"));
    kv.is_ok_and(|kv| {
        kv.contains_key("superseded") || kv.get("delivered").map(String::as_str) == Some("1")
    })
}

/// `(task, run, prefix, number)` of an agent check's run id (`t731r4599-check-cargo-workspace2`).
fn check_parts(id: &str) -> Option<(u64, u64, String, u64)> {
    let (task, run) = crate::hostedcheck::check_owner(id)?;
    let digits = id.chars().rev().take_while(char::is_ascii_digit).count();
    let (prefix, n) = id.split_at(id.len() - digits);
    Some((task, run, prefix.to_string(), n.parse().unwrap_or(0)))
}

/// The family of a test step's run id (`t7r9-u-unit`, `t7r9-u1700000000000-unit`; `-r` for a
/// rerun of failures) and its attempt number in it (0 for the first).
fn step_parts(id: &str) -> Option<(String, u64)> {
    for tag in ["-u", "-r"] {
        let mut from = 0;
        while let Some(i) = id[from..].find(tag) {
            let at = from + i + tag.len();
            let digits = id[at..].chars().take_while(char::is_ascii_digit).count();
            if id[at + digits..].starts_with('-') && id[..from + i].contains('r') {
                let n = id[at..at + digits].parse().unwrap_or(0);
                return Some((format!("{}{tag}{}", &id[..from + i], &id[at + digits..]), n));
            }
            from = at;
        }
    }
    None
}

/// Which of the adopted runs (key, record) nobody can collect: a check whose task's open run is
/// another one or none; a step run of a task with no open run; a check superseded by a newer
/// one of the same prefix for the same run, or one whose check record on disk says it was
/// superseded or its result given (#823). `open_run` gives a task's open run.
pub fn unwanted(
    adopted: &[(String, PathBuf)],
    open_run: &dyn Fn(u64) -> Option<u64>,
) -> Vec<PathBuf> {
    judge(adopted, &[], open_run)
        .into_iter()
        .map(|(r, _)| r)
        .collect()
}

/// [`unwanted`], with why, and counting `live` too: the runs this daemon is collecting. A run
/// started by a call the agent made again (a bridge without call-id replay, 2026-11-23) is a
/// newer one of the same suite, and it is `live`, never adopted: only with it in view is the
/// older, adopted one seen to be orphaned.
pub fn judge(
    adopted: &[(String, PathBuf)],
    live: &[(String, PathBuf)],
    open_run: &dyn Fn(u64) -> Option<u64>,
) -> Vec<(PathBuf, String)> {
    type Top = (u64, String);
    let mut out = vec![];
    let mut newest: std::collections::HashMap<(u64, u64, String), Top> = Default::default();
    let mut newest_step: std::collections::HashMap<(String, String), Top> = Default::default();
    for (key, record) in live.iter().chain(adopted.iter()) {
        let Some(id) = record_id(record) else {
            continue;
        };
        if let Some((t, r, p, n)) = check_parts(&id) {
            let e = newest.entry((t, r, p)).or_insert((n, id.clone()));
            if n > e.0 {
                *e = (n, id.clone());
            }
        } else if key.starts_with("task ") {
            if let Some((fam, n)) = step_parts(&id) {
                let e = newest_step
                    .entry((key.clone(), fam))
                    .or_insert((n, id.clone()));
                if n > e.0 {
                    *e = (n, id.clone());
                }
            }
        }
    }
    for (key, record) in adopted {
        let id = record_id(record).unwrap_or_default();
        let why = if let Some((t, r, p, n)) = check_parts(&id) {
            let (top, top_id) = &newest[&(t, r, p)];
            if open_run(t) != Some(r) {
                Some("its task's open run is another one or none".to_string())
            } else if *top > n {
                Some(format!("superseded by {top_id}"))
            } else if check_record_ended(record) {
                Some("its check record says superseded or delivered".to_string())
            } else {
                None
            }
        } else {
            let task = key
                .strip_prefix("task ")
                .and_then(|t| t.parse::<u64>().ok());
            let newer = key
                .starts_with("task ")
                .then(|| step_parts(&id))
                .flatten()
                .and_then(|(fam, n)| {
                    let (top, top_id) = &newest_step[&(key.clone(), fam)];
                    (*top > n).then(|| top_id.clone())
                });
            match (task, newer) {
                (Some(t), _) if open_run(t).is_none() => Some("its task has no open run".into()),
                (_, Some(top)) => Some(format!("superseded by {top}")),
                _ => None,
            }
        };
        if let Some(why) = why {
            out.push((record.clone(), why));
        }
    }
    out
}

/// The site of the project a run's ref names.
fn site_of_state(d: &Arc<Daemon>, st: &State) -> Option<Site> {
    let project = ref_project(&st.ref_name)?;
    let pid = d
        .projects()
        .values()
        .find(|p| {
            p.services.remote.is_some() && ref_name(&p.id, "") == format!("mason/run/{project}/")
        })
        .map(|p| p.id.clone())?;
    let p = project_cfg(d, &pid)?;
    site_for(d, &pid, &p.path)
}

/// Cancel the GitHub run behind `record` now (a check superseded on another tree, #823): its
/// run is cancelled, its ref deleted, its record ended and its hold removed, which gives the
/// units back. `false`: there is no such run out.
pub fn cancel_record(d: &Arc<Daemon>, record: &Path) -> bool {
    let Some(st) = State::load(record) else {
        return false;
    };
    if st.collected || st.released {
        return false;
    }
    let Some(site) = site_of_state(d, &st) else {
        return false;
    };
    cancel_hold(&site, &holds_dir(&d.cfg.hosts_dir()), record);
    true
}

/// Each run holding runner units, with its task and whether anything will still collect it
/// (`mason resources`, #823): `wanted` is false for a run [`unwanted`] names.
pub fn holder_views(d: &Daemon) -> Vec<serde_json::Value> {
    let holds = running_holds(&holds_dir(&d.cfg.hosts_dir()));
    let open: std::collections::HashMap<u64, u64> = d
        .world()
        .map(|w| {
            w.tasks
                .keys()
                .filter_map(|t| Some((t.0, w.open_run(*t)?.0)))
                .collect()
        })
        .unwrap_or_default();
    let pairs: Vec<(String, PathBuf)> = holds
        .iter()
        .map(|h| (h.key.clone(), h.record.clone()))
        .collect();
    let gone = unwanted(&pairs, &|t| open.get(&t).copied());
    holds
        .iter()
        .map(|h| {
            serde_json::json!({
                "label": h.label,
                "kind": h.kind,
                "task": (h.task != 0).then_some(h.task),
                "wanted": !gone.contains(&h.record),
            })
        })
        .collect()
}

/// How often an adopted run is asked about at GitHub, before the budget stretches it.
pub const REAP_EVERY_S: u64 = 120;

/// What the reaper remembers of each adopted run: when to look again and the ETag of the last
/// look.
#[derive(Default)]
pub struct ReapMemo {
    next_ms: std::collections::HashMap<PathBuf, u64>,
    etag: std::collections::HashMap<PathBuf, Option<String>>,
}

/// One pass over `adopted` (see [`reap_adopted`]). Nothing is sent while `quota_wait` says the
/// quota is used up. A run is cancelled when [`judge`] says nobody can collect it; otherwise it
/// is looked at only once its next time has come (`every_s` after the last look), with its ETag.
/// Returns what was done, for the log.
#[allow(clippy::too_many_arguments)]
pub fn reap_pass(
    adopted: &[(String, PathBuf)],
    live: &[(String, PathBuf)],
    open_run: &dyn Fn(u64) -> Option<u64>,
    site_of: &dyn Fn(&Path) -> Option<Site>,
    holds: &Path,
    memo: &mut ReapMemo,
    now_ms: u64,
    every_s: u64,
    quota_wait: Option<u64>,
) -> Vec<String> {
    let mut done = vec![];
    if quota_wait.is_some() {
        return done;
    }
    let gone = judge(adopted, live, open_run);
    memo.next_ms
        .retain(|r, _| adopted.iter().any(|(_, a)| a == r));
    memo.etag.retain(|r, _| adopted.iter().any(|(_, a)| a == r));
    for (_, record) in adopted {
        if State::load(record).is_none() {
            if let Some(id) = record_id(record) {
                remove_hold(holds, &id);
            }
            continue;
        }
        let cancel_why = gone.iter().find(|(r, _)| r == record).map(|(_, w)| w);
        if cancel_why.is_none() && memo.next_ms.get(record).is_some_and(|t| now_ms < *t) {
            continue;
        }
        let Some(site) = site_of(record) else {
            continue;
        };
        if let Some(why) = cancel_why {
            cancel_hold(&site, holds, record);
            memo.next_ms.remove(record);
            memo.etag.remove(record);
            done.push(format!(
                "cancelled the GitHub run of {}: {why}",
                record.display()
            ));
        } else {
            memo.next_ms.insert(record.clone(), now_ms + every_s * 1000);
            let etag = memo.etag.entry(record.clone()).or_default();
            if reap_hold_etag(&site, holds, record, etag) {
                memo.next_ms.remove(record);
                memo.etag.remove(record);
                done.push(format!(
                    "freed the units of the finished GitHub run {}",
                    record.display()
                ));
            }
        }
    }
    done
}

/// One pass over the runs adopted at start: free those GitHub has finished, cancel those no one
/// can collect (an older run of a suite that a newer call has run again counts: the agent's
/// call waits on the newer one). Off the caller's thread, one pass at a time. A run is asked
/// about every [`REAP_EVERY_S`] at most, stretched by the budget as background work, with its
/// ETag, and not at all while the quota is used up.
pub fn reap_adopted(d: &Arc<Daemon>) {
    static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static MEMO: std::sync::Mutex<Option<ReapMemo>> = std::sync::Mutex::new(None);
    if !any_remote(d) {
        return;
    }
    let adopted = d.broker.adopted_remote();
    if adopted.is_empty() || BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let d = d.clone();
    std::thread::spawn(move || {
        let _caller = github_budget::as_caller("remote-hold");
        let holds = holds_dir(&d.cfg.hosts_dir());
        let live: Vec<(String, PathBuf)> = running_holds(&holds)
            .into_iter()
            .map(|h| (h.key, h.record))
            .filter(|(_, r)| !adopted.iter().any(|(_, a)| a == r))
            .collect();
        // Without a world nothing is judged: every record would look unwanted.
        if let Ok(w) = d.world() {
            let open: std::collections::HashMap<u64, u64> = w
                .tasks
                .keys()
                .filter_map(|t| Some((t.0, w.open_run(*t)?.0)))
                .collect();
            let mut memo = MEMO
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .unwrap_or_default();
            let done = reap_pass(
                &adopted,
                &live,
                &|t| open.get(&t).copied(),
                &|rec| site_of_state(&d, &State::load(rec)?),
                &holds,
                &mut memo,
                now_ms(),
                github_budget::paced(REAP_EVERY_S, Priority::Background),
                github_budget::wait_s(),
            );
            for line in done {
                tracing::info!("{line}");
            }
            *MEMO.lock().unwrap_or_else(|e| e.into_inner()) = Some(memo);
        }
        BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

/// A hold that is written while a remote run is out and removed when this is dropped.
pub struct HoldGuard {
    dir: PathBuf,
    id: String,
    hosts: PathBuf,
    record: PathBuf,
}

impl HoldGuard {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        hosts: &Path,
        spec: &RunSpec,
        key: &str,
        label: &str,
        shards: u32,
        dir: &Path,
        kind: &str,
        task: u64,
    ) -> Self {
        let holds = holds_dir(hosts);
        crate::remote_runs::started(
            hosts,
            &spec.id,
            key,
            kind,
            &spec.suite,
            &spec.runs_on,
            shards,
            &record_path(dir, &spec.id),
        );
        write_hold(
            &holds,
            &spec.id,
            &RemoteHold {
                key: key.to_string(),
                label: label.to_string(),
                units: units_for(&spec.runs_on, shards),
                record: record_path(dir, &spec.id),
                kind: kind.to_string(),
                task,
            },
        );
        HoldGuard {
            dir: holds,
            id: spec.id.clone(),
            hosts: hosts.to_path_buf(),
            record: record_path(dir, &spec.id),
        }
    }
}

impl Drop for HoldGuard {
    fn drop(&mut self) {
        // How it ended, for the monitor, before the record can go.
        let outcome = State::load(&self.record).filter(|s| s.collected).map(|s| {
            if s.code == 0 && s.signal == 0 {
                "green"
            } else {
                "red"
            }
        });
        crate::remote_runs::finished(&self.hosts, &self.id, outcome);
        remove_hold(&self.dir, &self.id);
    }
}

// ---- the seams --------------------------------------------------------------------------------

/// The spec of a suite or check run, from its pinned definition.
#[allow(clippy::too_many_arguments)]
pub fn spec_for(
    project: &str,
    id: &str,
    suite: Option<&mason_testing::Suite>,
    cmd: &str,
    limit: Duration,
    env: &[(String, String)],
    runs_on: &str,
    tree_key: &str,
) -> RunSpec {
    RunSpec {
        id: id.to_string(),
        project: project.to_string(),
        suite: suite.map(|s| s.name.clone()).unwrap_or_default(),
        build: None,
        command: cmd.to_string(),
        rerun: suite.and_then(|s| s.rerun.clone()),
        results_file: suite.and_then(|s| s.results_file.clone()),
        limit,
        env: crate::jobs::with_run_budget(env.to_vec(), limit),
        runs_on: runs_on.to_string(),
        tree_key: tree_key.to_string(),
        shards: 0,
        only: vec![],
        shard_lists: vec![],
        shard_command: None,
        partial_lists: false,
        shards_wanted: 0,
    }
}

/// The project's `[remote_runs] test_env` added to the run's environment; a name the run
/// already sets keeps its own value.
pub fn with_site_env(mut spec: RunSpec, site: &Site) -> RunSpec {
    for (k, v) in &site.test_env {
        if !spec.env.iter().any(|(n, _)| n == k) {
            spec.env.push((k.clone(), v.clone()));
        }
    }
    spec
}

/// The suite's build as its own field: the runner runs it in a separate `sh -c` before the
/// command and before each shard's command, never joined with `&&` (two leading `cd`s break).
pub fn with_build(mut spec: RunSpec, build: Option<&str>) -> RunSpec {
    spec.build = build.map(str::to_string);
    spec
}

/// A run for a seam: `Ok(Some(outcome))`, `Ok(None)` to take the local path (the run could not
/// go remote and the project's `fallback` is `local`), or an error. An environmental failure is
/// an `Err` made by [`env_err`].
pub fn run_or_fallback(
    site: &Site,
    spec: &RunSpec,
    dir: &Path,
    stop: &dyn Fn() -> Option<StopWhy>,
    grant: &Grant,
) -> Result<Option<Outcome>, String> {
    let said = |t: &str| {
        if let Some(f) = &site.on_phase {
            f(t);
        }
    };
    let mut spec = spec.clone();
    // Granted too few runners: wait on GitHub for enough when this machine cannot run it.
    if let Some(why) = spec.too_few_runners() {
        match wait_for_grant(
            site,
            &spec,
            min_shards(spec.shards_wanted),
            &why,
            stop,
            grant,
        ) {
            Waited::Ready(s, note) => {
                said(&note);
                spec = *s;
            }
            Waited::Stopped(why) => return Ok(Some(Outcome::Stopped(why))),
            Waited::NotNeeded => {}
        }
    }
    match execute_sharded(site, &spec, dir, stop, grant) {
        Ok(o) => Ok(Some(o)),
        Err(Fail::Environmental(why)) if site.fallback_local => {
            if let Some(b) = barred_here(site) {
                let why = format!("{why}; not running it here: this Mac can't run it ({b})");
                tracing::warn!(run = %spec.id, "{why}");
                said(&why);
                return Err(env_err(&why));
            }
            tracing::warn!(run = %spec.id, "{why}; running it here instead (fallback = local)");
            // The run is not going to GitHub: its runner units go back before it takes local
            // slots (#818).
            grant.give_back();
            Ok(None)
        }
        Err(Fail::Environmental(why)) => Err(env_err(&why)),
        Err(Fail::Error(e)) => Err(e),
    }
}

/// Why this machine cannot run a run at all now (a hold, or a slot of limit 0), when the site
/// can fall back to it and says so. `None`: it can, or the site never falls back here.
fn barred_here(site: &Site) -> Option<String> {
    if !site.fallback_local {
        return None;
    }
    site.local_barred.as_ref().and_then(|f| f())
}

/// What waiting for a bigger grant came to.
enum Waited {
    /// Granted `want` runner units: the run split for them, and what to tell about it.
    Ready(Box<RunSpec>, String),
    /// Told to stop while it waited.
    Stopped(StopWhy),
    /// Nothing to wait for: the run may fall back to this machine (it can run it there), or the
    /// site has no way to split it again.
    NotNeeded,
}

/// A run GitHub cannot take at the grant it has, and this machine cannot run either (limit 0,
/// or held), waits on GitHub for a grant of at least `want` runner units instead of falling back
/// into a queue that never starts it. The units it holds go back first and nothing is held while
/// it waits: the broker's queue grants `want` when they are free, at the lease's own rank and
/// from when it first waited (#820). Then the run is split again for what it was given.
fn wait_for_grant(
    site: &Site,
    spec: &RunSpec,
    want: u32,
    why: &str,
    stop: &dyn Fn() -> Option<StopWhy>,
    grant: &Grant,
) -> Waited {
    let Some(resplit) = site.resplit.as_ref() else {
        return Waited::NotNeeded;
    };
    let Some(barred) = barred_here(site) else {
        return Waited::NotNeeded;
    };
    let note = format!(
        "{why}; this Mac can't run it ({barred}), so it waits on GitHub for at least {want}"
    );
    tracing::warn!(run = %spec.id, "{note}");
    if let Some(f) = &site.on_phase {
        f(&note);
    }
    if !grant.ask_again(units_for(&spec.runs_on, want), stop) {
        return Waited::Stopped(stop().unwrap_or(StopWhy::Gone));
    }
    Waited::Ready(Box::new(resplit(spec.clone(), want)), note)
}

/// Why a run ended without a result about the code (#873), or `None` when it gave one (green,
/// or red with a test named). Not a result: an outage, no runner within `pickup_timeout`, a
/// refused push, a run without an artifact, and a run cancelled at the runner limit that named
/// no failing test. A run that was stopped on purpose is not a miss.
pub fn no_result(r: &Result<Outcome, Fail>, limit: Duration) -> Option<String> {
    match r {
        Err(Fail::Environmental(why)) => Some(why.clone()),
        // A cut-off run that names a failing test, or tests that started and never finished,
        // answers about the code: a hang is a result.
        Ok(Outcome::Done(f))
            if f.timed_out
                && crate::rerun::failing_names(&f.output).is_empty()
                && crate::suites::still_running(&f.output).is_empty() =>
        {
            Some(format!(
                "the run reached the runner limit of {} s without naming a failing test",
                limit.as_secs()
            ))
        }
        _ => None,
    }
}

/// A run that is not to go (again) to GitHub: the local path when the project's `fallback` is
/// `local`, else an [`env_err`]. Told to `said` and logged.
fn no_github_run(
    site: &Site,
    spec: &RunSpec,
    why: String,
    said: &dyn Fn(&str),
    grant: &Grant,
    mut notes: Vec<String>,
) -> Result<(Option<Outcome>, Vec<String>), String> {
    tracing::warn!(run = %spec.id, "{why}");
    // A machine that cannot run it is no place to send it (limit 0, or held for heat): the run
    // is refused as it is when there is no fallback, and tried again later.
    if let Some(b) = barred_here(site) {
        let why = format!("{why}; not running it here: this Mac can't run it ({b})");
        said(&why);
        return Err(env_err(&why));
    }
    if site.fallback_local {
        let note = format!("{why}; running it on this Mac");
        said(&note);
        notes.push(note);
        grant.give_back();
        return Ok((None, notes));
    }
    said(&why);
    Err(env_err(&why))
}

/// A run that did not give a result, tried again once on GitHub (#873). Both misses are told to
/// `said`, which shows them in the check's progress. `Ok((outcome, notes))`: `outcome` is the
/// result of the run that gave one; `None` means both runs missed and the project's `fallback`
/// is `local`, so the caller runs it here. Two misses without that give an [`env_err`] worded
/// "no result (infrastructure): ...", never a failure of the code.
pub fn run_retrying(
    site: &Site,
    spec: &RunSpec,
    dir: &Path,
    stop: &dyn Fn() -> Option<StopWhy>,
    said: &dyn Fn(&str),
    grant: &Grant,
) -> Result<(Option<Outcome>, Vec<String>), String> {
    let mut notes: Vec<String> = vec![];
    let mut whys: Vec<String> = vec![];
    let mut spec = spec.clone();
    let spec = &mut spec;
    // Too few runners for the suite to finish inside its limit: not pushed at that size, and not
    // tried again either (the same shape would be refused again). When this machine cannot run
    // it, it waits on GitHub for enough runners instead of going here.
    if let Some(why) = spec.too_few_runners() {
        match wait_for_grant(
            site,
            spec,
            min_shards(spec.shards_wanted),
            &why,
            stop,
            grant,
        ) {
            Waited::Ready(s, note) => {
                *spec = *s;
                said(&note);
                notes.push(note);
            }
            Waited::Stopped(w) => return Ok((Some(Outcome::Stopped(w)), notes)),
            Waited::NotNeeded => return no_github_run(site, spec, why, said, grant, notes),
        }
    }
    let mut attempt = spec.clone();
    let mut waited_for_more = false;
    for n in 0..2u32 {
        if n > 0 {
            if let Some(why) = stop() {
                return Ok((Some(Outcome::Stopped(why)), notes));
            }
            attempt = spec.clone();
            attempt.id = format!("{}-r{n}", spec.id);
            let note = format!(
                "no result from the first run ({}); running it again on GitHub",
                whys[0]
            );
            tracing::warn!(run = %spec.id, "{note}");
            said(&note);
            notes.push(note);
        }
        let r = execute_sharded(site, &attempt, dir, stop, grant);
        match no_result(&r, spec.limit) {
            None => {
                return match r {
                    Ok(o) => Ok((Some(o), notes)),
                    Err(Fail::Environmental(why)) => Err(env_err(&why)),
                    Err(Fail::Error(e)) => Err(e),
                }
            }
            // A favor-remote run that came back here on purpose (#833) is the placement mode's
            // answer, not a miss to retry on GitHub.
            Some(why) if n == 0 && site.fallback_local && why.contains("(favor-remote:") => {
                said(&format!("{why}; running it on this Mac"));
                grant.give_back();
                return Ok((None, notes));
            }
            // A run that hit the runner's limit on fewer runners than the suite asked for would
            // hit it again as the same shape: it is not run again on GitHub.
            Some(why)
                if n == 0
                    && spec.short_of_runners()
                    && matches!(&r, Ok(Outcome::Done(f)) if f.timed_out) =>
            {
                let why = format!(
                    "the run hit the runner's limit on {} of {} shards; not running it again on one runner ({why})",
                    spec.runners(),
                    spec.shards_wanted
                );
                // A machine that cannot run it waits for every runner the suite asked for, and
                // the run goes out again at that size, once.
                if !waited_for_more {
                    waited_for_more = true;
                    match wait_for_grant(site, spec, spec.shards_wanted, &why, stop, grant) {
                        Waited::Ready(s, note) => {
                            *spec = *s;
                            said(&note);
                            notes.push(note);
                            whys.push(why);
                            continue;
                        }
                        Waited::Stopped(w) => return Ok((Some(Outcome::Stopped(w)), notes)),
                        Waited::NotNeeded => {}
                    }
                }
                return no_github_run(site, spec, why, said, grant, notes);
            }
            Some(why) => whys.push(why),
        }
    }
    let both = format!("{}; then {}", whys[0], whys[1]);
    if let Some(b) = barred_here(site) {
        let why = format!("{both}; not running it here: this Mac can't run it ({b})");
        said(&why);
        return Err(env_err(&why));
    }
    if site.fallback_local {
        let note = format!("no result from two runs on GitHub ({both}); running it on this Mac");
        tracing::warn!(run = %spec.id, "{note}");
        said(&note);
        grant.give_back();
        return Ok((None, notes));
    }
    Err(env_err(&both))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_sweep_deletes_only_old_refs_with_no_live_record() {
        use std::collections::{HashMap, HashSet};
        let day = 24 * 3600;
        let now = 100 * day;
        let refs: Vec<(String, String)> = [
            ("mason/run/p/old", "s1"),
            ("mason/run/p/fresh", "s2"),
            ("mason/run/p/kept", "s3"),
            ("mason/run/p/nodate", "s4"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        let times: HashMap<&str, u64> = [
            ("s1", now - 3 * day),
            ("s2", now - 3600),
            ("s3", now - 9 * day),
        ]
        .into();
        let live: HashSet<String> = ["mason/run/p/kept".to_string()].into();
        let mut deleted = vec![];
        let gone = sweep_run_refs(
            &refs,
            &live,
            now,
            &|sha| times.get(sha).copied(),
            &mut |r| {
                deleted.push(r.to_string());
                true
            },
        );
        assert_eq!(gone, vec!["mason/run/p/old"]);
        assert_eq!(deleted, vec!["mason/run/p/old"]);
    }

    #[test]
    fn a_record_that_is_pushed_and_not_collected_keeps_its_ref() {
        let d = tempfile::tempdir().unwrap();
        let mk = |id: &str, name: &str, deleted: bool| {
            let st = State {
                ref_name: name.into(),
                sha: "abc".into(),
                ref_deleted: deleted,
                ..State::default()
            };
            st.save(&record_path(d.path(), id));
        };
        mk("a", "mason/run/p/a", false);
        mk("b", "mason/run/p/b", true);
        let live = live_run_refs(&[d.path().to_path_buf()]);
        assert!(live.contains("mason/run/p/a"));
        assert!(!live.contains("mason/run/p/b"));
    }

    use super::*;
    use mason_adapter_api::http::{Request, Response, Transport};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    fn git_ok(cwd: &Path, args: &[&str]) -> String {
        let o = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    const BASE_WORKFLOW: &str = "name: mason-run\n# the base branch's copy\n";

    /// A repository with a base branch holding the workflow, a bare remote it was pushed to
    /// and a task branch whose tree edits the workflow, adds another and has a dirty worktree.
    struct Repo {
        _t: tempfile::TempDir,
        wt: PathBuf,
        bare: PathBuf,
    }

    fn repo() -> Repo {
        let t = tempfile::tempdir().unwrap();
        let (wt, bare) = (t.path().join("wt"), t.path().join("origin.git"));
        std::fs::create_dir_all(&wt).unwrap();
        git_ok(
            t.path(),
            &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
        );
        git_ok(&wt, &["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(wt.join(".github/workflows")).unwrap();
        std::fs::write(wt.join(".github/workflows/mason-run.yml"), BASE_WORKFLOW).unwrap();
        std::fs::write(wt.join(".github/workflows/ci.yml"), "name: ci\n").unwrap();
        std::fs::write(wt.join("a.txt"), "one\n").unwrap();
        std::fs::write(wt.join("gone.txt"), "x\n").unwrap();
        git_ok(&wt, &["add", "-A"]);
        git_ok(&wt, &["commit", "-qm", "base"]);
        git_ok(&wt, &["remote", "add", "origin", bare.to_str().unwrap()]);
        git_ok(&wt, &["push", "-q", "origin", "main"]);
        git_ok(&wt, &["fetch", "-q", "origin"]);
        // The task: edits the workflow to report green, adds another, writes its own run.json.
        git_ok(&wt, &["checkout", "-q", "-b", "task"]);
        std::fs::write(
            wt.join(".github/workflows/mason-run.yml"),
            "name: evil\non: push\njobs: {}\n",
        )
        .unwrap();
        std::fs::write(wt.join(".github/workflows/evil.yml"), "name: evil2\n").unwrap();
        std::fs::write(wt.join(".github/workflows/ci.yml"), "name: ci-changed\n").unwrap();
        std::fs::create_dir_all(wt.join(".mason")).unwrap();
        std::fs::write(wt.join(".mason/run.json"), "{\"command\":\"true\"}").unwrap();
        git_ok(&wt, &["add", "-A"]);
        git_ok(&wt, &["commit", "-qm", "task"]);
        // Dirty: a modified tracked file, a deleted one, an untracked one.
        std::fs::write(wt.join("a.txt"), "two\n").unwrap();
        std::fs::remove_file(wt.join("gone.txt")).unwrap();
        std::fs::write(wt.join("new.txt"), "fresh\n").unwrap();
        Repo { _t: t, wt, bare }
    }

    fn spec(id: &str) -> RunSpec {
        RunSpec {
            id: id.into(),
            project: "proj".into(),
            suite: "cargo-workspace".into(),
            build: None,
            command: "cargo test --workspace".into(),
            rerun: Some("cargo test -- --exact {tests}".into()),
            results_file: None,
            limit: Duration::from_secs(600),
            env: vec![
                ("MASON_TASK".into(), "7".into()),
                ("SECRET_TOKEN".into(), "do-not-send".into()),
            ],
            runs_on: "ubuntu-latest".into(),
            tree_key: "treekey+abc".into(),
            shards: 0,
            only: vec![],
            shard_lists: vec![],
            shard_command: None,
            partial_lists: false,
            shards_wanted: 0,
        }
    }

    struct FakeClock(AtomicU64);
    impl Clock for FakeClock {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
        fn sleep(&self, ms: u64) {
            self.0.fetch_add(ms, Ordering::SeqCst);
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Mode {
        /// Picked up after `queued` looks, finished after `running` more.
        Normal,
        /// No runner ever takes the job.
        NeverPicked,
        /// GitHub answers every call with this status.
        Status(u16),
        /// The run is found but the artifact never appears.
        NoArtifact,
        /// The first run is never picked up; the second runs normally (#873).
        MissFirst,
        /// The first run is picked up and never ends; the second runs normally (#873).
        HangFirst,
        /// The run never ends by itself; once cancelled it completes, with its artifact.
        Hung,
        /// As `Hung`, but no artifact ever appears.
        HungNoArtifact,
        /// GitHub stops the `run` step at its limit: the job ends 480 s after it started and the
        /// artifact holds `output.log` and no `result.json`.
        StepTimeout,
    }

    struct FakeGh {
        bare: PathBuf,
        zip: PathBuf,
        mode: Mode,
        queued: u32,
        running: u32,
        looks: Mutex<u32>,
        log: Mutex<Vec<String>>,
        cancels: Mutex<Vec<u64>>,
        runs_seen: Mutex<std::collections::BTreeSet<String>>,
        /// Panic (a daemon that dies) on this look at the run.
        die_at: Mutex<Option<u32>>,
        /// The bodies of the dispatches: a run exists only once one was made.
        dispatches: Mutex<Vec<String>>,
        /// The status of each job of the run, in order, when a test sets them one by one.
        job_status: Mutex<Option<Vec<&'static str>>>,
    }

    impl FakeGh {
        fn new(r: &Repo, zip: &Path, mode: Mode) -> Arc<FakeGh> {
            Self::picked_after(r, zip, mode, 1)
        }
        /// `Normal`, but a runner takes the job only after `queued` looks at the run.
        fn picked_after(r: &Repo, zip: &Path, mode: Mode, queued: u32) -> Arc<FakeGh> {
            Arc::new(FakeGh {
                bare: r.bare.clone(),
                zip: zip.to_path_buf(),
                mode,
                queued,
                running: 1,
                looks: Mutex::new(0),
                log: Mutex::new(vec![]),
                cancels: Mutex::new(vec![]),
                runs_seen: Mutex::new(Default::default()),
                die_at: Mutex::new(None),
                dispatches: Mutex::new(vec![]),
                job_status: Mutex::new(None),
            })
        }
        fn requests(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
        fn branch_of(&self, sha: &str) -> Option<String> {
            let o = Command::new("git")
                .args([
                    "for-each-ref",
                    "--format=%(refname:short)",
                    "--points-at",
                    sha,
                ])
                .arg("refs/heads/mason/run")
                .current_dir(&self.bare)
                .output()
                .ok()?;
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .map(String::from)
        }
        fn phase(&self) -> (&'static str, &'static str) {
            let n = *self.looks.lock().unwrap();
            let hung = matches!(self.mode, Mode::Hung | Mode::HungNoArtifact);
            let first_only = self.runs_seen.lock().unwrap().len() < 2;
            if self.mode == Mode::NeverPicked
                || (first_only && self.mode == Mode::MissFirst)
                || n <= self.queued
            {
                ("queued", "queued")
            } else if (first_only && self.mode == Mode::HangFirst)
                || (hung && self.cancels.lock().unwrap().is_empty())
                || (!hung && n <= self.queued + self.running)
            {
                ("in_progress", "in_progress")
            } else {
                ("completed", "completed")
            }
        }
    }

    fn ok(body: serde_json::Value) -> Result<Response, Transport> {
        Ok(Response {
            status: 200,
            headers: vec![],
            body: body.to_string(),
        })
    }

    impl Http for FakeGh {
        fn send(&self, r: &Request) -> Result<Response, Transport> {
            self.log
                .lock()
                .unwrap()
                .push(format!("{} {}", r.method, r.url));
            if let Mode::Status(code) = self.mode {
                return Ok(Response {
                    status: code,
                    headers: vec![],
                    body: "{\"message\":\"nope\"}".into(),
                });
            }
            let u = r.url.as_str();
            if r.method == "POST" && u.ends_with("/cancel") {
                self.cancels.lock().unwrap().push(4242);
                return Ok(Response {
                    status: 202,
                    headers: vec![],
                    body: "{}".into(),
                });
            }
            if r.method == "POST" && u.ends_with("/dispatches") {
                // GitHub starts the run on the ref's head, with no job `plan`.
                self.dispatches
                    .lock()
                    .unwrap()
                    .push(format!("{u} {}", r.body.clone().unwrap_or_default()));
                return Ok(Response {
                    status: 204,
                    headers: vec![],
                    body: String::new(),
                });
            }
            if r.method == "GET" && u.contains("/actions/caches?ref=") {
                return ok(serde_json::json!({"total_count": 1, "actions_caches": [{"id": 77}]}));
            }
            if r.method == "DELETE" && u.ends_with("/actions/caches/77") {
                return Ok(Response {
                    status: 204,
                    headers: vec![],
                    body: String::new(),
                });
            }
            if u.contains("/actions/runs?head_sha=") {
                let sha = u
                    .split("head_sha=")
                    .nth(1)
                    .unwrap()
                    .split('&')
                    .next()
                    .unwrap();
                let Some(branch) = self.branch_of(sha) else {
                    return ok(serde_json::json!({"workflow_runs": []}));
                };
                if self.dispatches.lock().unwrap().is_empty() {
                    // Pushed and not dispatched: nothing starts on the push.
                    return ok(serde_json::json!({"workflow_runs": []}));
                }
                self.runs_seen.lock().unwrap().insert(sha.to_string());
                let (st, _) = self.phase();
                return ok(serde_json::json!({"workflow_runs": [{
                    "id": 4242, "status": st, "head_branch": branch, "event": "workflow_dispatch",
                    "html_url": "https://github.com/o/r/actions/runs/4242",
                    "created_at": "2026-11-20T10:00:00Z"}]}));
            }
            if u.ends_with("/actions/runs/4242") {
                let n = {
                    let mut l = self.looks.lock().unwrap();
                    *l += 1;
                    *l
                };
                if *self.die_at.lock().unwrap() == Some(n) {
                    panic!("the daemon dies here");
                }
                let (st, _) = self.phase();
                let etag = format!("\"{st}\"");
                if r.headers
                    .iter()
                    .any(|(k, v)| k == "If-None-Match" && *v == etag)
                {
                    return Ok(Response {
                        status: 304,
                        headers: vec![],
                        body: String::new(),
                    });
                }
                let conclusion = (st == "completed").then_some("success");
                let branch = Command::new("git")
                    .args([
                        "for-each-ref",
                        "--format=%(refname:short)",
                        "refs/heads/mason/run",
                    ])
                    .current_dir(&self.bare)
                    .output()
                    .map(|o| {
                        String::from_utf8_lossy(&o.stdout)
                            .lines()
                            .next()
                            .unwrap_or("")
                            .to_string()
                    })
                    .unwrap_or_default();
                return Ok(Response {
                    status: 200,
                    headers: vec![("etag".into(), format!("\"{st}\""))],
                    body: serde_json::json!({"id": 4242, "status": st, "conclusion": conclusion,
                        "head_branch": branch, "event": "push",
                        "html_url": "https://github.com/o/r/actions/runs/4242",
                        "created_at": "2026-11-20T10:00:00Z"})
                    .to_string(),
                });
            }
            if u.contains("/actions/runs/4242/jobs") {
                let (_, job) = self.phase();
                let ended = (job == "completed").then_some(if self.mode == Mode::StepTimeout {
                    "2026-11-20T10:08:06Z"
                } else {
                    "2026-11-20T10:00:30Z"
                });
                // The jobs the workflow makes from the dispatch's inputs, as GitHub lists them.
                let last = self.dispatches.lock().unwrap().last().cloned();
                let inputs: serde_json::Value = last
                    .and_then(|d| serde_json::from_str(d.split_once(' ')?.1).ok())
                    .map(|v: serde_json::Value| v["inputs"].clone())
                    .unwrap_or_default();
                let names: Vec<String> = if inputs["sharded"] == "true" {
                    let m: Vec<u32> =
                        serde_json::from_str(inputs["matrix"].as_str().unwrap_or("[]"))
                            .unwrap_or_default();
                    m.iter().map(|k| format!("run ({k})")).collect()
                } else {
                    vec!["run".to_string()]
                };
                let jobs: Vec<_> = names
                    .iter()
                    .enumerate()
                    .map(|(i, n)| {
                        let own = self
                            .job_status
                            .lock()
                            .unwrap()
                            .as_ref()
                            .and_then(|v| v.get(i).copied());
                        let status = own.unwrap_or(job);
                        let started = (status != "queued").then_some("2026-11-20T10:00:06Z");
                        // A job that completed has an end time, as GitHub gives it.
                        let completed = (status == "completed").then_some(ended).flatten();
                        serde_json::json!({"id": 9000 + i, "name": n, "status": status,
                            "started_at": started, "completed_at": completed})
                    })
                    .collect();
                return ok(serde_json::json!({ "jobs": jobs }));
            }
            if u.contains("/actions/runs/4242/artifacts") {
                if matches!(self.mode, Mode::NoArtifact | Mode::HungNoArtifact) {
                    return ok(serde_json::json!({"artifacts": []}));
                }
                return ok(
                    serde_json::json!({"artifacts": [{"name": "mason-run", "expired": false,
                    "archive_download_url": "https://api.github.com/artifact/zip/9"}]}),
                );
            }
            Err(Transport(format!(
                "fake GitHub: no answer for {} {u}",
                r.method
            )))
        }

        fn download(&self, r: &Request, dest: &Path) -> Result<Response, Transport> {
            self.log
                .lock()
                .unwrap()
                .push(format!("{} {} (download)", r.method, r.url));
            std::fs::copy(&self.zip, dest).map_err(|e| Transport(e.to_string()))?;
            Ok(Response {
                status: 200,
                headers: vec![],
                body: String::new(),
            })
        }
    }

    /// A zip like the workflow uploads.
    fn artifact(dir: &Path, output: &str, code: i32, results: Option<(&str, &str)>) -> PathBuf {
        let src = dir.join("art-src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("output.log"), output).unwrap();
        std::fs::write(
            src.join("result.json"),
            format!("{{\"code\":{code},\"signal\":0,\"build_s\":3,\"run_s\":9}}"),
        )
        .unwrap();
        let mut members = vec!["output.log".to_string(), "result.json".to_string()];
        if let Some((rel, text)) = results {
            let p = src.join("results").join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text).unwrap();
            members.push("results".into());
        }
        let zip = dir.join("artifact.zip");
        let _ = std::fs::remove_file(&zip);
        let o = Command::new("zip")
            .arg("-qr")
            .arg(&zip)
            .args(&members)
            .current_dir(&src)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "zip: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        zip
    }

    /// A zip of a run GitHub stopped at the step limit: the output, no `result.json`.
    fn cut_artifact(dir: &Path, output: &str) -> PathBuf {
        let src = dir.join("cut-src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("output.log"), output).unwrap();
        let zip = dir.join("cut.zip");
        let _ = std::fs::remove_file(&zip);
        let o = Command::new("zip")
            .arg("-qr")
            .arg(&zip)
            .arg("output.log")
            .current_dir(&src)
            .output()
            .unwrap();
        assert!(o.status.success());
        zip
    }

    fn site(r: &Repo, gh: Arc<FakeGh>, clock: Arc<FakeClock>, fallback: bool) -> Site {
        Site {
            cwd: r.wt.clone(),
            remote: "origin".into(),
            base_ref: "origin/main".into(),
            workflow: "mason-run.yml".into(),
            pickup_timeout: Duration::from_secs(1200),
            fallback_local: fallback,
            spill_after: None,
            local_room: None,
            local_barred: None,
            resplit: None,
            test_env: vec![],
            gh: GitHub::new(
                gh,
                "https://api.github.com",
                "o/r",
                TokenRef::Env("MASON_TEST_REMOTE_TOKEN".into()),
            ),
            clock,
            poll_s: 10,
            on_phase: None,
        }
    }

    fn clock() -> Arc<FakeClock> {
        Arc::new(FakeClock(AtomicU64::new(1_000_000_000)))
    }

    fn token() {
        std::env::set_var("MASON_TEST_REMOTE_TOKEN", "t");
    }

    fn refs(bare: &Path) -> String {
        git_ok(
            bare,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/heads/mason/run",
            ],
        )
    }

    const GREEN: &str = "running 2 tests\ntest a ... ok\ntest b ... ok\n\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n";

    /// A hook on the bare remote that records every push and rejects the first `n`.
    fn reject_first(bare: &Path, n: u32) {
        let hook = bare.join("hooks/pre-receive");
        std::fs::create_dir_all(bare.join("hooks")).unwrap();
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\necho x >> \"{d}/pushes\"\n[ $(wc -l < \"{d}/pushes\") -le {n} ] && exit 1\nexit 0\n",
                d = bare.display()
            ),
        )
        .unwrap();
        // Executable bits are Unix-only; the Windows CI job builds these tests but does not run them.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn pushes(bare: &Path) -> usize {
        std::fs::read_to_string(bare.join("pushes"))
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }

    fn retry_state(r: &Repo) -> State {
        State {
            ref_name: "mason/run/p/retry".into(),
            sha: git_ok(&r.wt, &["rev-parse", "HEAD"]),
            ..State::default()
        }
    }

    #[test]
    fn a_push_rejected_twice_then_accepted_goes_on() {
        let r = repo();
        reject_first(&r.bare, 2);
        let gh = FakeGh::new(&r, &r.wt.join("none.zip"), Mode::Normal);
        let s = site(&r, gh, clock(), false);
        push(&s, &retry_state(&r)).unwrap();
        assert_eq!(pushes(&r.bare), 3);
    }

    #[test]
    fn a_push_rejected_every_time_gives_up_after_four_tries() {
        let r = repo();
        reject_first(&r.bare, 99);
        let gh = FakeGh::new(&r, &r.wt.join("none.zip"), Mode::Normal);
        let s = site(&r, gh, clock(), false);
        assert!(push(&s, &retry_state(&r)).is_err());
        assert_eq!(pushes(&r.bare), 4);
    }

    #[test]
    fn a_refused_credential_is_not_retried() {
        let auth = "fatal: Authentication failed for 'https://github.com/o/r'";
        assert!(push_refused(auth));
        assert!(!push_transient(auth));
        assert!(!push_transient(
            "! [remote rejected] x (protected branch hook declined)"
        ));
        assert!(push_transient(" ! [remote rejected] x -> y (failed)"));
        assert!(push_transient(
            "error: RPC failed; HTTP 502 The requested URL returned error: 502"
        ));
        assert!(push_transient("fatal: Connection reset by peer"));
        assert!(!push_transient("fatal: not a git repository"));
    }

    #[test]
    fn the_snapshot_holds_the_dirty_worktree_and_the_trusted_overlay() {
        let r = repo();
        let before = (
            git_ok(&r.wt, &["rev-parse", "HEAD"]),
            git_ok(&r.wt, &["status", "--porcelain", "--untracked-files=all"]),
            git_ok(&r.wt, &["diff", "--cached"]),
        );
        let json = run_json(&spec("x1"));
        let sha = snapshot(&r.wt, "origin/main", Some("mason-run.yml"), &json, "m").unwrap();
        let show = |p: &str| git_ok(&r.wt, &["show", &format!("{sha}:{p}")]);
        // What a local run would see...
        assert_eq!(show("a.txt"), "two");
        assert_eq!(show("new.txt"), "fresh");
        assert!(git_ok(&r.wt, &["ls-tree", "-r", "--name-only", &sha])
            .lines()
            .all(|l| l != "gone.txt"));
        // ...with the base branch's workflow and no other.
        assert_eq!(
            show(".github/workflows/mason-run.yml"),
            BASE_WORKFLOW.trim()
        );
        let wf: Vec<String> = git_ok(&r.wt, &["ls-tree", "-r", "--name-only", &sha, ".github"])
            .lines()
            .map(String::from)
            .collect();
        // The base's ci.yml is there (the task's evil.yml is not).
        assert_eq!(
            wf,
            vec![
                ".github/workflows/ci.yml".to_string(),
                ".github/workflows/mason-run.yml".to_string()
            ]
        );
        assert_eq!(show(".github/workflows/ci.yml"), "name: ci");
        // The change's files, made where the history is (s759); the dirty worktree is in it.
        let changed = show(".mason/changed-files.txt");
        assert!(changed.lines().any(|l| l == "a.txt"), "{changed}");
        assert!(changed.lines().any(|l| l == "new.txt"), "{changed}");
        assert!(changed.lines().any(|l| l == "gone.txt"), "{changed}");
        assert!(!show(".mason/base-sha").is_empty());
        // The spec is MASON's, not the task's.
        assert_eq!(show(".mason/run.json"), json);
        assert!(!show(".mason/run.json").contains("\"true\""));
        // Untouched: HEAD, the worktree's status and its index.
        assert_eq!(before.0, git_ok(&r.wt, &["rev-parse", "HEAD"]));
        assert_eq!(
            before.1,
            git_ok(&r.wt, &["status", "--porcelain", "--untracked-files=all"])
        );
        assert_eq!(before.2, git_ok(&r.wt, &["diff", "--cached"]));
    }

    #[test]
    fn a_base_workflow_that_runs_on_run_refs_refuses_the_snapshot() {
        let r = repo();
        git_ok(&r.wt, &["checkout", "-q", "main"]);
        std::fs::write(
            r.wt.join(".github/workflows/ci.yml"),
            "on:\n  push:\n    branches: [\"**\"]\n",
        )
        .unwrap();
        git_ok(&r.wt, &["commit", "-qam", "ci on every ref"]);
        git_ok(&r.wt, &["push", "-q", "origin", "main"]);
        git_ok(&r.wt, &["fetch", "-q", "origin"]);
        let e = snapshot(&r.wt, "origin/main", Some("mason-run.yml"), "{}", "m").unwrap_err();
        assert!(e.contains("ci.yml") && e.contains("mason/run/**"), "{e}");
    }

    #[test]
    fn a_base_branch_without_the_workflow_is_environmental_not_the_tasks() {
        let r = repo();
        let e = snapshot(&r.wt, "origin/main", Some("other.yml"), "{}", "m").unwrap_err();
        assert!(e.contains("the base branch"), "{e}");
        token();
        let gh = FakeGh::new(&r, &r.wt.join("none.zip"), Mode::Normal);
        let mut s = site(&r, gh, clock(), false);
        s.workflow = "other.yml".into();
        let d = tempfile::tempdir().unwrap();
        let e = execute(&s, &spec("x2"), d.path(), &|| None, &Grant::test())
            .err()
            .unwrap();
        assert!(matches!(e, Fail::Environmental(_)), "{e:?}");
    }

    #[test]
    fn the_projects_test_env_reaches_run_json_without_overriding_the_run() {
        let r = repo();
        token();
        let gh = FakeGh::new(&r, &r.wt.join("none.zip"), Mode::Normal);
        let mut site = site(&r, gh, clock(), false);
        site.test_env = vec![
            ("MASON_TEST_MIN_WAIT_S".into(), "180".into()),
            ("MASON_TASK".into(), "other".into()),
        ];
        let v: serde_json::Value =
            serde_json::from_str(&run_json(&with_site_env(spec("r1"), &site))).unwrap();
        assert_eq!(v["env"]["MASON_TEST_MIN_WAIT_S"], "180");
        assert_eq!(v["env"]["MASON_TASK"], "7");
    }

    #[test]
    fn the_run_json_carries_only_masons_environment_and_the_pinned_definition() {
        let v: serde_json::Value = serde_json::from_str(&run_json(&spec("x3"))).unwrap();
        assert_eq!(v["command"], "cargo test --workspace");
        assert_eq!(v["rerun"], "cargo test -- --exact {tests}");
        assert_eq!(v["runs_on"], "ubuntu-latest");
        assert_eq!(v["limit_s"], 600);
        // Two minutes under the limit, so the upload step runs before MASON cancels.
        assert_eq!(v["timeout_min"], 8);
        assert_eq!(v["env"]["MASON_TASK"], "7");
        assert!(v["xcode"].is_null());
        assert!(v["env"].get("SECRET_TOKEN").is_none());
    }

    #[test]
    fn a_runs_build_is_sent_beside_its_command_in_one_run_json() {
        let sp = with_build(spec("x3b"), Some("cargo test --no-run"));
        let v: serde_json::Value = serde_json::from_str(&run_json(&sp)).unwrap();
        assert_eq!(v["build"], "cargo test --no-run");
        assert_eq!(v["command"], "cargo test --workspace");
    }

    #[test]
    fn a_failed_run_says_which_part_ended_it() {
        token();
        for (said, want) in [
            (Some("build"), Some("build")),
            (Some("run"), Some("run")),
            (Some("other"), None),
            (None, None),
        ] {
            let r = repo();
            let t = tempfile::tempdir().unwrap();
            let _ = artifact(t.path(), "boom\n", 1, None);
            let meta = match said {
                Some(p) => format!(
                    "{{\"code\":1,\"signal\":0,\"build_s\":3,\"run_s\":0,\"phase\":\"{p}\"}}"
                ),
                None => "{\"code\":1,\"signal\":0,\"build_s\":3,\"run_s\":0}".to_string(),
            };
            let src = t.path().join("art-src");
            std::fs::write(src.join("result.json"), meta).unwrap();
            let zip = t.path().join("artifact2.zip");
            let o = Command::new("zip")
                .arg("-qr")
                .arg(&zip)
                .args(["output.log", "result.json"])
                .current_dir(&src)
                .output()
                .unwrap();
            assert!(o.status.success());
            let gh = FakeGh::new(&r, &zip, Mode::Normal);
            let s = site(&r, gh, clock(), false);
            let logs = tempfile::tempdir().unwrap();
            let Outcome::Done(f) =
                execute(&s, &spec("x3c"), logs.path(), &|| None, &Grant::test()).unwrap()
            else {
                panic!("not done")
            };
            assert_eq!(f.code, 1);
            assert_eq!(f.phase.as_deref(), want, "said {said:?}");
        }
    }

    #[test]
    fn run_json_names_the_toolchains_each_of_masons_suites_needs() {
        for (suite, want) in [
            ("cargo-workspace", "rust"),
            ("exit-scripts", "rust"),
            ("exit-scripts-landing", "rust"),
            ("fmt", "rust"),
            ("lint", "rust"),
            ("", "rust"),
            ("swift-clients", "swift"),
            ("compose-clients", "java"),
        ] {
            let mut sp = spec("tc");
            sp.suite = suite.into();
            let v: serde_json::Value = serde_json::from_str(&run_json(&sp)).unwrap();
            assert_eq!(v["toolchains"], serde_json::json!([want]), "{suite}");
        }
    }

    #[test]
    fn exit_scripts_runs_skip_swift_and_compose_but_cargo_workspace_does_not() {
        for (suite, skip) in [
            ("exit-scripts", true),
            ("exit-scripts-landing", true),
            ("cargo-workspace", false),
        ] {
            let mut sp = spec("sk");
            sp.suite = suite.into();
            let v: serde_json::Value = serde_json::from_str(&run_json(&sp)).unwrap();
            for k in ["MASON_SKIP_SWIFT", "MASON_SKIP_COMPOSE"] {
                let got = v["env"].get(k).and_then(|x| x.as_str());
                assert_eq!(got, skip.then_some("1"), "{suite} {k}");
            }
        }
    }

    #[test]
    fn the_workflows_restore_one_shared_key_and_only_the_base_branch_saves_it() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows");
        let run = std::fs::read_to_string(root.join("mason-run.yml")).unwrap();
        assert!(run.contains("shared-key: mason-run"));
        assert!(run.contains("save-if: ${{ github.ref == 'refs/heads/develop' }}"));
        assert!(run.contains("toolchains"));
        let warm = std::fs::read_to_string(root.join("mason-cache.yml")).unwrap();
        assert!(warm.contains("shared-key: mason-run"));
        assert!(warm.contains("cancel-in-progress: true"));
        assert!(warm.contains("--no-run"));
        assert!(!warm.contains("save-if"), "the warm-up always saves");
    }

    #[test]
    fn a_green_run_gives_its_output_and_the_ref_is_deleted() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let gh = FakeGh::new(&r, &zip, Mode::Normal);
        let s = site(&r, gh.clone(), clock(), false);
        let logs = tempfile::tempdir().unwrap();
        let out = execute(&s, &spec("x4"), logs.path(), &|| None, &Grant::test()).unwrap();
        let Outcome::Done(f) = out else {
            panic!("not done")
        };
        assert_eq!((f.code, f.signal, f.timed_out), (0, 0, false));
        assert_eq!(f.output, GREEN);
        assert_eq!(f.ran_ms, Some(12_000));
        assert!(
            f.ran_on.contains("ran on GitHub (ubuntu-latest)"),
            "{}",
            f.ran_on
        );
        assert!(
            f.ran_on.contains("actions/runs/4242") && f.ran_on.contains("picked up after 6 s"),
            "{}",
            f.ran_on
        );
        assert_eq!(
            std::fs::read_to_string(logs.path().join("x4.log")).unwrap(),
            GREEN
        );
        assert_eq!(refs(&r.bare), "", "the ref is deleted");
        // The record keeps the snapshot's SHA beside the tree key.
        let kv = read_kv(&record_path(logs.path(), "x4")).unwrap();
        assert_eq!(kv["tree_key"], "treekey+abc");
        assert_eq!(kv["sha"].len(), 40);
        assert_eq!(kv["collected"], "1");
        assert!(gh.requests().iter().any(|q| q.contains("head_sha=")));
        let q = gh.requests();
        assert!(
            q.iter().any(|q| q.starts_with("GET ")
                && q.contains("/actions/caches?ref=refs/heads/mason/run/")),
            "the deleted ref's caches are listed: {q:?}"
        );
        let deletes: Vec<&String> = q
            .iter()
            .filter(|q| q.starts_with("DELETE ") && q.contains("/actions/caches"))
            .collect();
        assert_eq!(
            deletes.len(),
            1,
            "one DELETE, by the listed cache's id: {deletes:?}"
        );
        assert!(deletes[0].ends_with("/actions/caches/77"), "{deletes:?}");
    }

    #[test]
    fn a_red_run_gives_its_exit_code_output_and_results_file_and_the_ref_is_deleted() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let red = "test a ... ok\ntest b ... FAILED\n\nfailures:\n    b\n\ntest result: FAILED. 1 passed; 1 failed;\n";
        let zip = artifact(t.path(), red, 101, Some(("build/res/TEST-a.xml", "<xml/>")));
        let gh = FakeGh::new(&r, &zip, Mode::Normal);
        let s = site(&r, gh, clock(), false);
        let logs = tempfile::tempdir().unwrap();
        let mut sp = spec("x5");
        sp.results_file = Some("build/res/*.xml".into());
        let Outcome::Done(f) = execute(&s, &sp, logs.path(), &|| None, &Grant::test()).unwrap()
        else {
            panic!("not done")
        };
        assert_eq!(f.code, 101);
        assert!(f.output.contains("test b ... FAILED"));
        assert_eq!(
            std::fs::read_to_string(r.wt.join("build/res/TEST-a.xml")).unwrap(),
            "<xml/>"
        );
        assert_eq!(refs(&r.bare), "");
    }

    fn map_text(commit: &str, main_text: &str, made_at: u64) -> String {
        crate::covmap::CoverageMap {
            commit: commit.into(),
            made_at,
            target: "crates/daemon/tests/it/main.rs".into(),
            main_hash: crate::covmap::hash_text(main_text),
            modules: [(
                "holds".to_string(),
                ["crates/daemon/src/holds.rs".to_string()].into(),
            )]
            .into(),
        }
        .encode()
    }

    #[test]
    fn a_due_map_pushes_one_covmap_run_and_returns_the_map_it_made_and_a_fresh_one_pushes_nothing()
    {
        use crate::covmap::{make_remote, Want, REMOTE_COMMAND};
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let (tip, main_text) = ("0123456789abcdef0123", "mod holds;\n");
        let text = map_text(tip, main_text, 5_000);
        let zip = artifact(t.path(), "built\n", 0, Some(("covmap.kv", &text)));
        let gh = FakeGh::new(&r, &zip, Mode::Normal);
        let s = site(&r, gh.clone(), clock(), false);
        let logs = tempfile::tempdir().unwrap();
        let want = Want {
            old: None,
            tip,
            main_text,
            changed_src: true,
            now_secs: 5_000,
        };
        let m = make_remote(
            &s,
            &want,
            "proj",
            logs.path(),
            Duration::from_secs(600),
            &|| None,
        )
        .unwrap()
        .expect("a map was due");
        assert_eq!(m.commit, tip);
        assert!(m.knows("crates/daemon/src/holds.rs"));
        assert_eq!(gh.runs_seen.lock().unwrap().len(), 1, "one run was pushed");
        // The runner was told what to run, and that it is a covmap run.
        let rec = read_kv(&record_path(logs.path(), "covmap-0123456789ab")).unwrap();
        assert_eq!(rec["sha"].len(), 40);
        let run_json = git_ok(
            &r.bare,
            &["show", &format!("{}:.mason/run.json", rec["sha"])],
        );
        let j: serde_json::Value = serde_json::from_str(&run_json).unwrap();
        assert_eq!(j["suite"], "covmap");
        assert_eq!(j["command"], REMOTE_COMMAND);
        assert_eq!(j["results_file"], "covmap.kv");
        assert_eq!(j["env"]["MASON_COVMAP_COMMIT"], tip);

        // A fresh map for the same main.rs: nothing is pushed.
        let gh2 = FakeGh::new(&r, &zip, Mode::Normal);
        let s2 = site(&r, gh2.clone(), clock(), false);
        let fresh = Want {
            old: Some(&m),
            tip: "fedcba9876543210fedc",
            main_text,
            changed_src: true,
            now_secs: 5_000 + 3600,
        };
        let none = make_remote(
            &s2,
            &fresh,
            "proj",
            logs.path(),
            Duration::from_secs(600),
            &|| None,
        )
        .unwrap();
        assert!(none.is_none());
        assert!(gh2.requests().is_empty(), "{:?}", gh2.requests());
        assert_eq!(refs(&r.bare), "");
    }

    #[test]
    fn a_map_of_another_commit_is_not_kept() {
        use crate::covmap::{make_remote, Want};
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(
            t.path(),
            "built\n",
            0,
            Some(("covmap.kv", &map_text("other", "mod x;\n", 1))),
        );
        let gh = FakeGh::new(&r, &zip, Mode::Normal);
        let s = site(&r, gh, clock(), false);
        let logs = tempfile::tempdir().unwrap();
        let want = Want {
            old: None,
            tip: "0123456789abcdef0123",
            main_text: "mod x;\n",
            changed_src: true,
            now_secs: 1,
        };
        let e = make_remote(
            &s,
            &want,
            "proj",
            logs.path(),
            Duration::from_secs(600),
            &|| None,
        )
        .unwrap_err();
        assert!(e.contains("not of"), "{e}");
    }

    #[test]
    fn a_skip_while_queued_or_running_cancels_the_run_and_deletes_the_ref() {
        token();
        // The 600 s limit counts from pickup (about 110 s), so 300 s is still running.
        for skip_after_s in [30u64, 300] {
            let r = repo();
            let t = tempfile::tempdir().unwrap();
            let zip = artifact(t.path(), GREEN, 0, None);
            // Queued for a long time, then running: the skip arrives in either state.
            let mut gh = Arc::try_unwrap(FakeGh::new(&r, &zip, Mode::Normal))
                .ok()
                .unwrap();
            gh.queued = 10;
            gh.running = 1000;
            let gh = Arc::new(gh);
            let clk = clock();
            let s = site(&r, gh.clone(), clk.clone(), false);
            let logs = tempfile::tempdir().unwrap();
            let start = clk.now_ms();
            let out = execute(
                &s,
                &spec("x6"),
                logs.path(),
                &|| (clk.now_ms() - start >= skip_after_s * 1000).then_some(StopWhy::Skipped),
                &Grant::test(),
            )
            .unwrap();
            assert!(matches!(out, Outcome::Stopped(StopWhy::Skipped)));
            assert_eq!(gh.cancels.lock().unwrap().len(), 1, "after {skip_after_s}");
            assert_eq!(refs(&r.bare), "");
            assert!(!record_path(logs.path(), "x6").exists());
        }
    }

    /// An adopted run: its ref pushed, its record saying it is out with run 4242, and its hold.
    fn adopted(r: &Repo, logs: &Path, id: &str) -> (PathBuf, PathBuf) {
        let name = ref_name("proj", id);
        let sha = git_ok(&r.wt, &["rev-parse", "HEAD"]);
        git_ok(
            &r.wt,
            &["push", "-q", "origin", &format!("HEAD:refs/heads/{name}")],
        );
        let rec = record_path(logs, id);
        State {
            ref_name: name,
            sha,
            run_id: Some(4242),
            runs_on: "ubuntu-latest".into(),
            ..Default::default()
        }
        .save(&rec);
        let holds = logs.join("remote-holds");
        write_hold(
            &holds,
            id,
            &RemoteHold {
                key: id.into(),
                label: id.into(),
                units: units_for("ubuntu-latest", 1),
                record: rec.clone(),
                ..Default::default()
            },
        );
        (holds, rec)
    }

    #[test]
    fn a_hold_whose_run_completed_while_no_daemon_ran_is_freed_and_its_ref_deleted() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let mut gh = Arc::try_unwrap(FakeGh::new(&r, &zip, Mode::Normal))
            .ok()
            .unwrap();
        gh.queued = 0;
        gh.running = 0;
        let s = site(&r, Arc::new(gh), clock(), false);
        let logs = tempfile::tempdir().unwrap();
        let (holds, rec) = adopted(&r, logs.path(), "d1");
        assert_eq!(running_holds(&holds).len(), 1);
        assert!(reap_hold(&s, &holds, &rec));
        assert!(running_holds(&holds).is_empty());
        assert!(record_ended(&rec), "the record no longer counts as out");
        assert_eq!(refs(&r.bare), "", "the ref is deleted");
        // The result is still there for a call that attaches later.
        let st = State::load(&rec).unwrap();
        assert!(st.released && st.ref_deleted && !st.collected);
    }

    #[test]
    fn a_hold_with_no_run_on_github_is_kept_while_young_and_freed_once_old() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        // Nothing was dispatched: GitHub lists no run for the commit.
        let gh = FakeGh::new(&r, &zip, Mode::Normal);
        let clk = clock();
        let s = site(&r, gh, clk.clone(), false);
        let logs = tempfile::tempdir().unwrap();
        let (holds, rec) = adopted(&r, logs.path(), "d9");
        let mut st = State::load(&rec).unwrap();
        st.run_id = None;
        st.pushed_ms = clk.now_ms();
        st.save(&rec);
        assert!(
            !reap_hold(&s, &holds, &rec),
            "a fresh push may still get its run"
        );
        assert_eq!(running_holds(&holds).len(), 1);
        clk.sleep(orphan_after_ms(&s) - 1);
        assert!(!reap_hold(&s, &holds, &rec), "not yet past the window");
        clk.sleep(1);
        assert!(
            reap_hold(&s, &holds, &rec),
            "no run was ever made: the unit is freed"
        );
        assert!(running_holds(&holds).is_empty());
        assert!(record_ended(&rec), "the record no longer counts as out");
        assert_eq!(refs(&r.bare), "", "the ref is deleted");
    }

    #[test]
    fn a_hold_whose_run_is_still_going_is_kept() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let mut gh = Arc::try_unwrap(FakeGh::new(&r, &zip, Mode::Normal))
            .ok()
            .unwrap();
        gh.queued = 0;
        gh.running = 1000;
        let s = site(&r, Arc::new(gh), clock(), false);
        let logs = tempfile::tempdir().unwrap();
        let (holds, rec) = adopted(&r, logs.path(), "d2");
        assert!(!reap_hold(&s, &holds, &rec));
        assert_eq!(running_holds(&holds).len(), 1);
        assert!(refs(&r.bare).contains("d2"));
    }

    #[test]
    fn an_unwanted_run_is_cancelled_its_hold_removed_and_its_ref_deleted() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let gh = FakeGh::new(&r, &zip, Mode::Normal);
        let s = site(&r, gh.clone(), clock(), false);
        let logs = tempfile::tempdir().unwrap();
        let (holds, rec) = adopted(&r, logs.path(), "t7r9-check-cargo-workspace0");
        cancel_hold(&s, &holds, &rec);
        assert_eq!(gh.cancels.lock().unwrap().len(), 1);
        assert!(running_holds(&holds).is_empty());
        assert!(!rec.exists());
        assert_eq!(refs(&r.bare), "");
    }

    #[test]
    fn a_replaced_step_attempt_is_cancelled_and_gives_its_unit_back_and_the_newer_one_stays() {
        // #1074: after a restart the step cancels the attempt a retry replaced (the same call
        // `suites::recover_attempts` makes); the retry it attaches to keeps its hold.
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let gh = FakeGh::new(&r, &zip, Mode::Normal);
        let s = site(&r, gh.clone(), clock(), false);
        let logs = tempfile::tempdir().unwrap();
        let (holds, old) = adopted(&r, logs.path(), "t7r9-u-unit");
        let (_, newer) = adopted(&r, logs.path(), "t7r9-u1700000000000-unit");
        assert_eq!(running_holds(&holds).len(), 2);
        cancel_hold(&s, &holds, &old);
        assert_eq!(gh.cancels.lock().unwrap().len(), 1, "one run cancelled");
        assert!(record_ended(&old) && !old.exists());
        let left = running_holds(&holds);
        assert_eq!(left.len(), 1, "only the older attempt's unit came back");
        assert_eq!(left[0].record, newer);
        assert!(!record_ended(&newer));
    }

    #[test]
    fn a_superseded_check_is_cancelled_queued_or_in_progress_and_its_hold_is_gone() {
        token();
        for (id, picked) in [
            ("t7r9-check-cargo-workspace0", None),
            ("t7r9-check-cargo-workspace1", Some(1)),
        ] {
            let r = repo();
            let t = tempfile::tempdir().unwrap();
            let zip = artifact(t.path(), GREEN, 0, None);
            let gh = FakeGh::new(&r, &zip, Mode::Normal);
            let s = site(&r, gh.clone(), clock(), false);
            let logs = tempfile::tempdir().unwrap();
            let (holds, rec) = adopted(&r, logs.path(), id);
            let mut st = State::load(&rec).unwrap();
            st.picked_ms = picked;
            st.save(&rec);
            assert!(!record_ended(&rec));
            cancel_hold(&s, &holds, &rec);
            assert_eq!(gh.cancels.lock().unwrap().len(), 1, "{id}");
            assert!(record_ended(&rec), "{id}: the units come back");
            assert!(running_holds(&holds).is_empty(), "{id}");
            assert_eq!(refs(&r.bare), "", "{id}");
        }
    }

    #[test]
    fn the_reaper_names_a_check_whose_record_says_superseded_or_delivered() {
        let dir = tempfile::tempdir().unwrap();
        let mut adopted = vec![];
        for (n, kv) in [
            (
                "a",
                vec![
                    ("key", "k"),
                    ("delivered", "1"),
                    ("superseded", "t7r9-check-cargo-workspace3"),
                ],
            ),
            ("b", vec![("key", "k"), ("delivered", "1")]),
            ("c", vec![("key", "k"), ("delivered", "0")]),
        ] {
            let id = format!("t7r9-check-{n}0");
            let kv: Vec<(&str, String)> = kv.into_iter().map(|(a, b)| (a, b.to_string())).collect();
            write_kv(&dir.path().join(format!("{id}.check")), &kv).unwrap();
            adopted.push((id.clone(), dir.path().join(format!("{id}.remote"))));
        }
        // Prefixes differ, so none is "newer"; the task's open run is this one for all.
        let got = unwanted(&adopted, &|_| Some(9));
        let names: Vec<String> = got
            .iter()
            .map(|p| p.file_stem().unwrap().to_str().unwrap().to_string())
            .collect();
        assert_eq!(names, vec!["t7r9-check-a0", "t7r9-check-b0"]);
    }

    #[test]
    fn checks_of_a_landed_task_or_an_older_run_or_superseded_are_unwanted() {
        let rec = |id: &str| (id.to_string(), PathBuf::from(format!("/x/{id}.remote")));
        let adopted = vec![
            rec("t7r9-check-cargo-workspace0"),
            rec("t7r9-check-cargo-workspace1"),
            rec("t7r9-check-cargo-workspace2"),
            rec("t7r9-check-other0"),
            rec("t8r3-check-cargo-workspace0"),
            ("task 9".to_string(), PathBuf::from("/x/step.remote")),
            ("debt-1".to_string(), PathBuf::from("/x/debt.remote")),
        ];
        // Task 7's open run is 9; task 8 has landed; task 9 has none.
        let open = |t: u64| (t == 7).then_some(9);
        let got: Vec<String> = unwanted(&adopted, &open)
            .iter()
            .map(|p| p.file_stem().unwrap().to_str().unwrap().to_string())
            .collect();
        assert_eq!(
            got,
            vec![
                "t7r9-check-cargo-workspace0",
                "t7r9-check-cargo-workspace1",
                "t8r3-check-cargo-workspace0",
                "step"
            ]
        );
    }

    /// A fake GitHub whose run stays queued, and a clock.
    fn queued_gh(r: &Repo) -> (Arc<FakeGh>, Arc<FakeClock>) {
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let mut gh = Arc::try_unwrap(FakeGh::new(r, &zip, Mode::Normal))
            .ok()
            .unwrap();
        gh.queued = 1000;
        (Arc::new(gh), clock())
    }

    #[allow(clippy::too_many_arguments)]
    fn pass(
        r: &Repo,
        gh: &Arc<FakeGh>,
        clk: &Arc<FakeClock>,
        adopted: &[(String, PathBuf)],
        live: &[(String, PathBuf)],
        holds: &Path,
        memo: &mut ReapMemo,
        quota_wait: Option<u64>,
    ) -> Vec<String> {
        reap_pass(
            adopted,
            live,
            &|t| (t == 7).then_some(9),
            &|_| Some(site(r, gh.clone(), clk.clone(), false)),
            holds,
            memo,
            clk.now_ms(),
            120,
            quota_wait,
        )
    }

    #[test]
    fn an_adopted_run_an_open_call_has_run_again_is_cancelled_and_the_newer_one_kept() {
        token();
        let r = repo();
        let (gh, clk) = queued_gh(&r);
        let logs = tempfile::tempdir().unwrap();
        let (holds, old) = adopted(&r, logs.path(), "t7r9-check-cargo-workspace-build19");
        // The replayed call's run was started by this daemon: a hold, not adopted.
        let (_, new) = adopted(&r, logs.path(), "t7r9-check-cargo-workspace-build20");
        let ad = vec![(
            "t7r9-check-cargo-workspace-build19".to_string(),
            old.clone(),
        )];
        let live = vec![(
            "t7r9-check-cargo-workspace-build20".to_string(),
            new.clone(),
        )];
        let mut memo = ReapMemo::default();
        let done = pass(&r, &gh, &clk, &ad, &live, &holds, &mut memo, None);
        assert_eq!(done.len(), 1, "{done:?}");
        assert!(done[0].contains("superseded by t7r9-check-cargo-workspace-build20"));
        assert_eq!(gh.cancels.lock().unwrap().len(), 1);
        assert!(!old.exists(), "the older record ends");
        assert!(new.exists(), "the newer run is kept");
        let kept: Vec<PathBuf> = running_holds(&holds)
            .into_iter()
            .map(|h| h.record)
            .collect();
        assert_eq!(kept, vec![new]);
    }

    #[test]
    fn the_newest_wanted_adopted_run_is_not_cancelled() {
        token();
        let r = repo();
        let (gh, clk) = queued_gh(&r);
        let logs = tempfile::tempdir().unwrap();
        let (holds, rec) = adopted(&r, logs.path(), "t7r9-check-cargo-workspace-build20");
        let ad = vec![(
            "t7r9-check-cargo-workspace-build20".to_string(),
            rec.clone(),
        )];
        let mut memo = ReapMemo::default();
        let done = pass(&r, &gh, &clk, &ad, &[], &holds, &mut memo, None);
        assert!(done.is_empty(), "{done:?}");
        assert!(gh.cancels.lock().unwrap().is_empty());
        assert!(rec.exists());
        assert_eq!(running_holds(&holds).len(), 1);
    }

    #[test]
    fn an_older_test_step_attempt_beside_a_newer_one_is_cancelled() {
        token();
        let r = repo();
        let (gh, clk) = queued_gh(&r);
        let logs = tempfile::tempdir().unwrap();
        let (holds, old) = adopted(&r, logs.path(), "t7r9-u-unit");
        let (_, new) = adopted(&r, logs.path(), "t7r9-u1700000000000-unit");
        let ad = vec![("task 7".to_string(), old.clone())];
        let live = vec![("task 7".to_string(), new.clone())];
        let mut memo = ReapMemo::default();
        let done = pass(&r, &gh, &clk, &ad, &live, &holds, &mut memo, None);
        assert_eq!(done.len(), 1, "{done:?}");
        assert!(!old.exists() && new.exists());
    }

    #[test]
    fn adopted_holds_are_not_asked_about_again_before_their_next_time() {
        token();
        let r = repo();
        let (gh, clk) = queued_gh(&r);
        let logs = tempfile::tempdir().unwrap();
        let (holds, rec) = adopted(&r, logs.path(), "t7r9-check-cargo-workspace-build20");
        let ad = vec![("t7r9-check-cargo-workspace-build20".to_string(), rec)];
        let mut memo = ReapMemo::default();
        let looks = || *gh.looks.lock().unwrap();
        pass(&r, &gh, &clk, &ad, &[], &holds, &mut memo, None);
        assert_eq!(looks(), 1);
        clk.sleep(30_000);
        pass(&r, &gh, &clk, &ad, &[], &holds, &mut memo, None);
        clk.sleep(60_000);
        pass(&r, &gh, &clk, &ad, &[], &holds, &mut memo, None);
        assert_eq!(looks(), 1, "no look before the next time");
        clk.sleep(30_000);
        pass(&r, &gh, &clk, &ad, &[], &holds, &mut memo, None);
        assert_eq!(looks(), 2);
        assert_eq!(memo.etag.len(), 1, "the ETag of the last look is kept");
    }

    #[test]
    fn nothing_is_sent_while_the_quota_is_used_up() {
        token();
        let r = repo();
        let (gh, clk) = queued_gh(&r);
        let logs = tempfile::tempdir().unwrap();
        let (holds, old) = adopted(&r, logs.path(), "t7r9-check-cargo-workspace-build19");
        let (_, new) = adopted(&r, logs.path(), "t7r9-check-cargo-workspace-build20");
        let ad = vec![
            ("a".to_string(), old.clone()),
            ("b".to_string(), new.clone()),
        ];
        let mut memo = ReapMemo::default();
        let done = pass(&r, &gh, &clk, &ad, &[], &holds, &mut memo, Some(600));
        assert!(done.is_empty());
        assert!(gh.requests().is_empty(), "{:?}", gh.requests());
        assert!(old.exists() && new.exists());
        // The quota back: the older one is cancelled.
        let done = pass(&r, &gh, &clk, &ad, &[], &holds, &mut memo, None);
        assert_eq!(done.len(), 1, "{done:?}");
    }

    const HUNG_OUT: &str = "test a ... ok\ntest holds_one has been running for over 60 seconds\ntest holds_two has been running for over 60 seconds\ntest holds_two ... ok\n";

    #[test]
    fn a_run_past_its_limit_keeps_the_artifact_output_and_names_the_unfinished_tests() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), HUNG_OUT, 0, None);
        let gh = FakeGh::new(&r, &zip, Mode::Hung);
        let s = site(&r, gh.clone(), clock(), false);
        let logs = tempfile::tempdir().unwrap();
        let Outcome::Done(f) =
            execute(&s, &spec("x9"), logs.path(), &|| None, &Grant::test()).unwrap()
        else {
            panic!("not done");
        };
        assert!(f.timed_out);
        assert!(f.output.starts_with("test a ... ok\n"), "{}", f.output);
        assert!(f
            .output
            .contains("the run was cancelled after its limit of 600 s"));
        assert!(
            f.output.contains("never finished: holds_one\n"),
            "{}",
            f.output
        );
        assert!(!f.output.contains("never finished: holds_one, holds_two"));
        assert_eq!(gh.cancels.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_run_the_step_limit_stopped_is_timed_out_with_its_output_not_a_run_without_a_result() {
        token();
        let r = repo();
        let zip = cut_artifact(r._t.path(), HUNG_OUT);
        let gh = FakeGh::new(&r, &zip, Mode::StepTimeout);
        let s = site(&r, gh.clone(), clock(), false);
        let logs = tempfile::tempdir().unwrap();
        let Outcome::Done(f) =
            execute(&s, &spec("st1"), logs.path(), &|| None, &Grant::test()).unwrap()
        else {
            panic!("not done");
        };
        assert!(f.timed_out && f.code == -1 && f.signal == 9);
        assert!(f.output.starts_with("test a ... ok\n"), "{}", f.output);
        assert!(f
            .output
            .contains("the run was cancelled after its limit of 600 s"));
        assert!(
            f.output.contains("never finished: holds_one\n"),
            "{}",
            f.output
        );
        assert!(
            gh.cancels.lock().unwrap().is_empty(),
            "it had ended already"
        );
    }

    /// `run_retrying` of a run on one runner of `wanted` that GitHub stops at the step limit.
    fn step_timeout_retry(wanted: u32, fallback: bool) -> (Retried, usize) {
        token();
        let r = repo();
        let zip = cut_artifact(r._t.path(), "test a ... ok\n");
        let gh = FakeGh::new(&r, &zip, Mode::StepTimeout);
        let s = site(&r, gh.clone(), clock(), fallback);
        let said = Mutex::new(vec![]);
        let dir = r._t.path().join("runs");
        let mut sp = spec("st2");
        sp.shards_wanted = wanted;
        let out = run_retrying(
            &s,
            &sp,
            &dir,
            &|| None,
            &|t| said.lock().unwrap().push(t.to_string()),
            &Grant::test(),
        );
        let pushes = gh.dispatches.lock().unwrap().len();
        ((out, gh, said.into_inner().unwrap()), pushes)
    }

    #[test]
    fn a_short_run_that_hit_the_step_limit_is_not_pushed_again_as_the_same_shape() {
        // 1 runner of 2 wanted (fewest 1): it ran, hit the limit, and goes to this Mac.
        let ((out, _, said), pushes) = step_timeout_retry(2, true);
        let (o, notes) = out.unwrap();
        assert!(o.is_none(), "the caller runs it here");
        assert_eq!(pushes, 1, "pushed once, not again");
        assert!(
            notes[0].contains("hit the runner's limit on 1 of 2 shards; not running it again"),
            "{notes:?}"
        );
        assert_eq!(said, notes);
        // Without a fallback it is an environmental error with the same wording.
        let ((out, _, _), pushes) = step_timeout_retry(2, false);
        let e = out.unwrap_err();
        assert!(
            environmental(&e).unwrap().contains("not running it again"),
            "{e}"
        );
        assert_eq!(pushes, 1);
    }

    #[test]
    fn a_run_with_its_full_runners_that_hit_the_step_limit_keeps_its_one_retry() {
        let ((out, _, _), pushes) = step_timeout_retry(1, false);
        let e = out.unwrap_err();
        assert!(environmental(&e).unwrap().contains("; then "), "{e}");
        assert_eq!(pushes, 2, "the one retry on GitHub");
    }

    #[test]
    fn a_run_granted_too_few_runners_is_never_pushed() {
        token();
        let r = repo();
        let zip = artifact(r._t.path(), GREEN, 0, None);
        let gh = FakeGh::new(&r, &zip, Mode::Normal);
        let s = site(&r, gh.clone(), clock(), true);
        let dir = r._t.path().join("runs");
        let mut sp = spec("st3");
        sp.shards_wanted = 8;
        let (o, notes) = run_retrying(&s, &sp, &dir, &|| None, &|_| {}, &Grant::test()).unwrap();
        assert!(o.is_none());
        assert!(
            notes[0].contains("granted 1 of 8 runner units"),
            "{notes:?}"
        );
        assert!(gh.dispatches.lock().unwrap().is_empty());
    }

    #[test]
    fn a_refused_or_fallen_back_run_with_an_emptied_lease_asks_for_no_units_and_holds_none() {
        use crate::sched::tests::{is_held, queue_gha, runner_broker};
        token();
        for fallback in [false, true] {
            let r = repo();
            let zip = artifact(r._t.path(), GREEN, 0, None);
            let gh = FakeGh::new(&r, &zip, Mode::Normal);
            let s = site(&r, gh.clone(), clock(), fallback);
            let dir = r._t.path().join("runs");
            let bdir = tempfile::tempdir().unwrap();
            let b = runner_broker(bdir.path(), 2, 1, 0);
            let lease = b
                .acquire(
                    mason_core::TaskId(1),
                    "#1 test step",
                    mason_scheduler::Rank::new(false, 3, 0, 1, 1, false),
                    mason_scheduler::Needs::of(&[("gha", 2)]),
                    &|| true,
                    &mut |_| {},
                )
                .unwrap();
            let grant = Grant::of(lease);
            // An earlier run of the same lease finished all its jobs: nothing is held.
            grant.give(&units_of(2));
            assert!(grant.units().is_empty());
            let mut sp = spec("rf");
            sp.shards_wanted = 8;
            let refused = execute_sharded(&s, &sp, &dir, &|| None, &grant);
            assert!(matches!(refused, Err(Fail::Environmental(_))));
            let r = run_retrying(&s, &sp, &dir, &|| None, &|_| {}, &grant);
            if fallback {
                assert!(r.unwrap().0.is_none(), "it runs on this Mac");
            } else {
                assert!(r.is_err());
            }
            // Nothing was pushed, nothing waits for units, and the grant holds none.
            assert!(gh.dispatches.lock().unwrap().is_empty());
            assert_eq!(b.gha_waiting(), 0);
            assert!(grant.units().is_empty());
            // So the whole pool is free for the next in line.
            let next = queue_gha(&b, 5);
            assert!(is_held(&b, next));
        }
    }

    #[test]
    fn the_fewest_runners_is_half_the_shards_and_a_grant_below_it_is_refused() {
        let lists = |n: u32| vec![vec![]; n as usize];
        let mk = |granted| split_for_grant(spec("m"), 8, "c", granted, false, lists);
        assert!(mk(Some(1)).too_few_runners().is_some());
        assert!(mk(Some(3)).too_few_runners().is_some());
        let five = mk(Some(5));
        assert_eq!(five.shards, 5);
        assert!(five.too_few_runners().is_none());
        assert!(mk(Some(4)).too_few_runners().is_none());
        let all = mk(None);
        assert_eq!(all.shards, 8);
        assert!(all.too_few_runners().is_none());
        // A suite of two or one shards can always run on one runner.
        let two = split_for_grant(spec("m"), 2, "c", Some(1), false, lists);
        assert!(two.too_few_runners().is_none());
        assert_eq!((min_shards(8), min_shards(3), min_shards(1)), (4, 2, 1));
    }

    #[test]
    fn a_run_past_its_limit_whose_artifact_never_comes_keeps_only_the_cut_off_line() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), HUNG_OUT, 0, None);
        let gh = FakeGh::new(&r, &zip, Mode::HungNoArtifact);
        let s = site(&r, gh, clock(), false);
        let logs = tempfile::tempdir().unwrap();
        let Outcome::Done(f) =
            execute(&s, &spec("x10"), logs.path(), &|| None, &Grant::test()).unwrap()
        else {
            panic!("not done");
        };
        assert!(f.timed_out);
        assert_eq!(
            f.output,
            "(the run was cancelled after its limit of 600 s on the runner)\n"
        );
    }

    #[test]
    fn no_runner_within_the_pickup_timeout_is_a_wait_charged_to_no_task() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let gh = FakeGh::new(&r, &zip, Mode::NeverPicked);
        let clk = clock();
        let s = site(&r, gh.clone(), clk.clone(), false);
        let logs = tempfile::tempdir().unwrap();
        let start = clk.now_ms();
        let e = run_or_fallback(&s, &spec("x7"), logs.path(), &|| None, &Grant::test())
            .err()
            .unwrap();
        let why = environmental(&e).expect("environmental").to_string();
        assert!(why.contains("no runner picked up"), "{why}");
        // Driven by the injected clock: twenty minutes passed on it, none on the wall.
        assert!(clk.now_ms() - start >= 1_200_000);
        assert_eq!(gh.cancels.lock().unwrap().len(), 1);
        assert_eq!(refs(&r.bare), "");
        // With `fallback = "local"` the same run takes the local path.
        let r2 = repo();
        let gh2 = FakeGh::new(&r2, &zip, Mode::NeverPicked);
        let s2 = site(&r2, gh2, clock(), true);
        let out = run_or_fallback(&s2, &spec("x7"), logs.path(), &|| None, &Grant::test()).unwrap();
        assert!(out.is_none());
    }

    /// A room function that says no (with a reason) for its first `no_for` asks and room after,
    /// and counts the asks.
    fn room_after(no_for: usize) -> (RoomFn, Arc<std::sync::atomic::AtomicUsize>) {
        let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let a = asked.clone();
        let f: RoomFn = Arc::new(move || {
            (a.fetch_add(1, Ordering::SeqCst) < no_for)
                .then(|| "this machine held for heat (die 94 C)".to_string())
        });
        (f, asked)
    }

    /// A stop that ends a wait that a bug would make endless (it is asked twice a second of the
    /// fake clock, so this is far more than any of these runs needs).
    fn give_up_after_many_looks() -> impl Fn() -> Option<StopWhy> {
        let n = std::sync::atomic::AtomicUsize::new(0);
        move || (n.fetch_add(1, Ordering::SeqCst) > 200_000).then_some(StopWhy::Gone)
    }

    /// #833: a favor-remote run unpicked past `spill_after` comes back at once when this machine
    /// has room (cancelled on GitHub, ref deleted), not after the pickup timeout; without room it
    /// keeps waiting on GitHub and comes back when room appears (it is no longer sent back at the
    /// timeout into a held queue); under plain `remote` it is not spilled. The fake clock drives
    /// time; only outcomes are asserted.
    #[test]
    fn an_unpicked_favor_remote_run_comes_back_early_when_this_machine_has_room() {
        token();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        // (fallback, asks answered "no room" before room appears, comes back before the timeout?)
        for (fallback, no_for, early, local) in [
            (true, 0, true, true),
            (true, 1000, false, true),
            (false, 0, false, false),
        ] {
            let r = repo();
            let gh = FakeGh::new(&r, &zip, Mode::NeverPicked);
            let clk = clock();
            let mut s = site(&r, gh.clone(), clk.clone(), fallback);
            s.spill_after = Some(Duration::from_secs(30));
            s.local_room = Some(room_after(no_for).0);
            let logs = tempfile::tempdir().unwrap();
            let start = clk.now_ms();
            // Plain `remote` is not spilled and, with no fallback, ends at the timeout.
            let out = run_or_fallback(
                &s,
                &spec("x9"),
                logs.path(),
                &give_up_after_many_looks(),
                &Grant::test(),
            );
            assert_eq!(matches!(out, Ok(None)), local, "{fallback} {no_for}");
            let waited = clk.now_ms() - start;
            assert_eq!(
                waited < s.pickup_timeout.as_millis() as u64,
                early,
                "{fallback} {no_for}: {waited}"
            );
            assert_eq!(gh.cancels.lock().unwrap().len(), 1);
            assert_eq!(refs(&r.bare), "");
        }
    }

    /// The pickup timeout has passed and this machine has no room: the run is not cancelled and
    /// keeps waiting on GitHub; it comes back the first time there is room. The room function
    /// was asked after the timeout, not only before it.
    #[test]
    fn a_run_past_the_pickup_timeout_waits_on_github_until_this_machine_has_room() {
        token();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let r = repo();
        let gh = FakeGh::new(&r, &zip, Mode::NeverPicked);
        let clk = clock();
        let mut s = site(&r, gh.clone(), clk.clone(), true);
        s.pickup_timeout = Duration::from_secs(60);
        let (room, asked) = room_after(40);
        s.local_room = Some(room);
        let logs = tempfile::tempdir().unwrap();
        let out = run_or_fallback(
            &s,
            &spec("x10"),
            logs.path(),
            &give_up_after_many_looks(),
            &Grant::test(),
        );
        assert!(matches!(out, Ok(None)), "it came back to this machine");
        assert!(
            asked.load(Ordering::SeqCst) > 40,
            "asked until there was room"
        );
        // Cancelled once, at the end, not when the timeout passed.
        assert_eq!(gh.cancels.lock().unwrap().len(), 1);
        assert_eq!(refs(&r.bare), "");
    }

    /// No room at the timeout, then a runner takes the job: it runs on GitHub, with no fallback.
    #[test]
    fn a_run_held_back_for_room_that_gets_a_runner_runs_on_github() {
        token();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let r = repo();
        // Picked up on the 20th look, well after the (shortened) timeout.
        let gh = FakeGh::picked_after(&r, &zip, Mode::Normal, 20);
        let clk = clock();
        let mut s = site(&r, gh.clone(), clk.clone(), true);
        s.pickup_timeout = Duration::from_secs(30);
        let (room, asked) = room_after(usize::MAX);
        s.local_room = Some(room);
        let logs = tempfile::tempdir().unwrap();
        let out = run_or_fallback(
            &s,
            &spec("x11"),
            logs.path(),
            &give_up_after_many_looks(),
            &Grant::test(),
        );
        assert!(matches!(out, Ok(Some(Outcome::Done(_)))), "ran on GitHub");
        assert!(
            asked.load(Ordering::SeqCst) > 0,
            "it was held back, not skipped"
        );
        assert!(gh.cancels.lock().unwrap().is_empty());
        assert_eq!(refs(&r.bare), "");
    }

    /// Without a room function the run comes back at the timeout as it always did, whatever this
    /// machine is doing.
    #[test]
    fn a_run_with_no_room_function_falls_back_at_the_pickup_timeout() {
        token();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let r = repo();
        let gh = FakeGh::new(&r, &zip, Mode::NeverPicked);
        let clk = clock();
        let mut s = site(&r, gh.clone(), clk.clone(), true);
        s.spill_after = Some(Duration::from_secs(30));
        assert!(s.local_room.is_none());
        let logs = tempfile::tempdir().unwrap();
        let start = clk.now_ms();
        let out = run_or_fallback(
            &s,
            &spec("x12"),
            logs.path(),
            &give_up_after_many_looks(),
            &Grant::test(),
        );
        assert!(matches!(out, Ok(None)));
        // Not before the timeout: spill needs a room function to say yes.
        assert!(clk.now_ms() - start >= s.pickup_timeout.as_millis() as u64);
        assert_eq!(gh.cancels.lock().unwrap().len(), 1);
        assert_eq!(refs(&r.bare), "");
    }

    /// #1025: the pickup clock starts when GitHub reports the run queued, not when it was pushed,
    /// and ignores a time before the push or in the future.
    #[test]
    fn the_pickup_clock_starts_at_the_runs_creation() {
        assert_eq!(pickup_clock_from(1_000, Some(5_000), 9_000), 5_000);
        assert_eq!(pickup_clock_from(1_000, None, 9_000), 1_000);
        assert_eq!(pickup_clock_from(1_000, Some(500), 9_000), 1_000);
        assert_eq!(pickup_clock_from(1_000, Some(20_000), 9_000), 1_000);
    }

    /// #1025: two runs reach their pickup timeout with one free unit between them. The room
    /// function takes the unit (as `try_take_fallback` does, atomically); the first run gets it
    /// and falls back, the second finds none, is not cancelled and is picked up on GitHub.
    #[test]
    fn two_runs_at_the_timeout_with_one_free_unit_send_only_one_back() {
        token();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let units = Arc::new(std::sync::atomic::AtomicU32::new(1));
        let take = |units: Arc<std::sync::atomic::AtomicU32>| -> RoomFn {
            Arc::new(move || {
                units
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                    .err()
                    .map(|_| "no compile unit free here".to_string())
            })
        };
        let mut outs = Vec::new();
        for (i, mode) in [Mode::NeverPicked, Mode::Normal].into_iter().enumerate() {
            let r = repo();
            let gh = if i == 0 {
                FakeGh::new(&r, &zip, mode)
            } else {
                FakeGh::picked_after(&r, &zip, mode, 20)
            };
            let clk = clock();
            let mut s = site(&r, gh.clone(), clk.clone(), true);
            s.pickup_timeout = Duration::from_secs(30);
            s.local_room = Some(take(units.clone()));
            let logs = tempfile::tempdir().unwrap();
            let id = format!("two{i}");
            let out = run_or_fallback(
                &s,
                &spec(&id),
                logs.path(),
                &give_up_after_many_looks(),
                &Grant::test(),
            );
            outs.push((out, gh.cancels.lock().unwrap().len()));
        }
        assert!(matches!(outs[0], (Ok(None), 1)), "the first came back");
        assert!(
            matches!(outs[1], (Ok(Some(Outcome::Done(_))), 0)),
            "the second stayed on GitHub, uncancelled, and got its result"
        );
        assert_eq!(units.load(Ordering::SeqCst), 0);
    }

    /// #1025: after a restart ten runs are in flight with their timeout long past; while this
    /// machine's gate is held none of them falls back, and each gets its one result when GitHub
    /// picks it up.
    #[test]
    fn runs_resent_after_a_restart_stay_on_github_while_the_gate_is_held() {
        token();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        for i in 0..10 {
            let r = repo();
            let gh = FakeGh::picked_after(&r, &zip, Mode::Normal, 200);
            let clk = clock();
            let mut s = site(&r, gh.clone(), clk.clone(), true);
            s.pickup_timeout = Duration::from_secs(5 + i);
            s.spill_after = Some(Duration::from_secs(1));
            s.local_room = Some(Arc::new(|| {
                Some("this machine held for heat (CPU held to 32% of its speed)".to_string())
            }));
            let logs = tempfile::tempdir().unwrap();
            let id = format!("rs{i}");
            let out = run_or_fallback(
                &s,
                &spec(&id),
                logs.path(),
                &give_up_after_many_looks(),
                &Grant::test(),
            );
            assert!(matches!(out, Ok(Some(Outcome::Done(_)))), "run {i}");
            assert!(gh.cancels.lock().unwrap().is_empty(), "run {i}");
        }
    }

    #[test]
    fn an_outage_or_a_refused_token_is_environmental() {
        token();
        let t = tempfile::tempdir().unwrap();
        // GitHub refusing the dispatch itself (an outage or a refused token) ends the attempt
        // at once and leaves no ref; waiting out an outage applies once the run is started.
        for code in [503u16, 401] {
            let r = repo();
            let zip = artifact(t.path(), GREEN, 0, None);
            let gh = FakeGh::new(&r, &zip, Mode::Status(code));
            let clk = clock();
            let s = site(&r, gh, clk.clone(), false);
            let logs = tempfile::tempdir().unwrap();
            let e = execute(&s, &spec("x8"), logs.path(), &|| None, &Grant::test())
                .err()
                .unwrap();
            assert!(matches!(e, Fail::Environmental(_)), "{code}: {e:?}");
            assert_eq!(refs(&r.bare), "");
        }
    }

    #[test]
    fn a_run_that_ends_without_an_artifact_is_environmental() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let gh = FakeGh::new(&r, &zip, Mode::NoArtifact);
        let s = site(&r, gh, clock(), false);
        let logs = tempfile::tempdir().unwrap();
        let e = execute(&s, &spec("x9"), logs.path(), &|| None, &Grant::test())
            .err()
            .unwrap();
        assert!(
            matches!(&e, Fail::Environmental(w) if w.contains("without a result artifact")),
            "{e:?}"
        );
        assert_eq!(refs(&r.bare), "");
    }

    #[test]
    fn a_setup_failure_is_environmental_and_its_retries_back_off() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let gh = FakeGh::new(&r, &zip, Mode::NoArtifact);
        let s = site(&r, gh, clock(), false);
        let logs = tempfile::tempdir().unwrap();
        let e = execute(&s, &spec("x9b"), logs.path(), &|| None, &Grant::test())
            .err()
            .unwrap();
        assert!(
            matches!(&e, Fail::Environmental(w) if w.contains("failed in setup")),
            "{e:?}"
        );
        let base = Duration::from_secs(30);
        assert_eq!(env_backoff(base, 0), base);
        assert!(env_backoff(base, 3) > env_backoff(base, 2));
        assert_eq!(env_backoff(base, 40), base * 64);
    }

    #[test]
    fn the_run_json_names_the_xcode_the_command_asks_for() {
        let mut sp = spec("x3x");
        sp.command =
            "DEVELOPER_DIR=/Applications/Xcode_16.4.app/Contents/Developer swift test".into();
        let v: serde_json::Value = serde_json::from_str(&run_json(&sp)).unwrap();
        assert_eq!(v["xcode"], "16.4");
    }

    #[test]
    fn a_limit_counts_from_when_a_runner_took_the_job_and_cancels_the_run() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let mut gh = Arc::try_unwrap(FakeGh::new(&r, &zip, Mode::Normal))
            .ok()
            .unwrap();
        gh.queued = 15; // 15 looks at 60 s: fifteen minutes queued, which is not the limit's time
        gh.running = 100_000;
        let gh = Arc::new(gh);
        let s = site(&r, gh.clone(), clock(), false);
        let logs = tempfile::tempdir().unwrap();
        let mut sp = spec("x10");
        sp.limit = Duration::from_secs(120);
        let Outcome::Done(f) = execute(&s, &sp, logs.path(), &|| None, &Grant::test()).unwrap()
        else {
            panic!("not done")
        };
        assert!(f.timed_out);
        assert_eq!(gh.cancels.lock().unwrap().len(), 1);
        assert_eq!(refs(&r.bare), "");
    }

    #[test]
    fn a_daemon_that_restarts_collects_the_same_run_and_starts_no_second_one() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let logs = tempfile::tempdir().unwrap();
        let gh = FakeGh::new(&r, &zip, Mode::Normal);
        *gh.die_at.lock().unwrap() = Some(2);
        let s = site(&r, gh.clone(), clock(), false);
        let died = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = execute(&s, &spec("x11"), logs.path(), &|| None, &Grant::test());
        }));
        assert!(died.is_err(), "the first daemon died mid-run");
        let first = State::load(&record_path(logs.path(), "x11")).expect("the record survives");
        assert!(first.run_id.is_some() && !first.collected);
        assert_ne!(refs(&r.bare), "", "the ref is still there");
        // A new daemon: nothing in memory but the record.
        *gh.die_at.lock().unwrap() = None;
        let s2 = site(&r, gh.clone(), clock(), false);
        let Outcome::Done(f) =
            execute(&s2, &spec("x11"), logs.path(), &|| None, &Grant::test()).unwrap()
        else {
            panic!("not done")
        };
        assert_eq!(f.output, GREEN);
        let after = State::load(&record_path(logs.path(), "x11")).unwrap();
        assert_eq!(after.sha, first.sha, "the same snapshot");
        assert_eq!(after.run_id, first.run_id, "the same run");
        assert_eq!(
            gh.runs_seen.lock().unwrap().len(),
            1,
            "one commit was ever run"
        );
        assert_eq!(refs(&r.bare), "");
        assert_eq!(
            gh.dispatches.lock().unwrap().len(),
            1,
            "the restarted daemon did not dispatch again"
        );
        // Asked again once collected, it answers from the record and starts nothing.
        let again = execute(&s2, &spec("x11"), logs.path(), &|| None, &Grant::test()).unwrap();
        assert!(matches!(again, Outcome::Done(g) if g.output == GREEN));
        assert_eq!(gh.runs_seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn an_environmental_retry_in_flight_at_a_restart_is_reattached_and_collected_once() {
        // #1074: the retry (`-u<ms>-`) was out when the daemon died; the recovered step names
        // it from its attempts record, and it is collected with no second push.
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let logs = tempfile::tempdir().unwrap();
        let gh = FakeGh::new(&r, &zip, Mode::Normal);
        *gh.die_at.lock().unwrap() = Some(2);
        let id = "t7r9-u1700000000000-unit";
        let s = site(&r, gh.clone(), clock(), false);
        let died = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = execute(&s, &spec(id), logs.path(), &|| None, &Grant::test());
        }));
        assert!(died.is_err());
        *gh.die_at.lock().unwrap() = None;
        let s2 = site(&r, gh.clone(), clock(), false);
        let Outcome::Done(f) =
            execute(&s2, &spec(id), logs.path(), &|| None, &Grant::test()).unwrap()
        else {
            panic!("not done")
        };
        assert_eq!(f.output, GREEN);
        assert_eq!(
            gh.dispatches.lock().unwrap().len(),
            1,
            "no extra push or dispatch"
        );
        assert_eq!(gh.runs_seen.lock().unwrap().len(), 1);
        assert_eq!(gh.cancels.lock().unwrap().len(), 0);
        assert_eq!(refs(&r.bare), "");
    }

    #[test]
    fn a_collected_run_is_reused_only_for_the_tree_it_ran_on() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let logs = tempfile::tempdir().unwrap();
        let gh = FakeGh::new(&r, &zip, Mode::Normal);
        let s = site(&r, gh.clone(), clock(), false);
        let a = spec("tk1");
        assert!(matches!(
            execute(&s, &a, logs.path(), &|| None, &Grant::test()).unwrap(),
            Outcome::Done(_)
        ));
        assert_eq!(gh.dispatches.lock().unwrap().len(), 1);
        // The same tree: answered from the record, nothing pushed or dispatched.
        assert!(matches!(
            execute(&s, &a, logs.path(), &|| None, &Grant::test()).unwrap(),
            Outcome::Done(_)
        ));
        assert_eq!(gh.dispatches.lock().unwrap().len(), 1);
        // Another tree: A's result is not B's.
        let mut b = spec("tk1");
        b.tree_key = "treekey+other".into();
        assert!(matches!(
            execute(&s, &b, logs.path(), &|| None, &Grant::test()).unwrap(),
            Outcome::Done(_)
        ));
        assert_eq!(gh.dispatches.lock().unwrap().len(), 2, "a new run was made");
        assert_eq!(
            State::load(&record_path(logs.path(), "tk1"))
                .unwrap()
                .tree_key,
            "treekey+other"
        );
        // A record from before keys were kept counts as another tree.
        let mut old = State::load(&record_path(logs.path(), "tk1")).unwrap();
        old.tree_key = String::new();
        old.save(&record_path(logs.path(), "tk1"));
        assert!(matches!(
            execute(&s, &b, logs.path(), &|| None, &Grant::test()).unwrap(),
            Outcome::Done(_)
        ));
        assert_eq!(gh.dispatches.lock().unwrap().len(), 3);
        assert_eq!(refs(&r.bare), "");
    }

    #[test]
    fn a_run_in_flight_on_another_tree_is_cancelled_and_started_again() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let logs = tempfile::tempdir().unwrap();
        let gh = FakeGh::new(&r, &zip, Mode::Normal);
        *gh.die_at.lock().unwrap() = Some(2);
        let s = site(&r, gh.clone(), clock(), false);
        let died = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = execute(&s, &spec("tk2"), logs.path(), &|| None, &Grant::test());
        }));
        assert!(died.is_err());
        *gh.die_at.lock().unwrap() = None;
        let mut b = spec("tk2");
        b.tree_key = "treekey+other".into();
        let s2 = site(&r, gh.clone(), clock(), false);
        let Outcome::Done(f) = execute(&s2, &b, logs.path(), &|| None, &Grant::test()).unwrap()
        else {
            panic!("not done")
        };
        assert_eq!(f.output, GREEN);
        assert_eq!(
            gh.cancels.lock().unwrap().len(),
            1,
            "the old run was cancelled"
        );
        assert_eq!(gh.dispatches.lock().unwrap().len(), 2, "and a new one made");
        assert_eq!(refs(&r.bare), "");
    }

    /// The inputs of the one dispatch the fake saw, as JSON.
    fn dispatched_inputs(gh: &FakeGh) -> serde_json::Value {
        let d = gh.dispatches.lock().unwrap();
        assert_eq!(d.len(), 1, "{d:?}");
        let body = d[0].split_once(' ').unwrap().1;
        serde_json::from_str(body).unwrap()
    }

    #[test]
    fn execute_pushes_the_ref_then_starts_the_run_once_with_the_runner_and_shards_of_the_spec() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let gh = FakeGh::new(&r, &zip, Mode::NeverPicked);
        let s = site(&r, gh.clone(), clock(), false);
        let logs = tempfile::tempdir().unwrap();
        let mut sp = spec("d1");
        sp.shards = 3;
        sp.shard_lists = vec![vec![], vec![], vec![]];
        sp.runs_on = "macos-15".into();
        // Stop once the jobs have been looked at.
        let g2 = gh.clone();
        let stop = move || {
            g2.requests()
                .iter()
                .any(|q| q.contains("/jobs"))
                .then_some(StopWhy::Skipped)
        };
        let out = execute(&s, &sp, logs.path(), &stop, &Grant::test()).unwrap();
        assert!(matches!(out, Outcome::Stopped(StopWhy::Skipped)));
        let log = gh.requests();
        let at = |needle: &str| log.iter().position(|q| q.contains(needle));
        // The dispatch is one POST to the workflow, on the pushed ref, and it came before any
        // look for the run.
        let posts: Vec<_> = log
            .iter()
            .filter(|q| q.starts_with("POST") && q.contains("/dispatches"))
            .collect();
        assert_eq!(posts.len(), 1, "{log:?}");
        assert!(posts[0].contains("/actions/workflows/mason-run.yml/dispatches"));
        assert!(at("/dispatches").unwrap() < at("head_sha=").unwrap());
        let body = dispatched_inputs(&gh);
        assert!(body["ref"].as_str().unwrap().starts_with("mason/run/proj/"));
        assert_eq!(body["inputs"]["runs_on"], "\"macos-15\"");
        assert_eq!(body["inputs"]["matrix"], "[1,2,3]");
        assert_eq!(body["inputs"]["sharded"], "true");
        assert_eq!(body["inputs"]["timeout_min"], "8");
        // The inputs are what run.json says, so the workflow's check passes.
        let j: serde_json::Value = serde_json::from_str(&run_json(&sp)).unwrap();
        assert_eq!(body["inputs"]["matrix"], j["matrix"].to_string());
        assert_eq!(body["inputs"]["runs_on"], j["runs_on"].to_string());
        assert_eq!(body["inputs"]["toolchains"], j["toolchains"].to_string());
        // From the first look the run's jobs are run (1) to run (3), with no `plan` job.
        let names: Vec<String> =
            s.gh.run_jobs(4242)
                .unwrap()
                .into_iter()
                .map(|j| j.name)
                .collect();
        assert_eq!(names, vec!["run (1)", "run (2)", "run (3)"]);
        assert!(refs(&r.bare).is_empty(), "a stopped run's ref is deleted");
    }

    #[test]
    fn a_restarted_daemon_finds_the_run_by_its_sha_and_does_not_start_it_again() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let sp = spec("d2");
        // A record written and a ref pushed by a daemon that died before it dispatched.
        let write = |logs: &Path| {
            let sha = snapshot(
                &r.wt,
                "origin/main",
                Some("mason-run.yml"),
                &run_json(&sp),
                "m",
            )
            .unwrap();
            let st = State {
                ref_name: ref_name(&sp.project, &sp.id),
                sha,
                tree_key: sp.tree_key.clone(),
                pushed_ms: 1_000_000_000,
                runs_on: sp.runs_on.clone(),
                ..Default::default()
            };
            st.save(&record_path(logs, &sp.id));
            git_ok(
                &r.wt,
                &[
                    "push",
                    "-q",
                    "origin",
                    &format!("{}:refs/heads/{}", st.sha, st.ref_name),
                ],
            );
        };
        // GitHub already has the run (the dispatch got through): no second dispatch.
        let logs = tempfile::tempdir().unwrap();
        write(logs.path());
        let gh = FakeGh::new(&r, &zip, Mode::Normal);
        gh.dispatches.lock().unwrap().push("earlier {}".into());
        let s = site(&r, gh.clone(), clock(), false);
        let out = execute(&s, &sp, logs.path(), &|| None, &Grant::test()).unwrap();
        assert!(matches!(out, Outcome::Done(f) if f.output == GREEN));
        assert_eq!(gh.dispatches.lock().unwrap().len(), 1, "started once only");
        // GitHub has none: the restarted daemon starts it, once.
        let r2 = repo();
        let logs2 = tempfile::tempdir().unwrap();
        let rec = |l: &Path| record_path(l, &sp.id);
        let sha = snapshot(
            &r2.wt,
            "origin/main",
            Some("mason-run.yml"),
            &run_json(&sp),
            "m",
        )
        .unwrap();
        State {
            ref_name: ref_name(&sp.project, &sp.id),
            sha,
            tree_key: sp.tree_key.clone(),
            pushed_ms: 1_000_000_000,
            runs_on: sp.runs_on.clone(),
            ..Default::default()
        }
        .save(&rec(logs2.path()));
        let gh2 = FakeGh::new(&r2, &zip, Mode::Normal);
        let s2 = site(&r2, gh2.clone(), clock(), false);
        let out = execute(&s2, &sp, logs2.path(), &|| None, &Grant::test()).unwrap();
        assert!(matches!(out, Outcome::Done(f) if f.output == GREEN));
        assert_eq!(gh2.dispatches.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_dispatch_github_refuses_leaves_no_ref_and_no_record() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let gh = FakeGh::new(&r, &zip, Mode::Status(403));
        let s = site(&r, gh, clock(), false);
        let logs = tempfile::tempdir().unwrap();
        let Err(e) = execute(&s, &spec("d3"), logs.path(), &|| None, &Grant::test()) else {
            panic!("the run started")
        };
        assert!(matches!(e, Fail::Environmental(m) if m.contains("could not start")));
        assert_eq!(refs(&r.bare), "");
        assert!(!record_path(logs.path(), "d3").exists());
    }

    #[test]
    fn the_run_workflow_is_one_wave_that_refuses_inputs_that_differ_from_run_json() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for rel in [
            ".github/workflows/mason-run.yml",
            "templates/github/mason-run.yml",
        ] {
            let text = std::fs::read_to_string(root.join(rel)).unwrap();
            // Started by dispatch only: a push to a run ref must not start it a second time.
            assert!(mason_adapters::runref::dispatches(&text), "{rel}");
            assert!(
                !mason_adapters::runref::fires_on(
                    &mason_adapters::runref::push_trigger(&text),
                    mason_adapters::runref::SAMPLE_REF
                ),
                "{rel} runs on push"
            );
            // No planning job: the one job takes its runner and matrix from the inputs.
            assert!(!text.contains("  plan:"), "{rel}");
            assert!(!text.contains("needs:"), "{rel}");
            assert!(text.contains("runs-on: ${{ fromJSON(inputs.runs_on) }}"));
            assert!(text.contains("shard: ${{ fromJSON(inputs.matrix) }}"));
            // The first step after checkout compares every input to run.json and fails the job.
            let at = text.find("name: inputs match run.json").expect(rel);
            let step = &text[at..];
            for (input, jq) in [
                ("runs_on", "jq -c .runs_on .mason/run.json"),
                ("matrix", "jq -c '.matrix // [1]' .mason/run.json"),
                ("timeout_min", "jq -c .timeout_min .mason/run.json"),
                ("sharded", "jq -r '(.shards // 1) > 1' .mason/run.json"),
                (
                    "toolchains",
                    "jq -c '.toolchains // [\"rust\"]' .mason/run.json",
                ),
            ] {
                assert!(
                    step.contains(&format!("same {input} ")) && step.contains(jq),
                    "{rel}: {input}"
                );
            }
            assert!(step.contains("exit \"$bad\""), "{rel}");
            // It comes before the step that runs the command.
            assert!(at < text.find("- name: run\n").unwrap(), "{rel}");
        }
    }

    fn units_of(n: u32) -> std::collections::BTreeMap<String, u32> {
        std::collections::BTreeMap::from([("gha".to_string(), n)])
    }

    #[test]
    fn a_finished_shard_gives_its_unit_back_at_once_and_a_restart_counts_only_those_still_out() {
        use crate::sched::tests::{is_held, queue_gha, runner_broker};
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let gh = FakeGh::new(&r, &zip, Mode::Hung);
        // Shard 2 of three has finished; 1 and 3 are still running.
        *gh.job_status.lock().unwrap() = Some(vec!["in_progress", "completed", "in_progress"]);
        let s = site(&r, gh.clone(), clock(), false);
        let logs = tempfile::tempdir().unwrap();
        let mut sp = spec("g1");
        sp.shards = 3;
        sp.shard_lists = vec![vec![], vec![], vec![]];
        let bdir = tempfile::tempdir().unwrap();
        let b = runner_broker(bdir.path(), 3, 1, 0);
        let lease = b
            .acquire(
                mason_core::TaskId(1),
                "#1 test step",
                mason_scheduler::Rank::new(false, 3, 0, 1, 1, false),
                mason_scheduler::Needs::of(&[("gha", 3)]),
                &|| true,
                &mut |_| {},
            )
            .unwrap();
        let grant = Grant::of(lease);
        let holds = logs.path().join("remote-holds");
        write_hold(
            &holds,
            "g1",
            &RemoteHold {
                key: "task 1".into(),
                label: "#1".into(),
                units: units_of(3),
                record: record_path(logs.path(), "g1"),
                kind: "test_step".into(),
                task: 1,
            },
        );
        grant.note_hold(&holds, "g1");
        // The next in line needs one unit and waits: all three are held.
        let waiter = queue_gha(&b, 9);
        assert!(!is_held(&b, waiter));
        // The run is left as soon as the waiter is granted, with two shards still running.
        // The hold file is read then, while the run (and its record) is still out.
        let b2 = b.clone();
        let seen: Mutex<Vec<RemoteHold>> = Mutex::new(vec![]);
        let stop = || {
            is_held(&b2, waiter).then(|| {
                *seen.lock().unwrap() = running_holds(&holds);
                StopWhy::Skipped
            })
        };
        let out = execute(&s, &sp, logs.path(), &stop, &grant).unwrap();
        assert!(matches!(out, Outcome::Stopped(StopWhy::Skipped)));
        assert!(
            is_held(&b, waiter),
            "granted while shards 1 and 3 still ran"
        );
        assert_eq!(grant.units(), units_of(2));
        let kept = seen.lock().unwrap().clone();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].units, units_of(2));
        // A daemon started with that hold file counts two units: of three, one more is free.
        let again = tempfile::tempdir().unwrap();
        let b3 = runner_broker(again.path(), 3, 1, 0);
        // The daemon died with the run still out: its record is there.
        State {
            sha: "abc".into(),
            ..Default::default()
        }
        .save(&kept[0].record);
        b3.adopt_remote(&kept);
        let first = queue_gha(&b3, 1);
        let second = queue_gha(&b3, 2);
        assert!(is_held(&b3, first));
        assert!(
            !is_held(&b3, second),
            "two of the three units are the run's"
        );
    }

    #[test]
    fn shards_run_again_ask_for_their_units_again_at_the_runs_own_rank() {
        use crate::sched::tests::{is_held, queue_gha, runner_broker};
        let bdir = tempfile::tempdir().unwrap();
        let b = runner_broker(bdir.path(), 1, 1, 0);
        let lease = b
            .acquire(
                mason_core::TaskId(1),
                "#1 test step",
                mason_scheduler::Rank::new(false, 3, 0, 1, 1, false),
                mason_scheduler::Needs::of(&[("gha", 1)]),
                &|| true,
                &mut |_| {},
            )
            .unwrap();
        let grant = Grant::of(lease);
        let first = queue_gha(&b, 8);
        // The shard finished: its unit goes to the one waiting.
        grant.give(&units_of(1));
        assert!(grant.units().is_empty());
        assert!(is_held(&b, first));
        // A younger request of the same kind queues; then the missing shard asks again.
        // (Through `acquire`, so it has waited as long as it says: `queue_gha` would age it.)
        let (tx, younger) = std::sync::mpsc::channel();
        let b9 = b.clone();
        std::thread::spawn(move || {
            let lease = b9.acquire(
                mason_core::TaskId(9),
                "#9 test step",
                mason_scheduler::Rank::new(false, 3, 0, 9, 9, false),
                mason_scheduler::Needs::of(&[("gha", 1)]),
                &|| true,
                &mut |_| {},
            );
            let _ = tx.send(lease);
        });
        let began = std::time::Instant::now();
        let queued = |n: usize| {
            while b.gha_waiting() < n {
                assert!(began.elapsed().as_secs() < 60, "{n} never queued");
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        queued(1);
        let g2 = grant.clone();
        let again = std::thread::spawn(move || g2.ask_again(units_of(1), &|| None));
        queued(2);
        b.release_for_test(first, mason_core::TaskId(8));
        while !again.is_finished() {
            assert!(began.elapsed().as_secs() < 60, "the rerun was not granted");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(again.join().unwrap());
        assert_eq!(grant.units(), units_of(1));
        assert!(
            younger.try_recv().is_err(),
            "the rerun went before the younger request"
        );
    }

    #[test]
    fn a_macos_run_holds_a_unit_of_each_pool_and_a_sharded_one_holds_one_per_job() {
        let u = units_for("macos-14", 1);
        assert_eq!((u["gha"], u["gha-macos"]), (1, 1));
        let u = units_for("ubuntu-latest", 3);
        assert_eq!(u["gha"], 3);
        assert!(!u.contains_key("gha-macos"));
        assert_eq!(units_for("macos-latest", 3)["gha-macos"], 3);
        assert!(is_macos("[\"macos-14\"]") && !is_macos("ubuntu-latest"));
    }

    #[test]
    fn a_hold_is_kept_until_the_run_ends_and_is_dropped_once_its_record_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let holds = dir.path().join("remote-holds");
        let rec = dir.path().join("a.remote");
        let st = State {
            sha: "abc".into(),
            ..Default::default()
        };
        st.save(&rec);
        let h = RemoteHold {
            key: "task 4".into(),
            label: "#4".into(),
            units: units_for("macos-14", 1),
            record: rec.clone(),
            kind: "debt".into(),
            task: 4,
        };
        write_hold(&holds, "a", &h);
        assert_eq!(running_holds(&holds), vec![h]);
        State {
            collected: true,
            ..st
        }
        .save(&rec);
        assert!(running_holds(&holds).is_empty());
    }

    fn job(
        name: &str,
        status: &str,
        steps: &[(&str, &str, &str)],
    ) -> mason_adapters::github::ActionJob {
        mason_adapters::github::ActionJob {
            name: name.into(),
            status: status.into(),
            started_at: None,
            completed_at: None,
            conclusion: None,
            labels: vec![],
            steps: steps
                .iter()
                .map(|(n, st, at)| mason_adapters::github::ActionStep {
                    name: (*n).into(),
                    status: (*st).into(),
                    started_at: (!at.is_empty()).then(|| (*at).into()),
                })
                .collect(),
        }
    }

    #[test]
    fn the_phase_of_a_run_in_progress_is_its_current_step_not_a_percentage() {
        let t0 = parse_iso("2026-11-20T10:00:00Z").unwrap() * 1000;
        let now = t0 + 4 * 60_000 + 5_000;
        let jobs = vec![
            job("plan", "completed", &[]),
            job(
                "run",
                "in_progress",
                &[
                    ("Set up job", "completed", "2026-11-20T10:00:00Z"),
                    ("run", "in_progress", "2026-11-20T10:00:00Z"),
                ],
            ),
        ];
        let text = phase_text(&jobs, 0, now, t0);
        assert_eq!(text, "on GitHub: running, 4 min");
        assert!(!text.contains('%'));
        let queued = vec![job("run", "queued", &[])];
        assert_eq!(
            phase_text(&queued, 0, now, t0),
            "on GitHub: waiting for a runner, 4 min"
        );
    }

    #[test]
    fn a_check_granted_fewer_units_than_its_shards_splits_that_many_ways_and_holds_them() {
        let spec = |id: &str| {
            spec_for(
                "p",
                id,
                None,
                "cargo test",
                Duration::from_secs(60),
                &[],
                "ubuntu-latest",
                "",
            )
        };
        let lists = |n: u32| vec![vec![]; n as usize];
        let dir = tempfile::tempdir().unwrap();
        let hosts = dir.path().join("hosts");
        let logs = dir.path().join("logs");
        let hold_units = |id: &str| {
            let kv = read_kv(&hold_file(&holds_dir(&hosts), id)).unwrap();
            kv.get("units").cloned().unwrap_or_default()
        };
        // Granted 3 of 8: three shards, three lists, and the hold records 3 units.
        let rs = split_for_grant(spec("c1"), 8, "shard-cmd", Some(3), false, lists);
        assert_eq!((rs.shards, rs.shard_lists.len()), (3, 3));
        assert_eq!(rs.shard_command.as_deref(), Some("shard-cmd"));
        let guard = HoldGuard::new(&hosts, &rs, "c1", "c1", rs.shards.max(1), &logs, "check", 1);
        assert_eq!(hold_units("c1"), "gha=3");
        drop(guard);
        // Granted 1: the whole command on one runner, one unit.
        let rs = split_for_grant(spec("c2"), 8, "shard-cmd", Some(1), false, lists);
        assert_eq!(rs.shards, 0);
        assert!(rs.shard_command.is_none() && rs.shard_lists.is_empty());
        let _guard = HoldGuard::new(&hosts, &rs, "c2", "c2", rs.shards.max(1), &logs, "check", 1);
        assert_eq!(hold_units("c2"), "gha=1");
        // Not a flexible lease: all eight.
        let rs = split_for_grant(spec("c3"), 8, "shard-cmd", None, false, lists);
        assert_eq!(rs.shards, 8);
    }

    #[test]
    fn a_sharded_run_says_how_many_shards_are_done() {
        let mut jobs: Vec<_> = (1..=8)
            .map(|k| {
                let st = if k <= 3 { "completed" } else { "in_progress" };
                job(&format!("run ({k})"), st, &[])
            })
            .collect();
        jobs.insert(0, job("plan", "completed", &[]));
        assert_eq!(phase_text(&jobs, 8, 0, 0), "on GitHub: 3 of 8 shards done");
    }

    #[test]
    fn a_run_in_progress_reports_its_phase_without_repeating_it() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), GREEN, 0, None);
        let gh = FakeGh::new(&r, &zip, Mode::Normal);
        let mut s = site(&r, gh, clock(), false);
        let said: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = said.clone();
        s.on_phase = Some(Arc::new(move |t: &str| {
            sink.lock().unwrap().push(t.to_string())
        }));
        let logs = tempfile::tempdir().unwrap();
        let Ok(Outcome::Done(_)) = execute(&s, &spec("ph1"), logs.path(), &|| None, &Grant::test())
        else {
            panic!("not done")
        };
        let said = said.lock().unwrap();
        assert!(!said.is_empty());
        assert!(
            said.iter()
                .all(|t| t.starts_with("on GitHub: ") && !t.contains('%')),
            "{said:?}"
        );
        assert!(said.windows(2).all(|w| w[0] != w[1]), "{said:?}");
    }

    #[test]
    fn iso_times_and_ref_names() {
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_iso("2026-11-20T10:00:06Z").unwrap() - parse_iso("2026-11-20T10:00:00Z").unwrap(),
            6
        );
        assert_eq!(
            parse_iso("2028-03-01T00:00:00Z").unwrap() - parse_iso("2028-02-28T00:00:00Z").unwrap(),
            2 * 86400
        );
        assert_eq!(
            parse_iso("2026-03-01T00:00:00Z").unwrap() - parse_iso("2026-02-28T00:00:00Z").unwrap(),
            86400
        );
        assert_eq!(parse_iso("garbage"), None);
        assert_eq!(
            ref_name("star app", "t5r7-u-cargo workspace"),
            "mason/run/star-app/t5r7-u-cargo-workspace"
        );
    }

    #[test]
    fn nothing_goes_remote_unless_a_test_forces_it() {
        assert_eq!(environmental(&env_err("x")), Some("x"));
        assert_eq!(environmental("plain"), None);
    }

    // ---- sharded runs (#701) ----

    /// A fake GitHub for a run split three ways: every shard has an artifact, except the shards
    /// in `lost` on the first run (its branch has no `-s` rerun suffix).
    struct ShardGh {
        bare: PathBuf,
        zips: std::collections::BTreeMap<u32, PathBuf>,
        lost: Vec<u32>,
        shas: Mutex<Vec<String>>,
    }

    impl ShardGh {
        fn branch(&self) -> String {
            git_ok(
                &self.bare,
                &[
                    "for-each-ref",
                    "--format=%(refname:short)",
                    "refs/heads/mason/run",
                ],
            )
            .lines()
            .next()
            .unwrap_or("")
            .to_string()
        }
    }

    impl Http for ShardGh {
        fn send(&self, r: &Request) -> Result<Response, Transport> {
            let u = r.url.as_str();
            let run = |branch: &str| {
                serde_json::json!({"id": 77, "status": "completed", "conclusion": "success",
                    "head_branch": branch, "event": "push",
                    "html_url": "https://github.com/o/r/actions/runs/77",
                    "created_at": "2026-11-20T10:00:00Z"})
            };
            if r.method == "POST" && u.ends_with("/dispatches") {
                return Ok(Response {
                    status: 204,
                    headers: vec![],
                    body: String::new(),
                });
            }
            if u.contains("/actions/runs?head_sha=") {
                let sha = u
                    .split("head_sha=")
                    .nth(1)
                    .unwrap()
                    .split('&')
                    .next()
                    .unwrap();
                let mut shas = self.shas.lock().unwrap();
                if !shas.iter().any(|s| s == sha) {
                    shas.push(sha.to_string());
                }
                return ok(serde_json::json!({"workflow_runs": [run(&self.branch())]}));
            }
            if u.ends_with("/actions/runs/77") {
                return ok(run(&self.branch()));
            }
            if u.contains("/actions/runs/77/artifacts") {
                let rerun = self.branch().contains("-s1");
                let name = u.split("name=").nth(1).unwrap_or("");
                let k: u32 = name.rsplit('-').next().unwrap().parse().unwrap();
                if !rerun && self.lost.contains(&k) {
                    return ok(serde_json::json!({"artifacts": []}));
                }
                return ok(
                    serde_json::json!({"artifacts": [{"name": name, "expired": false,
                    "archive_download_url": format!("https://api.github.com/artifact/zip/{k}")}]}),
                );
            }
            Err(Transport(format!("fake GitHub: no answer for {u}")))
        }

        fn download(&self, r: &Request, dest: &Path) -> Result<Response, Transport> {
            let k: u32 = r.url.rsplit('/').next().unwrap().parse().unwrap();
            std::fs::copy(&self.zips[&k], dest).map_err(|e| Transport(e.to_string()))?;
            Ok(Response {
                status: 200,
                headers: vec![],
                body: String::new(),
            })
        }
    }

    fn sharded_spec(id: &str, lists: &[&[&str]]) -> RunSpec {
        RunSpec {
            shards: lists.len() as u32,
            shard_lists: lists
                .iter()
                .map(|l| l.iter().map(|t| t.to_string()).collect())
                .collect(),
            shard_command: Some("python3 -I scripts/test-shard.py -- cargo test".into()),
            ..spec(id)
        }
    }

    /// Run a 3-way spec against `lost`; returns the result and the fake.
    fn run_three(outputs: [(&str, i32); 3], lost: Vec<u32>) -> (Finished, Arc<ShardGh>, Repo) {
        let r = repo();
        token();
        let mut zips = std::collections::BTreeMap::new();
        for (i, (out, code)) in outputs.iter().enumerate() {
            let d = r._t.path().join(format!("shard{}", i + 1));
            std::fs::create_dir_all(&d).unwrap();
            zips.insert(i as u32 + 1, artifact(&d, out, *code, None));
        }
        let gh = Arc::new(ShardGh {
            bare: r.bare.clone(),
            zips,
            lost,
            shas: Mutex::new(vec![]),
        });
        let site = Site {
            cwd: r.wt.clone(),
            remote: "origin".into(),
            base_ref: "origin/main".into(),
            workflow: "mason-run.yml".into(),
            pickup_timeout: Duration::from_secs(1200),
            fallback_local: false,
            spill_after: None,
            local_room: None,
            local_barred: None,
            resplit: None,
            test_env: vec![],
            gh: GitHub::new(
                gh.clone(),
                "https://api.github.com",
                "o/r",
                TokenRef::Env("MASON_TEST_REMOTE_TOKEN".into()),
            ),
            clock: clock(),
            poll_s: 10,
            on_phase: None,
        };
        let dir = r._t.path().join("runs");
        let spec = sharded_spec("sh1", &[&["a", "b"], &["c"], &["d"]]);
        let Ok(Outcome::Done(f)) = execute_sharded(&site, &spec, &dir, &|| None, &Grant::test())
        else {
            panic!("the sharded run did not finish");
        };
        (f, gh, r)
    }

    const A: &str = "test a ... ok\ntest b ... ok\n";
    const C: &str = "test c ... ok\n";
    const D: &str = "test d ... ok\n";

    #[test]
    fn three_shards_are_one_result_in_shard_order() {
        let (f, gh, r) = run_three([(A, 0), (C, 0), (D, 0)], vec![]);
        assert_eq!(f.output, format!("{A}{C}{D}"));
        assert_eq!((f.code, f.missing.len(), f.parts.len()), (0, 0, 3));
        assert_eq!(
            f.parts.iter().map(|p| p.listed).collect::<Vec<_>>(),
            [2, 1, 1]
        );
        // One run, one snapshot: the runners are told the matrix and their lists.
        let shas = gh.shas.lock().unwrap().clone();
        assert_eq!(shas.len(), 1);
        let json: serde_json::Value = serde_json::from_str(&git_ok(
            &r.wt,
            &["show", &format!("{}:.mason/run.json", shas[0])],
        ))
        .unwrap();
        assert_eq!(json["matrix"], serde_json::json!([1, 2, 3]));
        assert_eq!(json["shard_lists"][0], serde_json::json!(["a", "b"]));
        assert!(json["shard_command"]
            .as_str()
            .unwrap()
            .contains("test-shard.py"));
    }

    #[test]
    fn one_red_shard_makes_the_merge_red_and_keeps_the_others_output() {
        let (f, _, _) = run_three([(A, 0), ("test c ... FAILED\n", 101), (D, 0)], vec![]);
        assert_eq!(f.code, 101);
        assert!(f.output.contains("test c ... FAILED") && f.output.contains("test d ... ok"));
    }

    #[test]
    fn a_shard_that_never_ran_is_made_again_alone_and_the_others_are_kept() {
        let (f, gh, r) = run_three([(A, 0), (C, 0), (D, 0)], vec![2]);
        assert_eq!(f.output, format!("{A}{C}{D}"));
        assert_eq!(f.code, 0);
        let shas = gh.shas.lock().unwrap().clone();
        assert_eq!(shas.len(), 2, "a second run for the missing shard");
        let second: serde_json::Value = serde_json::from_str(&git_ok(
            &r.wt,
            &["show", &format!("{}:.mason/run.json", shas[1])],
        ))
        .unwrap();
        assert_eq!(second["matrix"], serde_json::json!([2]));
        assert_eq!(second["shards"], 3);
    }

    #[test]
    fn a_build_goes_in_its_own_field_not_joined_to_the_command() {
        let mut sp = spec("b1");
        sp.command = "cd sub && true".into();
        sp.shard_command = Some("cd sub && true {shard}".into());
        let sp = with_build(sp, Some("cd sub && true"));
        let v: serde_json::Value = serde_json::from_str(&run_json(&sp)).unwrap();
        assert_eq!(v["build"], "cd sub && true");
        assert_eq!(v["command"], "cd sub && true");
        assert_eq!(v["shard_command"], "cd sub && true {shard}");
        let none = with_build(spec("b2"), None);
        assert!(none.build.is_none());
    }
    #[test]
    fn job_names_of_a_sharded_run_are_run_jobs() {
        assert!(is_run_job("run") && is_run_job("run (3)"));
        assert!(!is_run_job("plan") && !is_run_job("running"));
    }

    /// A fake Actions API: per repository, runs in two lists (`in_progress`, `queued`) behind one
    /// ETag, jobs with labels, a repository the token cannot read, and a request log.
    type Retried = (
        Result<(Option<Outcome>, Vec<String>), String>,
        Arc<FakeGh>,
        Vec<String>,
    );
    type FakeJobs = Vec<(String, String)>;
    type FakeRuns = Vec<(u64, String, String, FakeJobs)>;
    type FakeSeed<'a> = Vec<(&'a str, Vec<(u64, &'a str, &'a str, FakeJobs)>)>;

    #[derive(Default)]
    struct FakeActions {
        etag: Mutex<String>,
        /// repo -> run id -> (workflow name, workflow file, jobs as (status, label))
        repos: Mutex<std::collections::BTreeMap<String, FakeRuns>>,
        log: Mutex<Vec<String>>,
    }

    impl Http for FakeActions {
        fn send(
            &self,
            r: &mason_adapter_api::http::Request,
        ) -> Result<mason_adapter_api::http::Response, mason_adapter_api::http::Transport> {
            use mason_adapter_api::http::Response;
            self.log.lock().unwrap().push(r.url.clone());
            let ok = |body: serde_json::Value, etag: Option<String>| {
                Ok(Response {
                    status: 200,
                    headers: etag.map(|e| ("etag".to_string(), e)).into_iter().collect(),
                    body: body.to_string(),
                })
            };
            let status = |code: u16| {
                Ok(Response {
                    status: code,
                    headers: vec![],
                    body: "{\"message\": \"x\"}".into(),
                })
            };
            let rest = r.url.split("/repos/").nth(1).unwrap_or_default();
            let repo: String = rest.split('/').take(2).collect::<Vec<_>>().join("/");
            let repos = self.repos.lock().unwrap();
            let Some(runs) = repos.get(&repo) else {
                return status(404);
            };
            let etag = self.etag.lock().unwrap().clone();
            let not_modified = r
                .headers
                .iter()
                .any(|(k, v)| k == "If-None-Match" && *v == etag);
            if let Some(q) = r.url.split("/actions/runs?status=").nth(1) {
                if not_modified {
                    return status(304);
                }
                let want = q.split('&').next().unwrap();
                let list: Vec<_> = runs
                    .iter()
                    .filter(|(_, _, _, jobs)| jobs.iter().any(|(st, _)| st == want))
                    .map(|(id, name, path, _)| {
                        serde_json::json!({"id": id, "name": name, "path": path})
                    })
                    .collect();
                return ok(serde_json::json!({"workflow_runs": list}), Some(etag));
            }
            if let Some(id) = r.url.split("/actions/runs/").nth(1) {
                if not_modified {
                    return status(304);
                }
                let id: u64 = id.split('/').next().unwrap().parse().unwrap();
                let jobs: Vec<_> = runs
                    .iter()
                    .filter(|(i, ..)| *i == id)
                    .flat_map(|(_, _, _, jobs)| jobs)
                    .map(|(st, label)| serde_json::json!({"name": "j", "status": st, "labels": [label]}))
                    .collect();
                return ok(serde_json::json!({"jobs": jobs}), Some(etag));
            }
            Err(mason_adapter_api::http::Transport(format!(
                "unexpected {}",
                r.url
            )))
        }
    }

    fn jobs(n: usize, status: &str, label: &str) -> Vec<(String, String)> {
        (0..n)
            .map(|_| (status.to_string(), label.to_string()))
            .collect()
    }

    fn source(api: &Arc<FakeActions>, repo: &str, own: &[&str]) -> (AccountRepo, GitHub) {
        token();
        (
            AccountRepo {
                api: "https://api.github.com".into(),
                repo: repo.into(),
                token: "env:MASON_TEST_REMOTE_TOKEN".into(),
                own: own.iter().map(|s| s.to_string()).collect(),
            },
            GitHub::new(
                api.clone(),
                "https://api.github.com",
                repo,
                TokenRef::Env("MASON_TEST_REMOTE_TOKEN".into()),
            ),
        )
    }

    fn fake(repos: FakeSeed) -> Arc<FakeActions> {
        let api = Arc::new(FakeActions {
            etag: Mutex::new("\"a\"".into()),
            ..Default::default()
        });
        for (repo, runs) in repos {
            api.repos.lock().unwrap().insert(
                repo.into(),
                runs.into_iter()
                    .map(|(i, n, p, j)| (i, n.to_string(), p.to_string(), j))
                    .collect(),
            );
        }
        api
    }

    #[test]
    fn queued_jobs_in_other_repositories_and_the_ceiling_limit_what_mason_may_take() {
        let api = fake(vec![
            (
                "o/star",
                vec![(
                    1,
                    "pr-gate",
                    ".github/workflows/pr-gate.yml",
                    jobs(3, "queued", "ubuntu-latest"),
                )],
            ),
            (
                "o/mason",
                vec![
                    (
                        2,
                        "CI",
                        ".github/workflows/ci.yml",
                        jobs(2, "in_progress", "ubuntu-latest"),
                    ),
                    (
                        3,
                        "MASON run",
                        ".github/workflows/mason-run.yml",
                        jobs(4, "in_progress", "ubuntu-latest"),
                    ),
                ],
            ),
        ]);
        let sources = [
            source(&api, "o/mason", &["mason-run.yml"]),
            source(&api, "o/star", &[]),
        ];
        let mut seen = AccountJobs::default();
        let other = seen.poll(&sources, 1000);
        assert_eq!(
            (other.gha(), other.macos()),
            (5, 0),
            "MASON's own run is not counted"
        );
        assert!(
            other.label.contains("star pr-gate 3 queued"),
            "{}",
            other.label
        );
        assert!(
            other.label.contains("mason CI 2 running"),
            "{}",
            other.label
        );
        let dir = tempfile::tempdir().unwrap();
        let b = crate::sched::tests::runner_broker(dir.path(), 16, 4, 0);
        b.set_limit("gha_ceiling", 6).unwrap();
        b.set_other_jobs(other);
        let ids: Vec<u64> = (1..=7)
            .map(|t| crate::sched::tests::queue_gha(&b, t))
            .collect();
        assert_eq!(
            ids.iter()
                .filter(|i| crate::sched::tests::is_held(&b, **i))
                .count(),
            4,
            "ceiling 6 less the 2 running jobs; the 3 queued ones take no unit (#818)"
        );
        // The jobs finish, under a new ETag.
        api.repos.lock().unwrap().get_mut("o/star").unwrap().clear();
        api.repos
            .lock()
            .unwrap()
            .get_mut("o/mason")
            .unwrap()
            .clear();
        *api.etag.lock().unwrap() = "\"b\"".into();
        let other = seen.poll(&sources, 2000);
        assert_eq!(other.gha(), 0);
        b.set_other_jobs(other);
        assert_eq!(
            ids.iter()
                .filter(|i| crate::sched::tests::is_held(&b, **i))
                .count(),
            6
        );
    }

    #[test]
    fn a_run_whose_list_status_is_unchanged_has_its_jobs_asked_again_only_after_a_while() {
        let one = |n: usize| {
            vec![(
                1,
                "CI",
                ".github/workflows/ci.yml",
                jobs(n, "in_progress", "ubuntu-latest"),
            )]
        };
        let put = |api: &Arc<FakeActions>, n: usize, etag: &str| {
            let (i, name, path, j) = one(n).remove(0);
            api.repos.lock().unwrap().insert(
                "o/r".into(),
                vec![(i, name.to_string(), path.to_string(), j)],
            );
            *api.etag.lock().unwrap() = etag.into();
        };
        let api = fake(vec![("o/r", one(3))]);
        let sources = [source(&api, "o/r", &[])];
        let mut seen = AccountJobs::default();
        assert_eq!(seen.poll(&sources, 1000).gha(), 3);
        // The lists changed (a new ETag) but the run is in the same list and was read a moment
        // ago: its jobs are not asked for again.
        put(&api, 1, "\"b\"");
        let asked = api.log.lock().unwrap().len();
        assert_eq!(seen.poll(&sources, 2000).gha(), 3);
        assert_eq!(
            api.log.lock().unwrap().len(),
            asked + 2,
            "only the two lists"
        );
        // Long enough later they are.
        put(&api, 1, "\"c\"");
        assert_eq!(seen.poll(&sources, 1000 + JOBS_AGAIN_MS).gha(), 1);
    }

    #[test]
    fn a_queued_run_is_looked_at_rarely_a_running_one_less_rarely_and_the_pickup_is_not_missed() {
        // Finding the run, or waiting for its artifact: the base interval.
        assert_eq!(run_poll_interval_s(10, false, false, false, 0, 600_000), 10);
        assert_eq!(run_poll_interval_s(10, true, true, true, 0, 600_000), 10);
        // Queued: a minute. Running: 30 s.
        assert_eq!(run_poll_interval_s(10, true, false, false, 0, 600_000), 60);
        assert_eq!(run_poll_interval_s(10, true, true, false, 0, 600_000), 30);
        // Queued and within two slow polls of the pickup timeout: quick again, so a runner that
        // takes it just before the timeout is seen.
        assert_eq!(
            run_poll_interval_s(10, true, false, false, 480_000, 600_000),
            10
        );
        assert_eq!(
            run_poll_interval_s(10, true, false, false, 479_999, 600_000),
            60
        );
    }

    #[test]
    fn macos_jobs_are_counted_apart() {
        let api = fake(vec![(
            "o/star",
            vec![(
                1,
                "CLI Build",
                ".github/workflows/cli.yml",
                jobs(2, "in_progress", "macos-14"),
            )],
        )]);
        let mut seen = AccountJobs::default();
        let other = seen.poll(&[source(&api, "o/star", &[])], 1);
        assert_eq!((other.gha(), other.macos()), (2, 2));
    }

    #[test]
    fn a_304_changes_nothing_and_asks_for_nothing_more() {
        let api = fake(vec![(
            "o/r",
            vec![(
                1,
                "CI",
                ".github/workflows/ci.yml",
                jobs(3, "in_progress", "ubuntu-latest"),
            )],
        )]);
        let sources = [source(&api, "o/r", &[])];
        let mut seen = AccountJobs::default();
        assert_eq!(seen.poll(&sources, 1).gha(), 3);
        api.repos.lock().unwrap().get_mut("o/r").unwrap().clear();
        let asked = api.log.lock().unwrap().len();
        // Nothing changed on GitHub's side: the count stays, and only the two conditional
        // list requests are made, with no job lookups.
        assert_eq!(seen.poll(&sources, 2).gha(), 3);
        assert_eq!(api.log.lock().unwrap().len(), asked + 2);
    }

    #[test]
    fn a_repository_the_token_cannot_read_is_named_and_not_counted() {
        let api = fake(vec![(
            "o/r",
            vec![(
                1,
                "CI",
                ".github/workflows/ci.yml",
                jobs(2, "in_progress", "ubuntu-latest"),
            )],
        )]);
        let sources = [source(&api, "o/r", &[]), source(&api, "o/private", &[])];
        let mut seen = AccountJobs::default();
        let other = seen.poll(&sources, 1);
        assert_eq!(other.gha(), 2);
        assert_eq!(other.not_counted.len(), 1);
        assert!(
            other.not_counted[0].starts_with("o/private"),
            "{:?}",
            other.not_counted
        );
    }

    #[test]
    fn a_failed_look_keeps_the_last_count_and_says_it_is_stale() {
        let api = fake(vec![(
            "o/r",
            vec![(
                1,
                "CI",
                ".github/workflows/ci.yml",
                jobs(2, "in_progress", "ubuntu-latest"),
            )],
        )]);
        let sources = [source(&api, "o/r", &[])];
        let mut seen = AccountJobs::default();
        assert!(!seen.poll(&sources, 100).stale);
        api.repos.lock().unwrap().clear();
        let other = seen.poll(&sources, 200);
        assert_eq!((other.gha(), other.stale, other.read_at_ms), (2, true, 100));
    }

    #[test]
    fn a_suite_holds_one_gha_unit_per_job_and_a_macos_runner_adds_gha_macos() {
        use std::collections::BTreeMap;
        let mut s = mason_testing::Suite::new("w", "cargo test", mason_testing::ParserKind::Tap);
        s.shards = 3;
        s.shard_command = Some("cargo test --shard {shard}".into());
        // The whole command is split; a narrowed run or another command is one job.
        assert_eq!(jobs_of(Some(&s), "cargo test"), 3);
        assert_eq!(jobs_of(Some(&s), "cargo test one"), 1);
        assert_eq!(jobs_of(None, "cargo test"), 1);
        s.narrowed = true;
        assert_eq!(jobs_of(Some(&s), "cargo test"), 1);
        assert_eq!(
            units_for("ubuntu-latest", jobs_of(Some(&s), "cargo test")),
            BTreeMap::from([("gha".to_string(), 1)])
        );
        // A suite on macOS in a project whose runner is ubuntu: 1 gha and 1 gha-macos.
        assert_eq!(
            units_for("macos-latest", 1),
            BTreeMap::from([("gha".to_string(), 1), ("gha-macos".to_string(), 1)])
        );
        // The machine's slots are replaced, other needs stay.
        let needs =
            mason_scheduler::Needs::of(&[("compile", 1), ("test-suite", 1), ("device:x", 1)]);
        let out = with_units(&needs, units_for("macos-latest", 1));
        assert_eq!(
            out.amounts,
            BTreeMap::from([
                ("device:x".to_string(), 1),
                ("gha".to_string(), 1),
                ("gha-macos".to_string(), 1)
            ])
        );
    }
    /// `run_retrying` against `mode`, with the pushes in `refuse_first` refused once.
    fn retrying(mode: Mode, refuse_first: bool, fallback: bool) -> Retried {
        let r = repo();
        token();
        if refuse_first {
            let hook = r.bare.join("hooks/pre-receive");
            let marker = r.bare.join("refused-once");
            std::fs::create_dir_all(r.bare.join("hooks")).unwrap();
            // GitHub refuses the first four pushes: the push's own tries (#772) are all used up
            // by the first run, and the fifth push is the retry's.
            std::fs::write(
                &hook,
                format!(
                    "#!/bin/sh\nn=$(cat '{m}' 2>/dev/null || echo 0)\nn=$((n + 1))\necho $n > '{m}'\nif [ $n -le 4 ]; then echo refused >&2; exit 1; fi\nexit 0\n",
                    m = marker.display()
                ),
            )
            .unwrap();
            // Executable bits are Unix-only; the Windows CI job builds these tests but does not run them.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let zip = artifact(r._t.path(), GREEN, 0, None);
        let gh = FakeGh::new(&r, &zip, mode);
        let s = site(&r, gh.clone(), clock(), fallback);
        let said = Mutex::new(vec![]);
        let dir = r._t.path().join("runs");
        let out = run_retrying(
            &s,
            &spec("rt1"),
            &dir,
            &|| None,
            &|t| said.lock().unwrap().push(t.to_string()),
            &Grant::test(),
        );
        let said = said.into_inner().unwrap();
        (out, gh, said)
    }

    #[test]
    fn a_run_nobody_picks_up_is_run_again_and_the_agent_sees_one_result() {
        let (out, gh, said) = retrying(Mode::MissFirst, false, false);
        let (Some(Outcome::Done(f)), notes) = out.unwrap() else {
            panic!("no result");
        };
        assert_eq!(f.code, 0);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("no runner picked up"), "{notes:?}");
        assert_eq!(said, notes, "the retry is shown in the check's progress");
        assert_eq!(gh.runs_seen.lock().unwrap().len(), 2);
    }

    #[test]
    fn a_refused_push_is_run_again_after_a_backoff() {
        let (out, _, said) = retrying(Mode::Normal, true, false);
        let (Some(Outcome::Done(f)), notes) = out.unwrap() else {
            panic!("no result");
        };
        assert_eq!(f.code, 0);
        assert!(notes[0].contains("could not push"), "{notes:?}");
        assert_eq!(said.len(), 1);
    }

    #[test]
    fn a_run_cancelled_at_the_limit_naming_no_test_is_run_again() {
        let (out, gh, _) = retrying(Mode::HangFirst, false, false);
        let (Some(Outcome::Done(f)), notes) = out.unwrap() else {
            panic!("no result");
        };
        assert!(!f.timed_out && f.code == 0, "the second run is the answer");
        assert!(notes[0].contains("runner limit"), "{notes:?}");
        assert_eq!(
            gh.cancels.lock().unwrap().len(),
            1,
            "the first run was cancelled"
        );
    }

    #[test]
    fn two_misses_are_no_result_not_a_failure() {
        let (out, _, said) = retrying(Mode::NeverPicked, false, false);
        let e = out.unwrap_err();
        let why = environmental(&e).expect("an environmental error, not a red result");
        assert!(why.contains("; then "), "{why}");
        assert_eq!(said.len(), 1, "one retry was shown");
    }

    #[test]
    fn two_misses_under_a_local_fallback_go_to_this_machine() {
        let (out, _, said) = retrying(Mode::NeverPicked, false, true);
        let (o, notes) = out.unwrap();
        assert!(o.is_none(), "the caller runs it here");
        assert_eq!(notes.len(), 1);
        assert_eq!(said.len(), 2, "the retry and the move to this Mac");
    }

    #[test]
    fn a_red_run_is_a_result_and_is_not_retried() {
        let r = repo();
        token();
        let zip = artifact(r._t.path(), "test a ... FAILED\n", 101, None);
        let gh = FakeGh::new(&r, &zip, Mode::Normal);
        let s = site(&r, gh.clone(), clock(), false);
        let dir = r._t.path().join("runs");
        let (Some(Outcome::Done(f)), notes) =
            run_retrying(&s, &spec("rt2"), &dir, &|| None, &|_| {}, &Grant::test()).unwrap()
        else {
            panic!("no result");
        };
        assert_eq!(f.code, 101);
        assert!(notes.is_empty());
        assert_eq!(gh.runs_seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_skip_while_a_run_waits_cancels_it_on_github() {
        let r = repo();
        token();
        let zip = artifact(r._t.path(), GREEN, 0, None);
        let gh = FakeGh::new(&r, &zip, Mode::NeverPicked);
        let clk = clock();
        let start = clk.now_ms();
        let s = site(&r, gh.clone(), clk.clone(), false);
        let dir = r._t.path().join("runs");
        let (o, _) = run_retrying(
            &s,
            &spec("rt3"),
            &dir,
            &|| (clk.now_ms() - start >= 60_000).then_some(StopWhy::Skipped),
            &|_| {},
            &Grant::test(),
        )
        .unwrap();
        assert!(matches!(o, Some(Outcome::Stopped(StopWhy::Skipped))));
        assert_eq!(gh.cancels.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_narrowed_run_of_three_targets_goes_out_as_two_jobs_with_the_narrowed_command() {
        let r = repo();
        token();
        let t = r._t.path();
        let mut zips = std::collections::BTreeMap::new();
        for k in 1..=2u32 {
            let d = t.join(format!("shard{k}"));
            std::fs::create_dir_all(&d).unwrap();
            zips.insert(k, artifact(&d, "test a ... ok\n", 0, None));
        }
        let gh = Arc::new(ShardGh {
            bare: r.bare.clone(),
            zips,
            lost: vec![],
            shas: Mutex::new(vec![]),
        });
        let mut s = site(
            &r,
            FakeGh::new(&r, &artifact(t, GREEN, 0, None), Mode::Normal),
            clock(),
            false,
        );
        s.gh = GitHub::new(
            gh.clone(),
            "https://api.github.com",
            "o/r",
            TokenRef::Env("MASON_TEST_REMOTE_TOKEN".into()),
        );
        let narrowed = "mason_rc=0; ( python3 -I scripts/test-shard.py -- cargo test -p a --lib ) || mason_rc=1; ( python3 -I scripts/test-shard.py -- cargo test -p b --test it ) || mason_rc=1; ( python3 -I scripts/test-shard.py -- cargo test -p c --lib ) || mason_rc=1; [ $mason_rc -eq 0 ]";
        let mut sp = sharded_spec("nr1", &[&["a", "b"], &["c"]]);
        sp.shard_command = Some(narrowed.into());
        sp.partial_lists = true;
        let dir = t.join("runs");
        let Ok(Outcome::Done(f)) = execute_sharded(&s, &sp, &dir, &|| None, &Grant::test()) else {
            panic!("the sharded run did not finish");
        };
        assert_eq!(f.parts.len(), 2);
        // A shard whose share of the whole suite's lists holds none of this narrowed run's tests
        // is no defect: no shard counts as having listed any.
        assert!(f.parts.iter().all(|p| p.listed == 0));
        let shas = gh.shas.lock().unwrap().clone();
        assert_eq!(shas.len(), 1, "one run, two jobs");
        let json: serde_json::Value = serde_json::from_str(&git_ok(
            &r.wt,
            &["show", &format!("{}:.mason/run.json", shas[0])],
        ))
        .unwrap();
        assert_eq!(json["matrix"], serde_json::json!([1, 2]));
        let cmd = json["shard_command"].as_str().unwrap();
        for g in ["-p a --lib", "-p b --test it", "-p c --lib"] {
            assert!(cmd.contains(g), "{cmd}");
        }
    }

    // ---- a run this machine cannot take back waits on GitHub (the fallback dead end) ----

    use crate::sched::LoadReading;

    /// A machine reading: `held` is the hold in force, `limit` the `test-suite` limit.
    fn reading(held: Option<&str>, limit: u32) -> LoadReading {
        LoadReading {
            limits: [
                ("compile".to_string(), 3),
                ("test-suite".to_string(), limit),
            ]
            .into(),
            held: held.map(str::to_string),
            ..Default::default()
        }
    }

    /// A site that falls back to a machine in this state, with the way to split a run again.
    fn site_for_machine(r: &Repo, gh: Arc<FakeGh>, machine: LoadReading) -> Site {
        let mut s = site(r, gh, clock(), true);
        let uses = std::collections::BTreeMap::from([("test-suite".to_string(), 1)]);
        s.local_barred = Some(Arc::new(move || {
            crate::place::cannot_run_here(&machine, &uses)
        }));
        s.resplit = Some(Arc::new(|mut rs: RunSpec, n| {
            rs.shards = n;
            rs.shard_command = Some("shard-cmd".into());
            rs.shard_lists = vec![vec![]; n as usize];
            rs
        }));
        s
    }

    /// A broker whose 8 runner units are 7 held by another task, and a flexible lease of the
    /// run's that was given the one left: a run split for 1 of the 8 it wanted.
    struct ShortGrant {
        _dir: tempfile::TempDir,
        b: Arc<crate::sched::Broker>,
        other: Option<crate::sched::Lease>,
        grant: Grant,
        spec: RunSpec,
    }

    fn short_grant() -> ShortGrant {
        let dir = tempfile::tempdir().unwrap();
        let b = crate::sched::tests::runner_broker(dir.path(), 8, 1, 0);
        let rank = mason_scheduler::Rank::new(false, 3, 0, 1, 1, false);
        let other = b
            .acquire(
                mason_core::TaskId(9),
                "#9 other",
                rank,
                mason_scheduler::Needs::of(&[("gha", 7)]),
                &|| true,
                &mut |_| {},
            )
            .unwrap();
        let lease = b
            .acquire(
                mason_core::TaskId(1),
                "#1 test step",
                rank,
                mason_scheduler::Needs::of(&[("gha", 8)]).flexible(),
                &|| true,
                &mut |_| {},
            )
            .unwrap();
        assert_eq!(lease.granted_runners(), Some(1));
        let mut sp = spec("w1");
        sp.shards_wanted = 8;
        ShortGrant {
            _dir: dir,
            b,
            other: Some(other),
            grant: Grant::of(lease),
            spec: sp,
        }
    }

    /// Runs `run` on a thread while the units it needs are not free; once it has queued for
    /// them, checks it holds nothing, then frees them and returns what `run` made.
    fn run_while_short<T: Send>(sg: &mut ShortGrant, run: impl FnOnce(&Grant) -> T + Send) -> T {
        let grant = sg.grant.clone();
        let b = sg.b.clone();
        let other = sg.other.take();
        std::thread::scope(|sc| {
            let h = sc.spawn(|| run(&grant));
            // It queues for the units the other task holds (a wait for a condition, not time).
            let mut looks = 0u64;
            while b.gha_waiting() == 0 {
                looks += 1;
                assert!(looks < 50_000_000, "it never asked again");
                std::thread::yield_now();
            }
            assert!(grant.units().is_empty(), "it holds nothing while it waits");
            assert_eq!(b.used("gha"), 7, "only the other task's units are held");
            drop(other);
            h.join().unwrap()
        })
    }

    /// Stops a run once it has looked at its jobs, so it ends after it was pushed.
    fn stop_once_pushed(gh: &Arc<FakeGh>) -> impl Fn() -> Option<StopWhy> + Send + Sync {
        let g = gh.clone();
        move || {
            g.requests()
                .iter()
                .any(|q| q.contains("/jobs"))
                .then_some(StopWhy::Skipped)
        }
    }

    #[test]
    fn a_refused_short_grant_waits_on_github_when_this_machine_cannot_run_it() {
        token();
        // A limit of 0 and a hold each mean it cannot.
        for (machine, why) in [
            (reading(None, 0), "test-suite limit 0"),
            (
                reading(Some("held for heat (die 94 C)"), 2),
                "held for heat",
            ),
        ] {
            let r = repo();
            let zip = artifact(r._t.path(), GREEN, 0, None);
            let gh = FakeGh::new(&r, &zip, Mode::NeverPicked);
            let s = site_for_machine(&r, gh.clone(), machine);
            let said: Mutex<Vec<String>> = Mutex::new(vec![]);
            let mut sg = short_grant();
            let (sp, dir) = (sg.spec.clone(), r._t.path().join("runs"));
            let stop = stop_once_pushed(&gh);
            let out = run_while_short(&mut sg, |grant| {
                run_retrying(
                    &s,
                    &sp,
                    &dir,
                    &stop,
                    &|t| said.lock().unwrap().push(t.into()),
                    grant,
                )
            });
            let (o, notes) = out.unwrap();
            assert!(
                matches!(o, Some(Outcome::Stopped(StopWhy::Skipped))),
                "it ran on GitHub, not here: {o:?}"
            );
            let said = said.into_inner().unwrap();
            assert!(
                said[0].contains("granted 1 of 8 runner units")
                    && said[0].contains("this Mac can't run it (")
                    && said[0].contains(why)
                    && said[0].contains("so it waits on GitHub for at least 4"),
                "{said:?}"
            );
            assert_eq!(notes[0], said[0]);
            // Pushed once, split the fewest ways that can finish: half of 8.
            assert_eq!(dispatched_inputs(&gh)["inputs"]["matrix"], "[1,2,3,4]");
            assert_eq!(sg.grant.units(), units_of(4));
        }
    }

    #[test]
    fn a_suite_run_refused_for_a_short_grant_also_waits_when_this_machine_cannot_run_it() {
        token();
        let r = repo();
        let zip = artifact(r._t.path(), GREEN, 0, None);
        let gh = FakeGh::new(&r, &zip, Mode::NeverPicked);
        let s = site_for_machine(&r, gh.clone(), reading(None, 0));
        let mut sg = short_grant();
        let (sp, dir) = (sg.spec.clone(), r._t.path().join("runs"));
        let stop = stop_once_pushed(&gh);
        let out = run_while_short(&mut sg, |grant| {
            run_or_fallback(&s, &sp, &dir, &stop, grant)
        });
        assert!(matches!(out, Ok(Some(Outcome::Stopped(StopWhy::Skipped)))));
        assert_eq!(dispatched_inputs(&gh)["inputs"]["matrix"], "[1,2,3,4]");
    }

    #[test]
    fn a_refused_short_grant_still_falls_back_when_this_machine_can_run_it() {
        token();
        let r = repo();
        let zip = artifact(r._t.path(), GREEN, 0, None);
        let gh = FakeGh::new(&r, &zip, Mode::Normal);
        // A limit above 0 and nothing held: it can, so it goes there as before, at once.
        let s = site_for_machine(&r, gh.clone(), reading(None, 2));
        let dir = r._t.path().join("runs");
        let mut sg = short_grant();
        let (o, notes) = run_retrying(&s, &sg.spec, &dir, &|| None, &|_| {}, &sg.grant).unwrap();
        assert!(o.is_none(), "the caller runs it here");
        assert!(notes[0].contains("running it on this Mac"), "{notes:?}");
        assert!(!notes[0].contains("waits on GitHub"));
        assert!(gh.dispatches.lock().unwrap().is_empty());
        assert!(sg.grant.units().is_empty(), "its unit went back");
        let mut sg2 = short_grant();
        let out = run_or_fallback(&s, &sg2.spec, &dir, &|| None, &sg2.grant);
        assert!(matches!(out, Ok(None)));
        assert!(gh.dispatches.lock().unwrap().is_empty());
        sg.other.take();
        sg2.other.take();
    }

    #[test]
    fn a_run_that_hit_the_limit_on_a_short_grant_waits_for_all_its_runners_when_this_machine_cannot_run_it(
    ) {
        token();
        let r = repo();
        let zip = cut_artifact(r._t.path(), "test a ... ok\n");
        let gh = FakeGh::new(&r, &zip, Mode::StepTimeout);
        let s = site_for_machine(&r, gh.clone(), reading(None, 0));
        // 1 runner of 2 wanted (the fewest is 1): the run ends at the step limit.
        let dir = r._t.path().join("runs");
        let mut sp = spec("w2");
        sp.shards_wanted = 2;
        let said: Mutex<Vec<String>> = Mutex::new(vec![]);
        let out = run_retrying(
            &s,
            &sp,
            &dir,
            &|| None,
            &|t| said.lock().unwrap().push(t.to_string()),
            &Grant::test(),
        );
        // It is not sent here: it waits for the 2 runners the suite asked for and goes out
        // again at that size (the fake artifact has no shard results, so that run misses too,
        // and the miss is not sent here either).
        let e = out.unwrap_err();
        assert!(
            environmental(&e).unwrap().contains("this Mac can't run it"),
            "{e}"
        );
        let said = said.into_inner().unwrap();
        assert!(
            said.iter()
                .any(|t| t.contains("waits on GitHub for at least 2")),
            "{said:?}"
        );
        let d = gh.dispatches.lock().unwrap();
        assert!(d[1].contains("[1,2]"), "{d:?}");
        assert_eq!(d.len(), 2);
    }
}
