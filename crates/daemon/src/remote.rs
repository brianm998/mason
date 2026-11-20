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
use std::sync::Arc;
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

/// Where this run goes: [`crate::place::decide`] for a task's suite or check (`place` names the
/// task and the suite); every other run (a landing batch, a base run, a debt) stays here until
/// #702. A test can force a project remote (`MASON_TEST_FORCE_REMOTE=1` in the daemon's own
/// environment, which a task cannot set, and only for a project that has a `[remote]`).
pub fn executor_for(
    d: &Daemon,
    project: &str,
    kind: Kind,
    place: Option<(mason_core::TaskId, &mason_testing::Suite)>,
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
    match crate::place::place_of(d, project, Some(task.0), suite, kind) {
        crate::place::Place::GitHub { runs_on, .. } => Executor::GitHub { runs_on },
        crate::place::Place::Local { .. } => Executor::Local,
    }
}

fn forced_for_tests() -> bool {
    std::env::var("MASON_TEST_FORCE_REMOTE").is_ok_and(|v| v == "1")
}

/// Does any project have a `[remote]` section? (Cheap: lets a seam skip finding its project.)
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
    pub gh: GitHub,
    pub clock: Arc<dyn Clock>,
    /// Seconds between looks at a run.
    pub poll_s: u64,
}

/// What a project's `[remote]` gives, for one worktree. `None`: the project has no `[remote]`.
pub fn site_for(d: &Daemon, project: &str, cwd: &Path) -> Option<Site> {
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
        fallback_local: r.fallback_local,
        gh: GitHub::new(http, &r.api, &r.repo, token),
        clock: Arc::new(SystemClock),
        poll_s: 10,
    })
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

// ---- the snapshot -----------------------------------------------------------------------------

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
    let timeout_min = spec.limit.as_secs().div_ceil(60).max(1);
    serde_json::json!({
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
    ref_deleted: bool,
    code: i32,
    signal: i32,
    timed_out: bool,
    ran_ms: Option<u64>,
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
                ("ref_deleted", (self.ref_deleted as u8).to_string()),
                ("code", self.code.to_string()),
                ("signal", self.signal.to_string()),
                ("timed_out", (self.timed_out as u8).to_string()),
                ("ran_ms", opt(self.ran_ms)),
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
            ref_deleted: flag("ref_deleted"),
            code: s("code").parse().unwrap_or(0),
            signal: s("signal").parse().unwrap_or(0),
            timed_out: flag("timed_out"),
            ran_ms: n("ran_ms"),
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

/// The safety sweep's part: for each project with `[remote]`, delete its stale run refs. At most
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
                git::run(
                    &site.cwd,
                    &["push", &site.remote, "--delete", &format!("refs/heads/{r}")],
                )
                .is_ok()
            },
        );
        if !gone.is_empty() {
            tracing::info!(project = %id, count = gone.len(), "swept stale run refs");
        }
    }
}

// ---- running ----------------------------------------------------------------------------------

pub enum Outcome {
    Done(Finished),
    /// The owner skipped or forced, or the task is gone: the run was cancelled.
    Stopped(StopWhy),
}

fn record_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.remote"))
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
                let text = format!(
                    "(the run was cancelled after its limit of {} s on the runner)\n",
                    spec.limit.as_secs()
                );
                let _ = std::fs::write(&log, &text);
                st.save(&rec);
                delete_ref(site, &mut st, &rec);
                return Ok(Outcome::Done(finished_from(&st, &log)));
            }
        } else if now.saturating_sub(st.pushed_ms) >= site.pickup_timeout.as_millis() as u64 {
            cancel(site, &st);
            abandon(&mut st);
            return Err(Fail::Environmental(format!(
                "no runner picked up {} within {} s",
                st.ref_name,
                site.pickup_timeout.as_secs()
            )));
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

fn push(site: &Site, st: &State) -> Result<(), String> {
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
    // Has a runner taken the job? A 304 on the run says nothing about its jobs.
    if st.picked_ms.is_none() && run.status != "completed" {
        let jobs = site.gh.remote_run_jobs(run.id)?;
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
            st.save(rec);
        }
    }
    if run.status != "completed" {
        // A shard that never got a runner while the others are done would hold the run until its
        // limit: end the run, and its missing artifact has that shard alone run again (#701).
        if let (false, Some(p)) = (st.shards.is_empty(), st.picked_ms) {
            if now >= p.saturating_add(site.pickup_timeout.as_millis() as u64) {
                let jobs = site.gh.remote_run_jobs(run.id)?;
                let of = |s: &str| jobs.iter().any(|j| is_run_job(&j.name) && j.status == s);
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
                "run {} ended ({conclusion}) without a result artifact",
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
                "run {} ended ({conclusion}) without a result artifact",
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
) -> Option<mason_scheduler::Needs> {
    const MACHINE: [&str; 3] = ["compile", "test-suite", "test-small"];
    if !needs.exclusive && !MACHINE.iter().any(|m| needs.amounts.contains_key(*m)) {
        return None;
    }
    let Executor::GitHub { runs_on } = executor_for(d, project, Kind::Suite, None) else {
        return None;
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

/// What a remote run holds, written when it starts and removed when it ends, so a daemon that
/// restarts counts the units of runs still out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteHold {
    /// `task N` when a task's step holds them (its lease takes them over when it asks again),
    /// else the run's id.
    pub key: String,
    pub label: String,
    pub units: std::collections::BTreeMap<String, u32>,
    /// The run's record (`<id>.remote`): the run has ended when it is gone or collected.
    pub record: PathBuf,
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
        ],
    );
}

/// Forget the hold of run `id`: it has ended.
pub fn remove_hold(dir: &Path, id: &str) {
    let _ = std::fs::remove_file(hold_file(dir, id));
}

/// Has the run behind this record ended? A missing record is a run that was abandoned or
/// never began; a collected one is done.
pub fn record_ended(record: &Path) -> bool {
    State::load(record).is_none_or(|s| s.collected)
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
            })
        })
        .collect();
    out.sort_by(|a, b| a.record.cmp(&b.record));
    out
}

/// A hold that is written while a remote run is out and removed when this is dropped.
pub struct HoldGuard {
    dir: PathBuf,
    id: String,
}

impl HoldGuard {
    pub fn new(
        hosts: &Path,
        spec: &RunSpec,
        key: &str,
        label: &str,
        shards: u32,
        dir: &Path,
    ) -> Self {
        let holds = holds_dir(hosts);
        write_hold(
            &holds,
            &spec.id,
            &RemoteHold {
                key: key.to_string(),
                label: label.to_string(),
                units: units_for(&spec.runs_on, shards),
                record: record_path(dir, &spec.id),
            },
        );
        HoldGuard {
            dir: holds,
            id: spec.id.clone(),
        }
    }
}

impl Drop for HoldGuard {
    fn drop(&mut self) {
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
    }
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
            Arc::new(FakeGh {
                bare: r.bare.clone(),
                zip: zip.to_path_buf(),
                mode,
                queued: 1,
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
            if self.mode == Mode::NeverPicked || n <= self.queued {
                ("queued", "queued")
            } else if n <= self.queued + self.running {
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
                if self.mode == Mode::NoArtifact {
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
            gh: GitHub::new(
                gh,
                "https://api.github.com",
                "o/r",
                TokenRef::Env("MASON_TEST_REMOTE_TOKEN".into()),
            ),
            clock,
            poll_s: 10,
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
        assert_eq!(wf, vec![".github/workflows/mason-run.yml".to_string()]);
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
    fn the_run_json_carries_only_masons_environment_and_the_pinned_definition() {
        let v: serde_json::Value = serde_json::from_str(&run_json(&spec("x3"))).unwrap();
        assert_eq!(v["command"], "cargo test --workspace");
        assert_eq!(v["rerun"], "cargo test -- --exact {tests}");
        assert_eq!(v["runs_on"], "ubuntu-latest");
        assert_eq!(v["limit_s"], 600);
        assert_eq!(v["timeout_min"], 10);
        assert_eq!(v["env"]["MASON_TASK"], "7");
        assert!(v["env"].get("SECRET_TOKEN").is_none());
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

    #[test]
    fn a_skip_while_queued_or_running_cancels_the_run_and_deletes_the_ref() {
        token();
        for polls_before_skip in [3u32, 40] {
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
            let s = site(&r, gh.clone(), clock(), false);
            let logs = tempfile::tempdir().unwrap();
            let n = std::cell::Cell::new(0u32);
            let out = execute(&s, &spec("x6"), logs.path(), &|| {
                n.set(n.get() + 1);
                (n.get() > polls_before_skip * 40).then_some(StopWhy::Skipped)
            })
            .unwrap();
            assert!(matches!(out, Outcome::Stopped(StopWhy::Skipped)));
            assert_eq!(
                gh.cancels.lock().unwrap().len(),
                1,
                "after {polls_before_skip}"
            );
            assert_eq!(refs(&r.bare), "");
            assert!(!record_path(logs.path(), "x6").exists());
        }
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

    #[test]
    fn iso_times_and_ref_names() {
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_iso("2026-11-20T10:00:06Z").unwrap() - parse_iso("2026-11-20T10:00:00Z").unwrap(),
            6
        );
        assert_eq!(
            parse_iso("2026-03-01T00:00:00Z").unwrap() - parse_iso("2026-02-28T00:00:00Z").unwrap(),
            2 * 86400
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
            gh: GitHub::new(
                gh.clone(),
                "https://api.github.com",
                "o/r",
                TokenRef::Env("MASON_TEST_REMOTE_TOKEN".into()),
            ),
            clock: clock(),
            poll_s: 10,
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
    fn job_names_of_a_sharded_run_are_run_jobs() {
        assert!(is_run_job("run") && is_run_job("run (3)"));
        assert!(!is_run_job("plan") && !is_run_job("running"));
    }
}
