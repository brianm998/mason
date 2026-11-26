// SPDX-License-Identifier: GPL-3.0-or-later

//! What runs where, now (#1155). [`crate::place::seen`] keeps the last placement *decisions*;
//! this module answers which runs are *in flight*, on which host.
//!
//! One list is put together from four live sources, so nothing in it outlives its run:
//!
//! - the registry here: node runs (entered where the job is followed, `node_exec::follow`), and
//!   local checks and command steps. Each entry is held by a [`Guard`] and leaves with it,
//!   however the run ends, an early return or a panic included;
//! - GitHub: the holds of runs that are out (`remote::running_holds`);
//! - agent sessions: the tasks that hold an `llm:` lease (`Broker::llm_holders`);
//! - local suites: the step hosts whose command still runs and that `suites.rs` started (they
//!   have its `run.key` file). `suites.rs` decides verdicts and is protected, so its runs are
//!   read from what it already leaves on disk and not registered by it.
//!
//! A run a node or GitHub gives back drops out of that host when it returns there, and the host
//! that runs it next enters it.

use crate::daemon::{now_ms, Daemon};
use mason_core::TaskId;
use mason_engine::World;
use mason_protocol::{HostRun, HostView};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Where a run is.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Host {
    /// This Mac.
    Local,
    /// A paired node, by the name the owner gave it.
    Node(String),
    /// GitHub Actions, on a runner with this label.
    GitHub(String),
}

impl Host {
    /// `local`, `node:<name>` or `github:<runs_on>`.
    pub fn key(&self) -> String {
        match self {
            Host::Local => "local".into(),
            Host::Node(n) => format!("node:{n}"),
            Host::GitHub(r) => format!("github:{r}"),
        }
    }

    /// `local`, `node` or `github`.
    pub fn kind(&self) -> &'static str {
        match self {
            Host::Local => "local",
            Host::Node(_) => "node",
            Host::GitHub(_) => "github",
        }
    }
}

/// What a run is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A step's LLM session.
    Agent,
    Check,
    /// A suite or command of a task's step.
    Suite,
    /// A landing batch's run, or another run of the project that no task owns.
    Batch,
    /// A run that pays a testing debt.
    Debt,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Agent => "agent",
            Kind::Check => "check",
            Kind::Suite => "suite",
            Kind::Batch => "batch",
            Kind::Debt => "debt",
        }
    }
}

/// One run in flight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// The run's host or record id (empty for an agent session).
    pub id: String,
    pub project: String,
    /// `None` for a landing batch's run, or a debt not yet matched to its task.
    pub task: Option<u64>,
    pub kind: Kind,
    /// The suite or check name; for an agent, the step.
    pub name: String,
    pub host: Host,
    pub since_ms: u64,
    /// The run's page (a GitHub run), empty when there is none.
    pub url: String,
}

/// What a host id says about its run: its kind and, when the id names it, its task. A debt's id
/// names the debt, not the task ([`runs`] looks the task up). Pure.
pub fn classify(id: &str) -> (Kind, Option<u64>) {
    let number = |s: &str| {
        let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
        digits.parse::<u64>().ok()
    };
    if let Some(n) = id.strip_prefix("ledger").and_then(number) {
        return (Kind::Debt, Some(n));
    }
    if id.strip_prefix("debt").and_then(number).is_some() {
        return (Kind::Debt, None);
    }
    if crate::sweep::parse_batch_host_id(id).is_some() || id.starts_with("full-") {
        return (Kind::Batch, None);
    }
    // `t<task>r<run>`, with `p<pass>` after the run for a later pass.
    let task = id.strip_prefix('t').and_then(|rest| {
        let (task, run) = rest.split_once('r')?;
        number(run).and(task.parse::<u64>().ok())
    });
    match task {
        Some(t) if id.contains("-check") => (Kind::Check, Some(t)),
        Some(t) => (Kind::Suite, Some(t)),
        None => (Kind::Check, None),
    }
}

// ---- the registry -----------------------------------------------------------------------------

/// Entry number to (the daemon it belongs to, the run). Tests run several daemons in one process.
static REG: Mutex<BTreeMap<u64, (PathBuf, Run)>> = Mutex::new(BTreeMap::new());
static NEXT: AtomicU64 = AtomicU64::new(1);

fn reg() -> std::sync::MutexGuard<'static, BTreeMap<u64, (PathBuf, Run)>> {
    REG.lock().unwrap_or_else(|e| e.into_inner())
}

/// A registered run; dropping it takes the run off the list.
#[must_use = "the run is listed only while the guard lives"]
pub struct Guard(u64);

impl Drop for Guard {
    fn drop(&mut self) {
        reg().remove(&self.0);
    }
}

/// Enter `run` for the daemon whose step hosts are under `scope`.
pub fn start(scope: &Path, run: Run) -> Guard {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    reg().insert(n, (scope.to_path_buf(), run));
    Guard(n)
}

/// The registered runs of the daemon whose step hosts are under `scope`, oldest entry first.
pub fn registered(scope: &Path) -> Vec<Run> {
    reg()
        .values()
        .filter(|(s, _)| s == scope)
        .map(|(_, r)| r.clone())
        .collect()
}

// ---- the sources that are read ----------------------------------------------------------------

fn project_name(w: &World, task: u64) -> Option<String> {
    let t = w.tasks.get(&TaskId(task))?;
    w.projects.get(&t.project).map(|p| p.name.clone())
}

/// The step sessions: every task that holds an `llm:` lease.
fn agents(d: &Daemon, w: &World) -> Vec<Run> {
    d.broker
        .llm_holders()
        .into_iter()
        .filter_map(|id| {
            let t = w.tasks.get(&id)?;
            let since = w
                .open_run(id)
                .and_then(|r| w.step_runs.get(&r))
                .map_or(0, |r| r.started_at.0);
            Some(Run {
                id: String::new(),
                project: project_name(w, id.0).unwrap_or_default(),
                task: Some(id.0),
                kind: Kind::Agent,
                name: t.step.clone(),
                host: Host::Local,
                since_ms: since,
                url: String::new(),
            })
        })
        .collect()
}

/// The suite a suite host's id ends with, the longest of `names` that does; the id itself when
/// none does. A fresh host after a lost one ends `-host1`. Pure.
fn suite_in<'a>(id: &'a str, names: &'a [String]) -> &'a str {
    let stem = id.strip_suffix("-host1").unwrap_or(id);
    names
        .iter()
        .filter(|n| {
            stem.strip_suffix(n.as_str())
                .is_some_and(|head| head.ends_with('-'))
        })
        .max_by_key(|n| n.len())
        .map_or(id, String::as_str)
}

/// The suites running under step hosts on this Mac that `suites.rs` started.
fn suite_hosts(d: &Daemon, w: &World) -> Vec<Run> {
    use mason_step_host::registry::{status_of, HostState};
    let Ok(dirs) = std::fs::read_dir(d.cfg.hosts_dir()) else {
        return vec![];
    };
    dirs.flatten()
        .filter_map(|e| {
            let dir = e.path();
            // The cheap looks first: most directories are of hosts that ended.
            if dir.join("exit.kv").exists() || !dir.join("run.key").exists() {
                return None;
            }
            let s = status_of(&dir).ok()?;
            if s.state != HostState::Running {
                return None;
            }
            let id = s.info.id;
            let (kind, task) = classify(&id);
            let mut run = Run {
                project: task.and_then(|t| project_name(w, t)).unwrap_or_default(),
                task,
                kind,
                name: id.clone(),
                host: Host::Local,
                since_ms: s.info.started_ms,
                url: String::new(),
                id,
            };
            complete(d, w, &mut run);
            let names: Vec<String> = d
                .project_id_by_name(&run.project)
                .map(|p| {
                    d.current_def(p)
                        .testing
                        .suites
                        .iter()
                        .map(|s| s.name.clone())
                        .collect()
                })
                .unwrap_or_default();
            run.name = suite_in(&run.id, &names).to_string();
            Some(run)
        })
        .collect()
}

/// The runs that are out on GitHub, from their holds.
fn github(d: &Daemon) -> Vec<Run> {
    crate::remote::running_holds(&crate::remote::holds_dir(&d.cfg.hosts_dir()))
        .into_iter()
        .map(|h| {
            let id = h
                .record
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let (url, runs_on) = crate::remote::record_view(&h.record);
            let kind = match h.kind.as_str() {
                "check" => Kind::Check,
                "test_step" => Kind::Suite,
                "debt" => Kind::Debt,
                _ => Kind::Batch,
            };
            Run {
                project: h.project,
                // A hold says 0 for no task; a project's first task is 0 too, and its run's id
                // names it.
                task: (h.task != 0).then_some(h.task).or_else(|| classify(&id).1),
                kind,
                name: if h.suite.is_empty() {
                    id.clone()
                } else {
                    h.suite
                },
                host: Host::GitHub(runs_on),
                since_ms: crate::remote::record_since(&h.record),
                url,
                id,
            }
        })
        .collect()
}

/// Fill in what the source of a run did not say: a debt's task, and the project.
fn complete(d: &Daemon, w: &World, run: &mut Run) {
    if run.task.is_none() && run.kind == Kind::Debt {
        let debt = run.id.strip_prefix("debt").and_then(|s| {
            let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u64>().ok()
        });
        if let Some(n) = debt {
            if let Some(row) = d
                .store
                .debts(None, false)
                .unwrap_or_default()
                .into_iter()
                .find(|r| r.id == n)
            {
                run.task = Some(row.task);
                if run.project.is_empty() {
                    run.project = row.project;
                }
            }
        }
    }
    if !run.project.is_empty() {
        return;
    }
    if let Some(p) = run.task.and_then(|t| project_name(w, t)) {
        run.project = p;
    } else if let Some(n) = crate::sweep::parse_batch_host_id(&run.id) {
        run.project = d
            .store
            .batches(None, 500)
            .unwrap_or_default()
            .into_iter()
            .find(|b| b.id == n)
            .map_or_else(String::new, |b| b.project);
    } else if let Some(rest) = run.id.strip_prefix("full-") {
        run.project = d
            .projects()
            .values()
            .map(|p| p.id.clone())
            .filter(|p| rest.starts_with(&format!("{p}-")))
            .max_by_key(String::len)
            .unwrap_or_default();
    }
}

/// Every run in flight, ordered by host, then start.
pub fn runs(d: &Daemon, w: &World) -> Vec<Run> {
    let mut out = registered(&d.cfg.hosts_dir());
    out.extend(github(d));
    for r in &mut out {
        complete(d, w, r);
    }
    out.extend(agents(d, w));
    out.extend(suite_hosts(d, w));
    out.sort_by(|a, b| {
        (&a.host, a.since_ms, a.task, a.kind, &a.name)
            .cmp(&(&b.host, b.since_ms, b.task, b.kind, &b.name))
    });
    out
}

// ---- the hosts --------------------------------------------------------------------------------

/// A host and what it is doing, in one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// `local`, `node` or `github`.
    pub kind: &'static str,
    /// "this Mac", the node's name, or "GitHub".
    pub name: String,
    pub status: String,
}

impl Line {
    fn holds(&self, h: &Host) -> bool {
        self.kind == h.kind()
            && match h {
                Host::Node(n) => *n == self.name,
                _ => true,
            }
    }
}

/// Put `runs` under their hosts. Every one of `lines` appears, idle or not; `project` (empty:
/// all) leaves other projects' runs out. A run on a host that is not in `lines` (a node removed
/// while it ran) gets one. Order: this Mac, the nodes, GitHub. Pure.
pub fn arrange(lines: Vec<Line>, runs: &[Run], project: &str) -> Vec<(Line, Vec<Run>)> {
    let mut out: Vec<(Line, Vec<Run>)> = lines.into_iter().map(|l| (l, vec![])).collect();
    for r in runs
        .iter()
        .filter(|r| project.is_empty() || r.project == project)
    {
        match out.iter_mut().find(|(l, _)| l.holds(&r.host)) {
            Some((_, v)) => v.push(r.clone()),
            None => {
                let (name, status) = match &r.host {
                    Host::Local => ("this Mac".to_string(), String::new()),
                    Host::Node(n) => (n.clone(), "not paired".to_string()),
                    Host::GitHub(_) => ("GitHub".to_string(), String::new()),
                };
                out.push((
                    Line {
                        kind: r.host.kind(),
                        name,
                        status,
                    },
                    vec![r.clone()],
                ));
            }
        }
    }
    out.sort_by_key(|(l, _)| match l.kind {
        "local" => 0,
        "node" => 1,
        _ => 2,
    });
    out
}

/// The projects this Mac takes no run of: placement `remote` (set for the project, or by the
/// global switch for a project with `[remote_runs]`) and no fallback to this machine. Returns
/// their names and how many projects there are.
fn remote_only_projects(d: &Daemon) -> (Vec<String>, usize) {
    let all = d.projects();
    let names = all
        .values()
        .filter(|p| {
            let (mode, _) = crate::place::project_effective(d, &p.id);
            mode == crate::place::Mode::Remote
                && !p.services.remote.as_ref().is_some_and(|r| r.fallback_local)
        })
        .map(|p| p.id.clone())
        .collect();
    (names, all.len())
}

/// This Mac in one line, leading with whether it takes new work and, when it does not, why.
/// `why`: a hold or a limit of 0 (`place::cannot_run_here`); `remote_only`: the projects whose
/// placement is `remote` with no fallback, of `projects` in all; `detail`: what is in use. Pure.
pub fn local_words(
    why: Option<&str>,
    remote_only: &[String],
    projects: usize,
    detail: &str,
) -> String {
    let mut reasons: Vec<String> = why.map(String::from).into_iter().collect();
    let all_remote = projects > 0 && remote_only.len() == projects;
    if all_remote {
        reasons.push(format!(
            "placement is remote with no fallback ({})",
            remote_only.join(", ")
        ));
    }
    let head = if !reasons.is_empty() {
        format!("not accepting: {}", reasons.join("; "))
    } else if !remote_only.is_empty() {
        format!(
            "accepting, but not for {} (placement is remote with no fallback)",
            remote_only.join(", ")
        )
    } else {
        "accepting".to_string()
    };
    if detail.is_empty() {
        head
    } else {
        format!("{head}. {detail}")
    }
}

fn local_status(d: &Daemon) -> String {
    let reading = d.broker.load_reading(now_ms());
    let uses = BTreeMap::from([
        (mason_scheduler::COMPILE.to_string(), 1),
        ("test-suite".to_string(), 1),
    ]);
    let why = crate::place::cannot_run_here(&reading, &uses);
    let mut parts: Vec<String> = vec![];
    let sessions = d.accounts.iter().fold((0, 0), |(u, l), a| {
        let r = crate::sched::llm_resource(&a.name);
        (u + d.broker.used(&r), l + d.broker.limit(&r).unwrap_or(0))
    });
    parts.push(format!("agents {} of {}", sessions.0, sessions.1));
    for name in [mason_scheduler::COMPILE, "test-suite", "test-small"] {
        if let Some(l) = reading.limits.get(name) {
            let used = reading.used.get(name).copied().unwrap_or(0);
            parts.push(format!("{name} {used} of {l}"));
        }
    }
    let (remote_only, projects) = remote_only_projects(d);
    local_words(why.as_deref(), &remote_only, projects, &parts.join(", "))
}

/// A paired node in one line: the words `mason node list` gives (`accepting`, `not accepting
/// (updating to runtime 57cb926816: waiting for 3 jobs to end)`, `lost (...)`), or why it takes
/// no more while it is up: full, or `[nodes]` not enabled.
fn node_status(on: bool, state: &crate::nodes::NodeState, status: Option<&Value>) -> String {
    use crate::nodes::NodeState as S;
    if !on {
        return "not accepting ([nodes] is not enabled)".into();
    }
    if matches!(state, S::Accepting) {
        let jobs = status.map(|s| &s["jobs"]);
        if let Some((running, max)) =
            jobs.and_then(|j| Some((j["running"].as_u64()?, j["max"].as_u64()?)))
        {
            if running >= max {
                return format!("not accepting (full: {running} of {max} jobs)");
            }
        }
    }
    state.text()
}

/// GitHub in one line, from the broker's `gha` view (what `mason resources` prints), MASON's
/// `gha` pool limit and the macOS pool's `(used, limit)`: whether MASON can start a GitHub run
/// now and, when it cannot, why, then the counts:
/// `accepting: 14 of 20 runners in use (MASON 6, other workflows 8 running, 20 queued: mason CI:
/// full 7); macOS 5 of 5`. The pool counts other workflows' running jobs too, so MASON starts a
/// run only while the account's use is below its own limit, which may be under the account
/// ceiling; then the line says so and gives both numbers:
/// `not accepting: MASON's limit is 14 runners and 19 are in use (MASON 16, other workflows 3
/// running); account 19 of 20; macOS 4 of 4`. A macOS request also waits while the macOS pool is
/// full. Pure.
pub fn github_words(g: &Value, limit: u32, macos: (u32, u32)) -> String {
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    let (mason, other, queued) = (n(&g["mason"]), n(&g["other"]), n(&g["other_queued"]));
    let ceiling = n(&g["ceiling"]);
    let limit = u64::from(limit);
    let used = mason + other;
    let waiting = |pool: &str| {
        g["waiters"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|w| w["uses"].get(pool).is_some_and(|u| n(u) > 0))
            .count()
    };
    let account_full = used >= ceiling;
    let limit_full = !account_full && used >= limit;
    let mut why: Vec<&str> = vec![];
    if account_full {
        why.push("the account is full");
    }
    if waiting(mason_scheduler::GHA_MACOS) > 0 && u64::from(macos.0) >= u64::from(macos.1) {
        why.push("macOS runners full");
    }
    let label = g["other_label"].as_str().unwrap_or("");
    let mut others = format!("other workflows {other} running");
    if queued > 0 {
        others += &format!(", {queued} queued");
    }
    if !label.is_empty() && other + queued > 0 {
        others += &format!(": {label}");
    }
    let breakdown = format!("(MASON {mason}, {others})");
    // MASON's own limit is the one that counts; the account's is named when it differs.
    let mut tail = String::new();
    if limit < ceiling {
        tail += &format!("; account {used} of {ceiling}");
    }
    if macos.1 > 0 {
        tail += &format!("; macOS {} of {}", macos.0, macos.1);
        let mac_ceiling = n(&g["macos"]["ceiling"]);
        if mac_ceiling > 0 && mac_ceiling != u64::from(macos.1) {
            tail += &format!(" (account {mac_ceiling})");
        }
    }
    let waits = g["waiters"].as_array().map_or(0, Vec::len);
    if waits > 0 {
        tail += &format!("; {waits} of MASON's waiting for a runner");
    }
    if g["stale"].as_bool() == Some(true) {
        tail += "; GitHub could not be read, the count is old";
    }
    if limit_full && why.is_empty() {
        let runners = if limit == 1 { "runner" } else { "runners" };
        let are = if used == 1 { "is" } else { "are" };
        return format!(
            "not accepting: MASON's limit is {limit} {runners} and {used} {are} in use \
             {breakdown}{tail}"
        );
    }
    let head = if limit < ceiling && used > limit {
        format!("{used} runners in use (MASON's limit {limit})")
    } else if limit < ceiling {
        format!("{used} of {limit} runners in use")
    } else {
        format!("{used} of {ceiling} runners in use")
    };
    let counts = format!("{head} {breakdown}{tail}");
    if limit_full {
        why.insert(0, "MASON's limit is full");
    }
    if why.is_empty() {
        format!("accepting: {counts}")
    } else {
        format!("not accepting: {}. {counts}", why.join(" and "))
    }
}

/// Every configured host: this Mac, each paired node, and GitHub when a project uses it.
pub fn lines(d: &Daemon) -> Vec<Line> {
    let mut out = vec![Line {
        kind: "local",
        name: "this Mac".into(),
        status: local_status(d),
    }];
    let cfg = d.nodes.cfg(&d.cfg.config_dir);
    let names: BTreeSet<(String, String)> = d
        .store
        .list_nodes()
        .unwrap_or_default()
        .into_iter()
        .map(|r| (r.name, r.nid))
        .collect();
    for (name, nid) in names {
        let view = d.nodes.view(&nid).unwrap_or_default();
        let state = crate::nodes::state_of(&view, cfg.lost_after_polls);
        out.push(Line {
            kind: "node",
            name,
            status: node_status(cfg.enabled, &state, view.status.as_ref()),
        });
    }
    if crate::remote::any_remote(d) {
        let mac = mason_scheduler::GHA_MACOS;
        let gha = d.broker.snapshot()["gha"].clone();
        out.push(Line {
            kind: "github",
            name: "GitHub".into(),
            status: github_words(
                &gha,
                d.broker
                    .limit(mason_scheduler::GHA)
                    .unwrap_or_else(|| gha["ceiling"].as_u64().unwrap_or(0) as u32),
                (d.broker.used(mac), d.broker.limit(mac).unwrap_or(0)),
            ),
        });
    }
    out
}

// ---- JSON, for /status and /tasks/{id} --------------------------------------------------------

/// One run as `/status` and `/tasks/{id}` give it.
pub fn run_json(r: &HostRun, now: u64) -> Value {
    let t = r.task.as_ref();
    json!({
        "task": t.map(|t| t.id),
        "title": t.map(|t| t.title.clone()),
        "project": t.map(|t| t.project.clone()),
        "state": t.map(|t| t.state.clone()),
        "activity": t.map(|t| t.activity.clone()),
        "kind": r.kind, "name": r.name, "host": r.host,
        "since_ms": r.since_ms,
        "elapsed_s": (r.since_ms > 0).then(|| now.saturating_sub(r.since_ms) / 1000),
        "url": r.url,
    })
}

/// One host and its runs as `/status` gives it.
pub fn host_json(h: &HostView, now: u64) -> Value {
    json!({
        "kind": h.kind, "name": h.name, "status": h.status,
        "runs": h.runs.iter().map(|r| run_json(r, now)).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(task: u64, project: &str, host: Host) -> Run {
        Run {
            id: format!("t{task}r1-check0"),
            project: project.into(),
            task: Some(task),
            kind: Kind::Check,
            name: "unit".into(),
            host,
            since_ms: 1,
            url: String::new(),
        }
    }

    fn line(kind: &'static str, name: &str) -> Line {
        Line {
            kind,
            name: name.into(),
            status: String::new(),
        }
    }

    fn hosts_of(scope: &Path) -> Vec<String> {
        let mut v: Vec<String> = registered(scope).iter().map(|r| r.host.key()).collect();
        v.sort();
        v
    }

    #[test]
    fn a_run_is_listed_while_its_guard_lives_and_no_longer() {
        let scope = Path::new("/scope/guard");
        let g = start(scope, run(1, "p", Host::Local));
        assert_eq!(registered(scope), vec![run(1, "p", Host::Local)]);
        assert!(
            registered(Path::new("/scope/other-daemon")).is_empty(),
            "another daemon does not see it"
        );
        drop(g);
        assert!(registered(scope).is_empty());
    }

    #[test]
    fn a_panic_takes_the_run_off_the_list() {
        let scope = Path::new("/scope/panic");
        let r = std::panic::catch_unwind(|| {
            let _g = start(scope, run(2, "p", Host::Local));
            assert_eq!(registered(scope).len(), 1);
            panic!("the run blew up");
        });
        assert!(r.is_err());
        assert!(registered(scope).is_empty());
    }

    #[test]
    fn two_runs_of_a_task_show_each_under_its_own_host() {
        let scope = Path::new("/scope/two");
        let _a = start(scope, run(3, "p", Host::Local));
        let _b = start(scope, run(3, "p", Host::Node("marmot".into())));
        assert_eq!(hosts_of(scope), ["local", "node:marmot"]);
        let out = arrange(
            vec![line("local", "this Mac"), line("node", "marmot")],
            &registered(scope),
            "",
        );
        for (l, runs) in &out {
            assert_eq!(runs.len(), 1, "{l:?}");
            assert!(l.holds(&runs[0].host), "{l:?}");
        }
    }

    #[test]
    fn a_run_moved_from_a_node_to_this_mac_shows_only_here() {
        let scope = Path::new("/scope/moved");
        let on_node = start(scope, run(4, "p", Host::Node("marmot".into())));
        assert_eq!(hosts_of(scope), ["node:marmot"]);
        drop(on_node);
        let _here = start(scope, run(4, "p", Host::Local));
        assert_eq!(hosts_of(scope), ["local"]);
    }

    #[test]
    fn idle_hosts_stay_and_a_project_filter_drops_only_runs() {
        let lines = vec![
            line("local", "this Mac"),
            line("node", "marmot"),
            line("github", "GitHub"),
        ];
        let runs = [run(5, "mine", Host::Local), run(6, "other", Host::Local)];
        let all = arrange(lines.clone(), &runs, "");
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].1.len(), 2);
        assert!(all[1].1.is_empty() && all[2].1.is_empty(), "idle, listed");
        let mine = arrange(lines.clone(), &runs, "mine");
        assert_eq!(
            mine.iter().map(|(l, _)| l.clone()).collect::<Vec<_>>(),
            lines
        );
        assert_eq!(
            mine[0].1.iter().map(|r| r.task).collect::<Vec<_>>(),
            [Some(5)]
        );
    }

    #[test]
    fn a_run_on_an_unlisted_node_gets_a_host_before_github() {
        let out = arrange(
            vec![line("local", "this Mac"), line("github", "GitHub")],
            &[run(7, "p", Host::Node("gone".into()))],
            "",
        );
        assert_eq!(
            out.iter().map(|(l, _)| l.kind).collect::<Vec<_>>(),
            ["local", "node", "github"]
        );
        assert_eq!(out[1].0.name, "gone");
    }

    #[test]
    fn host_ids_say_what_a_run_is() {
        assert_eq!(classify("t5r7-check-unit0"), (Kind::Check, Some(5)));
        assert_eq!(classify("t5r7-checkbuild0"), (Kind::Check, Some(5)));
        assert_eq!(classify("t5r7-unit"), (Kind::Suite, Some(5)));
        assert_eq!(classify("t5r7p1-unit-host1"), (Kind::Suite, Some(5)));
        assert_eq!(classify("b12-retest0"), (Kind::Batch, None));
        assert_eq!(classify("full-mason-unit"), (Kind::Batch, None));
        assert_eq!(classify("debt3-unit"), (Kind::Debt, None));
        assert_eq!(classify("ledger9-unit"), (Kind::Debt, Some(9)));
        assert_eq!(classify("redwatch-unit-abc0"), (Kind::Check, None));
        assert_eq!(classify("tree-check"), (Kind::Check, None));
    }

    #[test]
    fn the_longest_suite_name_at_the_end_of_a_host_id_wins() {
        let names = vec!["unit".to_string(), "slow-unit".to_string()];
        assert_eq!(suite_in("t1r2-slow-unit", &names), "slow-unit");
        assert_eq!(suite_in("t1r2-u-unit-host1", &names), "unit");
        assert_eq!(suite_in("t1r2-other", &names), "t1r2-other");
    }

    #[test]
    fn a_node_reads_as_node_list_does_and_says_why_it_takes_no_work() {
        use crate::nodes::NodeState as S;
        let jobs = |r: u64| json!({"jobs": {"running": r, "max": 2}});
        assert_eq!(
            node_status(true, &S::Accepting, Some(&jobs(1))),
            "accepting"
        );
        assert_eq!(
            node_status(true, &S::Accepting, Some(&jobs(2))),
            "not accepting (full: 2 of 2 jobs)"
        );
        let why = "updating to runtime 57cb926816: waiting for 3 jobs to end";
        let s = S::NotAccepting(why.into());
        assert_eq!(node_status(true, &s, None), s.text());
        assert_eq!(
            node_status(true, &s, None),
            format!("not accepting ({why})")
        );
        assert!(node_status(true, &S::Lost(3), None).starts_with("lost"));
        assert!(node_status(false, &S::Accepting, None).starts_with("not accepting"));
    }

    fn gha(mason: u64, other: u64, queued: u64, label: &str, waiters: Value) -> Value {
        json!({"mason": mason, "other": other, "other_queued": queued, "other_label": label,
               "ceiling": 20, "waiters": waiters, "stale": false})
    }

    #[test]
    fn github_says_how_full_it_is_with_others_queued() {
        let g = gha(
            6,
            8,
            20,
            "mason CI: full 7, timelapse_render_daemon v3-gate 13",
            json!([]),
        );
        assert_eq!(
            github_words(&g, 20, (5, 5)),
            "accepting: 14 of 20 runners in use (MASON 6, other workflows 8 running, 20 queued: \
             mason CI: full 7, timelapse_render_daemon v3-gate 13); macOS 5 of 5"
        );
    }

    #[test]
    fn github_says_why_a_waiting_check_gets_no_runner() {
        let waiting = |pool: &str| json!([{"position": 1, "uses": {pool: 1}}]);
        // The account is at its ceiling.
        let full = github_words(&gha(6, 14, 0, "", waiting("gha")), 20, (1, 5));
        assert!(
            full.starts_with("not accepting: the account is full. 20 of 20 runners in use"),
            "{full}"
        );
        assert!(full.contains("1 of MASON's waiting"), "{full}");
        // Room on the account, none on macOS.
        let mac = github_words(&gha(2, 3, 0, "", waiting("gha-macos")), 20, (5, 5));
        assert!(
            mac.starts_with("not accepting: macOS runners full. 5 of 20 runners in use"),
            "{mac}"
        );
        assert!(mac.contains("macOS 5 of 5"), "{mac}");
        // A wait with room is not a refusal.
        let room = github_words(&gha(2, 3, 0, "", waiting("gha")), 20, (1, 5));
        assert!(room.starts_with("accepting: "), "{room}");
    }

    #[test]
    fn github_judges_room_by_masons_limit_not_the_accounts() {
        // Limit 14 under the account's 20, use in between: no new run starts.
        let g = gha(16, 3, 1, "mason CI: full 1", json!([]));
        assert_eq!(
            github_words(&g, 14, (4, 4)),
            "not accepting: MASON's limit is 14 runners and 19 are in use (MASON 16, other \
             workflows 3 running, 1 queued: mason CI: full 1); account 19 of 20; macOS 4 of 4"
        );
        // Below the limit it accepts, and names both numbers.
        let g = gha(8, 3, 0, "", json!([]));
        assert_eq!(
            github_words(&g, 14, (1, 4)),
            "accepting: 11 of 14 runners in use (MASON 8, other workflows 3 running); \
             account 11 of 20; macOS 1 of 4"
        );
        // The account itself full keeps its own reason.
        let g = gha(6, 14, 0, "", json!([]));
        let full = github_words(&g, 14, (1, 4));
        assert!(
            full.starts_with("not accepting: the account is full. 20 runners in use"),
            "{full}"
        );
        // A macOS limit that differs from the account's macOS ceiling names the ceiling.
        let mut g = gha(2, 0, 0, "", json!([]));
        g["macos"] = json!({"ceiling": 5});
        assert!(
            github_words(&g, 20, (3, 4)).contains("macOS 3 of 4 (account 5)"),
            "{g}"
        );
    }

    #[test]
    fn this_mac_says_why_it_takes_no_work() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            local_words(None, &[], 2, "agents 1 of 4"),
            "accepting. agents 1 of 4"
        );
        assert_eq!(
            local_words(Some("this machine held for heat (die 94 C)"), &[], 1, ""),
            "not accepting: this machine held for heat (die 94 C)"
        );
        assert_eq!(
            local_words(Some("compile limit 0"), &[], 1, "agents 0 of 4"),
            "not accepting: compile limit 0. agents 0 of 4"
        );
        assert_eq!(
            local_words(None, &names(&["mason"]), 1, ""),
            "not accepting: placement is remote with no fallback (mason)"
        );
        assert_eq!(
            local_words(None, &names(&["mason"]), 2, ""),
            "accepting, but not for mason (placement is remote with no fallback)"
        );
        assert_eq!(local_words(None, &[], 0, ""), "accepting");
    }
}
