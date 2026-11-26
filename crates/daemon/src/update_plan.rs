// SPDX-License-Identifier: GPL-3.0-or-later
//! MASON starts its own updates (#1082). What `update.rs` does once told to go, this module
//! decides: whether a newer build matters to the running daemon, how urgent it is from what it
//! brings, and when a takeover costs least. The rules here are pure (a fake clock and a fake
//! channel drive the tests); the thin glue that reads the mirror, the store and the scheduler
//! is at the end.
//!
//! - **urgent**: a landed urgent task, a fix of a base red or a landing freeze, an
//!   `Update-Urgent:` line, or a task the owner marked with `mason task update-priority`.
//!   Goes as soon as running landing batches finish.
//! - **normal**: code the daemon runs changed. Waits for a cheap moment, at most
//!   `normal_max_wait_s`.
//! - **off-peak**: only low-impact parts changed (the CLI, tests, docs, examples). Waits for
//!   the off-peak window or a quiet machine; a later normal or urgent update carries it.
//! - **none**: nothing the daemon or the CLI runs changed. No takeover.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::daemon::{now_ms, Daemon};
use crate::update::{self, Runtimes, UpdateCfg};
use mason_landing::git;

/// How much a pending update matters. The order is the order of urgency.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    None,
    OffPeak,
    Normal,
    Urgent,
}

impl Priority {
    pub fn name(self) -> &'static str {
        match self {
            Priority::None => "none",
            Priority::OffPeak => "off-peak",
            Priority::Normal => "normal",
            Priority::Urgent => "urgent",
        }
    }
}

pub const DEFAULT_NORMAL_MAX_WAIT_S: u64 = 4 * 3600;
/// A takeover that would interrupt this many things or fewer counts as cheap.
pub const DEFAULT_COST_MAX: u32 = 0;
pub const DEFAULT_OFFPEAK: &str = "01:00-06:00";

/// Paths that decide a change's weight; entries are `dir/` prefixes, `*suffix` endings or
/// exact paths.
pub fn default_none() -> Vec<String> {
    [
        "docs/",
        "clients/",
        ".github/",
        "*.md",
        "LICENSE",
        ".gitignore",
    ]
    .map(String::from)
    .to_vec()
}

pub fn default_low() -> Vec<String> {
    ["crates/cli/", "tests/", "examples/", "scripts/", ".mason/"]
        .map(String::from)
        .to_vec()
}

/// `[auto]` of `update.toml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoCfg {
    /// Off unless `[auto] enabled = true`: the owner's rule is that an update needs their OK,
    /// so masond starts none of its own until the owner turns this on.
    pub enabled: bool,
    pub normal_max_wait_s: u64,
    /// The off-peak window as minutes of the local day, `(from, to)`; `None` has no window.
    pub offpeak: Option<(u32, u32)>,
    /// A normal update goes when a takeover would interrupt at most this many things.
    pub cost_max: u32,
    /// The owner's rules; they win over the repository's file and the defaults.
    pub none: Option<Vec<String>>,
    pub low: Option<Vec<String>>,
}

impl Default for AutoCfg {
    fn default() -> Self {
        AutoCfg {
            enabled: false,
            normal_max_wait_s: DEFAULT_NORMAL_MAX_WAIT_S,
            offpeak: parse_window(DEFAULT_OFFPEAK),
            cost_max: DEFAULT_COST_MAX,
            none: None,
            low: None,
        }
    }
}

fn str_list(v: Option<&toml_edit::Item>) -> Option<Vec<String>> {
    v.and_then(|v| v.as_array()).map(|a| {
        a.iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect()
    })
}

impl AutoCfg {
    pub fn from_doc(doc: &toml_edit::DocumentMut) -> AutoCfg {
        let mut c = AutoCfg::default();
        let Some(t) = doc.get("auto").and_then(|t| t.as_table_like()) else {
            return c;
        };
        let int = |k: &str| {
            t.get(k)
                .and_then(|v| v.as_integer())
                .map(|n| n.max(0) as u64)
        };
        if let Some(b) = t.get("enabled").and_then(|v| v.as_bool()) {
            c.enabled = b;
        }
        if let Some(n) = int("normal_max_wait_s") {
            c.normal_max_wait_s = n;
        }
        if let Some(n) = int("cost_max") {
            c.cost_max = n.min(1000) as u32;
        }
        if let Some(s) = t.get("offpeak").and_then(|v| v.as_str()) {
            c.offpeak = parse_window(s);
        }
        c.none = str_list(t.get("none"));
        c.low = str_list(t.get("low"));
        c
    }
}

/// `01:00-06:00` as minutes of the day; a window that crosses midnight is `22:00-04:00`.
pub fn parse_window(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.split_once('-')?;
    let m = |t: &str| {
        let (h, m) = t.trim().split_once(':')?;
        let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
        (h < 24 && m < 60).then_some(h * 60 + m)
    };
    let (from, to) = (m(a)?, m(b)?);
    (from != to).then_some((from, to))
}

pub fn in_window(w: (u32, u32), minute: u32) -> bool {
    if w.0 < w.1 {
        minute >= w.0 && minute < w.1
    } else {
        minute >= w.0 || minute < w.1
    }
}

/// Minutes from `minute` until the window next opens (0 inside it).
pub fn minutes_to_window(w: (u32, u32), minute: u32) -> u32 {
    if in_window(w, minute) {
        0
    } else {
        (w.0 + 1440 - minute) % 1440
    }
}

/// The path rules in force: the owner's, else the repository's, else the defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rules {
    pub none: Vec<String>,
    pub low: Vec<String>,
}

impl Rules {
    /// `repo_file` is the text of `.mason/update-priority.toml` at the target, if there is one.
    pub fn resolve(cfg: &AutoCfg, repo_file: Option<&str>) -> Rules {
        let repo: Option<toml_edit::DocumentMut> = repo_file.and_then(|t| t.parse().ok());
        let repo_list = |k: &str| repo.as_ref().and_then(|d| str_list(d.get(k)));
        Rules {
            none: cfg
                .none
                .clone()
                .or_else(|| repo_list("none"))
                .unwrap_or_else(default_none),
            low: cfg
                .low
                .clone()
                .or_else(|| repo_list("low"))
                .unwrap_or_else(default_low),
        }
    }
}

fn matches(pat: &str, path: &str) -> bool {
    if let Some(suffix) = pat.strip_prefix('*') {
        // `*.md` is a file of the top level; deeper ones need a `dir/` rule of their own.
        !path.contains('/') && path.ends_with(suffix)
    } else if pat.ends_with('/') {
        path.starts_with(pat)
    } else {
        path == pat
    }
}

/// What a set of changed files weighs: the heaviest file decides.
pub fn classify_paths(files: &[String], rules: &Rules) -> Priority {
    let mut worst = Priority::None;
    for f in files {
        let p = if rules.none.iter().any(|r| matches(r, f)) {
            Priority::None
        } else if rules.low.iter().any(|r| matches(r, f)) {
            Priority::OffPeak
        } else {
            Priority::Normal
        };
        worst = worst.max(p);
    }
    worst
}

/// What makes a landed range urgent.
#[derive(Clone, Debug, Default)]
pub struct Urgency {
    /// The task numbers named by the range's merge commits.
    pub landed: Vec<u64>,
    /// Tasks of urgent priority.
    pub urgent_tasks: BTreeSet<u64>,
    /// Fix tasks of a base red or a landing freeze.
    pub fix_tasks: BTreeSet<u64>,
    /// Tasks the owner marked with `mason task update-priority`.
    pub marked: BTreeSet<u64>,
    /// A commit message has an `Update-Urgent:` line.
    pub urgent_line: bool,
}

/// Why the range is urgent, if it is.
pub fn urgent_reason(u: &Urgency) -> Option<String> {
    if u.urgent_line {
        return Some("a commit carries an Update-Urgent: line".into());
    }
    let pick = |set: &BTreeSet<u64>| u.landed.iter().find(|t| set.contains(t)).copied();
    if let Some(t) = pick(&u.marked) {
        return Some(format!("the owner marked #{t} urgent for updates"));
    }
    if let Some(t) = pick(&u.fix_tasks) {
        return Some(format!("#{t} fixes a red base or a landing freeze"));
    }
    pick(&u.urgent_tasks).map(|t| format!("urgent task #{t} landed"))
}

/// The task numbers a git log names: `Merge branch 'mason/<project>/t<N>'`.
pub fn task_ids_in(log: &str) -> Vec<u64> {
    let mut out = vec![];
    for tok in log.split(|c: char| c.is_whitespace() || "'\"`,;()".contains(c)) {
        let parts: Vec<&str> = tok.split('/').collect();
        if let [m, _, t] = parts.as_slice() {
            if *m == "mason" || m.ends_with("/mason") {
                if let Some(n) = t.strip_prefix('t').and_then(|n| n.parse::<u64>().ok()) {
                    if !out.contains(&n) {
                        out.push(n);
                    }
                }
            }
        }
    }
    out
}

/// What a takeover would interrupt now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    /// Agent checks in flight whose bridge sent no call id: they would start over (#1051).
    pub checks_no_replay: u32,
    /// Test steps in the middle of their suites.
    pub test_steps: u32,
    /// Landing batches that are running.
    pub batches: u32,
    /// An exclusive request is running.
    pub exclusive: bool,
}

impl Cost {
    pub fn total(&self) -> u32 {
        self.checks_no_replay + self.test_steps + self.batches
    }

    /// "3 checks would start over, 1 test step is mid-suite".
    pub fn describe(&self) -> String {
        let mut v = vec![];
        let pl = |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        if self.exclusive {
            v.push("an exclusive request is running".to_string());
        }
        if self.checks_no_replay > 0 {
            v.push(format!(
                "{} would start over",
                pl(self.checks_no_replay, "check", "checks")
            ));
        }
        if self.test_steps > 0 {
            v.push(format!(
                "{} mid-suite",
                pl(self.test_steps, "test step is", "test steps are")
            ));
        }
        if self.batches > 0 {
            v.push(format!(
                "{} running",
                pl(self.batches, "landing batch is", "landing batches are")
            ));
        }
        v.join(", ")
    }
}

/// An update that waits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub target: String,
    pub priority: Priority,
    /// Why it has that priority, one entry per cause.
    pub reasons: Vec<String>,
    pub since_ms: u64,
    /// What it waits for now, and the latest time it goes (ms; 0: not bounded by time).
    pub waiting_for: String,
    pub latest_ms: u64,
}

impl Pending {
    pub fn why(&self) -> String {
        format!("{}: {}", self.priority.name(), self.reasons.join("; "))
    }
}

/// A newer build replaces the waiting one; the priority is the higher of the two and the
/// causes of both are kept. The wait began with the first of them.
pub fn merge(old: Option<&Pending>, new: Pending) -> Pending {
    let Some(old) = old else { return new };
    let mut reasons = old.reasons.clone();
    for r in &new.reasons {
        if !reasons.contains(r) {
            reasons.push(r.clone());
        }
    }
    Pending {
        priority: old.priority.max(new.priority),
        reasons,
        since_ms: old.since_ms.min(new.since_ms),
        ..new
    }
}

#[derive(Clone, Debug)]
pub struct DecideInput {
    pub priority: Priority,
    pub now_ms: u64,
    pub since_ms: u64,
    pub cost: Cost,
    /// Minutes of the local day.
    pub minute: u32,
    /// No agent is past its first hour, no landing verifies and no step runs.
    pub quiet: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Go(String),
    /// What it waits for, and the latest time it goes (0: no bound).
    Wait {
        for_what: String,
        latest_ms: u64,
    },
    /// No takeover at all.
    Never,
}

pub fn decide(cfg: &AutoCfg, i: &DecideInput) -> Verdict {
    match i.priority {
        Priority::None => Verdict::Never,
        Priority::Urgent => Verdict::Go("urgent".into()),
        _ if i.cost.exclusive => Verdict::Wait {
            for_what: "an exclusive request to finish".into(),
            latest_ms: 0,
        },
        Priority::Normal => {
            let latest = i.since_ms.saturating_add(cfg.normal_max_wait_s * 1000);
            if i.cost.total() <= cfg.cost_max {
                Verdict::Go("a takeover interrupts nothing".into())
            } else if i.now_ms >= latest {
                Verdict::Go("the longest wait is over".into())
            } else {
                Verdict::Wait {
                    for_what: i.cost.describe(),
                    latest_ms: latest,
                }
            }
        }
        Priority::OffPeak => {
            let window = cfg.offpeak;
            if window.is_some_and(|w| in_window(w, i.minute)) {
                Verdict::Go("the off-peak window".into())
            } else if i.quiet && i.cost.total() == 0 {
                Verdict::Go("a quiet machine".into())
            } else {
                let latest_ms = window.map_or(0, |w| {
                    i.now_ms + u64::from(minutes_to_window(w, i.minute)) * 60_000
                });
                Verdict::Wait {
                    for_what: "the off-peak window or a quiet machine".into(),
                    latest_ms,
                }
            }
        }
    }
}

// ---- what is in flight ----------------------------------------------------------------------------

/// What runs in this process that a takeover would cut.
#[derive(Debug)]
pub struct InFlight {
    checks_no_replay: AtomicUsize,
    test_steps: AtomicUsize,
}

impl InFlight {
    pub const fn new() -> InFlight {
        InFlight {
            checks_no_replay: AtomicUsize::new(0),
            test_steps: AtomicUsize::new(0),
        }
    }

    /// Held while a `run_check` call runs; only one without a call id counts (#1051).
    pub fn check(&'static self, has_call_id: bool) -> Option<Guard> {
        (!has_call_id).then(|| Guard::new(&self.checks_no_replay))
    }

    /// Held while a test step runs its suites.
    pub fn test_step(&'static self) -> Guard {
        Guard::new(&self.test_steps)
    }
}

impl Default for InFlight {
    fn default() -> Self {
        InFlight::new()
    }
}

static IN_FLIGHT: InFlight = InFlight::new();

/// One count, given back when dropped.
pub struct Guard(&'static AtomicUsize);

impl Guard {
    fn new(n: &'static AtomicUsize) -> Guard {
        n.fetch_add(1, Ordering::SeqCst);
        Guard(n)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn check_started(has_call_id: bool) -> Option<Guard> {
    IN_FLIGHT.check(has_call_id)
}

pub fn test_step_started() -> Guard {
    IN_FLIGHT.test_step()
}

/// The cost of a takeover now.
pub fn cost_now(d: &Daemon) -> Cost {
    Cost {
        checks_no_replay: IN_FLIGHT.checks_no_replay.load(Ordering::SeqCst) as u32,
        test_steps: IN_FLIGHT.test_steps.load(Ordering::SeqCst) as u32,
        batches: d.landing.busy_projects().len() as u32,
        exclusive: d.broker.exclusive_running(),
    }
}

// ---- the owner's marks ----------------------------------------------------------------------------

fn marks_file(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("update-urgent.txt")
}

pub fn marked_tasks(data_dir: &Path) -> BTreeSet<u64> {
    std::fs::read_to_string(marks_file(data_dir))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

/// `mason task update-priority <id> urgent|normal`: whether a landing of the task takes
/// over at once.
pub fn mark_task(data_dir: &Path, task: u64, urgent: bool) -> std::io::Result<()> {
    let mut s = marked_tasks(data_dir);
    if urgent {
        s.insert(task);
    } else {
        s.remove(&task);
    }
    let text: String = s.iter().map(|t| format!("{t}\n")).collect();
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(marks_file(data_dir), text)
}

// ---- state kept by the daemon ---------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct PlanState {
    pub pending: Option<Pending>,
    /// The target a planned update is building or taking over to; nothing else is scheduled
    /// meanwhile.
    pub running: Option<String>,
    /// The newest target found with priority none, and why.
    pub none_target: Option<(String, String)>,
    /// A target whose build failed, and when: not tried again for an hour.
    pub failed: Option<(String, u64)>,
}

const RETRY_AFTER_FAILURE_MS: u64 = 3_600_000;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the last look at the mirror says about `running..target`.
pub fn assess_range(
    d: &Daemon,
    cfg: &UpdateCfg,
    mirror: &Path,
    running: &str,
    target: &str,
) -> Result<(Priority, Vec<String>), String> {
    let range = format!("{running}..{target}");
    let files: Vec<String> = git::run(mirror, &["diff", "--name-only", running, target])
        .map_err(|e| e.to_string())?
        .lines()
        .map(String::from)
        .collect();
    let repo_file = git::run(
        mirror,
        &["show", &format!("{target}:.mason/update-priority.toml")],
    )
    .ok();
    let rules = Rules::resolve(&cfg.auto, repo_file.as_deref());
    let by_paths = classify_paths(&files, &rules);
    let log = git::run(mirror, &["log", "--format=%s%n%b", &range]).unwrap_or_default();
    let landed = task_ids_in(&log);
    let mut u = Urgency {
        urgent_line: log
            .lines()
            .any(|l| l.trim_start().starts_with("Update-Urgent:")),
        marked: marked_tasks(&d.cfg.data_dir),
        landed: landed.clone(),
        ..Default::default()
    };
    if let Ok(w) = d.world() {
        for t in &landed {
            if w.tasks
                .get(&mason_core::TaskId(*t))
                .is_some_and(|t| t.priority == mason_core::Priority::Urgent)
            {
                u.urgent_tasks.insert(*t);
            }
        }
    }
    if let Ok(fs) = d.store.freezes() {
        u.fix_tasks
            .extend(fs.into_iter().filter_map(|f| f.fix_task));
    }
    if let Some(why) = urgent_reason(&u) {
        // An urgent change that touches nothing the daemon runs needs no takeover.
        if by_paths != Priority::None {
            return Ok((Priority::Urgent, vec![why]));
        }
    }
    let tasks = if landed.is_empty() {
        String::new()
    } else {
        format!(
            " in {}",
            landed
                .iter()
                .take(4)
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let why = match by_paths {
        Priority::None => "nothing the daemon or the CLI runs changed".to_string(),
        Priority::OffPeak => format!("only low-impact parts changed{tasks}"),
        _ => format!("daemon code changed{tasks}"),
    };
    Ok((by_paths, vec![why]))
}

/// Called after every look at the channel: file what a newer build brings. A newer build
/// replaces a waiting one.
pub fn found_newer(d: &Arc<Daemon>, cfg: &UpdateCfg, target: &str) {
    // A channel switched to an improvement's own branch is the owner's to take over, as its
    // `switch` setting says.
    if !cfg.auto.enabled || crate::improve::switched(d).is_some() {
        lock(&d.update_plan).pending = None;
        return;
    }
    let Some(running) = d.cfg.runtime_sha.clone() else {
        return;
    };
    let rt = Runtimes::new(&d.cfg.data_dir);
    let (priority, reasons) = match assess_range(d, cfg, &rt.mirror(), &running, target) {
        Ok(x) => x,
        Err(e) => {
            tracing::warn!("auto-update: could not read what {target} brings: {e}");
            return;
        }
    };
    let mut p = lock(&d.update_plan);
    // The build under way is not a newer one: filing it again would replace it with itself.
    if p.running.as_deref() == Some(target) {
        return;
    }
    if p.failed
        .as_ref()
        .is_some_and(|(t, at)| t == target && now_ms().saturating_sub(*at) < RETRY_AFTER_FAILURE_MS)
    {
        return;
    }
    if priority == Priority::None {
        p.none_target = Some((target.to_string(), reasons.join("; ")));
        // What waits already carries the newer build's changes: it is built from the newer target.
        if let Some(old) = p.pending.take() {
            p.pending = Some(Pending {
                target: target.to_string(),
                ..old
            });
        }
        drop(p);
        d.poke();
        return;
    }
    p.none_target = None;
    let new = Pending {
        target: target.to_string(),
        priority,
        reasons,
        since_ms: now_ms(),
        waiting_for: String::new(),
        latest_ms: 0,
    };
    let same = p
        .pending
        .as_ref()
        .is_some_and(|o| o.target == target && o.priority >= priority);
    if !same {
        let merged = merge(p.pending.as_ref(), new);
        p.pending = Some(merged);
        if p.running.is_some() {
            // A planned update waits for landing batches: let it go and build the newer one.
            d.update_replace.store(true, Ordering::SeqCst);
        }
    }
    drop(p);
    d.poke();
}

/// One look at what waits: go, or say what it waits for. Cheap; no fetch.
pub fn tick(d: &Arc<Daemon>) {
    let Ok(cfg) = UpdateCfg::load(&d.cfg.config_dir) else {
        return;
    };
    if !cfg.auto.enabled {
        return;
    }
    let busy = matches!(
        lock(&d.update_status).phase.as_str(),
        "fetching" | "building" | "testing" | "waiting for landing" | "taking over"
    );
    let mut p = lock(&d.update_plan);
    if p.running.is_some() || busy {
        return;
    }
    let Some(mut pending) = p.pending.clone() else {
        return;
    };
    let now = now_ms();
    let minute = clock_minute(now);
    let (n, oldest) = d.broker.task_work(now);
    let cost = cost_now(d);
    let v = decide(
        &cfg.auto,
        &DecideInput {
            priority: pending.priority,
            now_ms: now,
            since_ms: pending.since_ms,
            cost,
            minute,
            quiet: n == 0 && oldest < 3_600_000 && d.landing.busy_projects().is_empty(),
        },
    );
    match v {
        Verdict::Never => {
            p.pending = None;
        }
        Verdict::Wait {
            for_what,
            latest_ms,
        } => {
            if pending.waiting_for != for_what || pending.latest_ms != latest_ms {
                pending.waiting_for = for_what;
                pending.latest_ms = latest_ms;
                p.pending = Some(pending);
                drop(p);
                d.poke();
            }
        }
        Verdict::Go(why) => {
            p.running = Some(pending.target.clone());
            p.pending = None;
            drop(p);
            tracing::info!(
                "auto-update to {} goes now ({}): {why}",
                pending.target,
                pending.why()
            );
            let d2 = d.clone();
            let spawned = std::thread::Builder::new()
                .name("update-auto".into())
                .spawn(move || run_planned(&d2, pending));
            if spawned.is_err() {
                lock(&d.update_plan).running = None;
            }
        }
    }
}

/// Local minute of the day (`date` knows the time zone).
fn clock_minute(now: u64) -> u32 {
    let t = update::clock_at(now);
    let t = t.trim_end_matches(" UTC");
    t.split_once(':')
        .and_then(|(h, m)| Some(h.parse::<u32>().ok()? * 60 + m.parse::<u32>().ok()?))
        .unwrap_or(0)
}

/// Build, prove and take over, following newer targets that replace this one while it waits
/// for landing batches.
fn run_planned(d: &Arc<Daemon>, first: Pending) {
    let mut cur = first;
    loop {
        d.update_replace.store(false, Ordering::SeqCst);
        let req = update::Request {
            sha: Some(cur.target.clone()),
            auto: true,
            planned: true,
            urgent: cur.priority == Priority::Urgent,
            ..Default::default()
        };
        let target = match update::run(d, &req) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("auto-update to {} did not go: {e}", cur.target);
                let mut p = lock(&d.update_plan);
                p.failed = Some((cur.target.clone(), now_ms()));
                p.running = None;
                // A newer build filed meanwhile must not cut a later drain short.
                d.update_replace.store(false, Ordering::SeqCst);
                return;
            }
        };
        std::thread::sleep(std::time::Duration::from_millis(500));
        update::drain(d);
        if d.update_replace.swap(false, Ordering::SeqCst) {
            let next = lock(&d.update_plan).pending.take();
            if let Some(next) = next {
                d.landing.set_draining(false);
                lock(&d.update_status).phase = "idle".into();
                tracing::info!("auto-update: {} replaces {target}", next.target);
                lock(&d.update_plan).running = Some(next.target.clone());
                cur = next;
                continue;
            }
        }
        // Only returns if the new runtime could not be started.
        update::take_over(d, &target);
        d.landing.set_draining(false);
        let mut p = lock(&d.update_plan);
        p.failed = Some((target, now_ms()));
        p.running = None;
        d.update_replace.store(false, Ordering::SeqCst);
        return;
    }
}

pub fn spawn_planner(d: &Arc<Daemon>) {
    crate::bg::spawn_loop(d, "update-plan", move |weak| {
        let Some(d) = weak.upgrade() else {
            return false;
        };
        tick(&d);
        drop(d);
        std::thread::sleep(std::time::Duration::from_secs(30));
        true
    });
}

/// The lines `mason update status` and the monitor's banner add.
pub fn status_lines(d: &Daemon, now: u64) -> Vec<String> {
    let p = lock(&d.update_plan);
    let mut out = vec![];
    if let Some(p) = &p.pending {
        out.push(format!(
            "{} update to {} waits ({})",
            p.priority.name(),
            &p.target[..p.target.len().min(8)],
            p.why()
        ));
        if !p.waiting_for.is_empty() {
            out.push(format!("waiting for: {}", p.waiting_for));
        }
        if p.latest_ms > now {
            out.push(format!(
                "goes at {} at the latest",
                update::clock_at(p.latest_ms)
            ));
        }
    }
    if let Some((t, why)) = &p.none_target {
        out.push(format!(
            "none: {} needs no takeover ({why})",
            &t[..t.len().min(8)]
        ));
    }
    out
}

/// The cost now, for `mason update status`.
pub fn cost_line(d: &Daemon) -> String {
    let c = cost_now(d);
    if c.total() == 0 && !c.exclusive {
        "a takeover would interrupt nothing".into()
    } else {
        format!("a takeover now: {}", c.describe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        Rules::resolve(&AutoCfg::default(), None)
    }
    fn files(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn paths_classify() {
        let r = rules();
        assert_eq!(
            classify_paths(&files(&["crates/daemon/src/x.rs"]), &r),
            Priority::Normal
        );
        assert_eq!(
            classify_paths(&files(&["Cargo.lock"]), &r),
            Priority::Normal
        );
        assert_eq!(
            classify_paths(&files(&["crates/cli/src/main.rs", "tests/a.sh"]), &r),
            Priority::OffPeak
        );
        assert_eq!(
            classify_paths(&files(&["README.md", "docs/a.md", "clients/x/y.swift"]), &r),
            Priority::None
        );
        // One daemon file among low ones makes the whole range normal: the low ones ride along.
        assert_eq!(
            classify_paths(&files(&["crates/cli/a.rs", "crates/daemon/b.rs"]), &r),
            Priority::Normal
        );
    }

    #[test]
    fn owner_rules_win_over_the_repository() {
        let repo = "none = [\"a/\"]\nlow = [\"b/\"]\n";
        let c = AutoCfg::default();
        let r = Rules::resolve(&c, Some(repo));
        assert_eq!(classify_paths(&files(&["a/x"]), &r), Priority::None);
        assert_eq!(classify_paths(&files(&["b/x"]), &r), Priority::OffPeak);
        let owner = AutoCfg {
            none: Some(vec!["b/".into()]),
            ..AutoCfg::default()
        };
        let r = Rules::resolve(&owner, Some(repo));
        assert_eq!(classify_paths(&files(&["b/x"]), &r), Priority::None);
        // The repository's `low` still applies where the owner said nothing.
        assert_eq!(
            classify_paths(&files(&["b/x", "a/x"]), &r),
            Priority::Normal
        );
    }

    #[test]
    fn urgency_comes_from_what_landed() {
        let mut u = Urgency {
            landed: vec![5, 6],
            ..Default::default()
        };
        assert_eq!(urgent_reason(&u), None);
        u.urgent_tasks.insert(6);
        assert!(urgent_reason(&u).unwrap().contains("#6"));
        let u2 = Urgency {
            landed: vec![5],
            fix_tasks: [5].into(),
            ..Default::default()
        };
        assert!(urgent_reason(&u2).unwrap().contains("fixes"));
        let u3 = Urgency {
            landed: vec![5],
            marked: [5].into(),
            ..Default::default()
        };
        assert!(urgent_reason(&u3).unwrap().contains("owner"));
        let u4 = Urgency {
            urgent_line: true,
            ..Default::default()
        };
        assert!(urgent_reason(&u4).is_some());
    }

    #[test]
    fn task_ids_are_read_from_merge_subjects() {
        let log = "Merge branch 'mason/mason/t975'\nMerge remote-tracking branch 'origin/develop' into claude/fix-1025\nMerge branch 'mason/star/t1058'\nRegenerate the README status block";
        assert_eq!(task_ids_in(log), vec![975, 1058]);
    }

    fn input(priority: Priority, cost: Cost) -> DecideInput {
        DecideInput {
            priority,
            now_ms: 1_000_000,
            since_ms: 1_000_000,
            cost,
            minute: 12 * 60,
            quiet: false,
        }
    }
    fn check_without_call() -> Cost {
        Cost {
            checks_no_replay: 1,
            ..Default::default()
        }
    }

    #[test]
    fn normal_waits_for_a_cheap_moment_or_its_longest_wait() {
        let c = AutoCfg::default();
        let busy = input(Priority::Normal, check_without_call());
        match decide(&c, &busy) {
            Verdict::Wait {
                for_what,
                latest_ms,
            } => {
                assert!(for_what.contains("1 check would start over"));
                assert_eq!(latest_ms, 1_000_000 + c.normal_max_wait_s * 1000);
            }
            v => panic!("{v:?}"),
        }
        // The check ends.
        assert!(matches!(
            decide(&c, &input(Priority::Normal, Cost::default())),
            Verdict::Go(_)
        ));
        // Or the longest wait runs out.
        let late = DecideInput {
            now_ms: 1_000_000 + c.normal_max_wait_s * 1000,
            ..busy.clone()
        };
        assert!(matches!(decide(&c, &late), Verdict::Go(_)));
        // Never in the middle of an exclusive request, even past the longest wait.
        let excl = DecideInput {
            cost: Cost {
                exclusive: true,
                ..Default::default()
            },
            ..late
        };
        assert!(matches!(decide(&c, &excl), Verdict::Wait { .. }));
    }

    #[test]
    fn urgent_goes_at_once_and_none_never() {
        let c = AutoCfg::default();
        let cost = Cost {
            checks_no_replay: 3,
            test_steps: 2,
            batches: 1,
            exclusive: false,
        };
        assert_eq!(
            decide(&c, &input(Priority::Urgent, cost.clone())),
            Verdict::Go("urgent".into())
        );
        assert_eq!(decide(&c, &input(Priority::None, cost)), Verdict::Never);
    }

    #[test]
    fn off_peak_waits_for_the_window_or_a_quiet_machine() {
        let c = AutoCfg::default();
        let noon = input(Priority::OffPeak, Cost::default());
        match decide(&c, &noon) {
            Verdict::Wait { latest_ms, .. } => {
                // 12:00 to 01:00 is 13 hours.
                assert_eq!(latest_ms, 1_000_000 + 13 * 3_600_000);
            }
            v => panic!("{v:?}"),
        }
        let night = DecideInput {
            minute: 2 * 60,
            ..noon.clone()
        };
        assert!(matches!(decide(&c, &night), Verdict::Go(_)));
        let quiet = DecideInput {
            quiet: true,
            ..noon
        };
        assert!(matches!(decide(&c, &quiet), Verdict::Go(_)));
    }

    #[test]
    fn windows_cross_midnight() {
        let w = parse_window("22:00-04:00").unwrap();
        assert!(in_window(w, 23 * 60));
        assert!(in_window(w, 3 * 60));
        assert!(!in_window(w, 12 * 60));
        assert_eq!(minutes_to_window(w, 21 * 60), 60);
        assert_eq!(parse_window("nonsense"), None);
    }

    fn pend(target: &str, p: Priority, why: &str, since: u64) -> Pending {
        Pending {
            target: target.into(),
            priority: p,
            reasons: vec![why.into()],
            since_ms: since,
            waiting_for: String::new(),
            latest_ms: 0,
        }
    }

    #[test]
    fn a_newer_build_replaces_a_waiting_one_and_keeps_the_higher_priority() {
        let old = pend("aaa", Priority::Urgent, "urgent task #1 landed", 10);
        let new = pend(
            "bbb",
            Priority::OffPeak,
            "only low-impact parts changed",
            50,
        );
        let m = merge(Some(&old), new);
        assert_eq!(m.target, "bbb");
        assert_eq!(m.priority, Priority::Urgent);
        assert_eq!(m.reasons.len(), 2);
        assert_eq!(m.since_ms, 10);
        let m2 = merge(
            Some(&pend("a", Priority::OffPeak, "x", 5)),
            pend("b", Priority::Normal, "y", 9),
        );
        assert_eq!(m2.priority, Priority::Normal);
        assert_eq!(merge(None, pend("c", Priority::Normal, "z", 1)).target, "c");
    }

    #[test]
    fn marks_round_trip() {
        let dir = std::env::temp_dir().join(format!("mason-marks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        mark_task(&dir, 7, true).unwrap();
        mark_task(&dir, 9, true).unwrap();
        mark_task(&dir, 7, false).unwrap();
        assert_eq!(marked_tasks(&dir), [9].into());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn guards_count_checks_without_a_call_id() {
        // Its own counters: other tests of this process run checks too.
        let f: &'static InFlight = Box::leak(Box::new(InFlight::new()));
        let counts = || {
            (
                f.checks_no_replay.load(Ordering::SeqCst),
                f.test_steps.load(Ordering::SeqCst),
            )
        };
        let a = f.check(false);
        let b = f.check(true);
        let s = f.test_step();
        assert_eq!(counts(), (1, 1));
        drop((a, b, s));
        assert_eq!(counts(), (0, 0));
    }

    #[test]
    fn auto_config_reads_the_table() {
        let doc: toml_edit::DocumentMut =
            "[auto]\nenabled = true\nnormal_max_wait_s = 60\noffpeak = \"02:00-03:00\"\ncost_max = 2\nnone = [\"x/\"]\n"
                .parse()
                .unwrap();
        let c = AutoCfg::from_doc(&doc);
        assert!(c.enabled);
        assert_eq!(c.normal_max_wait_s, 60);
        assert_eq!(c.offpeak, Some((120, 180)));
        assert_eq!(c.cost_max, 2);
        assert_eq!(c.none, Some(vec!["x/".to_string()]));
        // Nothing is switched on by silence: no `[auto]`, or one without `enabled`.
        assert!(!AutoCfg::from_doc(&"".parse().unwrap()).enabled);
        assert!(!AutoCfg::from_doc(&"[auto]\ncost_max = 1\n".parse().unwrap()).enabled);
    }
}
