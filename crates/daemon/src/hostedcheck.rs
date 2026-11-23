// SPDX-License-Identifier: GPL-3.0-or-later

//! A test command run under a step host, so that a daemon which restarts, or
//! takes over by `exec`, finds it again instead of leaving it orphaned.
//!
//! Used by the develop agent's `run_check` (host id `t<task>r<run>-check<n>`) and
//! by landing's re-test (`b<batch>-retest<n>`). Each run has a record next to its
//! log, `<id>.check`, holding a key (the tree it ran on, and the command) and
//! whether its result has been given to a caller. A call:
//!
//! - attaches to a run with the same key that is in flight (in this process, or
//!   found again after a restart);
//! - takes the result of a run with the same key that finished while the daemon
//!   was away and was never given to anyone;
//! - otherwise starts a new run on the next free number.
//!
//! The limit counts from the host's start, not from the call that collects it.

use crate::daemon::{now_ms, Daemon};
use mason_core::StepRunId;
use mason_landing::git;
use mason_step_host::client::{collect_to_file, collect_to_file_observed, ClientError, HostClient};
use mason_step_host::launcher::{spawn_host, SpawnOptions};
use mason_step_host::protocol::{CURRENT, PREVIOUS};
use mason_step_host::registry::{read_kv, write_kv, HostInfo};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

pub struct CheckSpec {
    /// `t5r7-check`, `b3-retest`: the run's number is appended.
    pub id_prefix: String,
    /// Where the log, exit record and run record go.
    pub log_dir: PathBuf,
    pub cmd: String,
    /// A build to run before `cmd` when the check goes to GitHub: it is sent as `RunSpec.build`
    /// of the same run, so one runner builds and tests (#771). Where the check runs here it
    /// is `None` (the build is its own run, holding a `compile` slot).
    pub build: Option<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub limit: Duration,
    /// What the command runs on, from [`tree_key`].
    pub tree: String,
    /// Polled while the command runs: `Some(note)` stops it (the owner skipped or forced
    /// what it belongs to). The outcome then carries the note in `stopped`.
    pub stop: Option<StopFn>,
    /// The machine slots the caller holds while the command runs (empty: none). Written next
    /// to the host so that a daemon which restarts counts them as held until the host's
    /// command ends (s589).
    pub holds: std::collections::BTreeMap<String, u32>,
    /// Whose run it is, as the resources list shows it: `#567 check`.
    pub label: String,
    /// The task and suite a develop-phase `run_check` runs for, so that the one function that
    /// decides where runs go (#698) can say. `None` for everything else.
    pub place: Option<(Option<mason_core::TaskId>, Box<mason_testing::Suite>)>,
    /// A landing check (#702): no task owns it, so it follows the global switch alone.
    pub global: bool,
}

/// What a step host's command holds of the machine, as recorded at its start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostHold {
    pub dir: PathBuf,
    pub label: String,
    pub uses: std::collections::BTreeMap<String, u32>,
}

const HOLDS_FILE: &str = "holds.kv";

fn write_holds(dir: &Path, spec: &CheckSpec) {
    if spec.holds.is_empty() {
        return;
    }
    let uses = spec
        .holds
        .iter()
        .map(|(k, n)| format!("{k}={n}"))
        .collect::<Vec<_>>()
        .join(",");
    let _ = write_kv(
        &dir.join(HOLDS_FILE),
        &[("label", spec.label.replace('\n', " ")), ("uses", uses)],
    );
}

/// Every step host under `hosts` whose command still runs and recorded slots it holds.
/// A host whose command ended, or that was lost, holds nothing.
pub fn running_holds(hosts: &Path) -> Vec<HostHold> {
    use mason_step_host::registry::{HostState, Registry};
    let Ok(all) = Registry::new(hosts).scan() else {
        return vec![];
    };
    all.into_iter()
        .filter(|h| h.state == HostState::Running)
        .filter_map(|h| {
            let kv = read_kv(&h.info.dir.join(HOLDS_FILE)).ok()?;
            let uses = kv
                .get("uses")?
                .split(',')
                .filter_map(|p| {
                    let (k, n) = p.split_once('=')?;
                    Some((k.to_string(), n.parse().ok()?))
                })
                .collect();
            Some(HostHold {
                dir: h.info.dir,
                label: kv
                    .get("label")
                    .cloned()
                    .unwrap_or_else(|| h.info.id.clone()),
                uses,
            })
        })
        .collect()
}

/// Where a check's live counts go: the develop step's run, as the sub-count `name`, from
/// the output of the suite's parser (none: nothing to count).
#[derive(Clone)]
pub struct CheckProgress {
    pub run: StepRunId,
    /// Which entry of the run's check history this run belongs to.
    pub check: usize,
    /// "check cargo-workspace".
    pub name: String,
    pub suite: Option<mason_testing::Suite>,
}

impl CheckProgress {
    /// A function that takes the output as it arrives, keeps the counts and the test
    /// executable cargo is on, and reports both. For display: no verdict comes of it.
    fn feeder(&self, d: &Arc<Daemon>) -> impl FnMut(&[u8]) {
        let mut live = self
            .suite
            .as_ref()
            .and_then(|s| mason_testing::LiveCount::new(s.parser, s))
            .map(|l| (l, Default::default()));
        let (d, run, check, name) = (d.clone(), self.run, self.check, self.name.clone());
        let mut partial: Vec<u8> = vec![];
        let mut exe = String::new();
        move |b: &[u8]| {
            if let Some((l, last)) = live.as_mut() {
                l.feed(b);
                let c: mason_testing::Counts = l.counts();
                if *last != Some(c.passed) {
                    *last = Some(c.passed);
                    d.progress_sub(run, &name, None, |s| {
                        if s.done.source != mason_core::progress::Source::Reported {
                            s.unit = "tests".into();
                            s.done = mason_core::progress::Figure::counted(c.passed);
                        }
                    });
                }
            }
            partial.extend_from_slice(b);
            while let Some(i) = partial.iter().position(|x| *x == b'\n') {
                let line: Vec<u8> = partial.drain(..=i).collect();
                if let Some(e) = executable_of(&String::from_utf8_lossy(&line)) {
                    if e != exe {
                        exe = e.clone();
                        d.check_edit(run, check, |c, _| c.current = e);
                    }
                }
            }
        }
    }
}

/// The test executable a cargo `Running unittests src/lib.rs (target/debug/deps/foo-0123abcd…)`
/// line names (the file's name without cargo's hash), `None` for any other line.
pub fn executable_of(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("Running ")?;
    let path = rest.rsplit_once('(').and_then(|(_, p)| p.strip_suffix(')'));
    let base = path.map(|p| p.rsplit('/').next().unwrap_or(p));
    let name = match base {
        Some(b) => match b.rsplit_once('-') {
            Some((n, h)) if h.len() >= 8 && h.bytes().all(|c| c.is_ascii_hexdigit()) => n,
            _ => b,
        },
        None => rest,
    };
    (!name.is_empty()).then(|| name.to_string())
}

/// See [`CheckSpec::stop`].
pub type StopFn = Arc<dyn Fn() -> Option<String> + Send + Sync>;

#[derive(Clone, Debug)]
pub struct CheckOutcome {
    pub passed: bool,
    pub timed_out: bool,
    /// The last lines of the output.
    pub output: String,
    /// All of the output, for parsing files out of it; `output` is the tail kept for notes.
    pub full: String,
    /// The id of the host that ran it.
    pub id: String,
    /// Why it was stopped before it ended, when [`CheckSpec::stop`] stopped it. Such a run
    /// did not pass or fail; it was not finished.
    pub stopped: Option<String>,
    /// For a run that carried a build and did not pass: which part ended it, "build" or
    /// "run", when the runner said. `None` otherwise.
    pub phase: Option<String>,
}

type Shared = Result<CheckOutcome, String>;

#[derive(Default)]
struct Slot {
    result: Mutex<Option<Shared>>,
    cv: Condvar,
}

fn inflight() -> &'static Mutex<HashMap<PathBuf, Arc<Slot>>> {
    static M: std::sync::OnceLock<Mutex<HashMap<PathBuf, Arc<Slot>>>> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// What the tree a command will run on is: its committed tree, and, when the
/// worktree differs from it, a hash of the difference. A key no two states share
/// if git cannot say.
pub fn tree_key(wt: &Path) -> String {
    let unique = || format!("unknown-{}", now_ms());
    let Ok(tree) = git::run(wt, &["rev-parse", "HEAD^{tree}"]) else {
        return unique();
    };
    let tree = tree.trim().to_string();
    let Ok(status) = git::run(
        wt,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain",
            "--untracked-files=all",
        ],
    ) else {
        return unique();
    };
    if status.trim().is_empty() {
        return tree;
    }
    let mut h = Sha256::new();
    h.update(status.as_bytes());
    let Ok(diff) = git::run(wt, &["--no-optional-locks", "diff", "HEAD"]) else {
        return unique();
    };
    h.update(diff.as_bytes());
    let Ok(untracked) = git::run(
        wt,
        &["ls-files", "--others", "--exclude-standard", "--full-name"],
    ) else {
        return unique();
    };
    let files: Vec<&str> = untracked.lines().filter(|l| !l.is_empty()).collect();
    for chunk in files.chunks(200) {
        let mut args = vec!["hash-object", "--no-filters", "--"];
        args.extend(chunk.iter().copied());
        match git::run(wt, &args) {
            Ok(o) => h.update(o.as_bytes()),
            Err(_) => return unique(),
        }
    }
    let sum: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    format!("{tree}+{}", &sum[..16])
}

/// A develop check this process is running: whose it is, and which suite (#826).
#[derive(Clone, Debug, PartialEq, Eq)]
struct LiveCheck {
    task: u64,
    suite: String,
}

fn live_checks() -> &'static Mutex<HashMap<String, LiveCheck>> {
    static M: std::sync::OnceLock<Mutex<HashMap<String, LiveCheck>>> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// Runs asked to cancel, by host id, with the words the owner reads.
fn cancel_requests() -> &'static Mutex<HashMap<String, String>> {
    static M: std::sync::OnceLock<Mutex<HashMap<String, String>>> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// The suite a develop check's host id names: `t5r7-check-exit-scripts9` is `exit-scripts`.
fn suite_of(id: &str) -> Option<String> {
    let (_, tail) = id.split_once("-check-")?;
    let s = tail.trim_end_matches(|c: char| c.is_ascii_digit());
    (!s.is_empty()).then(|| s.to_string())
}

/// Which of `live` a new run `id` of `task` and `suite` supersedes: every other run of the
/// same task and suite, whatever its run number. Other suites and other tasks are never named.
fn superseded_by(
    live: &HashMap<String, LiveCheck>,
    id: &str,
    task: u64,
    suite: &str,
) -> Vec<String> {
    let mut v: Vec<String> = live
        .iter()
        .filter(|(i, l)| i.as_str() != id && l.task == task && l.suite == suite)
        .map(|(i, _)| i.clone())
        .collect();
    v.sort();
    v
}

/// Ask the check `id` to end now: a remote one is cancelled on GitHub, its ref deleted and its
/// units freed by the path a skip takes; a local one has its host stopped. The cancellation
/// is logged, and noted beside the run's log for the task's progress.
fn cancel_check(d: &Daemon, id: &str, why: &str) {
    tracing::info!(host = %id, "{why}");
    cancel_requests()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.to_string(), why.to_string());
    if running(d, id) {
        stop_host(d, id);
    }
}

/// The agent step of `task` ended: cancel every check it still has running (#826).
pub fn cancel_task(d: &Daemon, task: u64) -> usize {
    let ids: Vec<String> = live_checks()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(_, l)| l.task == task)
        .map(|(i, _)| i.clone())
        .collect();
    for id in &ids {
        let suite = suite_of(id).unwrap_or_default();
        cancel_check(
            d,
            id,
            &format!("cancelled a {suite} run (#{task}): its develop step ended"),
        );
    }
    ids.len()
}

fn run_key(spec: &CheckSpec) -> String {
    let mut h = Sha256::new();
    h.update(spec.tree.as_bytes());
    h.update([0]);
    h.update(spec.cmd.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn record_path(spec: &CheckSpec, id: &str) -> PathBuf {
    spec.log_dir.join(format!("{id}.check"))
}

thread_local! {
    /// The task and run whose `run_check` call is on this thread, while it holds its results
    /// until the agent has its answer ([`hold_results`]).
    static DEFER: std::cell::RefCell<Option<(u64, u64)>> = const { std::cell::RefCell::new(None) };
}

fn deferred_owner() -> Option<(u64, u64)> {
    DEFER.with(|c| *c.borrow())
}

/// Finished runs whose result the agent has not been answered with yet: (log dir, id, key).
type Unanswered = Mutex<HashMap<(u64, u64), Vec<(PathBuf, String, String)>>>;

fn unanswered() -> &'static Unanswered {
    static M: OnceLock<Unanswered> = OnceLock::new();
    M.get_or_init(Default::default)
}

/// Ends [`hold_results`] when dropped.
pub struct HoldGuard(Option<(u64, u64)>);

impl Drop for HoldGuard {
    fn drop(&mut self) {
        DEFER.with(|c| *c.borrow_mut() = self.0);
    }
}

/// For the calls made on this thread while the guard lives, a finished run of `task`'s `run`
/// stays undelivered (so a replay of the call, with the same tree and command, takes its
/// result instead of running it again) until [`answered`] says the agent has the answer.
pub fn hold_results(task: u64, run: u64) -> HoldGuard {
    HoldGuard(DEFER.with(|c| c.replace(Some((task, run)))))
}

/// The agent has the whole answer of its `run_check` call: the runs it used are delivered.
pub fn answered(task: u64, run: u64) {
    let held = unanswered()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&(task, run))
        .unwrap_or_default();
    for (log_dir, id, key) in held {
        let _ = write_kv(
            &log_dir.join(format!("{id}.check")),
            &[("key", key), ("delivered", "1".into())],
        );
    }
}

fn mark_delivered(spec: &CheckSpec, id: &str, key: &str) {
    let _ = write_kv(
        &record_path(spec, id),
        &[("key", key.to_string()), ("delivered", "1".into())],
    );
}

/// Is there already a run of `spec` whose result nobody has been given: one in
/// flight (in this process, or found again after a restart) or one that
/// finished while the daemon was away? Such a call attaches or collects, and
/// starts nothing, so it has nothing to queue for.
pub fn pending(d: &Daemon, spec: &CheckSpec) -> bool {
    let key = run_key(spec);
    (0u32..)
        .map(|n| format!("{}{n}", spec.id_prefix))
        .map_while(|id| read_kv(&record_path(spec, &id)).ok().map(|kv| (id, kv)))
        .any(|(id, kv)| {
            kv.get("key") == Some(&key)
                && kv.get("delivered").map(String::as_str) != Some("1")
                && (inflight()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains_key(&d.cfg.hosts_dir().join(&id))
                    || matches!(
                        mason_step_host::registry::status_of(&d.cfg.hosts_dir().join(&id)),
                        Ok(s) if s.state != mason_step_host::registry::HostState::Lost
                    ))
        })
}

/// Run `spec` (or attach to, or take the result of, the run that already is it).
/// Blocks until there is a result.
pub fn run(d: &Arc<Daemon>, spec: &CheckSpec, grant: &crate::remote::Grant) -> Shared {
    run_with(d, spec, None, grant)
}

/// [`run`], feeding the develop step's progress with the run's live counts (for display only;
/// the verdict is what the command's exit says, as ever).
pub fn run_with(
    d: &Arc<Daemon>,
    spec: &CheckSpec,
    progress: Option<&CheckProgress>,
    grant: &crate::remote::Grant,
) -> Shared {
    let _ = std::fs::create_dir_all(&spec.log_dir);
    let key = run_key(spec);
    // Find or allocate, one caller at a time, so two calls never take one number.
    // Earlier runs of this prefix on another tree that nobody here is collecting: a call
    // on a new tree means their results will never be asked for.
    let mut superseded: Vec<String> = vec![];
    let (id, slot, collector) = {
        let mut map = inflight().lock().unwrap_or_else(|e| e.into_inner());
        let mut n = 0u32;
        loop {
            let id = format!("{}{n}", spec.id_prefix);
            let rec = record_path(spec, &id);
            let mapkey = d.cfg.hosts_dir().join(&id);
            match read_kv(&rec) {
                Err(_) => {
                    // A free number: this run is new.
                    for ext in ["log", "exit", "remote"] {
                        let _ = std::fs::remove_file(spec.log_dir.join(format!("{id}.{ext}")));
                    }
                    let _ = write_kv(&rec, &[("key", key.clone()), ("delivered", "0".into())]);
                    let slot = Arc::new(Slot::default());
                    map.insert(mapkey, slot.clone());
                    break (id, slot, true);
                }
                Ok(kv) => {
                    let same = kv.get("key") == Some(&key);
                    let delivered = kv.get("delivered").map(String::as_str) == Some("1");
                    if same && !delivered {
                        if let Some(slot) = map.get(&mapkey) {
                            break (id, slot.clone(), false);
                        }
                        // In flight or finished in a daemon that is gone: collect it.
                        let slot = Arc::new(Slot::default());
                        map.insert(mapkey, slot.clone());
                        break (id, slot, true);
                    }
                    if !same && !delivered && !map.contains_key(&mapkey) {
                        superseded.push(id);
                    }
                }
            }
            n += 1;
        }
    };
    for old in superseded {
        if running(d, &old) && stop_host(d, &old) {
            tracing::info!(host = %old, now = %id, "stopped a check on an older tree that nobody will collect");
        }
        let _ = write_kv(
            &record_path(spec, &old),
            &[
                (
                    "key",
                    read_kv(&record_path(spec, &old))
                        .ok()
                        .and_then(|kv| kv.get("key").cloned())
                        .unwrap_or_default(),
                ),
                ("delivered", "1".into()),
                ("superseded", id.clone()),
            ],
        );
    }
    let mine = check_owner(&id).zip(suite_of(&id));
    if collector {
        if let Some(((task, _), suite)) = &mine {
            let older = {
                let mut live = live_checks().lock().unwrap_or_else(|e| e.into_inner());
                let older = superseded_by(&live, &id, *task, suite);
                live.insert(
                    id.clone(),
                    LiveCheck {
                        task: *task,
                        suite: suite.clone(),
                    },
                );
                older
            };
            for old in older {
                cancel_check(
                    d,
                    &old,
                    &format!(
                        "cancelled an older {suite} run ({old}) for #{task}: {id} replaces it"
                    ),
                );
            }
        }
    }
    if !collector {
        let mut g = slot.result.lock().unwrap_or_else(|e| e.into_inner());
        while g.is_none() {
            g = slot.cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        return g.clone().unwrap_or_else(|| Err("no result".into()));
    }
    let res = collect(d, spec, &id, progress, grant);
    // A `favor-local` placement stood for this run only (#832).
    if let (Some((t, s)), Some(project)) = (
        spec.place.as_ref(),
        crate::remote::any_remote(d)
            .then(|| crate::remote::project_of(d, &spec.cwd))
            .flatten(),
    ) {
        crate::place::forget(&project, t.map(|t| t.0), &s.name, "check");
    }
    live_checks()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    cancel_requests()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    // Given to its callers now: a later call on the same tree runs again. A call that asked
    // for its results to be held (#1051) is not done until its answer is written to the
    // agent: until then a replay of the call, after a takeover, reuses this result.
    match deferred_owner().filter(|_| check_owner(&id).is_some()) {
        Some(owner) => unanswered()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(owner)
            .or_default()
            .push((spec.log_dir.clone(), id.clone(), key.clone())),
        None => mark_delivered(spec, &id, &key),
    }
    {
        let mut g = slot.result.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some(res.clone());
        slot.cv.notify_all();
    }
    inflight()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&d.cfg.hosts_dir().join(&id));
    res
}

/// Is the step host `id` alive with its command still running?
fn running(d: &Daemon, id: &str) -> bool {
    matches!(
        mason_step_host::registry::status_of(&d.cfg.hosts_dir().join(id)),
        Ok(s) if s.state == mason_step_host::registry::HostState::Running
    )
}

/// Ask the step host `id` to stop its command (and its children). `false`: it could not be
/// reached.
fn stop_host(d: &Daemon, id: &str) -> bool {
    let sock = d.cfg.hosts_dir().join(id).join("sock");
    HostClient::connect(&sock, PREVIOUS, CURRENT, "daemon").is_ok_and(|mut h| h.stop(1000).is_ok())
}

/// The task and run a develop check's host id names (`t575r3670-check-cargo-workspace2`).
pub fn check_owner(id: &str) -> Option<(u64, u64)> {
    let (head, _) = id.split_once("-check")?;
    let (t, r) = head.strip_prefix('t')?.split_once('r')?;
    Some((t.parse().ok()?, r.parse().ok()?))
}

/// Stop develop checks that survived a daemon restart and that no run can collect any more:
/// their task's open run is another one, or none. Their step hosts outlive the daemon by
/// design, and since s589 they keep their slots, so left alone they run a whole suite for
/// nobody while holding a slot (2026-11-18: three such runs beside two real ones, five full
/// suites under a limit of two). A check this process is collecting is never touched, nor
/// one whose run is still open: its agent may call again and attach to it.
pub fn stop_unwanted(d: &Arc<Daemon>) -> usize {
    use mason_step_host::registry::{HostState, Registry};
    let Ok(all) = Registry::new(d.cfg.hosts_dir()).scan() else {
        return 0;
    };
    let Ok(w) = d.world() else { return 0 };
    let mut n = 0;
    for h in all.into_iter().filter(|h| h.state == HostState::Running) {
        let id = h.info.id.clone();
        let Some((task, run)) = check_owner(&id) else {
            continue;
        };
        if inflight()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&h.info.dir)
        {
            continue;
        }
        if w.open_run(mason_core::TaskId(task)) == Some(StepRunId(run)) {
            continue;
        }
        if stop_host(d, &id) {
            tracing::info!(host = %id, "stopped a check whose run is over: nobody can collect it");
            n += 1;
        }
    }
    n
}

/// Where `spec` runs, in words for the agent's result: "on GitHub Actions" or "on this Mac".
pub fn where_ran(d: &Arc<Daemon>, spec: &CheckSpec) -> String {
    use crate::remote as rm;
    let local = || "on this Mac".to_string();
    if !rm::any_remote(d) {
        return local();
    }
    let Some(project) = rm::project_of(d, &spec.cwd) else {
        return local();
    };
    let exec = match spec.place.as_ref() {
        Some((t, s)) => rm::executor_for(d, &project, rm::Kind::Check, Some((*t, &**s))),
        None if spec.global => rm::executor_global(d, &project),
        None => rm::Executor::Local,
    };
    match exec {
        rm::Executor::GitHub { .. } => "on GitHub Actions".to_string(),
        _ => local(),
    }
}

/// The runner label when `spec` goes to GitHub, decided before anything is queued (#759).
fn runs_on_github(d: &Arc<Daemon>, spec: &CheckSpec) -> Option<String> {
    use crate::remote as rm;
    if !rm::any_remote(d) {
        return None;
    }
    let project = rm::project_of(d, &spec.cwd)?;
    let place = spec.place.as_ref().map(|(t, s)| (*t, &**s));
    let exec = match place {
        Some((t, s)) => rm::executor_for(d, &project, rm::Kind::Check, Some((t, s))),
        None if spec.global => rm::executor_global(d, &project),
        None => rm::executor_for(d, &project, rm::Kind::Check, None),
    };
    match exec {
        rm::Executor::GitHub { runs_on } => Some(runs_on),
        _ => None,
    }
}

/// What `spec` holds when it goes to GitHub: one `gha` unit per job it starts (its shards, else
/// one), and as many `gha-macos` on a macOS runner (#818). `None`: it runs here.
pub fn pool_units(
    d: &Arc<Daemon>,
    spec: &CheckSpec,
) -> Option<std::collections::BTreeMap<String, u32>> {
    let runs_on = runs_on_github(d, spec)?;
    let suite = spec.place.as_ref().map(|(_, s)| &**s);
    Some(crate::remote::units_for(
        &runs_on,
        crate::remote::jobs_of(suite, &spec.cmd),
    ))
}

/// Does `spec` go to GitHub? Then it needs no local `compile` or `test-suite` slot and no CPU
/// or heat gate: it uses nothing on this Mac (#759).
pub fn goes_remote(d: &Arc<Daemon>, spec: &CheckSpec) -> bool {
    runs_on_github(d, spec).is_some()
}

/// The check on GitHub Actions, when the one function that decides where runs go says so
/// (#697); `None` for the local path, which is every run until #698. The result is the same
/// `CheckOutcome`: `passed` is what the command's exit says. A remote run that is lost is run
/// again, not resumed. No live counts come from it.
/// The suite's `build` is a template until a caller expands it. A landing check carries either
/// its expanded build as `cmd` or a suite with no build (#817); a template is never passed on.
fn build_choice<'a>(suite: Option<&'a mason_testing::Suite>, cmd: &str) -> Option<&'a str> {
    suite
        .and_then(|s| s.build.as_deref())
        .filter(|b| !b.contains('{') && !cmd.starts_with(b))
}

/// The suite as a landing check's place: its placement rules, but no build step of its own.
pub fn without_build(suite: &mason_testing::Suite) -> mason_testing::Suite {
    let mut s = suite.clone();
    s.build = None;
    s
}

/// A local host for this run already exists: a restarted daemon is collecting a run that
/// started here, so its slots were taken (and counted from `holds.kv`) when it started.
fn dir_has_host(d: &Arc<Daemon>, id: &str) -> bool {
    d.cfg.hosts_dir().join(id).join("host.kv").exists()
}

/// The run spec of a check placed on GitHub. It names the suite the check runs for, so that the
/// workflow sets up that suite's toolchains (Java and Gradle for `compose*`, Xcode for `swift*`):
/// without the name every check got Rust alone, and each compose check failed to compile on the
/// runner's default JDK. Only the name is taken: the command is the check's own, so the suite's
/// rerun and results file stay out of it.
fn check_run_spec(
    project: &str,
    id: &str,
    spec: &CheckSpec,
    runs_on: &str,
) -> crate::remote::RunSpec {
    let mut rs = crate::remote::spec_for(
        project, id, None, &spec.cmd, spec.limit, &spec.env, runs_on, &spec.tree,
    );
    if let Some((_, suite)) = spec.place.as_ref() {
        rs.suite = suite.name.clone();
    }
    rs
}

fn remote_check(
    d: &Arc<Daemon>,
    spec: &CheckSpec,
    id: &str,
    progress: Option<&CheckProgress>,
    grant: &crate::remote::Grant,
) -> Option<Shared> {
    use crate::remote as rm;
    let runs_on = runs_on_github(d, spec)?;
    let project = rm::project_of(d, &spec.cwd)?;
    let place = spec.place.as_ref().map(|(t, s)| (*t, &**s));
    // The suite's build is the first part of what runs there (it was not run here).
    // It goes in `RunSpec.build`, a separate process, not joined with `&&` (two `cd`s break).
    let build = build_choice(place.map(|(_, s)| s), &spec.cmd);
    let build = spec.build.as_deref().or(build);
    let site = rm::site_for_run(
        d,
        &project,
        &spec.cwd,
        place.and_then(|(t, _)| t).map(|t| t.0),
        &spec.holds,
        false,
    )?;
    let rs = rm::with_site_env(
        rm::with_build(check_run_spec(&project, id, spec, &runs_on), build),
        &site,
    );
    // The suite's own command (whole, or narrowed with a shard command of its own, #873) goes
    // out as one job per shard.
    let rs = match place.map(|(_, s)| s) {
        Some(ps) if ps.shards > 1 && spec.cmd == ps.command => match ps
            .dev_shard_command
            .as_deref()
            // A narrowed check's own shard command (filled in, #880); else the whole suite's.
            .filter(|c| !c.contains('{'))
            .or(ps.shard_command.as_deref())
        {
            Some(sc) => {
                let mut rs = rs;
                rs.shards = ps.shards;
                rs.shard_command = Some(sc.to_string());
                rs.shard_lists = crate::suites::shard_lists(d, &project, ps);
                rs.partial_lists = crate::suites::lists_partial(ps);
                rs
            }
            None => rs,
        },
        _ => rs,
    };
    // The units it holds, on record until it ends, for a daemon that restarts (#700).
    let _hold = rm::HoldGuard::new(
        &d.cfg.hosts_dir(),
        &rs,
        id,
        id,
        rs.shards.max(1),
        &spec.log_dir,
        if spec.global || place.is_some_and(|(t, _)| t.is_none()) {
            "batch"
        } else {
            "check"
        },
        place.and_then(|(t, _)| t).map_or(0, |t| t.0),
    );
    let note: Mutex<Option<String>> = Mutex::new(None);
    let stop = || {
        let asked = cancel_requests()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned();
        let n = match asked {
            Some(n) => n,
            None => spec.stop.as_ref().and_then(|f| f())?,
        };
        *note.lock().unwrap_or_else(|e| e.into_inner()) = Some(n);
        Some(rm::StopWhy::Skipped)
    };
    // A run that gave no result is tried again here, not handed back to the agent (#873); the
    // retry is shown in the check's progress.
    let said = |t: &str| {
        if let Some(p) = progress {
            d.check_edit(p.run, p.check, |c, _| c.current = t.to_string());
        }
    };
    Some(
        match rm::run_retrying(&site, &rs, &spec.log_dir, &stop, &said, grant) {
            Ok((None, _)) => return None,
            Ok((Some(rm::Outcome::Stopped(_)), _)) => Ok(CheckOutcome {
                passed: false,
                timed_out: false,
                output: String::new(),
                full: String::new(),
                id: id.to_string(),
                stopped: note.lock().unwrap_or_else(|e| e.into_inner()).clone(),
                phase: None,
            }),
            Ok((Some(rm::Outcome::Done(f)), notes)) => {
                let retried: String = notes.iter().map(|n| format!("({n})\n")).collect();
                Ok(CheckOutcome {
                    passed: f.code == 0 && f.signal == 0 && !f.timed_out,
                    timed_out: f.timed_out,
                    output: format!(
                        "{retried}({})\n{}",
                        f.ran_on,
                        crate::jobs::tail(&f.output, 40)
                    ),
                    full: f.output.clone(),
                    id: id.to_string(),
                    stopped: None,
                    phase: f.phase.clone(),
                })
            }
            Err(e) => Err(match rm::environmental(&e) {
                // Two runs gave no result: not a failure of the code (#873). Landing waits out
                // such an outage by this prefix (#772).
                Some(why) => format!("{OUTAGE_PREFIX}{why}"),
                None => e,
            }),
        },
    )
}

/// What an error from [`run`] starts with when GitHub, not the code, kept the check from running.
pub const OUTAGE_PREFIX: &str = "no result (infrastructure): ";

/// Whether an error from [`run`] is that outage (#772).
pub fn is_outage(e: &str) -> bool {
    e.starts_with(OUTAGE_PREFIX)
}

fn collect(
    d: &Arc<Daemon>,
    spec: &CheckSpec,
    id: &str,
    progress: Option<&CheckProgress>,
    grant: &crate::remote::Grant,
) -> Shared {
    let bound_remote = runs_on_github(d, spec).is_some();
    if let Some(r) = remote_check(d, spec, id, progress, grant) {
        return r;
    }
    // Meant for GitHub but sent here (`fallback = "local"`): it skipped this machine's queue,
    // so it takes the slots it holds when placed here before it starts, under the same limits
    // and CPU and thermal holds as any local run. Held until the command ends.
    let _fallback = if bound_remote && !dir_has_host(d, id) {
        match crate::remote::project_of(d, &spec.cwd) {
            Some(project) => match crate::remote::fallback_lease_until(
                d,
                &project,
                spec.place.as_ref().and_then(|(t, _)| *t),
                &spec.holds,
                &format!("{} (no GitHub runner, run here)", spec.label),
                // A skip ends the wait for this machine's slots too (#873).
                &|| spec.stop.as_ref().is_some_and(|f| f().is_some()),
            ) {
                Ok(l) => l,
                Err(e) => {
                    return match spec.stop.as_ref().and_then(|f| f()) {
                        Some(note) => Ok(CheckOutcome {
                            passed: false,
                            timed_out: false,
                            output: String::new(),
                            full: String::new(),
                            id: id.to_string(),
                            stopped: Some(note),
                            phase: None,
                        }),
                        None => Err(e),
                    }
                }
            },
            None => None,
        }
    } else {
        None
    };
    let hosts = d.cfg.hosts_dir();
    let dir = hosts.join(id);
    if !dir.join("host.kv").exists() {
        let o = SpawnOptions {
            host_bin: d.cfg.host_bin.clone(),
            root: hosts.clone(),
            id: id.to_string(),
            cmd: vec![
                "sh".into(),
                "-c".into(),
                // A build that was folded into this check and fell back to this Mac: run it
                // first, in a subshell so that its `cd` does not leak into the command.
                match &spec.build {
                    Some(b) => format!("({b}) && ({})", spec.cmd),
                    None => spec.cmd.clone(),
                },
            ],
            cwd: Some(spec.cwd.clone()),
            env: spec.env.clone(),
            clear_env: false,
            stdin: None,
            linger: Duration::from_secs(600),
            heartbeat: d.cfg.heartbeat,
        };
        spawn_host(&o).map_err(|e| format!("cannot start the command: {e}"))?;
        write_holds(&dir, spec);
    }
    let started = HostInfo::read(&dir).map_or_else(|_| now_ms(), |i| i.started_ms);
    let deadline = started.saturating_add(spec.limit.as_millis() as u64);
    // Stop it when the limit, counted from the host's start, has passed.
    let done = Arc::new(AtomicBool::new(false));
    let fired = Arc::new(AtomicBool::new(false));
    let stopped: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    {
        let (done, fired, sock) = (done.clone(), fired.clone(), dir.join("sock"));
        let (stop, stopped) = (spec.stop.clone(), stopped.clone());
        std::thread::spawn(move || loop {
            if done.load(Ordering::SeqCst) {
                return;
            }
            if let Some(note) = stop.as_ref().and_then(|f| f()) {
                *stopped.lock().unwrap_or_else(|e| e.into_inner()) = Some(note);
                if let Ok(mut h) = HostClient::connect(&sock, PREVIOUS, CURRENT, "daemon") {
                    let _ = h.stop(1000);
                }
                return;
            }
            if now_ms() >= deadline {
                fired.store(true, Ordering::SeqCst);
                if let Ok(mut h) = HostClient::connect(&sock, PREVIOUS, CURRENT, "daemon") {
                    let _ = h.stop(1000);
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        });
    }
    let log = spec.log_dir.join(format!("{id}.log"));
    let exit = spec.log_dir.join(format!("{id}.exit"));
    let rec = match progress {
        None => collect_to_file(&dir, &log, &exit),
        Some(pr) => {
            let mut feed = pr.feeder(d);
            // After a restart the log is read again from its start, so the counts are exact.
            if let Ok(old) = std::fs::read(&log) {
                feed(&old);
            }
            collect_to_file_observed(&dir, &log, &exit, &mut |_, b| feed(b))
        }
    };
    done.store(true, Ordering::SeqCst);
    let rec = match rec {
        Ok(r) => r,
        Err(ClientError::HostLost { .. }) => {
            return Err("the check was lost (its host died); run it again".into());
        }
        Err(e) => return Err(format!("cannot collect the check's result: {e}")),
    };
    let stopped = stopped.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let timed_out = stopped.is_none()
        && (fired.load(Ordering::SeqCst)
            || (rec.signal != 0 && rec.ended_ms > 0 && rec.ended_ms + 1500 >= deadline));
    let full = std::fs::read_to_string(&log).unwrap_or_default();
    Ok(CheckOutcome {
        passed: stopped.is_none() && rec.code == 0 && rec.signal == 0 && !timed_out,
        timed_out,
        output: crate::jobs::tail(&full, 40),
        full,
        id: id.to_string(),
        stopped,
        phase: None,
    })
}

#[cfg(test)]
mod build_choice_tests {
    use super::*;

    fn live(pairs: &[(&str, u64, &str)]) -> HashMap<String, LiveCheck> {
        pairs
            .iter()
            .map(|(i, t, s)| {
                (
                    i.to_string(),
                    LiveCheck {
                        task: *t,
                        suite: s.to_string(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn a_new_run_supersedes_the_same_suite_of_the_same_task_only() {
        let l = live(&[
            ("t765r1-check-exit-scripts7", 765, "exit-scripts"),
            ("t765r2-check-exit-scripts8", 765, "exit-scripts"),
            ("t765r2-check-build3", 765, "build"),
            ("t788r1-check-exit-scripts1", 788, "exit-scripts"),
        ]);
        assert_eq!(
            superseded_by(&l, "t765r2-check-exit-scripts9", 765, "exit-scripts"),
            vec!["t765r1-check-exit-scripts7", "t765r2-check-exit-scripts8"]
        );
        assert!(superseded_by(&l, "t765r2-check-exit-scripts8", 765, "build").len() == 1);
    }

    #[test]
    fn the_suite_is_read_from_the_host_id() {
        assert_eq!(
            suite_of("t575r3670-check-cargo-workspace2").as_deref(),
            Some("cargo-workspace")
        );
        assert_eq!(suite_of("b3-retest1"), None);
    }

    fn suite(build: Option<&str>) -> mason_testing::Suite {
        let mut s = mason_testing::Suite::new("t", "c", mason_testing::ParserKind::Junit);
        s.build = build.map(str::to_string);
        s
    }

    #[test]
    fn a_raw_template_is_never_passed_through() {
        let s = suite(Some("{project_test} --no-run"));
        assert_eq!(build_choice(Some(&s), "cargo test --no-run"), None);
    }

    #[test]
    fn an_expanded_build_is_kept_and_none_is_none() {
        let s = suite(Some("cargo build"));
        assert_eq!(build_choice(Some(&s), "cargo test"), Some("cargo build"));
        assert_eq!(build_choice(None, "x"), None);
        assert_eq!(build_choice(Some(&without_build(&s)), "x"), None);
    }

    fn check(place: Option<&str>) -> CheckSpec {
        CheckSpec {
            id_prefix: "t1r1-check".into(),
            log_dir: PathBuf::from("/nonexistent"),
            cmd: "./gradlew build".into(),
            build: None,
            cwd: PathBuf::from("/nonexistent"),
            env: vec![],
            limit: Duration::from_secs(60),
            tree: String::new(),
            stop: None,
            holds: Default::default(),
            label: "#1 check".into(),
            place: place.map(|n| {
                let mut s = mason_testing::Suite::new(n, "x", mason_testing::ParserKind::Junit);
                s.rerun = Some("rerun {tests}".into());
                (Some(mason_core::TaskId(1)), Box::new(s))
            }),
            global: false,
        }
    }

    #[test]
    fn a_check_on_github_names_its_suite_so_the_runner_sets_up_its_toolchains() {
        for (suite, want) in [
            ("compose-clients", "java"),
            ("swift-clients", "swift"),
            ("cargo-workspace", "rust"),
        ] {
            let rs = check_run_spec("mason", "t1r1-check0", &check(Some(suite)), "ubuntu-latest");
            assert_eq!(rs.suite, suite);
            assert_eq!(crate::remote::toolchains(&rs.suite), vec![want], "{suite}");
            assert_eq!(
                rs.command, "./gradlew build",
                "the check's own command runs"
            );
            assert_eq!(rs.rerun, None, "the suite's rerun is not taken");
        }
        let rs = check_run_spec("mason", "t1r1-check0", &check(None), "ubuntu-latest");
        assert_eq!(rs.suite, "");
    }
}
