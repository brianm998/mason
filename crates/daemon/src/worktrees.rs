// SPDX-License-Identifier: GPL-3.0-or-later

//! Clearing up the pipeline's own worktrees (s1145, decision 1134).
//!
//! A canceled task whose branch is kept, and a failed task, lose their worktree (never the
//! branch) once they are old enough; a retry or a revive adds it again from the branch. A
//! worktree with no step for a while loses its build output but keeps the checkout. Landing
//! scratch worktrees go when unused. Nothing in use is touched, and a dirty worktree is never
//! removed.
//!
//! Every removal renames the directory into `<data_dir>/trash` under the repository lock (which
//! takes no time) and deletes it afterwards, outside the lock, one at a time. Time is a
//! parameter (`now`), so tests give their own clock.

use crate::config::ProjectCfg;
use crate::daemon::Daemon;
use mason_core::*;
use mason_landing::git;
use mason_step_host::registry::{HostState, Registry};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

const HOUR_MS: u64 = 3_600_000;

/// Directory names that are build output wherever git ignores them.
pub const BUILD_DIRS: [&str; 3] = ["target", "build", ".build"];

/// How long things are kept, from the project file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeepCfg {
    /// `worktree_keep_canceled_h`: a canceled task's worktree (branch kept).
    pub canceled_h: u64,
    /// `worktree_keep_failed_h`: a failed (or rejected self-change) task's worktree.
    pub failed_h: u64,
    /// `target_idle_h`: build output of a worktree with no step for this long.
    pub target_idle_h: u64,
    /// `landing_scratch_keep_h`: a landing scratch worktree unused for this long.
    pub scratch_h: u64,
    /// `landing_scratch_max`: the most landing scratch worktrees kept.
    pub scratch_max: u64,
    /// `build_outputs`: more build output directories. A name matches a directory of that
    /// name anywhere; a path with `/` matches that path from the worktree's root.
    pub build_outputs: Vec<String>,
}

impl Default for KeepCfg {
    fn default() -> Self {
        KeepCfg {
            canceled_h: 24,
            failed_h: 72,
            target_idle_h: 12,
            scratch_h: 48,
            scratch_max: 2,
            build_outputs: vec![],
        }
    }
}

impl KeepCfg {
    /// Read the keys from a project file.
    pub fn parse(id: &str, doc: &toml_edit::DocumentMut) -> Result<KeepCfg, String> {
        let mut c = KeepCfg::default();
        for (key, into) in [
            ("worktree_keep_canceled_h", &mut c.canceled_h),
            ("worktree_keep_failed_h", &mut c.failed_h),
            ("target_idle_h", &mut c.target_idle_h),
            ("landing_scratch_keep_h", &mut c.scratch_h),
            ("landing_scratch_max", &mut c.scratch_max),
        ] {
            if let Some(v) = doc.get(key) {
                *into = v
                    .as_integer()
                    .filter(|n| *n >= 0)
                    .ok_or_else(|| format!("{id}: `{key}` must be a whole number, 0 or more"))?
                    as u64;
            }
        }
        if let Some(v) = doc.get("build_outputs") {
            let bad = || {
                format!("{id}: `build_outputs` must be a list of directory names or relative paths")
            };
            for x in v.as_array().ok_or_else(bad)? {
                let s = x.as_str().ok_or_else(bad)?.trim_end_matches('/');
                let plain = Path::new(s)
                    .components()
                    .all(|c| matches!(c, Component::Normal(_)));
                if s.is_empty() || !plain {
                    return Err(bad());
                }
                c.build_outputs.push(s.to_string());
            }
        }
        Ok(c)
    }
}

/// What the keep rules look at for one task's worktree.
#[derive(Clone, Debug)]
pub struct Facts {
    pub state: State,
    /// A canceled task's branch was kept.
    pub keeps_branch: bool,
    /// A canceled task whose held self-change was not approved: kept as long as a failed one.
    pub rejected: bool,
    /// When the task entered its state.
    pub since_ms: u64,
    /// When the worktree was last added again for somebody to read (0: never).
    pub restored_ms: u64,
    /// Something is using the worktree, and what.
    pub in_use: Option<String>,
    pub branch_exists: bool,
    /// The branch's head is already in the base branch.
    pub contained: bool,
    /// Why the worktree may hold work that is nowhere else.
    pub dirty: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Removal {
    /// The rules never remove this one.
    Never,
    /// Not old enough yet.
    NotDue,
    /// Due, but left: why.
    Skip(String),
    /// Remove it: why.
    Remove(String),
}

/// Is the state one whose worktree may be removed at all?
fn removable_state(state: State, keeps_branch: bool) -> bool {
    state == State::Failed || (state == State::Canceled && keeps_branch)
}

/// The keep rule for one task's worktree. A canceled task with its branch kept is due after
/// `canceled_h`, or at once when its work is already in the base; a failed task after
/// `failed_h`. A due worktree that is in use, dirty or has no branch is left.
pub fn removal(f: &Facts, keep: &KeepCfg, now: u64) -> Removal {
    if !removable_state(f.state, f.keeps_branch) {
        return Removal::Never;
    }
    let failed = f.state == State::Failed || f.rejected;
    let hours = if failed {
        keep.failed_h
    } else {
        keep.canceled_h
    };
    let limit = hours.saturating_mul(HOUR_MS);
    let age = now.saturating_sub(f.since_ms.max(f.restored_ms));
    let restored_lately = f.restored_ms > 0 && now.saturating_sub(f.restored_ms) < limit;
    let landed_anyway = !failed && f.contained && !restored_lately;
    if age < limit && !landed_anyway {
        return Removal::NotDue;
    }
    if let Some(why) = &f.in_use {
        return Removal::Skip(why.clone());
    }
    if !f.branch_exists {
        return Removal::Skip("the branch is missing, so it could not be added again".into());
    }
    if let Some(why) = &f.dirty {
        return Removal::Skip(why.clone());
    }
    let state = if f.state == State::Failed {
        "failed"
    } else {
        "canceled"
    };
    Removal::Remove(if landed_anyway && age < limit {
        format!("{state}, and its branch is already in the base")
    } else {
        format!("{state} {} h ago (kept {hours} h)", age / HOUR_MS)
    })
}

/// Should the build output of a worktree that nothing is using go? Yes after `target_idle_h`
/// without a step, or at once under disk pressure.
pub fn trim_due(idle_since_ms: u64, pressure: bool, keep: &KeepCfg, now: u64) -> Option<String> {
    let idle = now.saturating_sub(idle_since_ms);
    if idle >= keep.target_idle_h.saturating_mul(HOUR_MS) {
        Some(format!("idle {} h", idle / HOUR_MS))
    } else if pressure {
        Some("disk space is low".into())
    } else {
        None
    }
}

/// Is free space under twice the floor? A floor of 0 is off.
pub fn under_pressure(free_mb: u64, floor_mb: u64) -> bool {
    floor_mb > 0 && free_mb < floor_mb.saturating_mul(2)
}

/// Does the ignored directory `rel` count as build output?
pub fn is_build_output(rel: &str, extra: &[String]) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    BUILD_DIRS.contains(&name)
        || extra
            .iter()
            .any(|e| if e.contains('/') { e == rel } else { e == name })
}

/// The build output directories of the checkout `wt`: directories git ignores (so nothing in
/// them is tracked or waiting to be committed) whose name or path is build output. Symlinks are
/// never followed or listed. Relative to `wt`.
pub fn build_outputs(wt: &Path, extra: &[String]) -> Vec<String> {
    if !wt.join(".git").exists() {
        return vec![];
    }
    let Ok(out) = git::run(
        wt,
        &[
            "--no-optional-locks",
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
        ],
    ) else {
        return vec![];
    };
    out.split('\0')
        .filter_map(|e| e.strip_suffix('/'))
        .filter(|rel| is_build_output(rel, extra))
        .filter(|rel| {
            Path::new(rel)
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
                && std::fs::symlink_metadata(wt.join(rel)).is_ok_and(|m| m.is_dir())
        })
        .map(String::from)
        .collect()
}

/// Why `wt` may hold work that is nowhere but in it, or `None` when it is clean and on
/// `branch`. Anything that cannot be checked counts as dirty.
pub fn dirty(wt: &Path, branch: &str) -> Option<String> {
    if !wt.join(".git").exists() {
        return Some("not a git worktree".into());
    }
    match git::run(wt, &["symbolic-ref", "-q", "HEAD"]) {
        Ok(h) if h.trim() == format!("refs/heads/{branch}") => {}
        _ => return Some("HEAD is not on the task's branch".into()),
    }
    match git::run(
        wt,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain",
            "--untracked-files=normal",
        ],
    ) {
        Ok(s) if s.trim().is_empty() => None,
        Ok(s) => Some(format!(
            "uncommitted changes ({} paths)",
            s.trim().lines().count()
        )),
        Err(e) => Some(format!("git status failed: {e}")),
    }
}

fn marker(d: &Daemon, task: TaskId, name: &str) -> PathBuf {
    d.task_dir(task).join(name)
}

const BUSY_MARK: &str = "worktree-busy";
const RESTORED_MARK: &str = "worktree-restored";
const SETUP_MARK: &str = "worktree-setup-due";

fn read_ms(p: &Path) -> u64 {
    std::fs::read_to_string(p)
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0)
}

fn write_mark(p: &Path, ms: u64) {
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(p, format!("{ms}\n")) {
        tracing::warn!("cannot write {}: {e}", p.display());
    }
}

/// A step's job for `task` has just ended: its worktree was in use until now.
pub fn note_job_ended(d: &Daemon, task: TaskId) {
    write_mark(&marker(d, task, BUSY_MARK), crate::daemon::now_ms());
}

/// Add the worktree of a canceled or failed task again, from its branch, for somebody who
/// wants to read it (an owner session, a diagnosis). A step adds its own.
pub fn restore(d: &Daemon, task: TaskId) -> Result<(), String> {
    let w = d.world().map_err(|e| e.to_string())?;
    let Some(t) = w.tasks.get(&task) else {
        return Ok(());
    };
    let Some(p) = d.project(t.project) else {
        return Ok(());
    };
    if !matches!(t.state, State::Failed | State::Canceled) {
        return Ok(());
    }
    let (wt, branch) = (p.worktree(&d.cfg, task.0), p.branch(task.0));
    let lock = d.repo_lock(t.project);
    let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
    if wt.join(".git").exists() || !git::branch_exists(&p.path, &branch) {
        return Ok(());
    }
    write_mark(&marker(d, task, RESTORED_MARK), crate::daemon::now_ms());
    git::ensure_worktree(&p.path, &wt, &branch, &p.base_ref()).map_err(|e| e.to_string())
}

/// Before a step uses a worktree the sweep took something from, run the definition's worktree
/// setup again: it may have put ignored files where the build output was.
pub(crate) fn run_due_setup(c: &crate::jobs::Ctx) -> Result<(), String> {
    let mark = marker(&c.d, c.task, SETUP_MARK);
    if !mark.exists() {
        return Ok(());
    }
    crate::jobs::setup_checkout(&c.def, &c.wt)?;
    let _ = std::fs::remove_file(mark);
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Task,
    Landing,
}

/// What the next sweep does to an item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// The whole worktree goes (never a branch).
    Remove(String),
    /// Its build output goes; the checkout stays.
    Trim(String),
}

/// One worktree and what is to happen to it.
#[derive(Clone, Debug)]
pub struct Item {
    pub kind: Kind,
    pub project: String,
    pub project_id: ProjectId,
    pub task: Option<TaskId>,
    /// The task's state in words; "unknown" for a directory no task owns; "idle" or
    /// "running" for a landing worktree.
    pub state: String,
    pub path: PathBuf,
    /// Build output directories, relative to `path`.
    pub outputs: Vec<String>,
    pub action: Option<Action>,
    /// Why something that is due is left alone.
    pub skipped: Option<String>,
    /// For ordering: since when it has been idle.
    pub idle_since_ms: u64,
}

/// Tasks with a step host running or being launched.
fn running_task_hosts(d: &Daemon) -> HashSet<u64> {
    let dir = d.cfg.hosts_dir();
    let launching = mason_step_host::launcher::launching_under(&dir)
        .into_iter()
        .map(|l| l.id);
    Registry::new(dir)
        .scan()
        .unwrap_or_default()
        .into_iter()
        .filter(|h| h.state == HostState::Running)
        .map(|h| h.info.id)
        .chain(launching)
        .filter_map(|id| crate::sweep::parse_host_id(&id).map(|x| x.0))
        .collect()
}

fn in_use(
    d: &Daemon,
    w: &mason_engine::World,
    task: TaskId,
    hosts: &HashSet<u64>,
) -> Option<String> {
    if d.has_job_for_task(task) || w.open_run(task).is_some() {
        Some("a step is running".into())
    } else if hosts.contains(&task.0) {
        Some("a step host is running".into())
    } else if d.landing.in_batch(task) {
        Some("in a landing batch".into())
    } else if crate::usersession::open_of(d, task).is_some() {
        Some("an owner session is open".into())
    } else {
        None
    }
}

/// Why the build output of a task's worktree must stay now. A job that only waits (for a
/// slot, the disk, an account) has no step running, so it does not keep the output, unless the
/// definition has a worktree setup: that runs when the job prepares the worktree and would not
/// run again before the build.
fn step_running(d: &Daemon, t: &Task, hosts: &HashSet<u64>) -> Option<String> {
    if hosts.contains(&t.id.0) {
        Some("a step host is running".into())
    } else if crate::usersession::open_of(d, t.id).is_some() {
        Some("an owner session is open".into())
    } else if d.has_job_for_task(t.id)
        && d.pinned_def(&t.def)
            .is_none_or(|def| !def.worktree_setup().is_empty())
    {
        Some("a step is running".into())
    } else {
        None
    }
}

/// What one look at all the worktrees shares.
struct Look<'a> {
    d: &'a Daemon,
    w: &'a mason_engine::World,
    hosts: HashSet<u64>,
    pressure: bool,
    /// List the build output of every task worktree, not only of those about to be trimmed.
    all_outputs: bool,
    now: u64,
}

/// The keep decision and, for an idle one, the build output decision for a task's worktree.
fn task_item(look: &Look, t: &Task, p: &ProjectCfg) -> Item {
    let Look {
        d,
        w,
        ref hosts,
        pressure,
        all_outputs,
        now,
    } = *look;
    let path = p.worktree(&d.cfg, t.id.0);
    let used = in_use(d, w, t.id, hosts);
    let last_run = w
        .step_runs
        .values()
        .filter(|r| r.task == t.id)
        .map(|r| r.started_at.0)
        .max()
        .unwrap_or(0);
    let idle_since = t
        .state_entered_at
        .0
        .max(last_run)
        .max(read_ms(&marker(d, t.id, BUSY_MARK)))
        .max(read_ms(&marker(d, t.id, RESTORED_MARK)));
    let mut action = None;
    let mut skipped = None;
    if removable_state(t.state, t.cancel_keeps_branch) {
        let branch = p.branch(t.id.0);
        let branch_exists = git::branch_exists(&p.path, &branch);
        let rejected = t.state == State::Canceled
            && d.store
                .hold(t.id.0)
                .ok()
                .flatten()
                .is_some_and(|h| h.approved_by.is_none());
        let mut f = Facts {
            state: t.state,
            keeps_branch: t.cancel_keeps_branch,
            rejected,
            since_ms: t.state_entered_at.0,
            restored_ms: read_ms(&marker(d, t.id, RESTORED_MARK)),
            in_use: used.clone(),
            branch_exists,
            contained: branch_exists && git::is_ancestor(&p.path, &branch, &p.base_ref()),
            dirty: None,
        };
        // `git status` walks the whole checkout: only for one that would otherwise go.
        if matches!(removal(&f, &p.keep, now), Removal::Remove(_)) {
            f.dirty = dirty(&path, &branch);
        }
        match removal(&f, &p.keep, now) {
            Removal::Remove(why) => action = Some(Action::Remove(why)),
            Removal::Skip(why) => skipped = Some(why),
            Removal::Never | Removal::NotDue => {}
        }
    }
    let due = if action.is_none() && step_running(d, t, hosts).is_none() {
        trim_due(idle_since, pressure, &p.keep, now)
    } else {
        None
    };
    let outputs = if all_outputs || due.is_some() {
        build_outputs(&path, &p.keep.build_outputs)
    } else {
        vec![]
    };
    if let Some(why) = due.filter(|_| !outputs.is_empty()) {
        action = Some(Action::Trim(why));
    }
    Item {
        kind: Kind::Task,
        project: p.id.clone(),
        project_id: t.project,
        task: Some(t.id),
        state: mason_store::state_name(t.state).to_string(),
        path,
        outputs,
        action,
        skipped,
        idle_since_ms: idle_since,
    }
}

/// `<project>-b<N>` (a batch's) or `<project>-post<N>` (its post-land run): the slot, and
/// whether it is a batch's.
fn scratch_slot(name: &str, project: &str) -> Option<(usize, bool)> {
    let rest = name.strip_prefix(project)?.strip_prefix('-')?;
    let (n, batch) = match rest.strip_prefix("post") {
        Some(n) => (n, false),
        None => (rest.strip_prefix('b')?, true),
    };
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((n.parse().ok()?, batch))
}

fn mtime_ms(p: &Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

/// The sweep's record that a landing worktree was in use.
fn scratch_mark(d: &Daemon, name: &str) -> PathBuf {
    d.cfg.landing_dir().join(format!(".used-{name}"))
}

/// When a landing worktree was last used: the sweep's record, or what the file system says (its
/// own time and its git index, which every checkout writes), whichever is newest.
fn scratch_used_ms(d: &Daemon, name: &str, wt: &Path) -> u64 {
    let index = std::fs::read_to_string(wt.join(".git"))
        .ok()
        .and_then(|t| {
            t.trim()
                .strip_prefix("gitdir: ")
                .map(|g| wt.join(g).join("index"))
        })
        .map_or(0, |i| mtime_ms(&i));
    read_ms(&scratch_mark(d, name)).max(mtime_ms(wt)).max(index)
}

/// One landing scratch worktree, as the rule sees it.
#[derive(Clone, Debug)]
pub struct Scratch {
    /// A batch's worktree (these count against the cap).
    pub batch: bool,
    pub used_ms: u64,
    pub running: bool,
}

/// Which of a project's landing scratch worktrees go, and why: those unused for `scratch_h`,
/// and the batch worktrees beyond the `scratch_max` most recently used. Never a running one.
pub fn scratch_removals(all: &[Scratch], keep: &KeepCfg, now: u64) -> Vec<Option<String>> {
    let mut order: Vec<usize> = (0..all.len()).filter(|i| all[*i].batch).collect();
    order.sort_by_key(|i| (!all[*i].running, std::cmp::Reverse(all[*i].used_ms)));
    let over: HashSet<usize> = order
        .into_iter()
        .skip(usize::try_from(keep.scratch_max).unwrap_or(usize::MAX))
        .collect();
    all.iter()
        .enumerate()
        .map(|(i, s)| {
            let idle = now.saturating_sub(s.used_ms);
            if s.running {
                None
            } else if idle >= keep.scratch_h.saturating_mul(HOUR_MS) {
                Some(format!(
                    "unused {} h (kept {} h)",
                    idle / HOUR_MS,
                    keep.scratch_h
                ))
            } else if over.contains(&i) {
                Some(format!("more than {} kept", keep.scratch_max))
            } else {
                None
            }
        })
        .collect()
}

fn landing_items(d: &Daemon, now: u64) -> Vec<Item> {
    let names: Vec<String> = std::fs::read_dir(d.cfg.landing_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    let mut out = vec![];
    for (pid, p) in &d.projects() {
        let running = d.landing.running_slots(*pid);
        let mine: Vec<(&String, Scratch)> = names
            .iter()
            .filter_map(|n| {
                let (slot, batch) = scratch_slot(n, &p.id)?;
                let wt = d.cfg.landing_dir().join(n);
                let busy = running.contains(&slot);
                if busy {
                    write_mark(&scratch_mark(d, n), now);
                }
                Some((
                    n,
                    Scratch {
                        batch,
                        used_ms: scratch_used_ms(d, n, &wt),
                        running: busy,
                    },
                ))
            })
            .collect();
        let facts: Vec<Scratch> = mine.iter().map(|m| m.1.clone()).collect();
        for ((name, s), why) in mine.iter().zip(scratch_removals(&facts, &p.keep, now)) {
            let path = d.cfg.landing_dir().join(name.as_str());
            out.push(Item {
                kind: Kind::Landing,
                project: p.id.clone(),
                project_id: *pid,
                task: None,
                state: if s.running { "running" } else { "idle" }.into(),
                path,
                outputs: vec![],
                action: why.map(Action::Remove),
                skipped: None,
                idle_since_ms: s.used_ms,
            });
        }
    }
    out
}

fn pressure(d: &Daemon) -> bool {
    d.broker
        .disk_free_and_floor(&d.cfg.worktrees_dir())
        .is_some_and(|(free, floor)| under_pressure(free, floor))
}

/// Every pipeline worktree and what the sweep would do with it at `now`. With `all_outputs`
/// the build output of every task worktree is listed (for the report), else only of those
/// about to be trimmed. Items to act on come oldest first.
pub fn plan(d: &Daemon, now: u64, all_outputs: bool) -> Vec<Item> {
    let Ok(w) = d.world() else { return vec![] };
    let look = Look {
        d,
        w: &w,
        hosts: running_task_hosts(d),
        pressure: pressure(d),
        all_outputs,
        now,
    };
    let mut out = vec![];
    for (pid, p) in &d.projects() {
        for e in std::fs::read_dir(d.cfg.worktrees_dir().join(&p.id))
            .into_iter()
            .flatten()
            .flatten()
        {
            if !e.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let Some(n) = e.file_name().to_str().and_then(|s| s.parse::<u64>().ok()) else {
                continue;
            };
            match w.tasks.get(&TaskId(n)).filter(|t| t.project == *pid) {
                Some(t) => out.push(task_item(&look, t, p)),
                None => out.push(Item {
                    kind: Kind::Task,
                    project: p.id.clone(),
                    project_id: *pid,
                    task: None,
                    state: "unknown".into(),
                    path: e.path(),
                    outputs: vec![],
                    action: None,
                    skipped: Some("no task owns this directory".into()),
                    idle_since_ms: 0,
                }),
            }
        }
    }
    out.extend(landing_items(d, now));
    out.sort_by(|a, b| (a.idle_since_ms, &a.path).cmp(&(b.idle_since_ms, &b.path)));
    out
}

fn trash_dir(d: &Daemon) -> PathBuf {
    d.cfg.data_dir.join("trash")
}

/// Rename `path` into the trash. `Err` leaves it where it was.
fn to_trash(d: &Daemon, path: &Path, now: u64) -> std::io::Result<PathBuf> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = trash_dir(d);
    std::fs::create_dir_all(&dir)?;
    let name = path
        .file_name()
        .map_or("dir".into(), |n| n.to_string_lossy().into_owned());
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let to = dir.join(format!("{name}-{now}-{n}"));
    std::fs::rename(path, &to)?;
    Ok(to)
}

/// Delete what is in the trash (also what a restart left there), one at a time, in the
/// background.
fn empty_trash(d: &Daemon) {
    for e in std::fs::read_dir(trash_dir(d))
        .into_iter()
        .flatten()
        .flatten()
    {
        crate::taskcache::remove_aside_in_background(e.path());
    }
}

/// Carry out one planned item, under the repository lock, after looking again. Returns whether
/// anything was removed.
fn act(d: &Arc<Daemon>, item: &Item, now: u64) -> bool {
    let Some(action) = &item.action else {
        return false;
    };
    let Some(p) = d.project(item.project_id) else {
        return false;
    };
    let lock = d.repo_lock(item.project_id);
    let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
    // A step may have started, or the task been revived, since the plan was made. A job that
    // starts from here on waits for this lock before it touches its worktree.
    let current = match (item.kind, item.task) {
        (Kind::Task, Some(task)) => {
            let Ok(w) = d.world() else { return false };
            let Some(t) = w.tasks.get(&task) else {
                return false;
            };
            let look = Look {
                d,
                w: &w,
                hosts: running_task_hosts(d),
                pressure: pressure(d),
                all_outputs: false,
                now,
            };
            task_item(&look, t, p)
        }
        (Kind::Landing, _) => match landing_items(d, now)
            .into_iter()
            .find(|i| i.path == item.path)
        {
            Some(i) => i,
            None => return false,
        },
        (Kind::Task, None) => return false,
    };
    match (action, &current.action) {
        (Action::Remove(why), Some(Action::Remove(_))) => {
            match to_trash(d, &item.path, now) {
                Ok(_) => {
                    let _ = git::run(&p.path, &["worktree", "prune"]);
                }
                Err(e) => {
                    tracing::warn!(
                        "cannot move {} aside ({e}); removing it in place",
                        item.path.display()
                    );
                    git::remove_worktree(&p.path, &item.path);
                }
            }
            if let Some(task) = item.task {
                write_mark(&marker(d, task, SETUP_MARK), now);
            } else if let Some(name) = item.path.file_name().and_then(|n| n.to_str()) {
                let _ = std::fs::remove_file(scratch_mark(d, name));
            }
            tracing::info!("removed worktree {}: {why}", item.path.display());
            !item.path.exists()
        }
        (Action::Trim(why), Some(Action::Trim(_))) => {
            let mut any = false;
            for rel in &current.outputs {
                let from = item.path.join(rel);
                // Before the removal, so a restart in between still runs the setup again.
                if let Some(task) = item.task {
                    write_mark(&marker(d, task, SETUP_MARK), now);
                }
                match to_trash(d, &from, now) {
                    Ok(_) => {
                        any = true;
                        tracing::info!("removed build output {}: {why}", from.display());
                    }
                    Err(e) => tracing::warn!("cannot remove {}: {e}", from.display()),
                }
            }
            any
        }
        _ => false,
    }
}

/// Held while a sweep runs, so the timer and `mason disk clean` remove one thing at a time.
static ONE_SWEEP: Mutex<()> = Mutex::new(());

/// One sweep at `now`: act on everything due, oldest first. Returns the items acted on.
pub fn pass(d: &Arc<Daemon>, now: u64) -> Vec<Item> {
    let _one = ONE_SWEEP.lock().unwrap_or_else(|e| e.into_inner());
    let mut done = vec![];
    for item in plan(d, now, false) {
        // Said once for each reason, not at every pass.
        if let Some(why) = &item.skipped {
            if d.worktree_sizes
                .lock()
                .logged
                .insert((item.path.clone(), why.clone()))
            {
                tracing::warn!("worktree {} left alone: {why}", item.path.display());
            }
        }
        if act(d, &item, now) {
            done.push(item);
        }
    }
    for i in &done {
        d.worktree_sizes.forget(&i.path);
    }
    empty_trash(d);
    if d.worktree_sizes.lock().wanted {
        refresh_in_background(d, now, MONITOR_AGE_MS);
    }
    done
}

#[derive(Clone, Copy, Default)]
struct Size {
    /// The whole directory.
    all: u64,
    /// Its build output.
    outputs: u64,
    at_ms: u64,
}

/// The measured sizes of the worktrees and the last plan, for `mason disk` and the monitor.
/// Walking hundreds of gigabytes takes minutes, so it is done on a thread of its own and only
/// when somebody asks.
#[derive(Default)]
pub struct Sizes {
    inner: Mutex<SizesInner>,
}

#[derive(Default)]
struct SizesInner {
    got: HashMap<PathBuf, Size>,
    plan: Vec<Item>,
    planned_ms: u64,
    working: bool,
    /// The monitor has asked for the totals since the last measurement began.
    wanted: bool,
    /// Skips already logged.
    logged: HashSet<(PathBuf, String)>,
}

impl Sizes {
    fn lock(&self) -> std::sync::MutexGuard<'_, SizesInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn forget(&self, path: &Path) {
        let mut g = self.lock();
        g.got.remove(path);
        g.plan.retain(|i| i.path != path);
        // What is left there is measured again at the next request.
        g.planned_ms = 0;
    }
}

fn measure(item: &Item, now: u64) -> Size {
    Size {
        all: crate::disk::dir_size(&item.path),
        outputs: item
            .outputs
            .iter()
            .map(|r| crate::disk::dir_size(&item.path.join(r)))
            .sum(),
        at_ms: now,
    }
}

/// Plan, and measure on this thread whatever was last measured more than `max_age_ms` ago.
fn refresh(d: &Daemon, now: u64, max_age_ms: u64) {
    let started = std::time::Instant::now();
    let items = plan(d, now, true);
    for i in &items {
        let fresh = d
            .worktree_sizes
            .lock()
            .got
            .get(&i.path)
            .is_some_and(|s| now.saturating_sub(s.at_ms) < max_age_ms);
        if !fresh {
            let s = measure(i, now);
            d.worktree_sizes.lock().got.insert(i.path.clone(), s);
        }
    }
    let mut g = d.worktree_sizes.lock();
    g.got.retain(|p, _| items.iter().any(|i| &i.path == p));
    g.plan = items;
    // When it finished: a walk longer than `max_age_ms` must not look old at once.
    g.planned_ms = now.saturating_add(started.elapsed().as_millis() as u64);
}

/// Start [`refresh`] in the background if the last plan is older than `max_age_ms` and none is
/// running. Returns whether one is running now.
fn refresh_in_background(d: &Arc<Daemon>, now: u64, max_age_ms: u64) -> bool {
    {
        let mut g = d.worktree_sizes.lock();
        if g.working {
            return true;
        }
        if g.planned_ms > 0 && now.saturating_sub(g.planned_ms) < max_age_ms {
            return false;
        }
        g.working = true;
        g.wanted = false;
    }
    let me = d.clone();
    let spawned = std::thread::Builder::new()
        .name("worktree-sizes".into())
        .spawn(move || {
            refresh(&me, now, max_age_ms);
            me.worktree_sizes.lock().working = false;
        });
    if spawned.is_err() {
        d.worktree_sizes.lock().working = false;
        return false;
    }
    true
}

/// How old the CLI's report may be before it is measured again.
const REPORT_AGE_MS: u64 = 60_000;
/// How old the monitor's totals may be before they are measured again.
const MONITOR_AGE_MS: u64 = 2 * HOUR_MS;

/// The totals of one kind of directory in one state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Total {
    pub kind: String,
    pub state: String,
    pub count: u32,
    pub bytes: u64,
    pub free_next_bytes: u64,
}

fn totals_of(g: &SizesInner) -> Vec<Total> {
    let mut by: BTreeMap<(&str, &str), Total> = BTreeMap::new();
    for i in &g.plan {
        let Some(s) = g.got.get(&i.path) else {
            continue;
        };
        let mut add = |kind: &'static str, bytes: u64, freed: u64| {
            let t = by.entry((kind, i.state.as_str())).or_insert_with(|| Total {
                kind: kind.into(),
                state: i.state.clone(),
                ..Default::default()
            });
            t.count += 1;
            t.bytes += bytes;
            t.free_next_bytes += freed;
        };
        let removed = matches!(i.action, Some(Action::Remove(_)));
        let trimmed = matches!(i.action, Some(Action::Trim(_)));
        match i.kind {
            Kind::Landing => add("landing scratch", s.all, if removed { s.all } else { 0 }),
            Kind::Task => {
                let rest = s.all.saturating_sub(s.outputs);
                add("task worktree", rest, if removed { rest } else { 0 });
                if !i.outputs.is_empty() {
                    let freed = if removed || trimmed { s.outputs } else { 0 };
                    add("build output", s.outputs, freed);
                }
            }
        }
    }
    by.into_values().collect()
}

/// The totals for the monitor, from the last measurement. Asking has the next sweep measure
/// again when that is old.
pub fn totals(d: &Daemon) -> Vec<Total> {
    let mut g = d.worktree_sizes.lock();
    g.wanted = true;
    totals_of(&g)
}

fn item_json(i: &Item, s: Option<&Size>) -> Value {
    let (action, why) = match &i.action {
        Some(Action::Remove(w)) => (Some("remove"), Some(w)),
        Some(Action::Trim(w)) => (Some("trim"), Some(w)),
        None => (None, None),
    };
    let frees = s.map(|s| match i.action {
        Some(Action::Remove(_)) => s.all,
        Some(Action::Trim(_)) => s.outputs,
        None => 0,
    });
    json!({
        "kind": match i.kind { Kind::Task => "task", Kind::Landing => "landing" },
        "project": i.project,
        "task": i.task.map(|t| t.0),
        "state": i.state,
        "path": i.path.display().to_string(),
        "outputs": i.outputs,
        "bytes": s.map(|s| s.all),
        "output_bytes": s.map(|s| s.outputs),
        "action": action,
        "why": why,
        "frees_bytes": frees,
        "skipped": i.skipped,
    })
}

fn totals_json(t: &[Total]) -> Value {
    t.iter()
        .map(|t| {
            json!({"kind": t.kind, "state": t.state, "count": t.count, "bytes": t.bytes,
                   "free_next_bytes": t.free_next_bytes})
        })
        .collect::<Vec<_>>()
        .into()
}

/// The report at `now`, measured on this thread: every worktree, its size, what the next sweep
/// would do, and the totals. Removes nothing.
pub fn report(d: &Daemon, now: u64) -> Value {
    refresh(d, now, 0);
    cached_report(d, false)
}

fn cached_report(d: &Daemon, measuring: bool) -> Value {
    let g = d.worktree_sizes.lock();
    let totals = totals_of(&g);
    json!({
        "measuring": measuring,
        "items": g.plan.iter().map(|i| item_json(i, g.got.get(&i.path))).collect::<Vec<_>>(),
        "bytes": totals.iter().map(|t| t.bytes).sum::<u64>(),
        "free_next_bytes": totals.iter().map(|t| t.free_next_bytes).sum::<u64>(),
        "totals": totals_json(&totals),
    })
}

/// The report for a client that must not wait: the last one, with `measuring` true while a new
/// one is being made.
pub fn report_async(d: &Arc<Daemon>, now: u64) -> Value {
    let measuring = refresh_in_background(d, now, REPORT_AGE_MS);
    cached_report(d, measuring)
}

/// `mason disk clean`: run the sweep at `now`, or (`dry_run`) only say what it would do.
pub fn clean(d: &Arc<Daemon>, now: u64, dry_run: bool) -> Value {
    let items = if dry_run {
        plan(d, now, false)
            .into_iter()
            .filter(|i| i.action.is_some() || i.skipped.is_some())
            .collect()
    } else {
        pass(d, now)
    };
    let g = d.worktree_sizes.lock();
    json!({
        "dry_run": dry_run,
        "items": items.iter().map(|i| item_json(i, g.got.get(&i.path))).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_000_000 * HOUR_MS;

    fn facts(state: State) -> Facts {
        Facts {
            state,
            keeps_branch: true,
            rejected: false,
            since_ms: T0,
            restored_ms: 0,
            in_use: None,
            branch_exists: true,
            contained: false,
            dirty: None,
        }
    }

    fn at(f: &Facts, hours: u64) -> Removal {
        removal(f, &KeepCfg::default(), T0 + hours * HOUR_MS)
    }

    fn goes(r: Removal) -> bool {
        matches!(r, Removal::Remove(_))
    }

    #[test]
    fn a_canceled_task_with_its_branch_is_kept_a_day() {
        let f = facts(State::Canceled);
        assert_eq!(at(&f, 23), Removal::NotDue);
        assert!(goes(at(&f, 24)));
    }

    #[test]
    fn a_branch_already_in_the_base_goes_at_once() {
        let f = Facts {
            contained: true,
            ..facts(State::Canceled)
        };
        assert!(goes(at(&f, 0)));
    }

    #[test]
    fn a_failed_task_is_kept_three_days_even_with_nothing_committed() {
        let f = Facts {
            contained: true,
            ..facts(State::Failed)
        };
        assert_eq!(at(&f, 71), Removal::NotDue);
        assert!(goes(at(&f, 72)));
        let rejected = Facts {
            rejected: true,
            ..facts(State::Canceled)
        };
        assert_eq!(at(&rejected, 71), Removal::NotDue);
        assert!(goes(at(&rejected, 72)));
    }

    #[test]
    fn a_dirty_or_used_worktree_is_never_removed() {
        for contained in [false, true] {
            let dirty = Facts {
                contained,
                dirty: Some("uncommitted changes (1 paths)".into()),
                ..facts(State::Canceled)
            };
            assert!(matches!(at(&dirty, 10_000), Removal::Skip(_)));
            let used = Facts {
                contained,
                in_use: Some("a step host is running".into()),
                ..facts(State::Canceled)
            };
            assert!(matches!(at(&used, 10_000), Removal::Skip(_)));
        }
        let gone = Facts {
            branch_exists: false,
            ..facts(State::Failed)
        };
        assert!(matches!(at(&gone, 10_000), Removal::Skip(_)));
    }

    #[test]
    fn other_states_are_never_removed() {
        for s in State::ALL {
            if !matches!(s, State::Canceled | State::Failed) {
                assert_eq!(at(&facts(s), 10_000), Removal::Never, "{s:?}");
            }
        }
        let deleting = Facts {
            keeps_branch: false,
            ..facts(State::Canceled)
        };
        assert_eq!(at(&deleting, 10_000), Removal::Never);
    }

    #[test]
    fn a_restored_worktree_is_kept_again() {
        let f = Facts {
            contained: true,
            restored_ms: T0 + 100 * HOUR_MS,
            ..facts(State::Canceled)
        };
        assert_eq!(at(&f, 101), Removal::NotDue);
        assert!(goes(at(&f, 124)));
    }

    #[test]
    fn build_output_goes_when_idle_or_when_space_is_low() {
        let k = KeepCfg::default();
        assert_eq!(trim_due(T0, false, &k, T0 + 11 * HOUR_MS), None);
        assert!(trim_due(T0, false, &k, T0 + 12 * HOUR_MS).is_some());
        assert!(trim_due(T0, true, &k, T0).is_some());
        assert!(under_pressure(39 * 1024, 20 * 1024));
        assert!(!under_pressure(40 * 1024, 20 * 1024));
        assert!(!under_pressure(0, 0));
    }

    #[test]
    fn build_output_is_matched_by_name_or_listed_path() {
        let extra = vec!["DerivedData".to_string(), "app/out".to_string()];
        for yes in [
            "target",
            "a/b/.build",
            "x/build",
            "y/DerivedData",
            "app/out",
        ] {
            assert!(is_build_output(yes, &extra), "{yes}");
        }
        for no in ["node_modules", "targets", "other/out", "out", ".cargo"] {
            assert!(!is_build_output(no, &extra), "{no}");
        }
    }

    #[test]
    fn landing_scratch_goes_when_old_or_over_the_cap() {
        let s = |used_h: u64, running: bool, batch: bool| Scratch {
            batch,
            used_ms: T0 + used_h * HOUR_MS,
            running,
        };
        let all = [
            s(0, false, true),
            s(90, false, true),
            s(99, false, true),
            s(98, false, true),
            s(0, true, true),
            s(99, false, false),
        ];
        let got = scratch_removals(&all, &KeepCfg::default(), T0 + 100 * HOUR_MS);
        let gone: Vec<bool> = got.iter().map(Option::is_some).collect();
        // The running one counts against the cap of two; the newest idle one takes the other.
        assert_eq!(gone, [true, true, false, true, false, false]);
    }

    #[test]
    fn scratch_names_are_a_projects_own() {
        assert_eq!(scratch_slot("mason-b0", "mason"), Some((0, true)));
        assert_eq!(scratch_slot("mason-post12", "mason"), Some((12, false)));
        for n in ["mason", "mason-edit", "mason-b", "mason-bx", "mason-two-b1"] {
            assert_eq!(scratch_slot(n, "mason"), None, "{n}");
        }
    }

    #[test]
    fn the_project_keys_are_read_and_checked() {
        let parse = |t: &str| KeepCfg::parse("p", &t.parse().unwrap());
        assert_eq!(parse("").unwrap(), KeepCfg::default());
        let c = parse(
            "worktree_keep_canceled_h = 1\nworktree_keep_failed_h = 2\ntarget_idle_h = 3\n\
             landing_scratch_keep_h = 4\nlanding_scratch_max = 5\nbuild_outputs = [\"out/\"]\n",
        )
        .unwrap();
        assert_eq!(
            (c.canceled_h, c.failed_h, c.target_idle_h, c.scratch_h),
            (1, 2, 3, 4)
        );
        assert_eq!((c.scratch_max, c.build_outputs), (5, vec!["out".into()]));
        for bad in [
            "target_idle_h = -1",
            "target_idle_h = \"x\"",
            "build_outputs = \"out\"",
            "build_outputs = [\"../x\"]",
            "build_outputs = [\"/x\"]",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
