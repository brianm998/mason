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

/// Words the batch's note carries for a run that was killed at its limit (s702): `mason report`
/// counts batches by them.
pub const TIMED_OUT_ONCE: &str = "timed out once";
/// The second kill of the same batch.
pub const TIMED_OUT_TWICE: &str = "timed out twice";

use crate::daemon::{now_ms, Daemon};
use crate::jobs::Ctx;
use crate::newtests::Added;
use mason_adapter_api::service::{
    BuildSystemAdapter, CheckCounts, Checks, CiJobCounts, CiRunJobs, CiState, GitHostAdapter,
    Observed, PollEvent, Poller, PrState, Review,
};
use mason_core::progress::{Change, Figure};
use mason_core::*;
use mason_landing::audit::{self, Removal};
use mason_landing::batch::{Assembled, Batch, BatchTask, Finish};
use mason_landing::config::{self, LandingConfig, Rung};
use mason_landing::git;
use mason_landing::plan::{self, Candidate, Footprint, Formation};
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
    /// The host refused our credentials: nothing is wrong with the change. Landing is held for
    /// the project; the task stays in landing and submits again when the hold lifts.
    Held,
}

/// The batch's phases: forming, integrating, running suites, landing.
const PHASES: u64 = 4;

#[derive(Clone, Debug)]
struct Wait {
    task: TaskId,
    /// The landing step run this task waits in (its progress record).
    run: StepRunId,
    branch: String,
    /// The branch's head when the task joined the queue: the commit the batch merges.
    head: String,
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
    /// Since when a project's landing configuration could not be read (git failed).
    unread_since: HashMap<ProjectId, Instant>,
    /// A self-update is waiting for the running batches to finish: no new batch starts and
    /// the queues keep what they hold. It only holds batches back; it never changes a verdict.
    draining: bool,
    /// Per project, the tasks the queue is collecting for: still moving toward landing while a
    /// batch waits out its window. Empty when nothing waits on anyone.
    collecting: HashMap<ProjectId, Vec<u64>>,
}

impl State {
    /// Count `elapsed` as idle while tasks wait with nothing running; any other look ends the
    /// idle spell, so the reading is how long the queue has idled now, not a lifetime total.
    fn note_idle(&mut self, pid: ProjectId, waiting: bool, running: bool, elapsed: Duration) {
        if waiting && !running {
            *self.idle_ms.entry(pid).or_default() += elapsed.as_millis() as u64;
        } else {
            self.idle_ms.remove(&pid);
        }
    }
}

/// The landing queues of every project.
#[derive(Default)]
pub struct Landing {
    m: Mutex<State>,
    cv: Condvar,
    /// Each batch's live CI, for the landing view (records only).
    pub ci: crate::batch_ci::CiBook,
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

    /// The tasks a waiting batch is held for (still moving toward landing), as of the last look.
    pub fn collecting(&self, p: ProjectId) -> Vec<u64> {
        self.lock().collecting.get(&p).cloned().unwrap_or_default()
    }

    pub fn queued(&self, p: ProjectId) -> Vec<TaskId> {
        self.lock()
            .waiting
            .get(&p)
            .map(|v| v.iter().map(|w| w.task).collect())
            .unwrap_or_default()
    }

    /// Hold new batches back (`true`) or let them start again. Queued tasks keep their place.
    pub fn set_draining(&self, on: bool) {
        self.lock().draining = on;
        self.wake();
    }

    pub fn draining(&self) -> bool {
        self.lock().draining
    }

    /// The projects with a batch running now.
    pub fn busy_projects(&self) -> Vec<ProjectId> {
        let mut v: Vec<ProjectId> = self
            .lock()
            .running
            .iter()
            .filter(|(_, r)| !r.is_empty())
            .map(|(p, _)| *p)
            .collect();
        v.sort();
        v
    }

    /// Wait on the condvar (a finishing batch wakes it) for at most `tick`, or not at all if
    /// no project has a batch running. `true`: nothing is running now.
    pub fn wait_idle_once(&self, tick: Duration) -> bool {
        let idle = |g: &State| g.running.values().all(Vec::is_empty);
        let g = self.lock();
        if idle(&g) {
            return true;
        }
        let g = self
            .cv
            .wait_timeout(g, tick)
            .unwrap_or_else(|e| e.into_inner())
            .0;
        idle(&g)
    }

    /// Is this task a member of a batch that is running (and owes it a verdict)?
    pub fn in_batch(&self, t: TaskId) -> bool {
        self.lock().owed.contains(&t)
    }

    /// The integration slots of the project's running batches: their worktrees are in use.
    pub fn running_slots(&self, p: ProjectId) -> Vec<usize> {
        self.lock()
            .running
            .get(&p)
            .map_or_else(Vec::new, |r| r.iter().map(|x| x.slot).collect())
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

/// Why the landing configuration gave no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The file is there and says something wrong: landing is refused.
    Refused(String),
    /// Git failed, so nobody knows what the file says. No decision may be made.
    Unread(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Refused(s) => f.write_str(s),
            ConfigError::Unread(s) => write!(f, "could not read .mason/landing.toml: {s}"),
        }
    }
}

/// The text of `.mason/landing.toml` on the base. `Ok("")` only when the base has no such
/// file (the defaults); a failed git call is an error, never an empty file.
fn read_config_text(p: &crate::config::ProjectCfg) -> Result<String, String> {
    let base = p.base_ref();
    let listed = git::run(&p.path, &["ls-tree", &base, "--", ".mason/landing.toml"])
        .map_err(|e| e.to_string())?;
    if listed.trim().is_empty() {
        return Ok(String::new());
    }
    git::run(&p.path, &["show", &format!("{base}:.mason/landing.toml")]).map_err(|e| e.to_string())
}

/// `.mason/landing.toml` on the base branch; absent means the defaults.
pub fn load_config(d: &Daemon, pid: ProjectId) -> Result<LandingConfig, String> {
    read_config(d, pid).map_err(|e| e.to_string())
}

/// [`load_config`], telling a refused configuration from one that could not be read.
pub fn read_config(d: &Daemon, pid: ProjectId) -> Result<LandingConfig, ConfigError> {
    let p = d
        .project(pid)
        .ok_or_else(|| ConfigError::Refused("no such project".into()))?;
    let text = read_config_text(p).map_err(ConfigError::Unread)?;
    let c = config::parse(&text).map_err(|e| {
        ConfigError::Refused(format!("{} (.mason/landing.toml): {}", p.id, e.join("; ")))
    })?;
    let problems = config::service_problems(
        &c,
        p.remote.is_some(),
        p.services.host.is_some(),
        p.services.ci.is_some(),
    );
    if !problems.is_empty() {
        return Err(ConfigError::Refused(format!(
            "{}: {}",
            p.id,
            problems.join("; ")
        )));
    }
    // A suite's `whole_later` is the definition's claim that landing runs it whole (#676).
    let claims: Vec<(String, String)> = d
        .current_def(pid)
        .testing
        .suites
        .iter()
        .filter_map(|s| Some((s.name.clone(), s.whole_later?.name().to_string())))
        .collect();
    let problems = config::whole_later_problems(&c, &claims, p.services.ci.is_some());
    if !problems.is_empty() {
        return Err(ConfigError::Refused(format!(
            "{}: {}",
            p.id,
            problems.join("; ")
        )));
    }
    // `narrow = true` is only safe with a scheduled full test to catch what narrowing misses (s804).
    let problems =
        config::narrow_problems(&c, crate::schedule::has_full_test_trigger(&p.schedules()));
    if !problems.is_empty() {
        return Err(ConfigError::Refused(format!(
            "{}: {}",
            p.id,
            problems.join("; ")
        )));
    }
    if let Some(b) = &c.base {
        if *b != p.base {
            return Err(ConfigError::Refused(format!(
                "{}: landing.toml says base `{b}` but the project's base is `{}`",
                p.id, p.base
            )));
        }
    }
    Ok(c)
}

/// What the `batch` tier runs now (s804): `None` when it does not narrow; `Some(Narrow)` when the
/// scheduled full test stands behind it; `Some(Whole(why))` when the last full test is red, too
/// old with untested commits since, or missing, so batches run the whole suite.
pub fn batch_gate(
    d: &Daemon,
    p: &crate::config::ProjectCfg,
    cfg: &LandingConfig,
) -> Option<mason_core::full_test::Gate> {
    use mason_core::full_test as full;
    if !cfg.narrows() {
        return None;
    }
    let mut rows: Vec<full::Tested> = vec![];
    for spec in p.schedules().iter().filter(|s| s.ci.is_some()) {
        rows.extend(crate::ci_schedule::tested(
            &d.store.full_tests(&p.id, &spec.name).unwrap_or_default(),
        ));
    }
    rows.sort_by(|a, b| {
        b.finished_ms
            .max(b.started_ms)
            .cmp(&a.finished_ms.max(a.started_ms))
    });
    let now = now_ms();
    let max_age = cfg
        .full_test_max_age
        .map_or(full::DEFAULT_MAX_AGE_MS, |m| m.as_millis() as u64);
    let base = p.base_ref();
    Some(full::narrow_gate(
        &rows,
        now,
        max_age,
        &|sha| {
            git::run(&p.path, &["rev-list", "--count", &format!("{sha}..{base}")])
                .ok()
                .and_then(|o| o.trim().parse().ok())
        },
        &|ms| crate::ci_schedule::when_text(ms, now),
    ))
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
    if let Some(v) = check_markers(c) {
        return Some(v);
    }
    if let Some(v) = check_content(c) {
        return Some(v);
    }
    if let Some(v) = check_dropped(c) {
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
        head: mason_landing::git::rev_parse(&c.project.path, &c.branch)
            .unwrap_or_else(|_| c.branch.clone()),
        rank: rank_of(priority, c.task),
        ready_at: landing_clock(d).saturating_sub(first_submit_age_s(d, c)),
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

/// The landing queue's clock, in seconds. It starts far from zero so that a task first
/// submitted before this daemon started can have a `ready_at` before the daemon's start.
fn landing_clock(d: &Daemon) -> u64 {
    const BIAS_S: u64 = 1 << 32;
    BIAS_S + d.started.elapsed().as_secs()
}

/// How long ago the task first joined the queue for this landing run (0 for a first
/// submission, or when the store cannot say). A restart does not start its window over.
fn first_submit_age_s(d: &Daemon, c: &Ctx) -> u64 {
    let now = now_ms();
    let first = d
        .store
        .first_landing_submit(c.task.0, c.run.0, now)
        .unwrap_or(now);
    now.saturating_sub(first) / 1000
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
    // A base that cannot be read now is waited for, not taken as having no numbered files.
    let read = crate::jobs::wait_out_environment(c, "reading the base's numbered files", || {
        Ok((
            crate::numbers::added_files(&c.project.path, &c.project.base_ref(), &c.branch)?,
            crate::numbers::base_paths(&c.project)?,
        ))
    });
    let (added, base) = match read {
        Ok(v) => v,
        Err(why) => return Some(Verdict::Failed(why)),
    };
    let bad = crate::numbers::check_added(&added, &base, &mine);
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

/// The names of the tests a fast tier's output says failed, read with the suite's own parser
/// (`fail_regex` for the regex parser). Empty when the parser finds none.
fn fast_failures(suite: &mason_testing::Suite, output: &str) -> Vec<String> {
    mason_testing::parse::parse(suite.parser, output, suite)
        .map(|r| r.failed_names())
        .unwrap_or_default()
}

/// A task whose branch adds unresolved merge conflict markers goes back, with the files and
/// lines, before it joins the queue. On 2026-11-13 one landed in `landing.rs` because nothing
/// read the text: testing was skipped and a batch that no suite verifies builds nothing.
fn check_markers(c: &Ctx) -> Option<Verdict> {
    let read = crate::jobs::wait_out_environment(
        c,
        "scanning the task's change for conflict markers",
        || crate::markers::added_since(&c.project.path, &c.project.base_ref(), &c.branch),
    );
    match read {
        Err(why) => Some(Verdict::Failed(why)),
        Ok(found) if found.is_empty() => None,
        Ok(found) => Some(Verdict::SentBack {
            reason: format!(
                "adds unresolved merge conflict markers: {}",
                crate::markers::places(&found)
            ),
            feedback: crate::markers::refusal(&found),
        }),
    }
}

/// A task whose branch adds a secret or an owner-banned word goes back before it joins the
/// queue. A list the owner wrote that cannot be read stops the landing: it is not skipped.
fn check_content(c: &Ctx) -> Option<Verdict> {
    let rules = crate::contentscan::Source::new(&c.d.cfg.config_dir, &c.project.id).rules();
    if !rules.problems.is_empty() {
        return Some(Verdict::Failed(format!(
            "the banned-word list cannot be applied: {}",
            rules.problems.join("; ")
        )));
    }
    let read = crate::jobs::wait_out_environment(
        c,
        "scanning the task's change for secrets and banned words",
        || {
            crate::contentscan::added_since(
                &rules,
                &c.project.path,
                &c.project.base_ref(),
                &c.branch,
            )
        },
    );
    match read {
        Err(why) => Some(Verdict::Failed(why)),
        Ok(scan) if scan.hits.is_empty() => None,
        Ok(scan) => Some(Verdict::SentBack {
            reason: format!(
                "adds a secret or a banned word: {}",
                crate::contentscan::places(&scan.hits)
            ),
            feedback: crate::contentscan::refusal(&scan.hits),
        }),
    }
}

/// The lines a task's submission says it removes on purpose (`change.json`).
pub fn removals_in(dir: &Path) -> Vec<Removal> {
    let v: serde_json::Value = std::fs::read_to_string(dir.join("change.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    v["removes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    Some(Removal {
                        file: r["file"].as_str()?.to_string(),
                        line: r["line"].as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A page on the git host for a task's landing: the batch's pull request or a failed check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiRef {
    pub label: String,
    pub url: String,
    pub kind: &'static str,
}

/// Only an http(s) URL is ever sent to a client to open.
fn is_web_url(u: &str) -> bool {
    let l = u.to_ascii_lowercase();
    (l.starts_with("https://") || l.starts_with("http://"))
        && !u.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Remember `links` for the task (`ci_links.json` in its directory), replacing an earlier
/// link with the same label. Links that are not http(s) are dropped.
pub fn record_ci_links(dir: &Path, links: &[CiRef]) {
    let mut all = ci_links_of(dir);
    for l in links.iter().filter(|l| is_web_url(&l.url)) {
        all.retain(|o| o.label != l.label);
        all.push(l.clone());
    }
    let v: Vec<serde_json::Value> = all
        .iter()
        .map(|l| serde_json::json!({"label": l.label, "url": l.url, "kind": l.kind}))
        .collect();
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(
        dir.join("ci_links.json"),
        serde_json::Value::Array(v).to_string(),
    );
}

/// The links recorded for the task, oldest first.
pub fn ci_links_of(dir: &Path) -> Vec<CiRef> {
    let v: serde_json::Value = std::fs::read_to_string(dir.join("ci_links.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let url = e["url"].as_str()?;
            is_web_url(url).then(|| CiRef {
                label: e["label"].as_str().unwrap_or_default().to_string(),
                url: url.to_string(),
                kind: match e["kind"].as_str() {
                    Some("pull_request") => "pull_request",
                    Some("run") => "run",
                    _ => "check",
                },
            })
        })
        .collect()
}

/// The task's deliberate removals as lines for the task detail: empty when there are none.
pub fn removals_view(dir: &Path) -> String {
    let v: serde_json::Value = std::fs::read_to_string(dir.join("change.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let mut s = String::new();
    for r in v["removes"].as_array().into_iter().flatten() {
        s.push_str(&format!(
            "\nremoves on purpose: {}: {} ({})",
            r["file"].as_str().unwrap_or("?"),
            r["line"].as_str().unwrap_or("?").trim(),
            r["reason"].as_str().unwrap_or("no reason")
        ));
    }
    s
}

/// One run of a split batch's search: the tasks it held and what failed (s394).
type SplitRun = (Vec<u64>, Vec<String>);

/// A subset of a red batch, assembled in a worktree and ready to run (#702).
struct SplitJob {
    subset: Vec<u64>,
    a: mason_landing::batch::Assembled,
    run_key: String,
    run_filter: String,
    prefix: String,
    wt: PathBuf,
}

/// What preparing a subset came to: its answer already (`red`, with the notes of an earlier red
/// run), or a job to run.
enum Prep {
    Done(bool, Option<Vec<String>>),
    Run(Box<SplitJob>),
}

/// The lines the base landed that each merge of it into the task's branch lost, less what
/// the task declares under `removes` (decision 0117).
fn dropped_in_merges(c: &Ctx, removes: &[Removal]) -> Result<Vec<audit::Dropped>, String> {
    let repo = &c.project.path;
    let base_ref = c.project.base_ref();
    crate::jobs::wait_out_environment(c, "auditing the task's merges with the base", || {
        let revs = git::run(
            repo,
            &[
                "rev-list",
                "--merges",
                "--first-parent",
                "--parents",
                &format!("{base_ref}..{}", c.branch),
            ],
        )?;
        let mut found: Vec<audit::Dropped> = vec![];
        for line in revs.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let Some((&p1, others)) = parts.get(1..).and_then(<[&str]>::split_first) else {
                continue;
            };
            for b in others {
                if !git::is_ancestor(repo, b, &base_ref) {
                    continue;
                }
                let mb = git::run(repo, &["merge-base", p1, b])?;
                for d in audit::audit(repo, &mb, b, p1, Some(&base_ref), &c.branch, removes)? {
                    if !found.contains(&d) {
                        found.push(d);
                    }
                }
            }
        }
        Ok(found)
    })
}

/// Undeclared dropped lines that the task's merge replaced in place, as a block for the
/// review step to settle; `None` when there are none (or they cannot be read now: the
/// landing audit still sends a task back for them).
pub fn replaced_for_review(c: &Ctx) -> Option<String> {
    let repo = &c.project.path;
    let dropped = dropped_in_merges(c, &removals_in(&c.d.task_dir(c.task))).ok()?;
    if dropped.is_empty() {
        return None;
    }
    let mut pairs = vec![];
    for d in &dropped {
        // The merge's diff against the base the line came from is read per line's merge,
        // so a pair is taken against the base tip: that is what the task would land on.
        let one = audit::replacements(
            repo,
            &c.project.base_ref(),
            &c.branch,
            std::slice::from_ref(d),
        )
        .ok()?;
        pairs.extend(one);
    }
    (!pairs.is_empty()).then(|| audit::review_block(&pairs))
}

/// A merge of the base in the task's own branch must have kept every line the base had
/// landed (decision 0117). Each merge of a base commit is audited against the branch's head.
fn check_dropped(c: &Ctx) -> Option<Verdict> {
    let removes = removals_in(&c.d.task_dir(c.task));
    let dropped = match dropped_in_merges(c, &removes) {
        Ok(v) => v,
        Err(why) => return Some(Verdict::Failed(why)),
    };
    if dropped.is_empty() {
        return None;
    }
    let repo = &c.project.path;
    let mut pairs = vec![];
    for d in dropped.iter().take(audit::MAX_LISTED) {
        if let Ok(one) = audit::replacements(
            repo,
            &c.project.base_ref(),
            &c.branch,
            std::slice::from_ref(d),
        ) {
            pairs.extend(one);
        }
    }
    Some(Verdict::SentBack {
        reason: audit::reason(&dropped),
        feedback: audit::feedback_with(&dropped, &pairs),
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
            // The dead batch's tasks showed its last line: say they wait to land again.
            for b in d.store.batches(Some(p.id.as_str()), 50).unwrap_or_default() {
                if b.finished_at.is_none() {
                    let tasks: Vec<u64> = b
                        .tasks
                        .split(',')
                        .filter_map(|t| t.trim().parse().ok())
                        .collect();
                    let _ = d.store.set_progress_message(
                        &tasks,
                        &format!(
                            "waiting to land again (batch #{} stopped for an update; its results are kept)",
                            b.id
                        ),
                    );
                }
            }
            // The abandoned batches' results are thrown away: stop their step hosts, which
            // would otherwise run on for ever holding test worlds.
            for id in d.store.abandon_batches(&p.id, now_ms()).unwrap_or_default() {
                d.stop_hosts(&format!("b{id}-"));
            }
            let _ = d
                .store
                .prune_batch_runs(now_ms().saturating_sub(2 * 24 * 3600 * 1000));
            sweep_integration(&d, pid);
            remove_refs(&d, pid, None, None);
        }
    }
    release_finished_integration(&d, pid);
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

/// A finished batch's integration branch is disposable, but the one-shot cleanup after the
/// batch ignores a momentary git failure (an index.lock in the worktree). Retry it on every
/// pass: delete `mason/int/<project>/b<id>` for batches whose row is finished. A running
/// batch (`finished_at` None) is never touched.
fn release_finished_integration(d: &Arc<Daemon>, pid: ProjectId) {
    let Some(p) = d.project(pid) else { return };
    let finished: Vec<String> = d
        .store
        .batches(Some(p.id.as_str()), 50)
        .unwrap_or_default()
        .into_iter()
        .filter(|b| b.finished_at.is_some())
        .map(|b| format!("mason/int/{}/b{}", p.id, b.id))
        .collect();
    if finished.is_empty() {
        return;
    }
    let lock = d.repo_lock(pid);
    let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
    delete_branches(&p.path, &finished);
}

/// Delete each of `branches` that exists, detaching any worktree standing on it first. A git
/// failure is logged and the branch stays for the next pass.
fn delete_branches(repo: &Path, branches: &[String]) {
    let wts = git::worktrees(repo).unwrap_or_default();
    for branch in branches {
        let exists = git::run(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .is_ok();
        if !exists {
            continue;
        }
        for (path, b) in &wts {
            if b.as_deref() == Some(branch.as_str()) {
                if let Err(e) = git::run(path, &["checkout", "--detach", "--quiet"]) {
                    tracing::warn!("detaching {} from {branch}: {e}", path.display());
                }
            }
        }
        if let Err(e) = git::run(repo, &["branch", "-D", branch]) {
            tracing::warn!("deleting {branch}: {e}");
        }
    }
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

/// How long a configuration may stay unreadable before the owner is told.
const UNREAD_ALERT_AFTER: Duration = Duration::from_secs(300);

/// Record that the landing configuration was read (`None`) or not (`Some(why)`); an alert goes
/// up once it has stayed unread for `after`, and goes away when it is read again.
fn note_read(d: &Daemon, pid: ProjectId, unread: Option<&str>, after: Duration) {
    let key = "config-unread";
    match unread {
        None => {
            if d.landing.lock().unread_since.remove(&pid).is_some() {
                d.landing.clear(pid, key);
            }
        }
        Some(why) => {
            let since = *d
                .landing
                .lock()
                .unread_since
                .entry(pid)
                .or_insert_with(Instant::now);
            if since.elapsed() >= after {
                d.landing.raise(
                    pid,
                    key,
                    "landing-config-unread",
                    &format!(
                        "landing is held: the configuration could not be read for {} s ({why})",
                        since.elapsed().as_secs()
                    ),
                );
            }
        }
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
        let running = g.running.get(&pid).is_some_and(|v| !v.is_empty());
        g.note_idle(pid, !waiting.is_empty(), running, elapsed);
    }
    if waiting.is_empty() {
        return (0, None);
    }
    if d.landing.draining() {
        // A self-update is waiting for the running batches: nothing new starts, and the
        // queue is left as it is. `set_draining(false)` wakes this again. A deliberate hold
        // is not idleness.
        d.landing.lock().idle_ms.remove(&pid);
        return (waiting.len(), Some(Duration::from_secs(1)));
    }
    let cfg = match read_config(d, pid) {
        Ok(c) => {
            note_read(d, pid, None, UNREAD_ALERT_AFTER);
            c
        }
        Err(ConfigError::Unread(e)) => {
            // Git failed: nobody knows what the configuration says, so nothing starts and
            // nothing is failed. The queue stays as it is; look again shortly.
            note_read(d, pid, Some(&e), UNREAD_ALERT_AFTER);
            // Held and alerted on its own: not idleness.
            d.landing.lock().idle_ms.remove(&pid);
            return (waiting.len(), Some(Duration::from_secs(2)));
        }
        Err(ConfigError::Refused(e)) => {
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
            Some("force") => singles.push((w.clone(), "force".to_string())),
            // Land now (also when pressed while the task was in a batch that did not land it)
            // gets a batch of its own without waiting for the window, but not past a freeze.
            Some("now" | crate::suites::LAND_NOW_RUNNING)
                if freeze.as_ref().is_none_or(|f| f.fix_task == Some(w.task.0)) =>
            {
                singles.push((w.clone(), "now".to_string()))
            }
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
    if normal.is_empty() && singles.is_empty() {
        // Every waiting task is held behind a freeze: a deliberate hold, not idleness.
        d.landing.lock().idle_ms.remove(&pid);
    }
    for (w, kind) in singles {
        take(d, pid, w.task);
        start(d, pid, vec![w], cfg.clone(), &kind);
    }
    let now = landing_clock(d);
    let moving = moving_tasks(d, pid);
    // `moving.len()` is what the plan sees; the setting makes the window always count.
    let others = if d.cfg.landing_window_always {
        moving.len().max(1)
    } else {
        moving.len()
    };
    let mut next_wait = None;
    let mut guard = 0;
    loop {
        guard += 1;
        let max_size = if cfg.mode == config::Mode::PrEach {
            1
        } else {
            cfg.max_size
        };
        match plan::form_batch(&normal, now, cfg.window, max_size, others) {
            Formation::Empty => break,
            Formation::Wait(t) => {
                // Look again soon: a task that stops moving has no event to wake this.
                next_wait = Some(t.min(Duration::from_secs(1)));
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
    {
        let mut g = d.landing.lock();
        if next_wait.is_some() {
            g.collecting.insert(pid, moving);
        } else {
            g.collecting.remove(&pid);
        }
    }
    let left = d.landing.lock().waiting.get(&pid).map_or(0, Vec::len);
    (left, next_wait)
}

/// The project's tasks that are still on their way to landing: in a step that can lead there,
/// not held behind a blocker, paused, failed, proposed or deferred, and neither in the landing
/// queue nor in a running batch. A batch waits out its window only while there are any.
fn moving_tasks(d: &Daemon, pid: ProjectId) -> Vec<u64> {
    let Ok(w) = d.world() else {
        return vec![]; // nothing known to wait for
    };
    let g = d.landing.lock();
    let queued: HashSet<TaskId> = g
        .waiting
        .get(&pid)
        .map(|v| v.iter().map(|x| x.task).collect())
        .unwrap_or_default();
    let mut v: Vec<u64> = w
        .tasks
        .values()
        .filter(|t| t.project == pid && !t.paused)
        .filter(|t| !queued.contains(&t.id) && !g.owed.contains(&t.id))
        .filter(|t| match t.state {
            mason_core::State::Exploring
            | mason_core::State::Developing
            | mason_core::State::Testing
            | mason_core::State::Review
            | mason_core::State::Landing => true,
            mason_core::State::Queued => t.blockers.is_empty(),
            _ => false,
        })
        .map(|t| t.id.0)
        .collect();
    v.sort_unstable();
    v
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
        if g.draining {
            // A drain began after the look at the queue: put the tasks back, start nothing.
            let q = g.waiting.entry(pid).or_default();
            for w in members {
                if !q.iter().any(|x| x.task == w.task) {
                    q.push(w);
                }
            }
            return;
        }
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
            // Its CI and pull-request polls are the landing's, not another caller's.
            let _caller = mason_adapter_api::github_budget::as_caller("landing");
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
            let member_tasks: Vec<TaskId> = members.iter().map(|w| w.task).collect();
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
                    waits: members.clone(),
                    member_tasks,
                    runs: Default::default(),
                    batch_id: Default::default(),
                    ci,
                    real_ci_red: std::sync::atomic::AtomicBool::new(false),
                    fast_tier: std::sync::atomic::AtomicBool::new(false),
                    floor_red: std::sync::atomic::AtomicBool::new(false),
                    reused: std::sync::atomic::AtomicUsize::new(0),
                    yielded: std::sync::atomic::AtomicBool::new(false),
                    lint_files: Mutex::new(vec![]),
                    lint_where: Mutex::new(String::new()),
                    host,
                    pr: Mutex::new(None),
                    merged_ref: Mutex::new(None),
                    service_error: Mutex::new(None),
                    stopped: Default::default(),
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

/// Cancel the CI runs of a ref MASON stops waiting on, before it is deleted (task 1108).
/// Best effort: a failure is only logged and changes no verdict or error.
pub(crate) fn cancel_runs(ci: &dyn BuildSystemAdapter, run: &str, git_ref: &str) -> usize {
    ci.cancel(run, git_ref).unwrap_or_else(|e| {
        tracing::warn!("CI on {git_ref} could not be cancelled: {}", e.detail);
        0
    })
}

/// What a batch's note says about runs cancelled on `name`; nothing when none were.
fn cancelled_note(name: &str, why: &str, n: usize) -> Option<String> {
    (n > 0).then(|| {
        format!(
            "CI on {name}: not waited for ({why}); {}",
            runs_cancelled(n)
        )
    })
}

fn runs_cancelled(n: usize) -> String {
    format!("{n} run{} cancelled", if n == 1 { "" } else { "s" })
}

/// Close the pull requests and delete the candidate refs a batch (or, with
/// `None`, every batch a dead daemon left) put on the remote. Their unfinished CI runs
/// are cancelled first, except those of `merged`, the ref of a pull request that landed.
fn remove_refs(d: &Arc<Daemon>, pid: ProjectId, batch: Option<u64>, merged: Option<u64>) {
    let Some(p) = d.project(pid) else { return };
    let host = crate::services::host(p).ok().flatten();
    let ci = crate::services::ci(p).ok().flatten();
    for r in d.store.open_refs(&p.id).unwrap_or_default() {
        if batch.is_some_and(|b| b != r.batch) {
            continue;
        }
        if let (Some(h), Some(n), "pr") = (&host, r.number, r.kind.as_str()) {
            let _ = h.close_pr(n);
        }
        if let Some((ci, _)) = ci.as_ref().filter(|_| merged != Some(r.id)) {
            // The ref row has no commit: the runs are found by the branch.
            let n = cancel_runs(ci.as_ref(), "", &r.name);
            let why = if batch.is_some() {
                "the batch ended"
            } else {
                "the daemon restarted"
            };
            if let Some(note) = cancelled_note(&r.name, why, n) {
                let _ = d.store.append_batch_note(r.batch, &note);
            }
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
    /// The members as they waited in the queue: a task deferred to the next batch is
    /// put back as it was, so it keeps its place.
    waits: Vec<Wait>,
    /// The batch's tasks, first by rank: the first ranks the batch's broker leases.
    member_tasks: Vec<TaskId>,
    /// Makes each suite run's host id its own: a host id names one run.
    runs: std::sync::atomic::AtomicU64,
    batch_id: std::sync::atomic::AtomicU64,
    ci: Option<crate::services::Ci>,
    /// Set by the last CI or PR verdict when a red job was judged a real failure (the
    /// judge is on): the red ladder then sends the batch back without a rerun.
    real_ci_red: std::sync::atomic::AtomicBool,
    /// The batch being verified had every task's testing skipped: `verify` runs the suites' fast
    /// unit tier after the build floor (s509).
    fast_tier: std::sync::atomic::AtomicBool,
    /// Set when the last verification's red came from the build floor (`build_floor`): the
    /// ladder reruns that as a build, and looks at the base before it blames anyone.
    floor_red: std::sync::atomic::AtomicBool,
    /// Run results taken from an earlier batch over the same base and task heads (s705).
    reused: std::sync::atomic::AtomicUsize,
    /// Set when a split search stopped at a run boundary because an update is taking over.
    yielded: std::sync::atomic::AtomicBool,
    /// The files the last lint red named (rustfmt's `Diff in`, clippy's `-->`): the ladder
    /// parks the tasks whose diff touches one, before it reruns or splits anything (s691).
    lint_files: Mutex<Vec<String>>,
    /// The `file:line` places of the last red lint, as ` (at a.rs:1)`: a task parked for a
    /// lint red is told where to look.
    lint_where: Mutex<String>,
    host: Option<Arc<dyn GitHostAdapter>>,
    /// The pull request the batch is about to merge (pr modes).
    pr: Mutex<Option<OpenPr>>,
    /// The candidate ref of the pull request that landed: its runs are not cancelled.
    merged_ref: Mutex<Option<u64>>,
    /// Set when a service could not answer: the batch ends with this reason.
    service_error: Mutex<Option<String>>,
    /// What Land now stopped in this batch's verification (`suite:NAME`, `ci:NAME`): the
    /// members owe it as deferred debts once they land.
    stopped: Arc<Mutex<BTreeSet<String>>>,
}

#[derive(Clone, Debug)]
struct OpenPr {
    number: u64,
    branch: String,
    ref_id: u64,
}

/// What a split red batch came to.
enum Settled {
    /// Tasks sent back; the batch's note.
    Parked(String),
    /// A base red: why nothing lands.
    Base(String),
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
                s.jobs = c.jobs.as_ref().map(breakdown);
            });
        }
        if let Some(j) = &c.jobs {
            self.say(&format!("checks: {}", jobs_summary(j)));
        }
    }

    /// A CI's jobs, one sub-count per workflow (jobs finished of jobs listed, told apart by
    /// state) and a message with the sum. Reporting only; no verdict reads it.
    fn ci_jobs(&self, what: &str, detail: &[CiRunJobs]) {
        if detail.is_empty() {
            return;
        }
        let mut sum = CiJobCounts::default();
        for w in detail {
            sum.merge(&w.jobs);
            for run in &self.member_runs {
                self.d.progress_sub(*run, &w.workflow, None, |s| {
                    s.unit = "jobs".into();
                    s.done = Figure::reported(w.jobs.succeeded + w.jobs.failed);
                    s.total = Figure::reported(w.jobs.total);
                    s.jobs = Some(breakdown(&w.jobs));
                });
            }
        }
        if sum.total > 0 {
            self.say(&format!("CI on {what}: {}", jobs_summary(&sum)));
        }
    }

    /// A poll got an answer and the work is still going: restart the quiet clock.
    fn alive(&self, poll_s: u64) {
        for run in &self.member_runs {
            self.d
                .progress_report(*run, |p, now| p.heartbeat(now, poll_s.saturating_mul(1000)));
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
        crate::contentscan::note_allowed(
            &crate::contentscan::Source::new(&d.cfg.config_dir, &p.id).rules(),
            &p.path,
            &p.base_ref(),
            tasks.iter().map(|(t, b)| (t.0, b.as_str())),
            &log_dir,
        );

        let mut active: Vec<BatchTask> = tasks
            .iter()
            .map(|(t, b)| BatchTask {
                id: t.0,
                branch: b.clone(),
                head: self
                    .waits
                    .iter()
                    .find(|w| w.task == *t)
                    .map_or_else(|| b.clone(), |w| w.head.clone()),
                removes: removals_in(&d.task_dir(*t)),
            })
            .collect();
        let mut parked: Vec<Parked> = vec![];
        // The head each task had when the union was last sent to verification.
        let mut tested: HashMap<u64, String>;
        // Tasks that conflict only with batch-mates: they stay in landing and wait for
        // the next batch (decision 0162).
        let mut deferred: Vec<u64> = vec![];
        // A parked task goes back to development at once, not when the batch ends: a
        // batch may wait hours for a review it needs.
        let mut delivered: HashSet<u64> = HashSet::new();
        let mut landed: Vec<u64> = vec![];
        let mut rounds = 0usize;
        let limit = self.cfg.max_fold as usize + ids.len() + 3;
        let mut note = String::new();
        let mut split_note = String::new();
        let mut commit = String::new();
        let force = self.kind == "force";
        // How long to wait before assembling again when the remote cannot be fetched.
        let mut fetch_wait = Duration::from_secs(1);

        // An update is waiting to take over and the batch is between runs: it stops here,
        // its results kept, and its tasks wait in the queue to land again (s705).
        macro_rules! yield_batch {
            ($l:lifetime) => {{
                for t in &active {
                    if !deferred.contains(&t.id) {
                        deferred.push(t.id);
                    }
                }
                note = "stopped for an update; its run results are kept for the next batch over the same base and task heads".to_string();
                self.say(&format!(
                    "waiting to land again (batch #{batch_id} stopped for an update; its results are kept)"
                ));
                break $l Ok(());
            }};
        }
        let outcome: Result<(), String> = 'outer: loop {
            for pk in parked.iter().filter(|x| delivered.insert(x.task)) {
                deliver(d, TaskId(pk.task), pk.verdict());
            }
            rounds += 1;
            let _ = self.d.store.set_batch_round(batch_id, rounds as u64);
            self.stopped
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
            if rounds > limit {
                break Err(format!("gave up after {rounds} rounds: the base kept moving or the batch kept changing"));
            }
            // A member whose run ended (retried, canceled) leaves the batch: neither landed
            // nor parked. The union then differs from what the last run tested, and the
            // round verifies it again.
            let gone: Vec<String> = active
                .iter()
                .filter(|t| self.left(t.id))
                .map(|t| format!("#{}", t.id))
                .collect();
            if !gone.is_empty() {
                active.retain(|t| !self.left(t.id));
                note = format!("{} left the batch: retried or canceled", gone.join(", "));
                self.say(&format!("batch #{batch_id}: {note}"));
                if active.is_empty() {
                    break Ok(());
                }
            }
            if rounds > 1 && self.yield_now() {
                yield_batch!('outer);
            }
            // -- assemble
            let asm = {
                let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
                batch.assemble(&active, &point)
            };
            let asm = match asm {
                Ok(a) => a,
                // A fetch that fails decides nothing (decision 0105): the batch waits for the
                // remote and assembles again, and its tasks stay in landing meanwhile. Waiting
                // is not a round.
                Err(e) if e.args.first().map(String::as_str) == Some("fetch") => {
                    tracing::warn!(
                        batch = batch_id,
                        "landing waits for the remote: {e}; trying again in {} s",
                        fetch_wait.as_secs()
                    );
                    rounds -= 1;
                    std::thread::sleep(fetch_wait);
                    fetch_wait = (fetch_wait * 2).min(Duration::from_secs(30));
                    continue 'outer;
                }
                Err(e) => break Err(format!("could not assemble the batch: {e}")),
            };
            // The integration worktree is MASON's own checkout: it gets the definition's
            // worktree setup (s312) after every assembly, as a task's worktree did. A setup
            // that fails is logged, not fatal: landing went without it before.
            if let Err(e) = crate::jobs::setup_checkout(&def, &int_wt) {
                tracing::warn!(batch = batch_id, "{e}");
            }
            for t in &asm.already {
                landed.push(*t);
            }
            for e in &asm.ejected {
                parked.push(Parked {
                    task: e.id,
                    reason: format!("conflicts with the rest of its batch in {}", e.files),
                    feedback: format!(
                        "Your change conflicts with other changes landing at the same time (batch #{batch_id}) in: {}. Bring in the latest base branch and resolve it.{}",
                        e.files,
                        if e.base_changes.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "\n\nThe base changed these files since you branched:\n{}",
                                e.base_changes
                            )
                        }
                    ),
                    conflict: true,
                });
            }
            for df in &asm.deferred {
                if !deferred.contains(&df.id) {
                    deferred.push(df.id);
                }
                let mates = df
                    .with
                    .iter()
                    .map(|m| format!("#{m}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                if let Some(w) = self.waits.iter().find(|w| w.task.0 == df.id) {
                    self.d.progress_report(w.run, |p, now| {
                        p.set_message(
                            &format!(
                                "waits: conflicts with {mates} in batch #{batch_id} ({})",
                                df.files
                            ),
                            now,
                        )
                    });
                }
            }
            for u in &asm.unkept {
                parked.push(Parked {
                    task: u.id,
                    reason: audit::reason(&u.lines),
                    feedback: audit::feedback(&u.lines),
                    conflict: false,
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

            // Every task in the batch had its test step skipped by the owner: when the base
            // moves, the batch is rebuilt but not re-tested (s429). Its tests were never run
            // for it, so the re-test was the only full run, and it held the queue for half
            // an hour each time anyone pushed to the base during a backlog.
            let tests_skipped = def
                .steps
                .iter()
                .find(|s| s.kind == mason_definition::StepType::Test)
                .is_some_and(|st| {
                    !active.is_empty()
                        && active.iter().all(|t| {
                            crate::suites::skip_for(&self.d, TaskId(t.id), "", &st.name).is_some()
                        })
                });
            // Every task in the batch was made immune to testing by the owner (a fast track:
            // its testing happens later, as its debt says). No test of any tier runs for the
            // batch; the lint tier and the build floor still do, so what lands builds.
            let all_immune = !active.is_empty()
                && active
                    .iter()
                    .all(|t| crate::suites::immune(&self.d, TaskId(t.id)).is_some());
            // The same batches get the fast unit tier after the build floor (s509), in this
            // verification and in every rerun of it; an immune batch does not.
            self.fast_tier.store(
                tests_skipped && !all_immune,
                std::sync::atomic::Ordering::SeqCst,
            );

            tested = active.iter().map(|t| (t.id, t.head.clone())).collect();
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
            self.real_ci_red
                .store(false, std::sync::atomic::Ordering::SeqCst);
            self.floor_red
                .store(false, std::sync::atomic::Ordering::SeqCst);
            let run_key = mason_store::batch_run_key(
                &active
                    .iter()
                    .map(|t| (t.id, t.head.clone()))
                    .collect::<Vec<_>>(),
            );
            let mut vr = self.verify_kept(
                &def,
                &suites,
                &int_wt,
                &asm.base_tip,
                &log_dir,
                &format!("b{batch_id}-r{rounds}"),
                &run_key,
                &format!(
                    "full|{}|fast={}|narrow={}",
                    suites.join(","),
                    self.fast_tier.load(std::sync::atomic::Ordering::SeqCst),
                    self.narrow_tag()
                ),
            );
            if let Some(e) = self.take_service_error() {
                break Err(e);
            }
            if vr.green && !vr.notes.is_empty() {
                note = vr.notes.join("; ");
            }
            // What splitting a red found stays in the closing note through later rounds.
            if !split_note.is_empty() && !note.contains(&split_note) {
                note = if note.is_empty() {
                    split_note.clone()
                } else {
                    format!("{split_note}; {note}")
                };
            }
            if !vr.green {
                self.point("batch_red");
                self.record(
                    batch_id,
                    "red",
                    &landed,
                    &parked,
                    &format!(
                        "{}{}",
                        vr.failed.iter().cloned().collect::<Vec<_>>().join(", "),
                        if vr.failing.is_empty()
                            && !vr.unfinished.is_empty()
                            && vr.failed.iter().all(|n| vr.unfinished.contains_key(n))
                        {
                            format!(" ({TIMED_OUT_ONCE})")
                        } else {
                            String::new()
                        }
                    ),
                    &asm.head,
                    false,
                );
                // The red ladder.
                let mut resolved = false;
                // A build that fails may be the base's own. A base that does not build makes
                // every batch red, and the ladder would blame the first task of each one in
                // turn: look at the base alone first. If it does not build, nobody is blamed,
                // nothing lands, and the owner is told.
                let floor = self.floor_red.load(std::sync::atomic::Ordering::SeqCst);
                if floor {
                    let failed: Vec<String> = vr.failed.iter().cloned().collect();
                    match self.base_builds(
                        &def,
                        &failed,
                        &int_wt,
                        &asm.base_tip,
                        &asm.head,
                        &log_dir,
                        &format!("b{batch_id}-r{rounds}-base"),
                    ) {
                        Ok(Some(why)) => {
                            let why = format!("batch #{batch_id}: {why}");
                            self.d.landing.raise(
                                self.pid,
                                "redbase",
                                "landing",
                                &format!("{}: {why}", self.project().id),
                            );
                            self.d.poke();
                            break 'outer Err(why);
                        }
                        Ok(None) => {}
                        Err(e) => break 'outer Err(e),
                    }
                }
                // s213: a red confined to CI jobs that map to local suites is searched for
                // locally. CI still has the last word: the next round verifies the remaining
                // union through it once.
                if !self.real_ci_red.load(std::sync::atomic::Ordering::SeqCst) {
                    if let Some(mapped) = self.local_suites(&def, &vr.failed) {
                        let jobs: Vec<String> = vr.failed.iter().cloned().collect();
                        let jobs_list = jobs.join(", ");
                        let suites_list = mapped.join(", ");
                        let base = self.project().base.clone();
                        // The base's own CI first: a red it has is not this batch's to find.
                        if let Ok((gh, _)) =
                            crate::ci_watch::source(self.project(), "the landing CI comparison")
                        {
                            let held = crate::ci_watch::base_red_jobs(&gh, &base, &jobs);
                            if !held.is_empty() {
                                break 'outer Err(format!(
                                    "{} red on {base} too: nobody is blamed and the batch is held under the freeze, not bisected",
                                    held.join(", ")
                                ));
                            }
                        }
                        let whole = self.run_local(
                            &def,
                            &mapped,
                            &int_wt,
                            &asm.base_tip,
                            &log_dir,
                            &format!("b{batch_id}-r{rounds}-loc"),
                        );
                        if let Some(e) = self.take_service_error() {
                            break 'outer Err(e);
                        }
                        if whole.green {
                            let why = format!(
                                "batch #{batch_id}: CI red in {jobs_list}, but {suites_list} is green here: environment difference; nobody is blamed and the batch waits for the owner"
                            );
                            self.d.landing.raise(
                                self.pid,
                                "redbase",
                                "landing",
                                &format!("{}: {why}", self.project().id),
                            );
                            self.d.poke();
                            break 'outer Err(why);
                        }
                        if active.len() == 1 {
                            let id = active[0].id;
                            self.park(&mut parked, &mut active, &[id], batch_id, &vr);
                            continue 'outer;
                        }
                        let (found, runs) = self.pin_red(
                            &def,
                            &batch,
                            &lock,
                            &active,
                            &int_wt,
                            &log_dir,
                            batch_id,
                            rounds,
                            Some(&mapped),
                            &mapped,
                            None,
                        );
                        if let Some(e) = self.take_service_error() {
                            break 'outer Err(e);
                        }
                        if self.yield_now() {
                            yield_batch!('outer);
                        }
                        match self.settle_pin(found, &runs, &mut parked, &mut active, batch_id, &vr)
                        {
                            Settled::Parked(n) => {
                                split_note = n.clone();
                                note = n;
                                continue 'outer;
                            }
                            Settled::Base(why) => {
                                self.raise_base(&why);
                                break 'outer Err(why);
                            }
                        }
                    }
                }
                // A run killed at its time limit, with no failing test named, is not a red to
                // search (s702): load can slow it past any limit and a subset would only finish
                // sooner. It is rerun once whole, and held when it times out again.
                let mut timeout_only = !floor
                    && vr.failing.is_empty()
                    && !vr.unfinished.is_empty()
                    && vr.failed.iter().all(|n| vr.unfinished.contains_key(n));
                // A lint red (or a red only in the fmt/SPDX gate tests) names its files:
                // the tasks whose diff touches one are sent back, the rest stay (s691).
                if let Some(files) = self.lint_blame_files(
                    &def,
                    &vr,
                    &int_wt,
                    &log_dir,
                    &format!("b{batch_id}-r{rounds}-lintblame"),
                    floor,
                ) {
                    let blamed = blame_by_files(&files, &active, &self.fps);
                    if !blamed.is_empty() {
                        self.park(&mut parked, &mut active, &blamed, batch_id, &vr);
                        continue 'outer;
                    }
                }
                if self.yield_now() {
                    yield_batch!('outer);
                }
                for rung in &self.cfg.ladder {
                    // A red the CI judged a real failure is not rerun, blamed or bisected
                    // by guesswork: the tasks hear of it now.
                    if self.real_ci_red.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                    match rung {
                        Rung::RerunFailed if timeout_only => {
                            let again: Vec<String> = vr.failed.iter().cloned().collect();
                            let first = vr.unfinished.clone();
                            self.say(&format!(
                                "batch #{batch_id} timed out in {}: rerunning it whole once",
                                again.join(", ")
                            ));
                            // The rerun takes the `test-suite` slot again, and taking a slot
                            // waits out the cpu and thermal gates: it starts when the load has
                            // dropped.
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
                                    "{} {TIMED_OUT_ONCE} and was green on a rerun",
                                    again.join(", ")
                                );
                                vr.green = true;
                                resolved = true;
                                break;
                            }
                            let timed_out_again = r2.failing.is_empty()
                                && !r2.unfinished.is_empty()
                                && r2.failed.iter().all(|n| r2.unfinished.contains_key(n));
                            if !timed_out_again {
                                // A different red: the ladder goes on with that one.
                                vr.failed = r2.failed;
                                vr.failing = r2.failing;
                                vr.unfinished = r2.unfinished;
                                vr.notes = r2.notes;
                                timeout_only = false;
                                continue;
                            }
                            // The tests still running at both kills.
                            let mut same: std::collections::BTreeMap<String, Vec<String>> =
                                Default::default();
                            for (suite, now) in &r2.unfinished {
                                let both: Vec<String> = first
                                    .get(suite)
                                    .into_iter()
                                    .flatten()
                                    .filter(|t| now.contains(t))
                                    .cloned()
                                    .collect();
                                if both.is_empty()
                                    || def.testing.suite(suite).is_none_or(|s| s.rerun.is_none())
                                {
                                    same.clear();
                                    break;
                                }
                                same.insert(suite.clone(), both);
                            }
                            if same.is_empty() {
                                let running = r2
                                    .unfinished
                                    .iter()
                                    .map(|(s, t)| {
                                        if t.is_empty() {
                                            format!("{s}: none read")
                                        } else {
                                            format!("{s}: {}", t.join(", "))
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join("; ");
                                let why = format!(
                                    "batch #{batch_id} {TIMED_OUT_TWICE} in {}; no test was still running at both kills, so nobody is blamed and the batch is held. Still running at the second kill: {running}",
                                    again.join(", ")
                                );
                                self.d.landing.raise(
                                    self.pid,
                                    "timeout",
                                    "landing",
                                    &format!("{}: {why}", self.project().id),
                                );
                                self.d.poke();
                                break 'outer Err(why);
                            }
                            if active.len() == 1 {
                                let id = active[0].id;
                                self.park(&mut parked, &mut active, &[id], batch_id, &vr);
                                continue 'outer;
                            }
                            let n: usize = same.values().map(Vec::len).sum();
                            self.say(&format!(
                                "batch #{batch_id} {TIMED_OUT_TWICE} with the same {n} test(s) running: splitting it on those alone"
                            ));
                            let (found, runs) = self.pin_red(
                                &def,
                                &batch,
                                &lock,
                                &active,
                                &int_wt,
                                &log_dir,
                                batch_id,
                                rounds,
                                None,
                                &vr.failed.iter().cloned().collect::<Vec<_>>(),
                                Some(&same),
                            );
                            if let Some(e) = self.take_service_error() {
                                break 'outer Err(e);
                            }
                            match self.settle_pin(
                                found,
                                &runs,
                                &mut parked,
                                &mut active,
                                batch_id,
                                &vr,
                            ) {
                                Settled::Parked(n) => {
                                    split_note = format!("{n} ({TIMED_OUT_TWICE}; bisected on the hanging test(s) alone)");
                                    note = split_note.clone();
                                    continue 'outer;
                                }
                                Settled::Base(why) => {
                                    self.raise_base(&why);
                                    break 'outer Err(why);
                                }
                            }
                        }
                        Rung::BlameByImpact | Rung::Bisect if timeout_only => {}
                        Rung::RerunFailed => {
                            let again: Vec<String> = vr.failed.iter().cloned().collect();
                            // A red floor is rerun as the floor, a build, not as the whole
                            // runs of the suites it names: no names, no suites, the floor.
                            let rerun = if floor { vec![] } else { again.clone() };
                            // The red run named its failing tests: rerun just those first,
                            // one at a time, so a red that came from load shows as green. Still
                            // red is a product red, and the ladder goes on to the search. Green
                            // is confirmed by the whole rerun below: nothing lands on a
                            // filtered run alone.
                            if !floor && crate::suites::can_run_alone(&def, &vr.failed, &vr.failing)
                            {
                                let n: usize = vr.failing.values().map(Vec::len).sum();
                                self.say(&format!(
                                    "batch #{batch_id} is red: rerunning the {n} failing test(s) alone"
                                ));
                                let r1 = self.run_alone(
                                    &def,
                                    &vr.failing,
                                    &int_wt,
                                    &log_dir,
                                    &format!("b{batch_id}-r{rounds}xn"),
                                    false,
                                    true,
                                );
                                if let Some(e) = self.take_service_error() {
                                    break 'outer Err(e);
                                }
                                if !r1.green {
                                    continue;
                                }
                            }
                            self.real_ci_red
                                .store(false, std::sync::atomic::Ordering::SeqCst);
                            self.floor_red
                                .store(false, std::sync::atomic::Ordering::SeqCst);
                            let r2 = self.verify_kept(
                                &def,
                                &rerun,
                                &int_wt,
                                &asm.base_tip,
                                &log_dir,
                                &format!("b{batch_id}-r{rounds}x"),
                                &run_key,
                                &format!("rerun|{}", rerun.join(",")),
                            );
                            if let Some(e) = self.take_service_error() {
                                break 'outer Err(e);
                            }
                            if r2.green {
                                note = format!(
                                    "{} was red once and green on a rerun",
                                    again.join(", ")
                                );
                                // Which jobs were infrastructure, for the batch note.
                                let judged: Vec<&String> =
                                    vr.notes.iter().filter(|n| n.ends_with(", rerun")).collect();
                                if !judged.is_empty() {
                                    note = format!(
                                        "{note} ({})",
                                        judged
                                            .iter()
                                            .map(|n| n.as_str())
                                            .collect::<Vec<_>>()
                                            .join("; ")
                                    );
                                }
                                vr.green = true;
                                resolved = true;
                                break;
                            }
                            // The rerun may itself be red for a real reason.
                            if self.real_ci_red.load(std::sync::atomic::Ordering::SeqCst) {
                                vr.notes.extend(r2.notes.clone());
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
                            // The failing tests the red run named, when every red suite
                            // has some and a `rerun` template: the search runs just those.
                            let alone = (!floor
                                && crate::suites::can_run_alone(&def, &vr.failed, &vr.failing))
                            .then_some(&vr.failing);
                            let (found, runs) = self.pin_red(
                                &def,
                                &batch,
                                &lock,
                                &active,
                                &int_wt,
                                &log_dir,
                                batch_id,
                                rounds,
                                None,
                                &vr.failed.iter().cloned().collect::<Vec<_>>(),
                                alone,
                            );
                            if let Some(e) = self.take_service_error() {
                                break 'outer Err(e);
                            }
                            if self.yield_now() {
                                yield_batch!('outer);
                            }
                            match self.settle_pin(
                                found,
                                &runs,
                                &mut parked,
                                &mut active,
                                batch_id,
                                &vr,
                            ) {
                                Settled::Parked(mut n) => {
                                    if let Some(a) = alone {
                                        let k: usize = a.values().map(Vec::len).sum();
                                        n = format!("{n} (bisected on {k} failing test(s))");
                                    }
                                    split_note = n.clone();
                                    note = n;
                                    continue 'outer;
                                }
                                Settled::Base(why) => {
                                    self.raise_base(&why);
                                    break 'outer Err(why);
                                }
                            }
                        }
                        Rung::Ask => break,
                    }
                }
                if !resolved {
                    // Asking: the owner is told by an alert; nothing lands, and the batch's
                    // tasks go back with the evidence rather than waiting on a guess.
                    let all: Vec<u64> = active.iter().map(|t| t.id).collect();
                    let failed_list = vr.failed.iter().cloned().collect::<Vec<_>>().join(", ");
                    let why = if self.real_ci_red.load(std::sync::atomic::Ordering::SeqCst) {
                        format!(
                            "batch #{batch_id} is red in {failed_list}: CI reports a real failure ({}), not rerun",
                            vr.notes
                                .iter()
                                .filter(|n| n.ends_with("real failure, not rerun"))
                                .cloned()
                                .collect::<Vec<_>>()
                                .join("; ")
                        )
                    } else {
                        format!(
                            "batch #{batch_id} is red in {failed_list} and the red cannot be pinned on one task"
                        )
                    };
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
            // A union that builds shows the base builds: an alert about it is over.
            self.d.landing.clear(self.pid, "redbase");
            let _ = self.d.store.clear_freeze(&self.project().id);

            // -- promote
            {
                let gone: Vec<String> = active
                    .iter()
                    .filter(|t| self.left(t.id))
                    .map(|t| format!("#{}", t.id))
                    .collect();
                let moved = mason_landing::batch::moved_after_testing(&active, &tested);
                if !gone.is_empty() || !moved.is_empty() {
                    for id in &moved {
                        if !self.left(*id) {
                            parked.push(Parked {
                                task: *id,
                                reason: "its branch moved after testing".into(),
                                feedback: "Your branch moved after the batch tested it, so it was not landed. Submit it again.".into(),
                                conflict: false,
                            });
                        }
                    }
                    active.retain(|t| !self.left(t.id) && !moved.contains(&t.id));
                    if !gone.is_empty() {
                        note = format!("{} left the batch: retried or canceled", gone.join(", "));
                    }
                    if active.is_empty() {
                        break Ok(());
                    }
                    continue 'outer;
                }
            }
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
                if force || self.forced() {
                    return true;
                }
                let outside = self.cfg.mode.uses_candidate_ref();
                if suites.is_empty() && !outside {
                    let Some(cmds) = legacy.as_ref() else {
                        return true;
                    };
                    // Under a step host, so a restarted daemon finds the run again.
                    let tree = crate::hostedcheck::tree_key(&wt_for_cmd);
                    let members = self.members(&def);
                    // `skippable`: a skip (not only a force) stops it. The rebuild is not: a
                    // skipped test leaves the build a batch must still pass.
                    let spec =
                        |prefix: &str, cmd: &str, skippable: bool| crate::hostedcheck::CheckSpec {
                            id_prefix: format!("b{batch_id}-{prefix}"),
                            log_dir: log_dir.clone(),
                            cmd: cmd.to_string(),
                            build: None,
                            cwd: wt_for_cmd.clone(),
                            env: vec![],
                            limit: p.test_timeout,
                            tree: tree.clone(),
                            holds: Default::default(),
                            label: String::new(),
                            stop: {
                                let (d, m) = (d.clone(), members.clone());
                                Some(Arc::new(move || {
                                    if skippable {
                                        m.stop_running(&d, "")
                                    } else {
                                        m.forced(&d)
                                    }
                                }))
                            },
                            place: None,
                            global: true,
                        };
                    // The base moved: the union is linted again, tests skipped or not (s521).
                    let lint = self.lint_checks(
                        &def,
                        &wt_for_cmd,
                        &log_dir,
                        &format!("b{batch_id}-r{rounds}-relint"),
                    );
                    if !lint.green {
                        tracing::warn!(
                            batch = batch_id,
                            "the lint check is red after the base moved: {}",
                            lint.notes.join("; ")
                        );
                        return false;
                    }
                    // A build first, as the test step does, holding one `compile`
                    // slot; the re-test then holds the rest of what the test step uses.
                    if let Some(build) = &cmds.build {
                        match self.check_through_outage(
                            &def,
                            d,
                            &spec("rebuild", build, false),
                            crate::sched::needs_of(&crate::jobs::build_uses(), None),
                        ) {
                            Ok(o) if o.stopped.is_some() => {
                                tracing::info!(
                                    batch = batch_id,
                                    "the rebuild was stopped: {}",
                                    o.stopped.unwrap_or_default()
                                );
                                return true;
                            }
                            Ok(o) if o.passed && all_immune => {
                                tracing::info!(
                                    batch = batch_id,
                                    "the base moved: rebuilt; not re-tested, every task is \
                                     immune to testing"
                                );
                                return true;
                            }
                            Ok(o) if o.passed && tests_skipped => {
                                tracing::info!(
                                    batch = batch_id,
                                    "the base moved: rebuilt; not re-tested, every task's \
                                     testing was skipped; running the fast tier"
                                );
                                // Small enough to run again on the moved base (s509).
                                let t = self.fast_tests(
                                    &def,
                                    &wt_for_cmd,
                                    &asm.base_tip,
                                    &log_dir,
                                    &format!("b{batch_id}-rebuild"),
                                );
                                return t.green;
                            }
                            Ok(o) if o.passed => {}
                            Ok(_) => return false,
                            Err(e) => {
                                tracing::warn!(
                                    batch = batch_id,
                                    "the re-test's build gave no result: {e}"
                                );
                                return false;
                            }
                        }
                    }
                    // The re-test holds what the test step uses, as any test does; the
                    // wait for it is outside the command's timeout.
                    let spec = spec("retest", &cmds.test, true);
                    return match self.check_through_outage(
                        &def,
                        d,
                        &spec,
                        self.test_step_needs(&def, cmds.build.is_some()),
                    ) {
                        Ok(o) if o.stopped.is_some() => {
                            tracing::info!(
                                batch = batch_id,
                                "the re-test was stopped, green for the batch: {}",
                                o.stopped.unwrap_or_default()
                            );
                            true
                        }
                        Ok(o) => {
                            if o.passed {
                                if let Some(t) = crate::cmdreuse::content_tree(&wt_for_cmd) {
                                    let key = crate::cmdreuse::command_key(
                                        cmds.build.as_deref(),
                                        &cmds.test,
                                    );
                                    crate::cmdreuse::record_green(
                                        d,
                                        &t,
                                        &key,
                                        &format!("landing re-test of batch {batch_id}"),
                                    );
                                }
                            }
                            o.passed || self.known_red_only(&def, &log_dir, &o, batch_id)
                        }
                        Err(e) => {
                            tracing::warn!(batch = batch_id, "the re-test gave no result: {e}");
                            false
                        }
                    };
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
                        self.defer_stopped(&landed);
                        break Ok(());
                    }
                    // Red after it was green, or a conflict with the moved base: assemble again.
                    Ok(PrEnd::Red) | Ok(PrEnd::Conflict) => continue 'outer,
                    Ok(PrEnd::BaseKeptMoving) => {
                        // Contention, not the change's fault: queue the tasks again.
                        for t in &active {
                            if !deferred.contains(&t.id) {
                                deferred.push(t.id);
                            }
                        }
                        break Err(format!(
                            "the base moved more than {} times while landing",
                            self.cfg.max_fold
                        ));
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
            // The repository lock is held only around git (the fold and the promote);
            // the re-verification between them runs with it released.
            let fin = batch.finish_with(
                &asm,
                &|| Box::new(lock.lock().unwrap_or_else(|e| e.into_inner())),
                &mut reverify,
                &point,
            );
            match fin {
                Err(e) => break Err(format!("landing failed: {e}")),
                Ok(Err(_red)) => continue 'outer, // red after folding: assemble on the new base, blame properly
                Ok(Ok(Finish::Conflict(_))) => continue 'outer, // the conflict shows up as an ejection
                Ok(Ok(Finish::BaseKeptMoving)) => {
                    // Contention, not the change's fault: queue the tasks again.
                    for t in &active {
                        if !deferred.contains(&t.id) {
                            deferred.push(t.id);
                        }
                    }
                    break Err(format!(
                        "the base moved more than {} times while landing",
                        self.cfg.max_fold
                    ));
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
                    // Before the verdict: the post-land payment runs what Land now stopped.
                    self.defer_stopped(&landed);
                    break Ok(());
                }
            }
        };

        // A refused push is not the change's fault: hold landing, charge no failure.
        let refused = match &outcome {
            Err(why) => crate::ssh_keys::hold_if_refused(d, &p.id, why),
            Ok(()) => false,
        };
        let yielded = self.yielded.load(std::sync::atomic::Ordering::SeqCst);
        let phase = match (&outcome, landed.is_empty(), parked.is_empty()) {
            _ if yielded && landed.is_empty() && parked.is_empty() => "yielded",
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
        let reused = self.reused.load(std::sync::atomic::Ordering::SeqCst);
        let final_note = if reused > 0 {
            format!("{final_note}; reused {reused} run result(s) from an earlier batch over the same base and task heads")
        } else {
            final_note
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
                if let Err(e) = git::run(&int_wt, &["checkout", "--detach", "--quiet"]) {
                    tracing::warn!("detaching {}: {e}", int_wt.display());
                }
            }
            if let Err(e) = git::run(&p.path, &["branch", "-D", &int_branch]) {
                tracing::warn!("deleting {int_branch}: {e}");
            }
        }
        // -- the verdicts
        let was_ok = outcome.is_ok();
        let all_ids: Vec<u64> = ids.clone();
        // Land now is for this batch: a task it did not land goes back to the ordinary queue.
        for id in all_ids.iter().filter(|i| !landed.contains(i)) {
            if d.store.landing_flag(*id).ok().flatten().as_deref()
                == Some(crate::suites::LAND_NOW_RUNNING)
            {
                let _ = d.store.clear_landing_flag(*id);
            }
        }
        match &outcome {
            Ok(()) => {
                for id in &all_ids {
                    if delivered.contains(id) || deferred.contains(id) {
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
                    if delivered.contains(id) || deferred.contains(id) {
                        continue;
                    }
                    if let Some(pk) = parked.iter().find(|x| x.task == *id) {
                        deliver(d, TaskId(*id), pk.verdict());
                    } else if landed.contains(id) {
                        deliver(d, TaskId(*id), Verdict::Landed);
                    } else if refused {
                        deliver(d, TaskId(*id), Verdict::Held);
                    } else {
                        deliver(d, TaskId(*id), Verdict::Failed(why.clone()));
                    }
                }
            }
        }
        // The deferred tasks did nothing wrong: they go back in the queue as they were, to
        // be assembled in the next batch, first if their mate did not land.
        if !deferred.is_empty() {
            {
                let mut g = d.landing.lock();
                for id in &deferred {
                    g.owed.remove(&TaskId(*id));
                    if let Some(w) = self.waits.iter().find(|w| w.task.0 == *id) {
                        let q = g.waiting.entry(self.pid).or_default();
                        q.retain(|x| x.task != w.task);
                        q.push(w.clone());
                    }
                }
            }
            d.landing.wake();
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

    /// The local suites that cover every failed CI job (`[ci.local]`), or `None` when any
    /// failed job has no mapping (or maps to a suite the project does not define): that red
    /// is searched for through CI, as before.
    fn local_suites(
        &self,
        def: &mason_definition::Definition,
        failed: &BTreeSet<String>,
    ) -> Option<Vec<String>> {
        if failed.is_empty() {
            return None;
        }
        let mut out: Vec<String> = vec![];
        for job in failed {
            let suites = self.cfg.ci_local.get(job)?;
            for s in suites {
                def.testing.suite(s)?;
                if !out.contains(s) {
                    out.push(s.clone());
                }
            }
        }
        Some(out)
    }

    /// A red re-test whose failing tests are all in the known-red ledger is green for the
    /// batch (s281). A timeout, a red with no readable test names, or any test that is not in
    /// the ledger stays red (and is bisected as before).
    fn known_red_only(
        &self,
        def: &mason_definition::Definition,
        log_dir: &Path,
        o: &crate::hostedcheck::CheckOutcome,
        batch_id: u64,
    ) -> bool {
        if o.timed_out {
            return false;
        }
        let step = def
            .steps
            .iter()
            .find(|s| s.kind == mason_definition::StepType::Test)
            .map_or("test", |s| s.name.as_str());
        let Ok(text) = std::fs::read_to_string(log_dir.join(format!("{}.log", o.id))) else {
            return false;
        };
        let failing = crate::rerun::failing_names(&text);
        let known = crate::knownred::known_for(&self.d.store, step);
        if !crate::knownred::all_known(&failing, &known) {
            return false;
        }
        let (_, hit) = crate::knownred::split(&failing, &known);
        tracing::warn!(
            batch = batch_id,
            "the re-test is red only in known-red tests (green for the batch): {}",
            crate::knownred::lines(&hit).join("; ")
        );
        true
    }

    fn legacy_cmd(&self, def: &mason_definition::Definition) -> Option<crate::jobs::TestCommands> {
        crate::jobs::test_commands(&self.d, def, self.pid, None)
    }

    /// Run the named suites over the worktree, locally: names that are not suites (CI job
    /// names) have nothing to run and are left out. No CI.
    fn run_local(
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
            failing: Default::default(),
            ..Default::default()
        };
        // Names that are not suites (CI job names) have nothing to run locally.
        let local: Vec<String> = suites
            .iter()
            .filter(|s| {
                def.testing.suite(s).is_some()
                    || self.cfg.batch_suites().is_some_and(|t| t.contains(s))
            })
            .cloned()
            .collect();
        let (local, stopped) = self.without_stopped(def, local);
        let mut notes = stopped;
        // The batch tier's narrowed selection, when `narrow = true` and the full test stands behind it.
        let narrowed = self.narrowed_def(def, &local, wt, base, &mut notes);
        let reuse = narrowed.is_none();
        let def = narrowed.as_ref().unwrap_or(def);
        if local.is_empty() {
            let mut g = green();
            g.notes = notes;
            g
        } else {
            // Only the suites that run on this machine take its slots; one bound for GitHub takes
            // them if it comes back here (s943).
            let (_lease, cover) = match self.hold_suites(def, &local, true) {
                Ok(l) => l,
                Err(e) => {
                    self.fail_service(e.clone());
                    return crate::suites::OverRun {
                        green: false,
                        failed: BTreeSet::new(),
                        notes: vec![e],
                        failing: Default::default(),
                        ..Default::default()
                    };
                }
            };
            let n = self.runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let stop = std::sync::atomic::AtomicBool::new(false);
            let mut r = std::thread::scope(|sc| {
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
                    true,
                    &self.members(def),
                    reuse,
                    // What the batch's lease holds: a suite it holds slots for that falls back
                    // from GitHub runs under it (s911); any other takes its own (s943).
                    &cover,
                );
                stop.store(true, std::sync::atomic::Ordering::SeqCst);
                r
            });
            r.notes.splice(0..0, notes);
            r
        }
    }

    /// `narrow` or `whole`, for the keys that keep a red result: the two prove different things.
    fn narrow_tag(&self) -> &'static str {
        match crate::landing::batch_gate(&self.d, self.project(), &self.cfg) {
            Some(mason_core::full_test::Gate::Narrow) => "narrow",
            _ => "whole",
        }
    }

    /// The definition with the batch tier's suites narrowed to the union diff of `wt` against
    /// `base` (s804), or `None` to run them whole: the tier does not narrow, the gate says whole
    /// (a note says why), or no suite of `local` narrows. A suite whose pick is everything stays
    /// whole, with the reason in a note. Every rung of the red ladder runs through here, so a
    /// bisect step narrows to the union of its own subset.
    fn narrowed_def(
        &self,
        def: &mason_definition::Definition,
        local: &[String],
        wt: &Path,
        base: &str,
        notes: &mut Vec<String>,
    ) -> Option<mason_definition::Definition> {
        let tier = self.cfg.batch_suites()?;
        match crate::landing::batch_gate(&self.d, self.project(), &self.cfg)? {
            mason_core::full_test::Gate::Narrow => {}
            mason_core::full_test::Gate::Whole(why) => {
                if local.iter().any(|s| tier.contains(s)) {
                    notes.push(format!("the batch runs the whole suite: {why}"));
                }
                return None;
            }
        }
        let mut out = def.clone();
        let mut any = false;
        for name in local.iter().filter(|s| tier.contains(s)) {
            let Some(suite) = def.testing.suite(name) else {
                continue;
            };
            match crate::suites::narrow_for_batch(def, suite, wt, base) {
                Ok((s, note)) => {
                    if let Some(slot) = out.testing.suites.iter_mut().find(|x| x.name == *name) {
                        *slot = s;
                        any = true;
                        notes.push(note);
                    }
                }
                Err(why) => notes.push(format!("{name}: the batch runs it whole because {why}")),
            }
        }
        any.then_some(out)
    }

    /// Split off the suites the owner has already stopped for the whole batch (every member
    /// skipped them, a member is forced, or Land now): they run nothing, so they must not
    /// queue for a slot. Returns the rest and a note per stopped suite, as `run_over` writes.
    fn without_stopped(
        &self,
        def: &mason_definition::Definition,
        names: Vec<String>,
    ) -> (Vec<String>, Vec<String>) {
        let members = self.members(def);
        let mut rest = vec![];
        let mut notes = vec![];
        for name in names {
            match members.stop_running(&self.d, &name) {
                Some(why) => {
                    if members.land_now(&self.d).is_some() {
                        members.note_stopped(&format!("suite:{name}"));
                    }
                    notes.push(format!("{name}: {why}"));
                }
                None => rest.push(name),
            }
        }
        (rest, notes)
    }

    /// Run only the failing tests of each suite (#682): what a rerun and a bisect need when
    /// the red run already named them. `build`: build each suite first (a bisect subset is a
    /// fresh tree); `serial`: one test at a time. Proves less than a whole run, so nothing
    /// lands on it alone.
    #[allow(clippy::too_many_arguments)]
    fn run_alone(
        &self,
        def: &mason_definition::Definition,
        failing: &std::collections::BTreeMap<String, Vec<String>>,
        wt: &Path,
        log_dir: &Path,
        prefix: &str,
        build: bool,
        serial: bool,
    ) -> crate::suites::OverRun {
        let names: Vec<String> = failing.keys().cloned().collect();
        let (names, stopped) = self.without_stopped(def, names);
        if names.is_empty() {
            return crate::suites::OverRun {
                green: true,
                notes: stopped,
                ..Default::default()
            };
        }
        let (_lease, cover) = match self.hold_suites(def, &names, true) {
            Ok(l) => l,
            Err(e) => {
                self.fail_service(e.clone());
                return crate::suites::OverRun {
                    green: false,
                    failed: BTreeSet::new(),
                    notes: vec![e],
                    failing: Default::default(),
                    ..Default::default()
                };
            }
        };
        let n = self.runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let stop = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|sc| {
            sc.spawn(|| self.mirror(&stop));
            let r = crate::suites::run_alone(
                &self.d,
                self.project(),
                def,
                failing,
                wt,
                log_dir,
                &format!("{prefix}-{n}"),
                self.sink(),
                build,
                serial,
                &cover,
            );
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
            r
        })
    }

    /// The batch's tasks, for what the owner does to them while it verifies.
    fn members(&self, def: &mason_definition::Definition) -> crate::suites::Members {
        let mut m =
            crate::suites::Members::new(&self.member_tasks, def).in_project(&self.project().id);
        m.stopped = self.stopped.clone();
        m
    }

    /// Has the owner pressed Land now on a member of this running batch (and no freeze
    /// holds it back)?
    fn land_now(&self) -> bool {
        crate::suites::Members {
            tasks: self.member_tasks.clone(),
            project: self.project().id.clone(),
            ..Default::default()
        }
        .land_now(&self.d)
        .is_some()
    }

    /// The members landed: what Land now stopped is now owed by each, to be run on the base.
    fn defer_stopped(&self, landed: &[u64]) {
        let scopes: Vec<String> = self
            .stopped
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect();
        if !scopes.is_empty() {
            crate::suites::defer_stopped(&self.d, &self.project().id, landed, &scopes);
        }
    }

    /// Is any member of the batch forced? Read live, as the owner may press Force while the
    /// batch is already verifying; the batch then lands as a forced one would.
    fn forced(&self) -> bool {
        self.member_tasks
            .iter()
            .any(|t| self.d.store.landing_flag(t.0).ok().flatten().as_deref() == Some("force"))
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
            failing: Default::default(),
            ..Default::default()
        };
        // Force lands past verification, CI included.
        if self.kind == "force" || self.forced() {
            return green();
        }
        // The lint tier runs first (s691): it takes seconds and names the files, where a red
        // found after the batch suite cost 15-25 min and then a bisect of the whole suite.
        let l = self.lint_checks(def, wt, log_dir, prefix);
        if !l.green {
            return crate::suites::OverRun {
                green: false,
                failed: l.failed,
                notes: l.notes,
                failing: Default::default(),
                ..Default::default()
            };
        }
        let mut r = self.run_local(def, suites, wt, base, log_dir, prefix);
        // Land now stopped the tests (kept: the freeze, the merge and the build floor); what it
        // stopped is owed after the merge. What lands must still build.
        let land_now = self.land_now();
        // Forced while it ran: the suites were stopped, and nothing else is run.
        if self.forced() {
            r.green = true;
            r.failed.clear();
            r.notes
                .push("forced by the owner: landing without verification".into());
            return r;
        }
        if !r.green {
            return r;
        }
        r.notes.extend(l.notes);
        let outside = if self.cfg.mode.uses_candidate_ref() {
            self.ci_verify(wt)
        } else if self.cfg.mode.uses_pr() {
            self.pr_verify(wt)
        } else {
            // No suites for the batch and no CI: nothing has built what is about to land.
            if self.cfg.batch_suites().is_none() || land_now {
                let f = self.build_floor(def, wt, base, log_dir, prefix);
                if !f.green {
                    self.floor_red
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
                r.green = f.green;
                r.failed.extend(f.failed);
                r.notes.extend(f.notes);
                if f.green && land_now {
                    // Land now stopped the testing, the fast tier and the new tests of the
                    // skipped tasks included: they are tests, not the build floor, and the
                    // new-tests check can go to GitHub and fail the batch for want of a runner
                    // (batch #613, 2026-11-22). Owed after the merge, as any stopped suite is.
                    self.owe_stopped_tiers(&mut r, def, wt, base);
                } else {
                    if f.green && self.fast_tier.load(std::sync::atomic::Ordering::SeqCst) {
                        let t = self.fast_tests(def, wt, base, log_dir, prefix);
                        r.green = t.green;
                        r.failed.extend(t.failed);
                        r.notes.extend(t.notes);
                    }
                    if r.green {
                        self.merge_new_tests(&mut r, def, wt, base, log_dir, prefix);
                    }
                }
            }
            return r;
        };
        match outside {
            Ok((true, notes, _)) => {
                r.notes.extend(notes);
                // CI was not waited for: nothing but the build floor has built the union.
                let ci_skipped = self
                    .stopped
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    .any(|s| s.starts_with("ci:"));
                if ci_skipped && self.cfg.mode.uses_candidate_ref() {
                    let f = self.build_floor(def, wt, base, log_dir, prefix);
                    if !f.green {
                        self.floor_red
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    r.green = f.green;
                    r.failed.extend(f.failed);
                    r.notes.extend(f.notes);
                    if r.green && land_now {
                        // As above: Land now stopped the tests; the new ones are owed.
                        self.owe_stopped_tiers(&mut r, def, wt, base);
                    } else if r.green {
                        self.merge_new_tests(&mut r, def, wt, base, log_dir, prefix);
                    }
                }
            }
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

    /// Land now is in effect: the fast tier and the new-tests check are not run. Each suite
    /// they would have run counts as stopped (`suite:NAME`), so `defer_stopped` makes it a debt
    /// of every landed member, run on the base after the merge (s943). Says so in the notes.
    fn owe_stopped_tiers(
        &self,
        r: &mut crate::suites::OverRun,
        def: &mason_definition::Definition,
        wt: &Path,
        base: &str,
    ) {
        let Some(step) = def
            .steps
            .iter()
            .find(|s| s.kind == mason_definition::StepType::Test)
        else {
            return;
        };
        let members = self.members(def);
        let mut owed: BTreeSet<String> = BTreeSet::new();
        // The fast tier: each selected suite that declares a fast command.
        if self.fast_tier.load(std::sync::atomic::Ordering::SeqCst) {
            if let Ok(t) = git::run(wt, &["diff", "--name-only", &format!("{base}..HEAD")]) {
                let paths: Vec<String> = t.lines().map(String::from).collect();
                if !paths.is_empty() {
                    let names = match def.testing.impact.select(&paths, &step.suites) {
                        Selection::All => step.suites.clone(),
                        Selection::Some(v) => v,
                    };
                    owed.extend(names.into_iter().filter(|n| {
                        def.testing
                            .suite(n)
                            .is_some_and(|s| s.fast_command.is_some())
                    }));
                }
            }
        }
        // The new tests of the tasks whose testing was skipped: they count under the suite
        // `new_tests` names for them.
        let files: Vec<String> = self
            .member_tasks
            .iter()
            .filter(|t| crate::suites::skip_for(&self.d, **t, "", &step.name).is_some())
            .filter_map(|t| self.fps.get(&t.0))
            .flat_map(|fp| fp.files.iter().cloned())
            .collect();
        if !files.is_empty() {
            if let Ok(a) = crate::newtests::in_range(wt, base, "HEAD") {
                let added = a.restricted(&files);
                if !added.names().is_empty() {
                    let project_test = self.legacy_cmd(def).map(|c| c.test);
                    if let Some(n) = step.suites.iter().find(|n| {
                        def.testing.suite(n).is_some_and(|s| {
                            crate::suites::with_project_test(s, project_test.as_deref()).is_ok()
                                && s.rerun.as_deref().is_some_and(|t| t.contains("{tests}"))
                        })
                    }) {
                        owed.insert(n.clone());
                    }
                }
                if !added.exits.is_empty() {
                    owed.insert(
                        step.suites
                            .iter()
                            .find(|n| n.starts_with("exit-scripts"))
                            .cloned()
                            .unwrap_or_else(|| "exit-scripts".into()),
                    );
                }
            }
        }
        for n in owed {
            members.note_stopped(&format!("suite:{n}"));
            r.notes.push(format!(
                "{n}: the fast tier and the new tests were not run: land now by the owner; owed after the merge"
            ));
        }
    }

    /// The fast unit tier (s509): a batch whose every task had its testing skipped is built by the
    /// floor, which compiles test code and runs none of it. Each selected suite that declares a
    /// `fast_command` runs it here, so a merge that breaks a quick test (two green tasks each adding
    /// the same line to a list) is caught. The full run stays owed, as the skip debts say. A red
    /// puts the failing tests' names in `failed` (the suite's name when none can be read).
    fn fast_tests(
        &self,
        def: &mason_definition::Definition,
        wt: &Path,
        base: &str,
        log_dir: &Path,
        prefix: &str,
    ) -> crate::suites::OverRun {
        let d = &self.d;
        let mut out = crate::suites::OverRun {
            green: true,
            failed: BTreeSet::new(),
            notes: vec![],
            failing: Default::default(),
            ..Default::default()
        };
        let paths: Vec<String> =
            match git::run(wt, &["diff", "--name-only", &format!("{base}..HEAD")]) {
                Ok(t) => t.lines().map(String::from).collect(),
                Err(e) => {
                    let why = format!("the fast tier could not read the batch's diff: {e}");
                    self.fail_service(why.clone());
                    out.green = false;
                    out.notes.push(why);
                    return out;
                }
            };
        let Some(step) = def
            .steps
            .iter()
            .find(|s| s.kind == mason_definition::StepType::Test)
        else {
            return out;
        };
        if paths.is_empty() {
            return out;
        }
        let names = match def.testing.impact.select(&paths, &step.suites) {
            Selection::All => step.suites.clone(),
            Selection::Some(v) => v,
        };
        let members = self.members(def);
        for name in &names {
            let Some(suite) = def.testing.suite(name) else {
                continue;
            };
            let Some(cmd) = suite.fast_command.clone() else {
                continue;
            };
            let spec = crate::hostedcheck::CheckSpec {
                id_prefix: format!("{prefix}-fast-{name}"),
                log_dir: log_dir.to_path_buf(),
                cmd,
                build: None,
                cwd: wt.to_path_buf(),
                env: vec![],
                limit: self.project().test_timeout,
                tree: crate::hostedcheck::tree_key(wt),
                holds: Default::default(),
                label: String::new(),
                stop: Some({
                    let (d, m) = (d.clone(), members.clone());
                    Arc::new(move || m.forced(&d))
                }),
                place: Some((None, Box::new(crate::hostedcheck::without_build(suite)))),
                global: false,
            };
            match self.check_through_outage(
                def,
                d,
                &spec,
                self.test_step_needs(def, suite.build.is_some()),
            ) {
                Ok(o) if o.stopped.is_some() => out.notes.push(format!(
                    "{name}: fast tier not run to the end: {}",
                    o.stopped.unwrap_or_default()
                )),
                Ok(o) if o.passed => out.notes.push(format!("{name}: fast tier passes")),
                Ok(o) => {
                    out.green = false;
                    let failing = fast_failures(suite, &o.output);
                    // The failing tests are what is red, so the ladder bisects for the task
                    // whose change breaks them instead of blaming everyone the suite covers.
                    if failing.is_empty() {
                        out.failed.insert(name.clone());
                    } else {
                        out.failed.extend(failing.iter().cloned());
                    }
                    out.notes.push(if failing.is_empty() {
                        format!(
                            "{name}: the fast tier failed{}\n{}",
                            if o.timed_out { " (timed out)" } else { "" },
                            crate::jobs::tail(&o.output, 15)
                        )
                    } else {
                        format!("{name}: the fast tier failed in {}", failing.join(", "))
                    });
                }
                Err(e) => {
                    let why = format!("{name}: the fast tier gave no result: {e}");
                    self.fail_service(why.clone());
                    out.green = false;
                    out.notes.push(why);
                    return out;
                }
            }
        }
        out
    }

    /// [`Self::new_tests`] after a green floor, its red recorded as the floor's own so that the
    /// ladder blames by file (the files that add the failing tests).
    fn merge_new_tests(
        &self,
        r: &mut crate::suites::OverRun,
        def: &mason_definition::Definition,
        wt: &Path,
        base: &str,
        log_dir: &Path,
        prefix: &str,
    ) {
        let t = self.new_tests(def, wt, base, log_dir, prefix);
        if !t.green {
            self.floor_red
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        r.green = t.green;
        r.failed.extend(t.failed);
        r.notes.extend(t.notes);
    }

    /// The tests the batch's skipped tasks add (#812). The floor only proves the merged tree
    /// builds, so a fast-tracked task whose change adds a failing test, or whose code never got
    /// made, used to land. The new `#[test]` fns of the tasks whose testing was skipped run here
    /// by name (and their new exit scripts), and nothing else of the suites does. A red names the
    /// files that add the failing tests (`lint_files`), so only the task that added them is parked.
    fn new_tests(
        &self,
        def: &mason_definition::Definition,
        wt: &Path,
        base: &str,
        log_dir: &Path,
        prefix: &str,
    ) -> crate::suites::OverRun {
        let d = &self.d;
        let mut out = crate::suites::OverRun {
            green: true,
            failed: BTreeSet::new(),
            notes: vec![],
            failing: Default::default(),
            ..Default::default()
        };
        let Some(step) = def
            .steps
            .iter()
            .find(|s| s.kind == mason_definition::StepType::Test)
        else {
            return out;
        };
        // Only the tasks whose testing the owner skipped: the others had theirs run.
        let mut files: Vec<String> = vec![];
        for t in &self.member_tasks {
            if crate::suites::skip_for(d, *t, "", &step.name).is_some() {
                if let Some(fp) = self.fps.get(&t.0) {
                    files.extend(fp.files.iter().cloned());
                }
            }
        }
        if files.is_empty() {
            return out;
        }
        let added = match crate::newtests::in_range(wt, base, "HEAD") {
            Ok(a) => a.restricted(&files),
            Err(e) => {
                let why = format!("the new-tests tier could not read the batch's diff: {e}");
                self.fail_service(why.clone());
                out.green = false;
                out.notes.push(why);
                return out;
            }
        };
        if added.is_empty() {
            return out;
        }
        self.lint_files
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        let project_test = self.legacy_cmd(def).map(|c| c.test);
        // (suite it counts under, command, the tests it runs)
        let mut runs: Vec<(String, String, crate::newtests::Added)> = vec![];
        let names = added.names();
        if !names.is_empty() {
            for sname in &step.suites {
                let Some(suite) = def.testing.suite(sname) else {
                    continue;
                };
                let Ok(suite) = crate::suites::with_project_test(suite, project_test.as_deref())
                else {
                    continue;
                };
                let Some(t) = suite.rerun.as_deref().filter(|t| t.contains("{tests}")) else {
                    continue;
                };
                let words: Vec<String> = names
                    .iter()
                    .map(|n| format!("'{}'", n.replace('\'', "'\\''")))
                    .collect();
                let cmd = t
                    .replace("--exact ", "")
                    .replace("{tests}", &words.join(" "));
                let only = Added {
                    tests: added.tests.clone(),
                    exits: vec![],
                };
                runs.push((sname.clone(), cmd, only));
                break;
            }
        }
        if !added.exits.is_empty() {
            let list: Vec<String> = added.exits.iter().map(|e| format!("'{e}'")).collect();
            let cmd = format!(
                "for s in {}; do bash \"$s\" || exit 1; done",
                list.join(" ")
            );
            let name = step
                .suites
                .iter()
                .find(|n| n.starts_with("exit-scripts"))
                .cloned()
                .unwrap_or_else(|| "exit-scripts".into());
            runs.push((
                name,
                cmd,
                Added {
                    tests: vec![],
                    exits: added.exits.clone(),
                },
            ));
        }
        let members = self.members(def);
        for (name, cmd, only) in runs {
            let spec = crate::hostedcheck::CheckSpec {
                id_prefix: format!("{prefix}-new-{name}"),
                log_dir: log_dir.to_path_buf(),
                cmd,
                build: None,
                cwd: wt.to_path_buf(),
                env: vec![],
                limit: self.project().test_timeout,
                tree: crate::hostedcheck::tree_key(wt),
                holds: Default::default(),
                label: String::new(),
                stop: Some({
                    let (d, m) = (d.clone(), members.clone());
                    Arc::new(move || m.forced(&d)) as crate::hostedcheck::StopFn
                }),
                place: None,
                global: true,
            };
            match self.run_held(def, d, &spec, self.test_step_needs(def, false)) {
                Ok(o) if o.stopped.is_some() => out.notes.push(format!(
                    "{name}: the new tests were not run to the end: {}",
                    o.stopped.unwrap_or_default()
                )),
                Ok(o) if o.passed => out.notes.push(format!(
                    "{name}: the new tests of the skipped tasks pass ({})",
                    only.describe()
                )),
                Ok(o) => {
                    out.green = false;
                    out.failed.insert(name.clone());
                    let bad = crate::newtests::failing_among(&only, &o.full);
                    // The files that add what failed; every file that adds a test when no
                    // name can be read.
                    let blame: Vec<String> = if bad.is_empty() {
                        only.files()
                    } else {
                        only.restricted(
                            &only
                                .tests
                                .iter()
                                .filter(|(_, n)| bad.contains(n))
                                .map(|(f, _)| f.clone())
                                .collect::<Vec<_>>(),
                        )
                        .files()
                    };
                    let mut lf = self.lint_files.lock().unwrap_or_else(|e| e.into_inner());
                    for f in blame {
                        if !lf.contains(&f) {
                            lf.push(f);
                        }
                    }
                    out.notes.push(format!(
                        "{name}: a test a skipped task adds failed{}\n{}",
                        if bad.is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", bad.join(", "))
                        },
                        crate::jobs::tail(&o.output, 15)
                    ));
                }
                Err(e) => {
                    let why = format!("{name}: the new tests gave no result: {e}");
                    self.fail_service(why.clone());
                    out.green = false;
                    out.notes.push(why);
                    return out;
                }
            }
        }
        out
    }

    /// Every file any member task's diff touches: the only paths a guard's `<path>: <message>`
    /// line may blame.
    fn union_files(&self) -> Vec<String> {
        let all: BTreeSet<&String> = self.fps.values().flat_map(|f| f.files.iter()).collect();
        all.into_iter().cloned().collect()
    }

    /// The files a lint-shaped red names. A red of the lint tier has them from `lint_checks`;
    /// a red whose only failing tests are the `ci_gates` fmt and SPDX checks has them from a
    /// lint run now (it takes seconds). `None` for any other red.
    fn lint_blame_files(
        &self,
        def: &mason_definition::Definition,
        vr: &crate::suites::OverRun,
        wt: &Path,
        log_dir: &Path,
        prefix: &str,
        floor_red: bool,
    ) -> Option<Vec<String>> {
        let tier_suites: Vec<String> = self
            .cfg
            .tier("lint")
            .map(|t| t.suites.clone())
            .unwrap_or_default();
        if vr.failed.is_empty() {
            return None;
        }
        if vr.failed.iter().all(|f| tier_suites.contains(f)) {
            let f = self
                .lint_files
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            return Some(f);
        }
        if floor_red && !vr.failed.is_empty() {
            let f = self
                .lint_files
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            return Some(f);
        }
        if vr.failed.iter().all(|f| is_gate_test(f)) {
            let l = self.lint_checks(def, wt, log_dir, prefix);
            if l.green {
                return None;
            }
            let f = self
                .lint_files
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            return Some(f);
        }
        None
    }

    /// The lint tier (`[[tier]] name = "lint"` in `.mason/landing.toml`): the `command` of each
    /// suite it names runs in the batch's worktree on every batch, whether or not its tests
    /// were skipped; only the owner's force stops it. A red names the files and lines the
    /// output points at, so the parked batch says what to fix.
    fn lint_checks(
        &self,
        def: &mason_definition::Definition,
        wt: &Path,
        log_dir: &Path,
        prefix: &str,
    ) -> crate::suites::OverRun {
        let d = &self.d;
        let mut out = crate::suites::OverRun {
            green: true,
            failed: BTreeSet::new(),
            notes: vec![],
            failing: Default::default(),
            ..Default::default()
        };
        let Some(tier) = self.cfg.tier("lint") else {
            return out;
        };
        let members = self.members(def);
        for name in &tier.suites {
            let Some(suite) = def.testing.suite(name) else {
                out.green = false;
                out.failed.insert(name.clone());
                out.notes.push(format!(
                    "{name}: the lint tier names a suite the definition does not have"
                ));
                continue;
            };
            if let Some(why) = members.forced(d) {
                out.notes.push(format!("{name}: not run: {why}"));
                continue;
            }
            // #811: the lint tier runs on this Mac unless the tier says `where = "github"`;
            // then the suite's own `remote`/`runs_on` decide, as for any suite.
            let github = tier.place.as_deref() == Some("github");
            let spec = crate::hostedcheck::CheckSpec {
                id_prefix: format!("{prefix}-lint-{name}"),
                log_dir: log_dir.to_path_buf(),
                cmd: suite.command.clone(),
                build: None,
                cwd: wt.to_path_buf(),
                env: vec![],
                limit: self.project().test_timeout,
                tree: crate::hostedcheck::tree_key(wt),
                holds: Default::default(),
                label: String::new(),
                stop: Some({
                    let (d, m) = (d.clone(), members.clone());
                    Arc::new(move || m.forced(&d)) as crate::hostedcheck::StopFn
                }),
                place: match (github, self.member_tasks.first()) {
                    (true, Some(t)) => {
                        Some((Some(*t), Box::new(crate::hostedcheck::without_build(suite))))
                    }
                    _ => None,
                },
                global: false,
            };
            let remote = github && crate::hostedcheck::goes_remote(d, &spec);
            // Work that goes to GitHub holds runner units, not a local slot, while it waits (#818).
            let began = std::time::Instant::now();
            let ran_on = if remote { "on GitHub" } else { "on this Mac" };
            let took = |s: u64| {
                if s >= 60 {
                    format!("{}m{:02}s", s / 60, s % 60)
                } else {
                    format!("{s}s")
                }
            };
            match self.check_through_outage(
                def,
                d,
                &spec,
                crate::sched::needs_of(&crate::jobs::build_uses(), None),
            ) {
                Ok(o) if o.stopped.is_some() => out.notes.push(format!(
                    "{name}: not run to the end: {}",
                    o.stopped.unwrap_or_default()
                )),
                Ok(o) if o.passed => out.notes.push(format!(
                    "{name}: clean ({ran_on}, {})",
                    took(began.elapsed().as_secs())
                )),
                Ok(o) => {
                    out.green = false;
                    out.failed.insert(name.clone());
                    *self.lint_files.lock().unwrap_or_else(|e| e.into_inner()) =
                        lint_files(&o.full, &self.union_files());
                    *self.lint_where.lock().unwrap_or_else(|e| e.into_inner()) =
                        lint_places(&o.full);
                    out.notes.push(format!(
                        "{name}: the batch is not lint clean ({ran_on}){}{}\n{}",
                        if o.timed_out { " (timed out)" } else { "" },
                        lint_places(&o.full),
                        crate::jobs::tail(&o.output, 15)
                    ));
                }
                Err(e) => {
                    let why = format!("{name}: the lint check gave no result: {e}");
                    self.fail_service(why.clone());
                    out.green = false;
                    out.notes.push(why);
                    return out;
                }
            }
        }
        out
    }

    /// The build floor. A batch that no suite verifies (the project has no batch tier) and no
    /// CI checks was, until 2026-11-13, promoted with nothing having built it: a task's own
    /// test run covers the task, never the union, and when the owner skips testing it covers
    /// nothing at all. Nine batches of that day stacked on a `landing.rs` that did not parse.
    ///
    /// The floor builds the union: the `build` of the test step's suites that the impact map
    /// selects for the batch's diff, once each, unless this exact tree already passed the
    /// suite. That compiles (and links) the tests without running them, which is what stops
    /// code that does not build, for a fraction of a test run, and in the integration
    /// worktree, whose build directory stays warm. A suite with no `build` has nothing to
    /// run here.
    fn build_floor(
        &self,
        def: &mason_definition::Definition,
        wt: &Path,
        base: &str,
        log_dir: &Path,
        prefix: &str,
    ) -> crate::suites::OverRun {
        let green = || crate::suites::OverRun {
            green: true,
            failed: BTreeSet::new(),
            notes: vec![],
            failing: Default::default(),
            ..Default::default()
        };
        let paths: Vec<String> =
            match git::run(wt, &["diff", "--name-only", &format!("{base}..HEAD")]) {
                Ok(t) => t.lines().map(String::from).collect(),
                Err(e) => {
                    let why = format!("the build floor could not read the batch's diff: {e}");
                    self.fail_service(why.clone());
                    return crate::suites::OverRun {
                        green: false,
                        failed: BTreeSet::new(),
                        notes: vec![why],
                        failing: Default::default(),
                        ..Default::default()
                    };
                }
            };
        let Some(step) = def
            .steps
            .iter()
            .find(|s| s.kind == mason_definition::StepType::Test)
        else {
            return green();
        };
        if paths.is_empty() {
            return green();
        }
        let names = match def.testing.impact.select(&paths, &step.suites) {
            Selection::All => step.suites.clone(),
            Selection::Some(v) => v,
        };
        self.build_suites(def, &names, wt, log_dir, prefix, "the batch", true)
    }

    /// Run the `build` of each named suite in `wt`, holding one `compile` slot for each. `what`
    /// is what is being built, for the notes. A build that fails (or times out) is red for its
    /// suite, with the end of its output; a build that cannot be run ends the batch, as any
    /// service that cannot answer does.
    #[allow(clippy::too_many_arguments)]
    fn build_suites(
        &self,
        def: &mason_definition::Definition,
        names: &[String],
        wt: &Path,
        log_dir: &Path,
        prefix: &str,
        what: &str,
        // The owner's skips and force stop the builds (the batch's floor, not the base's).
        stoppable: bool,
    ) -> crate::suites::OverRun {
        let d = &self.d;
        let members = if stoppable {
            self.members(def)
        } else {
            crate::suites::Members::default()
        };
        let mut out = crate::suites::OverRun {
            green: true,
            failed: BTreeSet::new(),
            notes: vec![],
            failing: Default::default(),
            ..Default::default()
        };
        let tree = git::run(wt, &["rev-parse", "HEAD^{tree}"])
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let project_test = self.legacy_cmd(def).map(|c| c.test);
        for name in names {
            let Some(suite) = def.testing.suite(name) else {
                continue;
            };
            if suite.build.is_none() {
                continue;
            }
            // Identical code never tests twice: a tree that passed the suite builds.
            if !tree.is_empty() && matches!(d.store.evidence(&tree, name), Ok(Some(_))) {
                out.notes.push(format!(
                    "{name}: this exact tree already passed, not built again"
                ));
                continue;
            }
            let build = match crate::suites::with_project_test(suite, project_test.as_deref()) {
                Ok(s) => s.build,
                // A definition that cannot say what to build is not a red of the tree: the batch
                // ends with the reason, nobody is blamed, and nothing lands.
                Err(e) => {
                    let why = format!("{name}: cannot build {what}: {e}");
                    self.fail_service(why.clone());
                    out.green = false;
                    out.notes.push(why);
                    return out;
                }
            };
            let Some(build) = build else {
                continue;
            };
            if let Some(why) = members.stop(d, name) {
                out.notes.push(format!("{name}: {what} not built: {why}"));
                continue;
            }
            let floor_needs = crate::sched::needs_of(&crate::jobs::build_uses(), None);
            // #816: the floor builds on this Mac (warm target directory, no runner queue) unless
            // `floor_where = "github"`, or the suite names a runner of another OS than this
            // Mac's. Then the suite's own placement (#815) decides, as before.
            let here = self.cfg.floor_where.as_deref() != Some("github")
                && suite
                    .runs_on
                    .as_deref()
                    .is_none_or(|r| r.starts_with("macos"));
            let mk = |id_prefix: String, github: bool| crate::hostedcheck::CheckSpec {
                id_prefix,
                log_dir: log_dir.to_path_buf(),
                cmd: build.clone(),
                build: None,
                cwd: wt.to_path_buf(),
                env: vec![],
                limit: self.project().test_timeout,
                tree: crate::hostedcheck::tree_key(wt),
                holds: Default::default(),
                label: String::new(),
                stop: stoppable.then(|| {
                    let (d, m, name) = (d.clone(), members.clone(), name.clone());
                    Arc::new(move || m.stop(&d, &name)) as crate::hostedcheck::StopFn
                }),
                // #815: the suite's own placement (its `runs_on`, `remote = false`, device rules)
                // decides; no task owns a batch.
                place: github.then(|| (None, Box::new(crate::hostedcheck::without_build(suite)))),
                global: false,
            };
            let spec = mk(format!("{prefix}-floor-{name}"), !here);
            let ran_on = if !here && crate::hostedcheck::goes_remote(d, &spec) {
                "on GitHub"
            } else {
                "on this Mac"
            };
            let began = std::time::Instant::now();
            let took = || {
                let s = began.elapsed().as_secs();
                if s >= 60 {
                    format!("{}m{:02}s", s / 60, s % 60)
                } else {
                    format!("{s}s")
                }
            };
            let mut ran_on = ran_on;
            let mut result = self.check_through_outage(def, d, &spec, floor_needs.clone());
            // #962: no GitHub runner took the floor build within the outage limit, three times
            // over. A build is the same anywhere, so it is built on this Mac rather than failing
            // the batch for GitHub's queue.
            if !here
                && result
                    .as_ref()
                    .is_err_and(|e| crate::hostedcheck::is_outage(e))
            {
                out.notes.push(format!(
                    "{name}: no GitHub runner took the build of {what}; building it on this Mac"
                ));
                ran_on = "on this Mac";
                let local = mk(format!("{prefix}-floor-{name}-here"), false);
                result = self.check_through_outage(def, d, &local, floor_needs);
            }
            match result {
                Ok(o) if o.stopped.is_some() => out.notes.push(format!(
                    "{name}: {what} not built to the end: {}",
                    o.stopped.unwrap_or_default()
                )),
                Ok(o) if o.passed => out
                    .notes
                    .push(format!("{name}: {what} builds ({ran_on}, {})", took())),
                Ok(o) => {
                    out.green = false;
                    out.failed.insert(name.clone());
                    // A build error names `--> file:line` too: the floor blames by file.
                    *self.lint_files.lock().unwrap_or_else(|e| e.into_inner()) =
                        lint_files(&o.full, &self.union_files());
                    out.notes.push(format!(
                        "{name}: {what} does not build ({ran_on}, {}){}{}\n{}",
                        took(),
                        if o.timed_out {
                            " (the build timed out)"
                        } else {
                            ""
                        },
                        lint_places(&o.full),
                        crate::jobs::tail(&o.output, 15)
                    ));
                }
                Err(e) => {
                    let why = format!("{name}: the build of {what} gave no result: {e}");
                    self.fail_service(why.clone());
                    out.green = false;
                    out.notes.push(why);
                    return out;
                }
            }
        }
        out
    }

    /// Does the base itself build the suites that failed? `Some(why)` says it does not. The
    /// look is taken in the integration worktree, whose build directory is warm, and the
    /// batch's tree is put back as it was before this returns.
    #[allow(clippy::too_many_arguments)]
    fn base_builds(
        &self,
        def: &mason_definition::Definition,
        names: &[String],
        wt: &Path,
        base: &str,
        head: &str,
        log_dir: &Path,
        prefix: &str,
    ) -> Result<Option<String>, String> {
        let branch = self.project().base.clone();
        let lock = self.d.repo_lock(self.pid);
        {
            let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
            git::run(wt, &["checkout", "--force", "--detach", base]).map_err(|e| {
                format!("could not check out {branch} to see whether it builds: {e}")
            })?;
        }
        let r = self.build_suites(def, names, wt, log_dir, prefix, "the base", false);
        {
            let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
            git::run(wt, &["checkout", "--force", "--detach", head]).map_err(|e| {
                format!("could not put the batch's tree back after building {branch}: {e}")
            })?;
        }
        if let Some(e) = self.take_service_error() {
            return Err(e);
        }
        if r.green {
            return Ok(None);
        }
        let first = r
            .notes
            .iter()
            .find(|n| n.contains("does not build"))
            .and_then(|n| n.lines().next())
            .unwrap_or("it does not build");
        // The first line only says which suite; the compiler's own first error says why, and
        // without it nine identical failures cannot be told from one cause.
        let cause = r
            .notes
            .iter()
            .filter(|n| n.contains("does not build"))
            .flat_map(|n| n.lines().skip(1))
            .find(|l| l.contains("error"))
            .map(|l| format!(": {}", l.trim()))
            .unwrap_or_default();
        Ok(Some(format!(
            "{branch} does not build ({first}{cause}), so the red is not this batch's: nobody is blamed and nothing lands until it does"
        )))
    }

    /// Run a check that may go to GitHub. A run GitHub does not take or fails waits again at
    /// the batch's landing rank, and three such failures in a row are an error (#822). When
    /// GitHub would not take the run at all (a refused push or dispatch), wait (charged to no
    /// task) and run it again, for up to [`OUTAGE_LIMIT_S`]; only then is it an error (#772).
    /// The caller raises an error as a service failure.
    fn check_through_outage(
        &self,
        def: &mason_definition::Definition,
        d: &Arc<Daemon>,
        spec: &crate::hostedcheck::CheckSpec,
        local: mason_scheduler::Needs,
    ) -> Result<crate::hostedcheck::CheckOutcome, String> {
        let key = self.alert_key("service");
        let r = retry_outage(
            || {
                if let Some(note) = spec.stop.as_ref().and_then(|f| f()) {
                    return Ok(crate::hostedcheck::CheckOutcome {
                        passed: false,
                        timed_out: false,
                        output: String::new(),
                        full: String::new(),
                        id: String::new(),
                        stopped: Some(note),
                        phase: None,
                    });
                }
                self.run_held(def, d, spec, local.clone())
            },
            |waited, why| {
                self.d.landing.raise(
                    self.pid,
                    &key,
                    "landing",
                    &format!(
                        "{}: waiting for GitHub: {why} (waited {waited} s)",
                        self.project().id
                    ),
                );
                self.alive(OUTAGE_STEP_S);
            },
            |s| self.nap(s),
        );
        self.d.landing.clear(self.pid, &key);
        r
    }

    /// Note why the batch cannot go on; it ends with this reason.
    fn fail_service(&self, e: String) {
        self.service_error
            .lock()
            .unwrap_or_else(|x| x.into_inner())
            .get_or_insert(e);
    }

    /// What the definition's test step uses: the re-test of a landing holds it,
    /// without `compile` when a build ran first.
    fn test_step_needs(
        &self,
        def: &mason_definition::Definition,
        built: bool,
    ) -> mason_scheduler::Needs {
        def.steps
            .iter()
            .find(|s| s.kind == mason_definition::StepType::Test)
            .map_or_else(Default::default, |s| {
                if built {
                    crate::sched::needs_of(&crate::jobs::tests_uses(&s.uses), None)
                } else {
                    crate::sched::needs_of(&s.uses, None)
                }
            })
    }

    /// Take `needs` from the broker for a verification, ranked as the batch's first
    /// task. The wait is outside every timeout (those start after it). With `watch`,
    /// the wait ends if all the batch's tasks have left the landing step. Nothing
    /// to hold gives `Ok(None)`.
    fn hold(
        &self,
        def: &mason_definition::Definition,
        needs: mason_scheduler::Needs,
        watch: bool,
    ) -> Result<Option<crate::sched::Lease>, String> {
        if !needs.exclusive && needs.amounts.is_empty() {
            return Ok(None);
        }
        let d = &self.d;
        if needs.amounts.keys().any(|n| n.starts_with("device:")) {
            d.targets.declare_all(d);
        }
        for name in needs.amounts.keys() {
            if !name.starts_with("llm:") && !name.starts_with("device:") {
                d.broker
                    .declare(name, def.resources.get(name).copied().unwrap_or(1));
            }
        }
        let owner = match self.owner(def, watch) {
            Ok(Some(o)) => o,
            Ok(None) => return Ok(None),
            Err(e) => return Err(e),
        };
        let alive = || owner.alive(d);
        d.broker
            .acquire(
                owner.task,
                &format!(
                    "batch #{}",
                    self.batch_id.load(std::sync::atomic::Ordering::SeqCst)
                ),
                owner.rank,
                needs,
                &alive,
                &mut |_| {},
            )
            .map(Some)
            .map_err(|_| "every task of the batch left the landing step while it waited".into())
    }

    /// Hold what `spec` needs, ranked as the batch's first task, then run it: a check that goes
    /// to GitHub takes the runner units of its jobs and no slot of this machine, and is pushed
    /// once they are granted (#818); one that runs here takes `local`. A run that attaches to
    /// one already out takes the local slots as before and pushes nothing new. The wait is
    /// outside the command's timeout, and the hold ends with the run.
    fn run_held(
        &self,
        def: &mason_definition::Definition,
        d: &Arc<Daemon>,
        spec: &crate::hostedcheck::CheckSpec,
        local: mason_scheduler::Needs,
    ) -> Result<crate::hostedcheck::CheckOutcome, String> {
        let units = crate::hostedcheck::pool_units(d, spec)
            .filter(|_| !crate::hostedcheck::pending(d, spec));
        let (lease, grant) = match units {
            Some(u) => {
                let lease = self.hold(def, hold_needs(Default::default(), u), true)?;
                (
                    None,
                    lease.map_or_else(crate::remote::Grant::attached, crate::remote::Grant::of),
                )
            }
            None => (
                self.hold(def, local, true)?,
                crate::remote::Grant::attached(),
            ),
        };
        let out = crate::hostedcheck::run(d, spec, &grant);
        drop(grant);
        drop(lease);
        out
    }

    /// The batch as the owner of its broker requests, ranked as its first task (`None`: no
    /// member). `watch`: a wait ends if all the batch's tasks have left the landing step.
    fn owner(
        &self,
        def: &mason_definition::Definition,
        watch: bool,
    ) -> Result<Option<crate::remote::BatchOwner>, String> {
        let Some(first) = self.member_tasks.first().copied() else {
            return Ok(None);
        };
        let step = def
            .steps
            .iter()
            .find(|s| s.kind == mason_definition::StepType::Landing)
            .map_or_else(String::new, |s| s.name.clone());
        let rank = match self
            .d
            .world()
            .ok()
            .and_then(|w| w.tasks.get(&first).cloned())
        {
            Some(t) => self.d.rank_for(&t, def, &step),
            None => return Err("the batch's first task is gone".into()),
        };
        Ok(Some(crate::remote::BatchOwner {
            task: first,
            rank,
            tasks: self.member_tasks.clone(),
            runs: self.member_runs.clone(),
            watch,
        }))
    }

    /// Of `names`, the suites that run on this machine and the ones bound for GitHub, decided
    /// as a suite's run decides where it goes (`remote::suite_goes_remote`, s943). A suite bound
    /// for GitHub holds nothing here while it is out; if it comes back, or runs a baseline or a
    /// build here, it takes its slots then, as the batch.
    fn by_placement(
        &self,
        def: &mason_definition::Definition,
        names: &[String],
    ) -> (Vec<String>, Vec<String>) {
        let mut here = vec![];
        let mut there = vec![];
        for n in names {
            let remote = def.testing.suite(n).is_some_and(|s| {
                crate::remote::suite_goes_remote(&self.d, &self.project().id, None, s)
            });
            if remote {
                there.push(n.clone());
            } else {
                here.push(n.clone());
            }
        }
        (here, there)
    }

    /// Hold the slots of the suites of `names` that run on this machine, and say what the lease
    /// holds, for `suites::run_over`/`run_alone` (s943).
    fn hold_suites(
        &self,
        def: &mason_definition::Definition,
        names: &[String],
        watch: bool,
    ) -> Result<(Option<crate::sched::Lease>, crate::suites::BatchCover), String> {
        let (here, there) = self.by_placement(def, names);
        let needs = suites_needs(def, &here);
        // A suite bound for GitHub asks the broker for the runner units of the jobs it starts
        // (its shards, and `gha-macos` on a macOS runner) at the batch's landing rank, and
        // takes no slot of this machine (#818). One hold covers both.
        let project = self.project().id.clone();
        let mut units: std::collections::BTreeMap<String, u32> = Default::default();
        for n in &there {
            let Some(s) = def.testing.suite(n) else {
                continue;
            };
            if let crate::remote::RunPlace::GitHub { units: u, .. } = crate::remote::run_place(
                &self.d,
                &project,
                None,
                Some(s),
                &s.command,
                crate::remote::Kind::Suite,
            ) {
                for (k, v) in u {
                    let have = units.entry(k).or_insert(0);
                    *have = (*have).max(v);
                }
            }
        }
        let lease = self.hold(def, hold_needs(needs.clone(), units), watch)?;
        let cover = crate::suites::BatchCover {
            owner: self.owner(def, watch)?.map(Arc::new),
            held: lease.as_ref().map(|_| needs),
            pool: lease.as_ref().and_then(crate::suites::BatchPool::new),
        };
        Ok((lease, cover))
    }

    /// An update is waiting to take over: a batch between runs stops at the next boundary
    /// with its results kept (s705).
    fn yield_now(&self) -> bool {
        if self.d.landing.draining() {
            self.yielded
                .store(true, std::sync::atomic::Ordering::SeqCst);
            true
        } else {
            self.yielded.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// What an earlier run found over exactly this base, these task heads and this filter.
    /// A red is reused until the owner retries a task in it or presses Land now on one
    /// (`http::forget_red_runs`, decision 0962), so a truly red task does not run again by
    /// itself; a green only when `reuse_green` (a split's subsets, whose answer only blames,
    /// never lets anything land).
    fn cached_run(
        &self,
        base: &str,
        key: &str,
        filter: &str,
        reuse_green: bool,
    ) -> Option<crate::suites::OverRun> {
        let (green, detail) = self
            .d
            .store
            .batch_run(&self.project().id, base, key, filter)
            .ok()??;
        if green && !reuse_green {
            return None;
        }
        let v: serde_json::Value = serde_json::from_str(&detail).ok()?;
        let strs = |x: &serde_json::Value| -> Vec<String> {
            x.as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        };
        let r = crate::suites::OverRun {
            green,
            failed: strs(&v["failed"]).into_iter().collect(),
            notes: strs(&v["notes"]),
            failing: v["failing"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, x)| (k.clone(), strs(x)))
                .collect(),
            unfinished: Default::default(),
        };
        self.floor_red.store(
            v["floor"].as_bool().unwrap_or(false),
            std::sync::atomic::Ordering::SeqCst,
        );
        self.real_ci_red.store(
            v["real_ci"].as_bool().unwrap_or(false),
            std::sync::atomic::Ordering::SeqCst,
        );
        *self.lint_files.lock().unwrap_or_else(|e| e.into_inner()) = strs(&v["lint"]);
        self.reused
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Some(r)
    }

    /// Keep what a run found, unless the run failed for a reason of the service.
    fn keep_run(&self, base: &str, key: &str, filter: &str, r: &crate::suites::OverRun) {
        if self
            .service_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
        {
            return;
        }
        let detail = serde_json::json!({
            "failed": r.failed,
            "notes": r.notes,
            "failing": r.failing,
            "floor": self.floor_red.load(std::sync::atomic::Ordering::SeqCst),
            "real_ci": self.real_ci_red.load(std::sync::atomic::Ordering::SeqCst),
            "lint": *self.lint_files.lock().unwrap_or_else(|e| e.into_inner()),
        })
        .to_string();
        let _ = self.d.store.put_batch_run(
            &self.project().id,
            base,
            key,
            filter,
            r.green,
            &detail,
            now_ms(),
        );
    }

    /// [`Runner::verify`], through the kept results of the batch's earlier runs.
    #[allow(clippy::too_many_arguments)]
    fn verify_kept(
        &self,
        def: &mason_definition::Definition,
        suites: &[String],
        wt: &Path,
        base: &str,
        log_dir: &Path,
        prefix: &str,
        key: &str,
        filter: &str,
    ) -> crate::suites::OverRun {
        if let Some(r) = self.cached_run(base, key, filter, false) {
            self.say(&format!(
                "batch #{}: reusing the red result of an earlier run over the same tree",
                self.batch_id.load(std::sync::atomic::Ordering::SeqCst)
            ));
            return r;
        }
        let r = self.verify(def, suites, wt, base, log_dir, prefix);
        self.keep_run(base, key, filter, &r);
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
        // A push that cannot reach GitHub at all (no DNS, no route) is the network's fault, not
        // the code's: wait it out like any other outage before failing the landing.
        let key = self.alert_key("service");
        let pushed = retry_outage(
            || {
                git::run(
                    wt,
                    &[
                        "push",
                        "--porcelain",
                        &remote,
                        &format!("{sha}:refs/heads/{name}"),
                    ],
                )
                .map_err(|e| {
                    let e = e.to_string();
                    if is_network_unreachable(&e) {
                        format!("{}{e}", crate::hostedcheck::OUTAGE_PREFIX)
                    } else {
                        e
                    }
                })
            },
            |waited, why| {
                self.d.landing.raise(
                    self.pid,
                    &key,
                    "landing",
                    &format!(
                        "{}: waiting for the network to push {name}: {why} (waited {waited} s)",
                        self.project().id
                    ),
                );
                self.alive(OUTAGE_STEP_S);
            },
            |s| self.nap(s),
        );
        self.d.landing.clear(self.pid, &key);
        pushed.map_err(|e| {
            let e = e
                .strip_prefix(crate::hostedcheck::OUTAGE_PREFIX)
                .unwrap_or(&e);
            format!("could not push {name}: {e}")
        })?;
        self.point(&format!("{kind}_pushed"));
        Ok((sha, id))
    }

    /// [`cancel_runs`] with the project's CI; nothing without a `[ci]` section.
    fn cancel_ci(&self, run: &str, git_ref: &str) -> usize {
        self.ci
            .as_ref()
            .map_or(0, |(ci, _)| cancel_runs(ci.as_ref(), run, git_ref))
    }

    /// Give up on a pull request's checks: close it, cancel their runs, delete its branch.
    /// Returns how many runs were cancelled.
    fn abandon_pr(&self, number: u64, branch: &str, ref_id: u64) -> usize {
        if let Some(host) = &self.host {
            let _ = host.close_pr(number);
        }
        let n = self.cancel_ci("", branch);
        self.drop_candidate(branch, ref_id);
        n
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
                    self.alive(poll_s);
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

    /// Record one look at a pending run and raise or clear the "CI has not changed" alert.
    /// `at` is (CI branch, run id, run URL or empty).
    fn watch_stuck(
        &self,
        looks: &mut Vec<mason_core::ci_stuck::Look>,
        key: &str,
        now_s: u64,
        after_s: u64,
        at: (&str, &str, &str),
        detail: &[CiRunJobs],
    ) {
        use mason_core::ci_stuck::{span, stuck_for, Look};
        let mut sum = CiJobCounts::default();
        for w in detail {
            sum.merge(&w.jobs);
        }
        let mut names = sum.running_names.clone();
        names.sort();
        let signature = format!(
            "q{} r{} ok{} bad{} {}",
            sum.queued,
            sum.running,
            sum.succeeded,
            sum.failed,
            names.join("|")
        );
        // No job detail at all: the run is pending, so something is unfinished.
        let unfinished = sum.total == 0 || sum.queued + sum.running > 0;
        // Only the stretch of the last signature matters: keep its first and latest look.
        if looks.last().is_some_and(|l| l.signature != signature) {
            looks.clear();
        }
        looks.push(Look {
            at_s: now_s,
            signature,
            unfinished,
        });
        if looks.len() > 2 {
            let (first, last) = (looks[0].clone(), looks[looks.len() - 1].clone());
            *looks = vec![first, last];
        }
        let Some(quiet) = stuck_for(looks, after_s) else {
            self.d.landing.clear(self.pid, key);
            return;
        };
        let (branch, run, url) = at;
        let jobs = if sum.total == 0 {
            "the CI gave no job detail".to_string()
        } else {
            let mut parts = vec![];
            if sum.running > 0 {
                parts.push(format!(
                    "{} running ({})",
                    sum.running,
                    sum.running_names.join(", ")
                ));
            }
            if sum.queued > 0 {
                parts.push(format!("{} queued, no runner yet", sum.queued));
            }
            parts.join("; ")
        };
        let batch = self.batch_id.load(std::sync::atomic::Ordering::SeqCst);
        let page = if url.is_empty() { run } else { url };
        let help = if url.is_empty() {
            "see `mason landing`"
        } else {
            "`gh run cancel <id>` for the run, or `mason landing` for the batch"
        };
        let text = format!(
            "{}: batch {batch}: CI on {branch} ({page}) has not changed for {}: {jobs}; {help}",
            self.project().id,
            // In steps of `stuck_after`, so the text (and any announcement of it) holds still.
            span(quiet - quiet % after_s)
        );
        self.d.landing.raise(self.pid, key, "landing", &text);
        self.d.poke();
    }

    /// `direct-ci` and `local-only-ci`: push `mason/ci/<project>-b<batch>-<n>`, run CI
    /// on it, take the answer, delete the ref. `(green, notes, failed job names)`.
    fn ci_verify(&self, wt: &Path) -> Result<(bool, Vec<String>, Vec<String>), String> {
        let (ci, cfg) = self.ci.clone().ok_or("the project has no [ci] section")?;
        self.real_ci_red
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let batch = self.batch_id.load(std::sync::atomic::Ordering::SeqCst);
        let name = format!("mason/ci/{}-b{batch}-{}", self.project().id, self.next_n());
        let (sha, ref_id) = self.push_candidate(wt, "ci", &name)?;
        let started = Instant::now();
        self.d.landing.ci.start(batch, &name, now_ms());
        self.say(&format!("waiting for CI on {name}"));
        let skipped = std::cell::Cell::new(false);
        let looks = std::cell::RefCell::new(Vec::<mason_core::ci_stuck::Look>::new());
        let stuck_key = self.alert_key("ci-stuck");
        // The commit until the trigger names the run.
        let mut run_id = sha.clone();
        let r = (|| {
            let run = self.ask("CI trigger", started, cfg.timeout_s, cfg.poll_s, || {
                ci.trigger(&name, &sha).map(Some)
            })?;
            run_id.clone_from(&run);
            self.ask("CI", started, cfg.timeout_s, cfg.poll_s, || {
                // Land now: stop waiting. CI is owed on the base after the merge.
                if self.land_now() {
                    skipped.set(true);
                    return Ok(Some(CiState::Green));
                }
                let (state, detail) = ci.status_detail(&run)?;
                Ok(match state {
                    CiState::Pending => {
                        self.ci_jobs(&name, &detail);
                        // For the landing view only; a failed read changes nothing.
                        let mut url = String::new();
                        if let Ok(p) = ci.progress(&run) {
                            self.d.landing.ci.update(batch, &p);
                            url = p
                                .workflows
                                .iter()
                                .map(|w| w.url.clone())
                                .find(|u| !u.is_empty())
                                .unwrap_or_default();
                        }
                        // Only looks: nothing is cancelled and no verdict changes.
                        self.watch_stuck(
                            &mut looks.borrow_mut(),
                            &stuck_key,
                            started.elapsed().as_secs(),
                            cfg.stuck_after_s,
                            (&name, &run, &url),
                            &detail,
                        );
                        None
                    }
                    done => Some(done),
                })
            })
        })();
        // The alert never outlives the run: finished, red, yielded, timed out or failed.
        self.d.landing.clear(self.pid, &stuck_key);
        match &r {
            Ok(CiState::Green) => self.d.landing.ci.finish(batch, "green"),
            Ok(CiState::Red { .. }) => self.d.landing.ci.finish(batch, "red"),
            _ => {}
        }
        self.point("ci_finished");
        // No verdict (land now, timeout, an error): the run must not carry on by itself.
        let cancelled = if skipped.get() || r.is_err() {
            self.cancel_ci(&run_id, &name)
        } else {
            0
        };
        self.drop_candidate(&name, ref_id);
        if skipped.get() {
            self.d.landing.ci.finish(batch, "skipped");
            self.stopped
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(crate::suites::ci_scope(self.project()));
            let cancelled = if cancelled > 0 {
                format!("; {}", runs_cancelled(cancelled))
            } else {
                String::new()
            };
            return Ok((
                true,
                vec![format!(
                    "CI on {name}: not waited for (land now){cancelled}; it runs on the base after the merge"
                )],
                vec![],
            ));
        }
        let r = r.map_err(|e| match cancelled_note(&name, "no verdict", cancelled) {
            Some(note) => format!("{e}; {note}"),
            None => e,
        });
        Ok(match r? {
            CiState::Green => {
                self.d.landing.clear(self.pid, "redbase");
                (true, vec![format!("CI on {name}: green")], vec![])
            }
            CiState::Red {
                failed,
                summary,
                jobs,
            } => {
                let mut notes = vec![format!("CI on {name}: {summary}")];
                if judges(&cfg) {
                    notes.extend(self.judge_notes(&jobs));
                }
                notes.extend(self.red_notes(&jobs));
                (false, notes, failed)
            }
            CiState::Pending => unreachable!("pending is waited out"),
        })
    }

    /// Remember pages on the host against every task of the batch, for the task detail.
    fn note_ci_links(&self, links: &[CiRef]) {
        for t in self.fps.keys() {
            record_ci_links(&self.d.task_dir(TaskId(*t)), links);
        }
    }

    /// One note per failed job the CI judged ("pr-gate/macOS: runner lost, rerun" or
    /// "pr-gate/Linux: real failure, not rerun"), and the flag that stops the red ladder's
    /// rerun when any of them is a real failure. Nothing is judged (no notes, no flag)
    /// when the CI gave no per-job reasons.
    fn judge_notes(&self, jobs: &[mason_adapter_api::service::JobFailure]) -> Vec<String> {
        let mut real = false;
        let notes = jobs
            .iter()
            .map(|j| match &j.infra {
                Some(why) => format!("{}/{}: {why}, rerun", j.workflow, j.job),
                None => {
                    real = true;
                    format!("{}/{}: real failure, not rerun", j.workflow, j.job)
                }
            })
            .collect();
        self.real_ci_red
            .store(real, std::sync::atomic::Ordering::SeqCst);
        notes
    }

    /// What a red batch tells its tasks and the owner about the failed jobs (s174): the end
    /// of each job's log (about 60 lines in all), and the base branch's own state. A job
    /// whose failing tests all fail on the base too, with the same text, blames nobody and
    /// the batch waits ("red on <base> too"); with different text it waits and the owner
    /// is alerted; a test or job that is green on the base is still the batch's.
    fn red_notes(&self, jobs: &[mason_adapter_api::service::JobFailure]) -> Vec<String> {
        let mut notes = vec![];
        let logged: Vec<_> = jobs.iter().filter(|j| j.log.is_some()).collect();
        let each = (60 / logged.len().max(1)).max(10);
        for j in &logged {
            if let Some(l) = &j.log {
                notes.push(format!(
                    "--- {}/{}: end of the job's log ---\n{}",
                    j.workflow,
                    j.job,
                    mason_core::red_base::feedback_lines(l, each)
                ));
            }
        }
        let base = self.project().base.clone();
        let verdict = match crate::ci_watch::source(self.project(), "the landing CI comparison") {
            Ok((gh, _)) if !jobs.is_empty() => crate::ci_watch::assess(&gh, &base, jobs),
            _ => return notes,
        };
        use mason_core::red_base::Verdict;
        match verdict {
            Verdict::Blame => self.d.landing.clear(self.pid, "redbase"),
            Verdict::RedOnBase(names) => {
                self.d.landing.clear(self.pid, "redbase");
                let why = format!(
                    "{names} red on {base} too (the same tests fail there): nobody is blamed, the batch waits"
                );
                notes.push(why.clone());
                self.fail_service(why);
            }
            Verdict::Differs(texts) => {
                let why = format!(
                    "the same tests fail on {base} and in the batch, with different text: {texts}; nobody is blamed, the batch waits for the owner"
                );
                self.d.landing.raise(
                    self.pid,
                    "redbase",
                    "landing",
                    &format!("{}: {why}", self.project().id),
                );
                self.d.poke();
                notes.push(why.clone());
                self.fail_service(why);
            }
        }
        notes
    }

    /// The pull-request modes: push a branch, open a PR from it, wait for its checks.
    /// Red closes the PR; green keeps it (in `self.pr`) for the merge.
    fn pr_verify(&self, wt: &Path) -> Result<(bool, Vec<String>, Vec<String>), String> {
        let host = self
            .host
            .clone()
            .ok_or("the project has no [host] section")?;
        self.real_ci_red
            .store(false, std::sync::atomic::Ordering::SeqCst);
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
                let n = self.cancel_ci("", &name);
                self.drop_candidate(&name, ref_id);
                return Err(match cancelled_note(&name, "no verdict", n) {
                    Some(note) => format!("{e}; {note}"),
                    None => e,
                });
            }
        };
        let _ = self.d.store.set_ref_number(ref_id, pr.number);
        self.point("pr_opened");
        self.note_ci_links(&[CiRef {
            label: format!("PR #{}", pr.number),
            url: pr.url.clone(),
            kind: "pull_request",
        }]);
        // A newer green PR replaces an older one the batch was holding.
        let mut replaced = None;
        let old = self.pr.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(old) = old {
            let n = self.abandon_pr(old.number, &old.branch, old.ref_id);
            replaced = cancelled_note(&old.branch, "a newer pull request", n);
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
                let n = self.abandon_pr(pr.number, &name, ref_id);
                let notes = replaced
                    .into_iter()
                    .chain(cancelled_note(&name, "no verdict", n));
                return Err(notes.fold(e, |e, note| format!("{e}; {note}")));
            }
        };
        self.point("pr_checked");
        self.note_ci_links(
            &st.jobs
                .iter()
                .filter_map(|j| {
                    Some(CiRef {
                        label: j.job.clone(),
                        url: j.url.clone()?,
                        kind: "check",
                    })
                })
                .collect::<Vec<_>>(),
        );
        if st.checks == Checks::Red || st.state == PrState::Closed {
            let cancelled = self.abandon_pr(pr.number, &name, ref_id);
            let why = if st.state == PrState::Closed && st.checks != Checks::Red {
                "the PR was closed"
            } else {
                "checks failed"
            };
            let mut notes = vec![format!(
                "PR #{} ({name}): {why}: {}",
                pr.number,
                st.failed.join(", ")
            )];
            if self.project().services.ci.as_ref().is_some_and(judges) {
                notes.extend(self.judge_notes(&st.jobs));
            }
            notes.extend(self.red_notes(&st.jobs));
            notes.extend(replaced);
            if cancelled > 0 {
                notes.push(format!("CI on {name}: {}", runs_cancelled(cancelled)));
            }
            return Ok((false, notes, st.failed));
        }
        *self.pr.lock().unwrap_or_else(|e| e.into_inner()) = Some(OpenPr {
            number: pr.number,
            branch: name.clone(),
            ref_id,
        });
        let mut notes = vec![format!("PR #{}: checks green", pr.number)];
        notes.extend(replaced);
        Ok((true, notes, vec![]))
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
                    *self.merged_ref.lock().unwrap_or_else(|e| e.into_inner()) = Some(open.ref_id);
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
        let merged = *self.merged_ref.lock().unwrap_or_else(|e| e.into_inner());
        remove_refs(&self.d, self.pid, Some(batch), merged);
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
                reason: format!(
                    "red in batch #{batch_id}: {}{}",
                    vr.failed.iter().cloned().collect::<Vec<_>>().join(", "),
                    self.lint_reason_places(vr)
                ),
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

    /// Where the lint pointed, for a parked task's reason, when every failed suite is in the
    /// lint tier; empty otherwise.
    fn lint_reason_places(&self, vr: &crate::suites::OverRun) -> String {
        let tier: Vec<String> = self
            .cfg
            .tier("lint")
            .map(|t| t.suites.clone())
            .unwrap_or_default();
        if vr.failed.is_empty() || !vr.failed.iter().all(|f| tier.contains(f)) {
            return String::new();
        }
        self.lint_where
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// True when the task's landing run is no longer its open run (retried or canceled).
    fn left(&self, id: u64) -> bool {
        let Some(w) = self.waits.iter().find(|w| w.task.0 == id) else {
            return false;
        };
        match self.d.world() {
            Ok(world) => world.open_run(w.task) != Some(w.run),
            Err(_) => false,
        }
    }

    fn raise_base(&self, why: &str) {
        self.d.landing.raise(
            self.pid,
            "redbase",
            "landing",
            &format!("{}: {why}", self.project().id),
        );
        self.freeze_base(why);
        self.d.poke();
    }

    /// A red base freezes landing (only a fix passes) and files one urgent fix task, so the
    /// next batches do not each re-verify the same red and fail their tasks' land step with
    /// the same reason. A freeze already in force (CI's or post-land's) is left alone. A green
    /// union later lifts it.
    fn freeze_base(&self, why: &str) {
        let d = &self.d;
        let p = self.project();
        let held = d
            .store
            .freezes()
            .ok()
            .is_some_and(|v| v.iter().any(|f| f.project == p.id));
        if held {
            return;
        }
        let fix = d
            .emit(EventKind::TaskFiled {
                project: self.pid,
                title: "Fix the base: it is red on its own".to_string(),
                body: format!("{why}."),
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
        let _ = d.store.set_freeze(&p.id, why, fix, now_ms());
    }

    /// Split a red batch (decision 0387): run the suites over subsets of it (the integration
    /// branch rebuilt from each) until the task or tasks that reproduce the red are known.
    /// `mapped`: the local suites a CI red maps to; none: the batch's own suites. Returns
    /// what was found and, for each subset that was red, its tasks and its output.
    #[allow(clippy::too_many_arguments)]
    fn pin_red(
        &self,
        def: &mason_definition::Definition,
        batch: &Batch,
        lock: &crate::daemon::RepoLock,
        active: &[BatchTask],
        int_wt: &Path,
        log_dir: &Path,
        batch_id: u64,
        rounds: usize,
        mapped: Option<&[String]>,
        failed: &[String],
        alone: Option<&std::collections::BTreeMap<String, Vec<String>>>,
    ) -> (plan::Pin, Vec<SplitRun>) {
        let ids: Vec<u64> = active.iter().map(|t| t.id).collect();
        let runs: Vec<SplitRun> = vec![];
        // The second integration worktree of a split that runs both halves at once (#702).
        let int_wt_b = {
            let mut n = int_wt.as_os_str().to_owned();
            n.push("-half");
            PathBuf::from(n)
        };
        let int_branch_b = format!("{}-half", batch.int_branch);
        let batch_b = Batch {
            int_wt: &int_wt_b,
            int_branch: &int_branch_b,
            ..batch.clone()
        };
        // Assemble a subset into a worktree and see whether it must run at all.
        let prepare = |subset: &[u64], b: &Batch, wt: &Path, tried: &mut usize| -> Prep {
            let sub: Vec<BatchTask> = active
                .iter()
                .filter(|t| subset.contains(&t.id) && !self.left(t.id))
                .cloned()
                .collect();
            let a = {
                let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
                b.assemble(&sub, &|_| {})
            };
            // A base that cannot be checked out keeps the old answer (the base's), not a blame.
            let Ok(a) = a else {
                return Prep::Done(subset.is_empty(), None);
            };
            if self.yield_now() {
                // The search stops here; the caller sees it and leaves the batch.
                return Prep::Done(false, None);
            }
            let run_key = mason_store::batch_run_key(
                &sub.iter()
                    .map(|t| (t.id, t.head.clone()))
                    .collect::<Vec<_>>(),
            );
            let run_filter = format!(
                "split|alone={:?}|mapped={:?}|failed={}|narrow={}",
                alone,
                mapped,
                failed.join(","),
                self.narrow_tag()
            );
            if let Some(r) = self.cached_run(&a.base_tip, &run_key, &run_filter, true) {
                self.say(&format!(
                    "batch #{batch_id} is red: reusing an earlier run of {} task(s)",
                    subset.len()
                ));
                return Prep::Done(!r.green, (!r.green).then(|| r.notes.clone()));
            }
            *tried += 1;
            let which = if subset.is_empty() {
                "the base alone".to_string()
            } else {
                subset
                    .iter()
                    .map(|t| format!("#{t}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let how = alone.map_or(String::new(), |a| {
                format!(
                    "bisecting on {} failing test(s); ",
                    a.values().map(Vec::len).sum::<usize>()
                )
            });
            self.say(&format!(
                "batch #{batch_id} is red: {how}splitting it to find the task (run {tried}: {which})",
                tried = *tried
            ));
            Prep::Run(Box::new(SplitJob {
                subset: subset.to_vec(),
                a,
                run_key,
                run_filter,
                prefix: format!("b{batch_id}-r{rounds}-bis{}", *tried),
                wt: wt.to_path_buf(),
            }))
        };
        // Run an assembled subset to its verdict and keep it. Reads only: two of these can run
        // at once, each in its own worktree.
        let execute = |j: &SplitJob| -> crate::suites::OverRun {
            // Build the subset and run only the failing tests. A build that fails or a run
            // that cannot tell names no test: that is no proof of the same red, so the
            // whole suites run, as before.
            let small =
                alone.map(|a| self.run_alone(def, a, &j.wt, log_dir, &j.prefix, true, false));
            let r = match small {
                Some(r) if r.green || !r.failing.is_empty() => r,
                _ => match mapped {
                    Some(m) => self.run_local(def, m, &j.wt, &j.a.base_tip, log_dir, &j.prefix),
                    // The base alone (`plan::pin` asks when every task was red alone): a base
                    // has no diff to select suites by, so it runs the ones that were red.
                    None if j.subset.is_empty() => {
                        self.verify(def, failed, &j.wt, &j.a.base_tip, log_dir, &j.prefix)
                    }
                    None => {
                        let s = self.suites_for(def, batch, &j.a);
                        self.verify(def, &s, &j.wt, &j.a.base_tip, log_dir, &j.prefix)
                    }
                },
            };
            self.keep_run(&j.a.base_tip, &j.run_key, &j.run_filter, &r);
            r
        };
        let note_red = |runs: &mut Vec<SplitRun>, subset: &[u64], notes: Vec<String>| {
            let mut k = subset.to_vec();
            k.sort_unstable();
            runs.push((k, notes));
        };
        // Both halves of a split at once, when every suite it can run goes off this machine, to
        // GitHub or a node (#702).
        // The verdicts are the ones the one-at-a-time search gets; only the wall time differs.
        let side_by_side = {
            let mut names: std::collections::BTreeSet<&String> = failed.iter().collect();
            names.extend(mapped.into_iter().flatten());
            names.extend(alone.into_iter().flat_map(|a| a.keys()));
            !names.is_empty()
                && names.into_iter().all(|n| {
                    def.testing.suite(n).is_some_and(|s| {
                        crate::remote::executor_for(
                            &self.d,
                            &self.project().id,
                            crate::remote::Kind::Suite,
                            Some((None, s)),
                        )
                        .is_off_machine()
                    })
                })
        };
        let runs = std::cell::RefCell::new(runs);
        let tried = std::cell::Cell::new(0usize);
        let mut red = |subset: &[u64]| -> bool {
            let mut t = tried.get();
            let p = prepare(subset, batch, int_wt, &mut t);
            tried.set(t);
            match p {
                Prep::Done(red, notes) => {
                    if let Some(n) = notes {
                        note_red(&mut runs.borrow_mut(), subset, n);
                    }
                    red
                }
                Prep::Run(j) => {
                    let r = execute(&j);
                    if !r.green {
                        note_red(&mut runs.borrow_mut(), subset, r.notes.clone());
                    }
                    !r.green
                }
            }
        };
        let mut red_pair = |x: &[u64], y: &[u64]| -> (bool, bool) {
            let mut t = tried.get();
            let px = prepare(x, batch, int_wt, &mut t);
            let py = prepare(y, &batch_b, &int_wt_b, &mut t);
            tried.set(t);
            let (jx, jy) = (px, py);
            let (rx, ry) = std::thread::scope(|sc| {
                let run = |p: &Prep| match p {
                    Prep::Run(j) => Some(execute(j)),
                    Prep::Done(..) => None,
                };
                let jy_ref = &jy;
                let h = sc.spawn(move || run(jy_ref));
                (run(&jx), h.join().ok().flatten())
            });
            let mut out = [false; 2];
            for (i, (p, r, sub)) in [(&jx, rx, x), (&jy, ry, y)].into_iter().enumerate() {
                match (p, r) {
                    (Prep::Done(red, notes), _) => {
                        if let Some(n) = notes {
                            note_red(&mut runs.borrow_mut(), sub, n.clone());
                        }
                        out[i] = *red;
                    }
                    (Prep::Run(_), Some(r)) => {
                        if !r.green {
                            note_red(&mut runs.borrow_mut(), sub, r.notes.clone());
                        }
                        out[i] = !r.green;
                    }
                    // A run that panicked proves nothing: not red.
                    (Prep::Run(_), None) => {}
                }
            }
            (out[0], out[1])
        };
        let found = if side_by_side {
            plan::pin_paired(&ids, &mut red, &mut red_pair)
        } else {
            plan::pin(&ids, &mut red)
        };
        (found, runs.into_inner())
    }

    /// Act on what splitting found: send back the task or tasks that reproduce the red
    /// (with the output of the run that showed it) and keep the rest; or, for a base red,
    /// blame nobody. Returns the batch's closing note, or the reason nothing lands.
    fn settle_pin(
        &self,
        found: plan::Pin,
        runs: &[(Vec<u64>, Vec<String>)],
        parked: &mut Vec<Parked>,
        active: &mut Vec<BatchTask>,
        batch_id: u64,
        vr: &crate::suites::OverRun,
    ) -> Settled {
        let groups = match found {
            plan::Pin::Base => {
                return Settled::Base(format!(
                    "batch #{batch_id} is red in {} and every task in it reproduces the red alone: it is the base's own, nobody is blamed and nothing lands until it is fixed",
                    vr.failed.iter().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
            plan::Pin::Groups(g) => g,
        };
        let red_in = vr.failed.iter().cloned().collect::<Vec<_>>().join(", ");
        let mut said = vec![];
        for g in &groups {
            let mut k = g.clone();
            k.sort_unstable();
            let out = runs
                .iter()
                .find(|(ids, _)| *ids == k)
                .map_or(&vr.notes, |(_, n)| n)
                .join("\n");
            let names = g
                .iter()
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join(" and ");
            let (reason, feedback) = if g.len() == 1 {
                (
                    format!("red in batch #{batch_id}: {red_in}"),
                    format!(
                        "Your change made {red_in} red on its own, when the batch (#{batch_id}) was split to find it.\n{out}"
                    ),
                )
            } else {
                (
                    format!("red in batch #{batch_id} together with {names}: {red_in}"),
                    format!(
                        "Your change makes {red_in} red together with {names} (batch #{batch_id}); no one of them does alone. Bring in the latest base branch and make them work together.\n{out}"
                    ),
                )
            };
            for t in g.iter().filter(|t| !self.left(**t)) {
                parked.push(Parked {
                    task: *t,
                    reason: reason.clone(),
                    feedback: feedback.clone(),
                    conflict: false,
                });
            }
            said.push(if g.len() == 1 {
                format!("{names} reproduces it alone")
            } else {
                format!("{names} reproduce it only together")
            });
        }
        let all: Vec<u64> = groups.iter().flatten().copied().collect();
        active.retain(|t| !all.contains(&t.id));
        Settled::Parked(format!(
            "batch #{batch_id} was red in {red_in}; split to find it: {}; the rest went on",
            said.join("; ")
        ))
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
        if let Err(e) = crate::jobs::setup_checkout(def, &wt) {
            tracing::warn!(batch = batch_id, "post-land suites not run: {e}");
            let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
            git::remove_worktree(&p.path, &wt);
            return;
        }
        // Held like any suite run; the batch's tasks have left landing by now, so
        // there is nothing to stop waiting for.
        let (_lease, cover) = match self.hold_suites(def, &tier.suites, false) {
            Ok(l) => l,
            Err(_) => {
                let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
                git::remove_worktree(&p.path, &wt);
                return;
            }
        };
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
            // `commit` is the tree under test: only a known-red test may excuse a red.
            false,
            &crate::suites::Members::default(),
            false,
            &cover,
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

/// A hold for suites that run here (`local`) and on GitHub (`units`): the local slots plus the
/// runner units, and nothing of this machine for a suite that runs on GitHub.
fn hold_needs(
    mut local: mason_scheduler::Needs,
    units: std::collections::BTreeMap<String, u32>,
) -> mason_scheduler::Needs {
    for (k, v) in units {
        let have = local.amounts.entry(k).or_insert(0);
        *have = (*have).max(v);
    }
    local
}

/// What running these suites holds: each suite's `uses` and its environment's,
/// the largest of each, as the test step's own acquisition computes it.
pub(crate) fn suites_needs(
    def: &mason_definition::Definition,
    names: &[String],
) -> mason_scheduler::Needs {
    let mut n = mason_scheduler::Needs::default();
    for s in names.iter().filter_map(|s| def.testing.suite(s)) {
        let mut uses = s.uses.clone();
        if let Some(e) = def.testing.environment(&s.env) {
            if let Some(kind) = crate::targets::kind_of_env(e) {
                let u = uses.entry(kind.capability()).or_insert(0);
                *u = (*u).max(1);
            }
            for (k, v) in &e.uses {
                let u = uses.entry(k.clone()).or_insert(0);
                *u = (*u).max(*v);
            }
        }
        let more = crate::sched::needs_of(&uses, None);
        n.exclusive |= more.exclusive;
        for (k, v) in more.amounts {
            let have = n.amounts.entry(k).or_insert(0);
            *have = (*have).max(v);
        }
    }
    n
}

/// Does this CI judge its failed jobs? GitHub Actions only when `infra_judge` is on (it
/// lists failed jobs, with their logs, either way); another kind judges when it says so.
fn judges(c: &crate::config::CiCfg) -> bool {
    c.infra_judge || !matches!(c.kind, crate::config::CiKind::GithubActions { .. })
}

/// The progress record's form of a CI's job counts.
fn breakdown(j: &CiJobCounts) -> mason_core::progress::JobBreakdown {
    mason_core::progress::JobBreakdown {
        queued: j.queued,
        running: j.running,
        succeeded: j.succeeded,
        failed: j.failed,
        running_names: j.running_names.clone(),
    }
}

/// "7 jobs: 2 queued, 1 running (macOS build), 3 passed, 1 failed"; states with no jobs are left out.
fn jobs_summary(j: &CiJobCounts) -> String {
    let mut parts = vec![];
    if j.queued > 0 {
        parts.push(format!("{} queued", j.queued));
    }
    if j.running > 0 {
        let mut r = format!("{} running", j.running);
        if !j.running_names.is_empty() {
            r.push_str(&format!(" ({})", j.running_names.join(", ")));
        }
        parts.push(r);
    }
    if j.succeeded > 0 {
        parts.push(format!("{} passed", j.succeeded));
    }
    if j.failed > 0 {
        parts.push(format!("{} failed", j.failed));
    }
    format!(
        "{} {}{}{}",
        j.total,
        if j.total == 1 { "job" } else { "jobs" },
        if parts.is_empty() { "" } else { ": " },
        parts.join(", ")
    )
}

#[cfg(test)]
mod idle_tests {
    use super::*;

    #[test]
    fn idle_spell_ends_when_a_batch_runs_or_the_queue_empties() {
        let p = ProjectId(1);
        let mut s = State::default();
        let d = Duration::from_millis(500);
        s.note_idle(p, true, false, d);
        s.note_idle(p, true, false, d);
        assert_eq!(s.idle_ms.get(&p).copied(), Some(1000));
        s.note_idle(p, true, true, d);
        assert_eq!(s.idle_ms.get(&p).copied(), None);
        s.note_idle(p, true, false, d);
        s.note_idle(p, false, false, d);
        assert_eq!(s.idle_ms.get(&p).copied(), None);
    }

    #[test]
    fn a_deliberate_hold_resets_the_idle_spell() {
        let p = ProjectId(1);
        let mut s = State::default();
        s.note_idle(p, true, false, Duration::from_millis(500));
        // What a drain, an unread configuration or a freeze does in `form_and_start`.
        s.idle_ms.remove(&p);
        assert_eq!(s.idle_ms.get(&p).copied(), None);
    }
}

/// Is this failing test one of `ci_gates`' repository checks (fmt, SPDX)?
fn is_gate_test(name: &str) -> bool {
    let n = name.rsplit("::").next().unwrap_or(name);
    n == "workspace_is_fmt_clean" || n.contains("spdx")
}

/// `s` without ANSI escape sequences (CSI `ESC [ ... final`, and other two-byte escapes):
/// GitHub's runs colour cargo's output, which hides `-->` from the line parsers.
pub(crate) fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if it.next() == Some('[') {
            for n in it.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&n) {
                    break;
                }
            }
        }
    }
    out
}

/// The distinct files a lint's output names, without lines: rustfmt's `Diff in file:line`
/// and clippy's `--> file:line:col`, plus the repository's own guards' `<path>: <message>` and
/// `<path>:<line>: <message>`, the last only for a path in `known` (the batch's union diff), so
/// a prose line with a colon never names a file.
fn lint_files(output: &str, known: &[String]) -> Vec<String> {
    let norm = |f: &str| f.trim_start_matches("./").to_string();
    let output = strip_ansi(output);
    let mut v: Vec<String> = vec![];
    for l in output.lines() {
        let t = l.trim();
        let f = if let Some(r) = t.strip_prefix("--> ") {
            r.rsplitn(3, ':').last().map(str::to_string)
        } else if let Some(r) = t.strip_prefix("Diff in ") {
            let r = r.split(" at line ").next().unwrap_or(r);
            let r = r.trim_end_matches(':');
            Some(match r.rsplit_once(':') {
                Some((f, n)) if n.chars().all(|c| c.is_ascii_digit()) => f.to_string(),
                _ => r.to_string(),
            })
        } else if let Some((head, _)) = t.split_once(": ") {
            let path = match head.rsplit_once(':') {
                Some((f, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => f,
                _ => head,
            };
            let path = norm(path);
            known.iter().find(|k| norm(k) == path).map(|k| norm(k))
        } else {
            None
        };
        if let Some(f) = f {
            if !f.is_empty() && !v.contains(&f) {
                v.push(f);
            }
        }
    }
    v
}

/// The tasks whose diff touches one of `files` (paths match as written or by suffix, as
/// rustfmt may print them with a leading `./`).
fn blame_by_files(
    files: &[String],
    active: &[BatchTask],
    fps: &HashMap<u64, Footprint>,
) -> Vec<u64> {
    let norm = |f: &str| f.trim_start_matches("./").to_string();
    active
        .iter()
        .filter(|t| {
            fps.get(&t.id).is_some_and(|fp| {
                fp.files
                    .iter()
                    .any(|x| files.iter().any(|f| norm(f) == norm(x)))
            })
        })
        .map(|t| t.id)
        .collect()
}

/// The `file:line` places a lint's output points at: clippy's `--> file:line:col` and
/// rustfmt's `Diff in file:line`. At most ten, as ` (at a.rs:1, b.rs:2)`; empty when none.
fn lint_places(output: &str) -> String {
    let output = strip_ansi(output);
    let mut seen: Vec<String> = vec![];
    for l in output.lines() {
        let t = l.trim();
        let place = if let Some(r) = t.strip_prefix("--> ") {
            r.rsplit_once(':').map(|(f, _)| f.to_string())
        } else if let Some(r) = t.strip_prefix("Diff in ") {
            r.split(" at line ")
                .next()
                .map(|f| f.trim_end_matches(':').to_string())
                .map(|f| match t.rsplit_once(" at line ") {
                    Some((_, n)) => format!("{f}:{}", n.trim_end_matches(':')),
                    None => f,
                })
        } else {
            None
        };
        if let Some(p) = place {
            if !seen.contains(&p) {
                seen.push(p);
            }
        }
        if seen.len() == 10 {
            break;
        }
    }
    if seen.is_empty() {
        String::new()
    } else {
        format!(" (at {})", seen.join(", "))
    }
}

/// Is this git error "the remote could not be reached" (DNS, route, timeout), as opposed to a
/// refusal?
fn is_network_unreachable(e: &str) -> bool {
    let l = e.to_ascii_lowercase();
    [
        "could not resolve hostname",
        "could not resolve host",
        "network is unreachable",
        "no route to host",
        "connection timed out",
        "operation timed out",
        "connection refused",
        "connection reset",
        "temporary failure in name resolution",
    ]
    .iter()
    .any(|p| l.contains(p))
}

/// How long a landing waits out a GitHub outage, and the first and longest pause between tries.
const OUTAGE_LIMIT_S: u64 = 30 * 60;
const OUTAGE_STEP_S: u64 = 30;
const OUTAGE_STEP_MAX_S: u64 = 300;

/// Run `f`; while it fails with GitHub's outage (not the code's fault), `waiting(total, why)` is
/// told and, after `sleep`, `f` runs again, until [`OUTAGE_LIMIT_S`] has passed. Any other
/// result, or the last outage error, is returned.
fn retry_outage<T>(
    mut f: impl FnMut() -> Result<T, String>,
    mut waiting: impl FnMut(u64, &str),
    sleep: impl Fn(u64),
) -> Result<T, String> {
    let (mut waited, mut step) = (0u64, OUTAGE_STEP_S);
    loop {
        match f() {
            Err(e)
                if crate::hostedcheck::is_outage(&e)
                    && !crate::remote::gave_up_in_place(&e)
                    && waited < OUTAGE_LIMIT_S =>
            {
                waiting(
                    waited,
                    e.trim_start_matches(crate::hostedcheck::OUTAGE_PREFIX),
                );
                sleep(step);
                waited += step;
                step = (step * 2).min(OUTAGE_STEP_MAX_S);
            }
            r => return r,
        }
    }
}

#[cfg(test)]
mod outage_tests {
    use super::*;

    #[test]
    fn a_dns_failure_is_unreachable_and_a_refusal_is_not() {
        assert!(is_network_unreachable(
            "ssh: Could not resolve hostname github.com: nodename nor servname provided, or not known"
        ));
        assert!(!is_network_unreachable(
            "Permission denied (publickey). [remote rejected]"
        ));
    }

    fn outage() -> String {
        format!(
            "{}push rejected; run it again",
            crate::hostedcheck::OUTAGE_PREFIX
        )
    }

    #[test]
    fn an_outage_waits_and_retries_and_fails_no_task() {
        let mut calls = 0;
        let (mut waits, slept) = (vec![], std::cell::RefCell::new(vec![]));
        let r = retry_outage(
            || {
                calls += 1;
                if calls <= 2 {
                    Err(outage())
                } else {
                    Ok("clean")
                }
            },
            |_, why| waits.push(why.to_string()),
            |s| slept.borrow_mut().push(s),
        );
        assert_eq!(r, Ok("clean"));
        assert_eq!(calls, 3);
        assert_eq!(waits.len(), 2);
        assert!(waits[0].starts_with("push rejected"));
        assert_eq!(*slept.borrow(), vec![30, 60]);
    }

    /// #822: a run that already waited at its place three times in a row is the batch's error
    /// at once; there is no second wait around it.
    #[test]
    fn a_run_that_already_waited_in_place_is_not_waited_for_again() {
        let spent = format!(
            "{}no runner picked up x (3 environmental failures in a row, waiting again at its place each time)",
            crate::hostedcheck::OUTAGE_PREFIX
        );
        let mut calls = 0;
        let r: Result<(), String> = retry_outage(
            || {
                calls += 1;
                Err(spent.clone())
            },
            |_, _| panic!("no wait"),
            |_| panic!("no sleep"),
        );
        assert_eq!((r, calls), (Err(spent), 1));
    }

    #[test]
    fn an_outage_that_does_not_end_is_an_error_after_the_limit() {
        let slept = std::cell::Cell::new(0u64);
        let r: Result<(), String> =
            retry_outage(|| Err(outage()), |_, _| {}, |s| slept.set(slept.get() + s));
        assert_eq!(r, Err(outage()));
        assert!(slept.get() >= OUTAGE_LIMIT_S);
    }

    #[test]
    fn another_error_is_not_waited_for() {
        let mut calls = 0;
        let r: Result<(), String> = retry_outage(
            || {
                calls += 1;
                Err("git broke".into())
            },
            |_, _| panic!("no wait"),
            |_| panic!("no sleep"),
        );
        assert_eq!(r, Err("git broke".into()));
        assert_eq!(calls, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lint_parsers_read_coloured_output_and_rustfmt_diffs() {
        let out = "\x1b[1m\x1b[91merror\x1b[0m: x\n  \x1b[1m\x1b[94m--> \x1b[0mcrates/a/b.rs:969:42\nDiff in c/d.rs:3:\n";
        assert_eq!(lint_files(out, &[]), vec!["crates/a/b.rs", "c/d.rs"]);
        assert_eq!(lint_places(out), " (at crates/a/b.rs:969, c/d.rs:3)");
    }

    #[test]
    fn lint_files_reads_guard_lines_only_for_files_in_the_diff() {
        let known = vec!["scripts/exit/s960.sh".to_string(), "a/b.sh".to_string()];
        let out = "scripts/exit/s960.sh: runs swift without testing MASON_SKIP_SWIFT\n\
                   a/b.sh:12: bad thing\n\
                   scripts/other.sh: not in the diff\n\
                   note: something happened\n";
        assert_eq!(
            lint_files(out, &known),
            vec!["scripts/exit/s960.sh", "a/b.sh"]
        );
        assert!(lint_files("scripts/other.sh: x\nnote: y\n", &known).is_empty());
        assert!(lint_files(out, &[]).is_empty());
    }

    #[test]
    fn a_file_far_from_the_end_is_found_in_the_full_output() {
        let mut out = String::from("\x1b[94m--> \x1b[0mfar/away.rs:1:2\n");
        for i in 0..100 {
            out.push_str(&format!("note {i}\n"));
        }
        assert_eq!(lint_files(&out, &[]), vec!["far/away.rs"]);
        assert!(lint_files(&crate::jobs::tail(&out, 40), &[]).is_empty());
    }

    #[test]
    fn lint_places_names_the_files_and_lines_of_clippy_and_rustfmt() {
        let out = "warning: x\n  --> crates/store/src/lib.rs:1527:9\n   |\nDiff in /w/crates/a.rs at line 12:\n  --> crates/store/src/lib.rs:1527:9\n";
        assert_eq!(
            lint_places(out),
            " (at crates/store/src/lib.rs:1527, /w/crates/a.rs:12)"
        );
        assert_eq!(lint_places("all fine"), "");
    }

    #[test]
    fn the_jobs_summary_names_states_with_jobs_and_keeps_failed_visible() {
        let mut j = CiJobCounts::default();
        j.add("macOS build", "in_progress", None);
        j.add("a", "queued", None);
        j.add("b", "queued", None);
        j.add("c", "completed", Some("success"));
        j.add("d", "completed", Some("failure"));
        assert_eq!(
            jobs_summary(&j),
            "5 jobs: 2 queued, 1 running (macOS build), 1 passed, 1 failed"
        );
        assert_eq!(jobs_summary(&CiJobCounts::default()), "0 jobs");
    }

    /// A task first submitted before the daemon started has a `ready_at` before the
    /// daemon's start, so a restart does not make it wait out the window again.
    #[test]
    fn a_requeued_task_counts_its_window_from_its_first_submission() {
        let window = Duration::from_secs(3600);
        let clock = (1u64 << 32) + 5; // the new daemon is five seconds old
        let cand = |age_s: u64| plan::Candidate {
            id: 1,
            rank: 0,
            ready_at: clock.saturating_sub(age_s),
            footprint: Footprint::default(),
        };
        assert!(matches!(
            plan::form_batch(&[cand(4000)], clock, window, 0, 1),
            Formation::Form(_)
        ));
        assert!(matches!(
            plan::form_batch(&[cand(0)], clock, window, 0, 1),
            Formation::Wait(_)
        ));
    }

    #[test]
    fn a_hold_takes_runner_units_for_a_github_suite_and_local_slots_for_the_rest() {
        use crate::remote::units_for;
        use std::collections::BTreeMap;
        // The cargo workspace goes to GitHub sharded 3 ways (3 units); lint runs here and takes
        // one `compile`: the hold is both, and no `test-suite`.
        let lint = crate::sched::needs_of(&BTreeMap::from([("compile".to_string(), 1)]), None);
        let needs = hold_needs(lint, units_for("ubuntu-latest", 3));
        assert_eq!(
            needs.amounts,
            BTreeMap::from([("compile".to_string(), 1), ("gha".to_string(), 3)])
        );
        assert!(!needs.amounts.contains_key("test-suite"));
    }

    #[test]
    fn a_branch_held_by_a_git_lock_is_deleted_on_a_later_pass() {
        fn g(dir: &Path, args: &[&str]) {
            let o = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
        }
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("repo");
        let wt = t.path().join("wt");
        std::fs::create_dir(&repo).unwrap();
        g(&repo, &["init", "-q", "-b", "main"]);
        g(&repo, &["commit", "-q", "--allow-empty", "-m", "x"]);
        g(&repo, &["branch", "mason/int/p/b1"]);
        g(&repo, &["branch", "mason/int/p/b2"]);
        g(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                wt.to_str().unwrap(),
                "mason/int/p/b1",
            ],
        );
        let lock = repo.join(".git/worktrees/wt/index.lock");
        std::fs::write(&lock, "").unwrap();
        // b2 belongs to a running batch: it is not in the finished list.
        let finished = vec!["mason/int/p/b1".to_string()];
        delete_branches(&repo, &finished);
        let list = |d: &Path| git::run(d, &["branch", "--list", "mason/int/*"]).unwrap();
        assert!(list(&repo).contains("mason/int/p/b1"));
        std::fs::remove_file(&lock).unwrap();
        delete_branches(&repo, &finished);
        let left = list(&repo);
        assert!(!left.contains("b1"), "{left}");
        assert!(left.contains("b2"), "{left}");
    }
}
