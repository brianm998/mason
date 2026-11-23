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
    match crate::place::place_of(d, project, task.map(|t| t.0), suite, kind) {
        crate::place::Place::GitHub { runs_on, .. } => Executor::GitHub { runs_on },
        crate::place::Place::Local { .. } => Executor::Local,
    }
}

/// Where a check no task owns and no suite describes (a landing lint or re-test) goes: the global
/// switch alone, for a project with a `[remote_runs]` (#702).
pub fn executor_global(d: &Daemon, project: &str) -> Executor {
    if forced_for_tests() {
        if let Some(r) = remote_cfg(d, project) {
            return Executor::GitHub { runs_on: r.runs_on };
        }
    }
    match crate::place::global_place(d, project) {
        crate::place::Place::GitHub { runs_on, .. } => Executor::GitHub { runs_on },
        crate::place::Place::Local { .. } => Executor::Local,
    }
}

/// Where a whole step of `task` goes (`pool_needs`): the task's mode, else the project's, else
/// the global one, with no suite's rules.
pub fn executor_task(d: &Daemon, project: &str, task: Option<u64>) -> Executor {
    if forced_for_tests() {
        if let Some(r) = remote_cfg(d, project) {
            return Executor::GitHub { runs_on: r.runs_on };
        }
    }
    match crate::place::plain_place(d, project, task) {
        crate::place::Place::GitHub { runs_on, .. } => Executor::GitHub { runs_on },
        crate::place::Place::Local { .. } => Executor::Local,
    }
}

fn forced_for_tests() -> bool {
    std::env::var("MASON_TEST_FORCE_REMOTE").is_ok_and(|v| v == "1")
}

/// Does any project have a `[remote_runs]` section? (Cheap: lets a seam skip finding its project.)
pub fn any_remote(d: &Daemon) -> bool {
    d.projects().values().any(|p| p.services.remote.is_some())
}

fn project_cfg(d: &Daemon, project: &str) -> Option<ProjectCfg> {
    d.project_id_by_name(project)
        .and_then(|i| d.project(i))
        .cloned()
}

fn remote_cfg(d: &Daemon, project: &str) -> Option<RemoteCfg> {
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
}

impl RunSpec {
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

/// A run waiting for pickup for `waited_ms` comes back here (#833): the spill time is set, it
/// has passed, and `room` says this machine has room. Pure.
pub fn spill_due(waited_ms: u64, spill_after: Option<Duration>, room: bool) -> bool {
    room && spill_after.is_some_and(|a| waited_ms >= a.as_millis() as u64)
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
        site.spill_after = Some(Duration::from_secs(after));
        site.local_room = Some(Arc::new(move || {
            crate::place::no_room_for(&d.broker.load_reading(now_ms()), &uses, covered)
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
            match Self::poll_repo(st, gh, &r.own) {
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
                out.gha += run.running + run.queued;
                out.macos += run.macos_running + run.macos_queued;
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

    fn poll_repo(st: &mut RepoState, gh: &GitHub, own: &[String]) -> Result<(), String> {
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
        let mut live: std::collections::BTreeMap<u64, String> = Default::default();
        for (id, name, path) in st.lists.iter().flatten() {
            if own
                .iter()
                .any(|o| path.rsplit('/').next() == Some(o.as_str()))
            {
                continue;
            }
            live.insert(*id, name.clone());
        }
        st.runs.retain(|id, _| live.contains_key(id));
        for (id, name) in live {
            let run = st.runs.entry(id).or_default();
            let Some((jobs, etag)) = gh
                .remote_run_jobs_changed(id, run.etag.as_deref())
                .map_err(|e| e.to_string())?
            else {
                continue;
            };
            *run = RunJobs {
                etag,
                name,
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

/// Keep the broker told what the account's other workflows have on GitHub's runners, every 30
/// seconds. Nothing runs where no project names a GitHub repository.
pub fn spawn_other_workflows(d: &Arc<Daemon>) {
    if forced_for_tests() || account_repos(d).is_empty() {
        return;
    }
    let mut seen = AccountJobs::default();
    crate::bg::spawn_loop(d, "other-workflows", move |weak| {
        std::thread::sleep(Duration::from_secs(30));
        let Some(d) = weak.upgrade() else {
            return false;
        };
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
    let timeout_min = spec.limit.as_secs().div_ceil(60).saturating_sub(2).max(1);
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
    workflow: &str,
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
    std::thread::spawn(move || sweep_stale_refs_now(&d, now));
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
    let meta = regular("result.json").ok_or("the artifact has no result.json")?;
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
) -> Result<Outcome, Fail> {
    let _ = std::fs::create_dir_all(dir);
    let rec = record_path(dir, &spec.id);
    let log = dir.join(format!("{}.log", spec.id));
    let (mut st, resumed) = match State::load(&rec) {
        Some(s) => (s, true),
        None => {
            let name = ref_name(&spec.project, &spec.id);
            let sha = snapshot(
                &site.cwd,
                &site.base_ref,
                &site.workflow,
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
    }
    let mut check_pushed = resumed;
    let mut poller = Poller::new(site.poll_s);
    let mut outage_since: Option<u64> = None;
    let mut artifact_misses = 0u32;
    let mut live = Live::default();
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
                st.timed_out = true;
                st.code = -1;
                st.signal = 9;
                st.collected = true;
                // The output so far (the workflow uploads it even for a cut-off run), then the
                // cut-off line naming the tests that never finished.
                let kept = output_after_cancel(site, &st, &scratch);
                let mut text = String::from_utf8_lossy(&kept).into_owned();
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&format!(
                    "(the run was cancelled after its limit of {} s on the runner)\n",
                    spec.limit.as_secs()
                ));
                let hung = crate::suites::still_running(&text);
                if !hung.is_empty() {
                    text.push_str(&format!(
                        "Tests that started and never finished: {}\n",
                        hung.join(", ")
                    ));
                }
                let _ = std::fs::remove_dir_all(&scratch);
                let _ = std::fs::write(&log, &text);
                st.save(&rec);
                delete_ref(site, &mut st, &rec);
                return Ok(Outcome::Done(finished_from(&st, &log)));
            }
        } else {
            // Not picked up yet. A run that is due to come back here (favor-remote's
            // `spill_after`, or `pickup_timeout`) does so only with room for it here when the site
            // can say (`local_room`); otherwise it stays on GitHub, where a runner may still take
            // it. A site without a room function falls back at the timeout as it always did.
            let waited = now.saturating_sub(st.pushed_ms);
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
        let step = poll(
            site,
            &mut st,
            &mut poller,
            &mut check_pushed,
            &mut artifact_misses,
            &mut live,
            &scratch,
            &rec,
            now,
        );
        match step {
            Ok(Step::Wait) => outage_since = None,
            Ok(Step::Fail(why)) => {
                cancel(site, &st);
                abandon(&mut st);
                let _ = std::fs::remove_dir_all(&scratch);
                return Err(Fail::Environmental(why));
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

enum Step {
    Wait,
    Got(Artifact),
    /// A sharded run's artifacts, and the shards that sent none.
    Parts(Vec<Artifact>, Vec<u32>),
    Fail(String),
}

/// One look at the run: find it, see whether a runner has it, and when it is over take its
/// artifact. A transient failure is returned as the error, for the poller's backoff.
#[allow(clippy::too_many_arguments)]
fn poll(
    site: &Site,
    st: &mut State,
    poller: &mut Poller,
    check_pushed: &mut bool,
    misses: &mut u32,
    live: &mut Live,
    scratch: &Path,
    rec: &Path,
    now: u64,
) -> Result<Step, mason_adapter_api::service::ApiError> {
    let secs = now / 1000;
    let run = match st.run_id {
        None => {
            let found = site
                .gh
                .runs_for_sha(&st.sha)?
                .into_iter()
                .find(|r| r.branch == st.ref_name);
            let Some(r) = found else {
                if std::mem::take(check_pushed) {
                    // A restarted daemon whose record was written but whose push may not have
                    // happened: pushing the same commit to the same ref again is harmless.
                    let _ = push(site, st);
                }
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
    // rate limit); the last answer stands until they change.
    if run.status != "completed" {
        live.refresh(site, run.id)?;
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
                Err(e) => return Ok(Step::Fail(format!("run {}: shard {k}: {e}", run.url))),
            }
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
        Err(e) => Ok(Step::Fail(format!("run {}: {e}", run.url))),
    }
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
}

impl Live {
    fn refresh(
        &mut self,
        site: &Site,
        id: u64,
    ) -> Result<(), mason_adapter_api::service::ApiError> {
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
) -> Result<Outcome, Fail> {
    if !spec.sharded() {
        return execute(site, spec, dir, stop);
    }
    let mut kept: Vec<Part> = vec![];
    let mut attempt_spec = spec.clone();
    for attempt in 0..=SHARD_RERUNS {
        if attempt > 0 {
            attempt_spec.id = format!("{}-s{attempt}", spec.id);
        }
        let f = match execute(site, &attempt_spec, dir, stop)? {
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
    let runs_on = match executor_task(d, project, task.map(|t| t.0)) {
        Executor::GitHub { runs_on } => runs_on,
        Executor::Local => return None,
    };
    let mut out = mason_scheduler::Needs {
        exclusive: false,
        amounts: needs
            .amounts
            .iter()
            .filter(|(k, _)| !MACHINE.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), *v))
            .collect(),
    };
    for (k, v) in units_for(&runs_on, 1) {
        out.amounts.insert(k, v);
    }
    Some(out)
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
    matches!(
        executor_for(d, project, Kind::Suite, Some((task, suite))),
        Executor::GitHub { .. }
    )
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
    let needs = crate::sched::needs_of(uses, None);
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
    let owner = task.and_then(|t| d.world().ok().and_then(|w| w.tasks.get(&t).cloned()));
    let rank = fallback_rank(d, task, owner.as_ref(), def.as_deref());
    let who = task.unwrap_or(mason_core::TaskId(0));
    let alive = || {
        !stopped()
            && match task {
                Some(t) => d
                    .world()
                    .map(|w| w.tasks.get(&t).is_some_and(|x| x.state.is_step()))
                    .unwrap_or(true),
                None => true,
            }
    };
    tracing::info!(project, %label, ?uses, "no GitHub runner: queueing for this machine's slots (fallback = local)");
    d.broker
        .acquire_beside(who, label, rank, needs, &alive, &mut |_| {})
        .map(Some)
        .map_err(|_| format!("{label}: the wait for this machine's slots was ended"))
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
        Some(rid) => match site.gh.run_changed(rid, None) {
            Ok(r) => r.map(|(r, _)| r),
            Err(_) => return false,
        },
        None => match site.gh.runs_for_sha(&st.sha) {
            Ok(v) => v.into_iter().find(|r| r.branch == st.ref_name),
            Err(_) => return false,
        },
    };
    let Some(run) = run else { return false };
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

/// Which of the adopted runs (key, record) nobody can collect: a check whose task's open run is
/// another one or none; a step run of a task with no open run; a check superseded by a newer
/// one of the same prefix for the same run, or one whose check record on disk says it was
/// superseded or its result given (#823). `open_run` gives a task's open run.
pub fn unwanted(
    adopted: &[(String, PathBuf)],
    open_run: &dyn Fn(u64) -> Option<u64>,
) -> Vec<PathBuf> {
    let mut out = vec![];
    let mut newest: std::collections::HashMap<(u64, u64, String), u64> = Default::default();
    let parts: Vec<Option<(u64, u64, String, u64)>> = adopted
        .iter()
        .map(|(_, r)| record_id(r).and_then(|i| check_parts(&i)))
        .collect();
    for (t, r, p, n) in parts.iter().flatten() {
        let e = newest.entry((*t, *r, p.clone())).or_insert(*n);
        *e = (*e).max(*n);
    }
    for ((key, record), part) in adopted.iter().zip(&parts) {
        let bad = match part {
            Some((t, r, p, n)) => {
                open_run(*t) != Some(*r)
                    || newest[&(*t, *r, p.clone())] > *n
                    || check_record_ended(record)
            }
            None => key
                .strip_prefix("task ")
                .and_then(|t| t.parse::<u64>().ok())
                .is_some_and(|t| open_run(t).is_none()),
        };
        if bad {
            out.push(record.clone());
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

/// One pass over the runs adopted at start: free those GitHub has finished, cancel those no one
/// can collect. Off the caller's thread, one pass at a time.
pub fn reap_adopted(d: &Arc<Daemon>) {
    static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !any_remote(d) {
        return;
    }
    let adopted = d.broker.adopted_remote();
    if adopted.is_empty() || BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let d = d.clone();
    std::thread::spawn(move || {
        let holds = holds_dir(&d.cfg.hosts_dir());
        let gone = d
            .world()
            .ok()
            .map(|w| {
                let open: std::collections::HashMap<u64, u64> = w
                    .tasks
                    .keys()
                    .filter_map(|t| Some((t.0, w.open_run(*t)?.0)))
                    .collect();
                unwanted(&adopted, &|t| open.get(&t).copied())
            })
            .unwrap_or_default();
        for (_, record) in &adopted {
            let Some(st) = State::load(record) else {
                if let Some(id) = record_id(record) {
                    remove_hold(&holds, &id);
                }
                continue;
            };
            let Some(site) = site_of_state(&d, &st) else {
                continue;
            };
            if gone.contains(record) {
                cancel_hold(&site, &holds, record);
                tracing::info!(record = %record.display(), "cancelled a GitHub run nobody can collect");
            } else if reap_hold(&site, &holds, record) {
                tracing::info!(record = %record.display(), "freed the units of a finished GitHub run");
            }
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
        env: env.to_vec(),
        runs_on: runs_on.to_string(),
        tree_key: tree_key.to_string(),
        shards: 0,
        only: vec![],
        shard_lists: vec![],
        shard_command: None,
        partial_lists: false,
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
) -> Result<Option<Outcome>, String> {
    match execute_sharded(site, spec, dir, stop) {
        Ok(o) => Ok(Some(o)),
        Err(Fail::Environmental(why)) if site.fallback_local => {
            tracing::warn!(run = %spec.id, "{why}; running it here instead (fallback = local)");
            Ok(None)
        }
        Err(Fail::Environmental(why)) => Err(env_err(&why)),
        Err(Fail::Error(e)) => Err(e),
    }
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
) -> Result<(Option<Outcome>, Vec<String>), String> {
    let mut notes: Vec<String> = vec![];
    let mut whys: Vec<String> = vec![];
    let mut attempt = spec.clone();
    for n in 0..2u32 {
        if n > 0 {
            if let Some(why) = stop() {
                return Ok((Some(Outcome::Stopped(why)), notes));
            }
            attempt.id = format!("{}-r{n}", spec.id);
            let note = format!(
                "no result from the first run ({}); running it again on GitHub",
                whys[0]
            );
            tracing::warn!(run = %spec.id, "{note}");
            said(&note);
            notes.push(note);
        }
        let r = execute_sharded(site, &attempt, dir, stop);
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
                return Ok((None, notes));
            }
            Some(why) => whys.push(why),
        }
    }
    let both = format!("{}; then {}", whys[0], whys[1]);
    if site.fallback_local {
        let note = format!("no result from two runs on GitHub ({both}); running it on this Mac");
        tracing::warn!(run = %spec.id, "{note}");
        said(&note);
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
                self.runs_seen.lock().unwrap().insert(sha.to_string());
                let (st, _) = self.phase();
                return ok(serde_json::json!({"workflow_runs": [{
                    "id": 4242, "status": st, "head_branch": branch, "event": "push",
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
                let started = (job != "queued").then_some("2026-11-20T10:00:06Z");
                return ok(serde_json::json!({"jobs": [
                    {"name": "plan", "status": "completed", "started_at": "2026-11-20T10:00:01Z"},
                    {"name": "run", "status": job, "started_at": started}]}));
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
        let sha = snapshot(&r.wt, "origin/main", "mason-run.yml", &json, "m").unwrap();
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
        let e = snapshot(&r.wt, "origin/main", "mason-run.yml", "{}", "m").unwrap_err();
        assert!(e.contains("ci.yml") && e.contains("mason/run/**"), "{e}");
    }

    #[test]
    fn a_base_branch_without_the_workflow_is_environmental_not_the_tasks() {
        let r = repo();
        let e = snapshot(&r.wt, "origin/main", "other.yml", "{}", "m").unwrap_err();
        assert!(e.contains("the base branch"), "{e}");
        token();
        let gh = FakeGh::new(&r, &r.wt.join("none.zip"), Mode::Normal);
        let mut s = site(&r, gh, clock(), false);
        s.workflow = "other.yml".into();
        let d = tempfile::tempdir().unwrap();
        let e = execute(&s, &spec("x2"), d.path(), &|| None).err().unwrap();
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
            let Outcome::Done(f) = execute(&s, &spec("x3c"), logs.path(), &|| None).unwrap() else {
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
        let out = execute(&s, &spec("x4"), logs.path(), &|| None).unwrap();
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
        let Outcome::Done(f) = execute(&s, &sp, logs.path(), &|| None).unwrap() else {
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
            let out = execute(&s, &spec("x6"), logs.path(), &|| {
                (clk.now_ms() - start >= skip_after_s * 1000).then_some(StopWhy::Skipped)
            })
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
        let Outcome::Done(f) = execute(&s, &spec("x9"), logs.path(), &|| None).unwrap() else {
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
    fn a_run_past_its_limit_whose_artifact_never_comes_keeps_only_the_cut_off_line() {
        token();
        let r = repo();
        let t = tempfile::tempdir().unwrap();
        let zip = artifact(t.path(), HUNG_OUT, 0, None);
        let gh = FakeGh::new(&r, &zip, Mode::HungNoArtifact);
        let s = site(&r, gh, clock(), false);
        let logs = tempfile::tempdir().unwrap();
        let Outcome::Done(f) = execute(&s, &spec("x10"), logs.path(), &|| None).unwrap() else {
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
        let e = run_or_fallback(&s, &spec("x7"), logs.path(), &|| None)
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
        let out = run_or_fallback(&s2, &spec("x7"), logs.path(), &|| None).unwrap();
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
            let out = run_or_fallback(&s, &spec("x9"), logs.path(), &give_up_after_many_looks());
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
        let out = run_or_fallback(&s, &spec("x10"), logs.path(), &give_up_after_many_looks());
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
        let out = run_or_fallback(&s, &spec("x11"), logs.path(), &give_up_after_many_looks());
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
        let out = run_or_fallback(&s, &spec("x12"), logs.path(), &give_up_after_many_looks());
        assert!(matches!(out, Ok(None)));
        // Not before the timeout: spill needs a room function to say yes.
        assert!(clk.now_ms() - start >= s.pickup_timeout.as_millis() as u64);
        assert_eq!(gh.cancels.lock().unwrap().len(), 1);
        assert_eq!(refs(&r.bare), "");
    }

    #[test]
    fn an_outage_or_a_refused_token_is_environmental() {
        token();
        let t = tempfile::tempdir().unwrap();
        for (code, quick) in [(503u16, false), (401, true)] {
            let r = repo();
            let zip = artifact(t.path(), GREEN, 0, None);
            let gh = FakeGh::new(&r, &zip, Mode::Status(code));
            let clk = clock();
            let s = site(&r, gh, clk.clone(), false);
            let logs = tempfile::tempdir().unwrap();
            let start = clk.now_ms();
            let e = execute(&s, &spec("x8"), logs.path(), &|| None)
                .err()
                .unwrap();
            assert!(matches!(e, Fail::Environmental(_)), "{code}: {e:?}");
            if !quick {
                assert!(
                    clk.now_ms() - start >= 1_200_000,
                    "an outage is waited out first"
                );
            }
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
        let e = execute(&s, &spec("x9"), logs.path(), &|| None)
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
        let e = execute(&s, &spec("x9b"), logs.path(), &|| None)
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
        gh.queued = 30; // 30 looks at 10 s: five minutes queued, which is not the limit's time
        gh.running = 100_000;
        let gh = Arc::new(gh);
        let s = site(&r, gh.clone(), clock(), false);
        let logs = tempfile::tempdir().unwrap();
        let mut sp = spec("x10");
        sp.limit = Duration::from_secs(120);
        let Outcome::Done(f) = execute(&s, &sp, logs.path(), &|| None).unwrap() else {
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
            let _ = execute(&s, &spec("x11"), logs.path(), &|| None);
        }));
        assert!(died.is_err(), "the first daemon died mid-run");
        let first = State::load(&record_path(logs.path(), "x11")).expect("the record survives");
        assert!(first.run_id.is_some() && !first.collected);
        assert_ne!(refs(&r.bare), "", "the ref is still there");
        // A new daemon: nothing in memory but the record.
        *gh.die_at.lock().unwrap() = None;
        let s2 = site(&r, gh.clone(), clock(), false);
        let Outcome::Done(f) = execute(&s2, &spec("x11"), logs.path(), &|| None).unwrap() else {
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
        // Asked again once collected, it answers from the record and starts nothing.
        let again = execute(&s2, &spec("x11"), logs.path(), &|| None).unwrap();
        assert!(matches!(again, Outcome::Done(g) if g.output == GREEN));
        assert_eq!(gh.runs_seen.lock().unwrap().len(), 1);
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
        let Ok(Outcome::Done(_)) = execute(&s, &spec("ph1"), logs.path(), &|| None) else {
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
        let Ok(Outcome::Done(f)) = execute_sharded(&site, &spec, &dir, &|| None) else {
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
            (other.gha, other.macos),
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
            1
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
        assert_eq!(other.gha, 0);
        b.set_other_jobs(other);
        assert_eq!(
            ids.iter()
                .filter(|i| crate::sched::tests::is_held(&b, **i))
                .count(),
            6
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
        assert_eq!((other.gha, other.macos), (2, 2));
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
        assert_eq!(seen.poll(&sources, 1).gha, 3);
        api.repos.lock().unwrap().get_mut("o/r").unwrap().clear();
        let asked = api.log.lock().unwrap().len();
        // Nothing changed on GitHub's side: the count stays, and only the two conditional
        // list requests are made, with no job lookups.
        assert_eq!(seen.poll(&sources, 2).gha, 3);
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
        assert_eq!(other.gha, 2);
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
        assert_eq!((other.gha, other.stale, other.read_at_ms), (2, true, 100));
    }

    /// `run_retrying` against `mode`, with the pushes in `refuse_first` refused once.
    fn retrying(
        mode: Mode,
        refuse_first: bool,
        fallback: bool,
    ) -> (
        Result<(Option<Outcome>, Vec<String>), String>,
        Arc<FakeGh>,
        Vec<String>,
    ) {
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
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let zip = artifact(r._t.path(), GREEN, 0, None);
        let gh = FakeGh::new(&r, &zip, mode);
        let s = site(&r, gh.clone(), clock(), fallback);
        let said = Mutex::new(vec![]);
        let dir = r._t.path().join("runs");
        let out = run_retrying(&s, &spec("rt1"), &dir, &|| None, &|t| {
            said.lock().unwrap().push(t.to_string())
        });
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
            run_retrying(&s, &spec("rt2"), &dir, &|| None, &|_| {}).unwrap()
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
        let Ok(Outcome::Done(f)) = execute_sharded(&s, &sp, &dir, &|| None) else {
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
}
