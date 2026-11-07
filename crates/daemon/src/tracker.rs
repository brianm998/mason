// SPDX-License-Identifier: GPL-3.0-or-later

//! The issue tracker as an outside signal (Part 3, Part 11). One thread polls
//! each project's tracker conditionally (an ETag, backoff, a permanent error
//! stops the poll and raises attention at once); a webhook or a person opening
//! a task refreshes at once. Items become tasks by these rules:
//!
//! - **Outside issues are ignored until the owner labels them.** An issue by
//!   the owner is imported; anyone else's only once it carries the project's
//!   label (only people with triage rights can put one on). Pull requests are
//!   never tasks.
//! - **Outside text stays untrusted**: an issue by someone else is filed with
//!   `Outside` trust, so prompts fence it, whoever labeled it.
//! - **The tracker owns title, body, priority and closure**; the database owns
//!   state. A change there reaches the task (`TrackerEdited`); a closure
//!   cancels it. State goes back out as a comment or a close, best effort: a
//!   failure to mirror never blocks the work.

use crate::config::TrackerCfg;
use crate::daemon::Daemon;
use mason_adapter_api::service::*;
use mason_core::*;
use mason_store::{state_name, TrackerItemRow};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

#[derive(Default)]
pub struct Hub {
    pollers: Mutex<HashMap<ProjectId, Poller>>,
    /// The last listing and what the tracker said about its dependencies, so the
    /// dependencies can be looked at again on a timer when the listing's ETag
    /// does not change (a dependency edit may not touch it).
    seen: Mutex<HashMap<ProjectId, Seen>>,
    /// One look at a time: the poll thread and a refresh asked for by a person
    /// must not both import the same issue.
    one_at_a_time: Mutex<()>,
    cv: Condvar,
}

struct Seen {
    items: Vec<Item>,
    /// When the dependencies were last asked about; `None` means ask again now.
    deps_at: Option<u64>,
}

/// Dependencies are asked again every this many poll intervals when the
/// listing has not changed.
const DEPS_EVERY_INTERVALS: u64 = 5;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub imported: usize,
    pub updated: usize,
    pub closed: usize,
    pub ignored: usize,
}

impl Hub {
    /// Ask now: the next look does not wait for its schedule, and a stopped
    /// poll gets one more try.
    pub fn refresh(&self, p: ProjectId) {
        // The dependencies are asked about again too, whatever the ETag says.
        if let Some(seen) = self
            .seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&p)
        {
            seen.deps_at = None;
        }
        if let Some(poller) = self
            .pollers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&p)
        {
            poller.refresh();
        }
        self.cv.notify_all();
    }

    pub fn refresh_all(&self) {
        for seen in self
            .seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values_mut()
        {
            seen.deps_at = None;
        }
        for p in self
            .pollers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values_mut()
        {
            p.refresh();
        }
        self.cv.notify_all();
    }

    pub fn stopped(&self, p: ProjectId) -> Option<String> {
        self.pollers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&p)
            .and_then(|x| x.stopped().map(String::from))
    }
}

/// Start the polling thread if any project has a tracker.
pub fn spawn(d: &Arc<Daemon>) {
    if !d.projects.values().any(|p| p.services.tracker.is_some()) {
        return;
    }
    let weak: Weak<Daemon> = Arc::downgrade(d);
    let _ = std::thread::Builder::new()
        .name("tracker".into())
        .spawn(move || loop {
            let Some(d) = weak.upgrade() else { return };
            for (pid, p) in &d.projects {
                let Ok(Some((adapter, cfg))) = crate::services::tracker(p) else {
                    continue;
                };
                let due = {
                    let mut m = d.tracker.pollers.lock().unwrap_or_else(|e| e.into_inner());
                    let poller = m.entry(*pid).or_insert_with(|| Poller::new(cfg.interval_s));
                    poller.due(d.started.elapsed().as_secs())
                };
                if due {
                    let _ = poll_once(&d, *pid, adapter.as_ref(), &cfg);
                }
            }
            let g = d.tracker.pollers.lock().unwrap_or_else(|e| e.into_inner());
            let _ = d.tracker.cv.wait_timeout(g, Duration::from_millis(500));
        });
}

/// One look at the tracker: conditional, classified, and applied.
pub fn poll_once(
    d: &Arc<Daemon>,
    pid: ProjectId,
    adapter: &dyn TrackerAdapter,
    cfg: &TrackerCfg,
) -> Result<Summary, String> {
    let _one = d
        .tracker
        .one_at_a_time
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let p = d.project(pid).ok_or("no such project")?;
    let now = d.started.elapsed().as_secs();
    let etag = {
        let mut m = d.tracker.pollers.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(pid)
            .or_insert_with(|| Poller::new(cfg.interval_s))
            .etag
            .clone()
    };
    let listing = adapter.list(etag.as_deref());
    let (observed, items) = match listing {
        Ok(Listing::NotModified) => (Observed::Unchanged, None),
        Ok(Listing::Items { items, etag }) => (Observed::Changed { etag }, Some(items)),
        Err(e) => (Observed::Failed(e), None),
    };
    let event = d
        .tracker
        .pollers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(pid)
        .or_insert_with(|| Poller::new(cfg.interval_s))
        .observe(now, observed);
    let key = "tracker";
    let mut summary = Summary::default();
    match event {
        PollEvent::Stopped { why } => {
            let text = format!("{}: the tracker poll stopped: {why}", p.id);
            d.landing().raise(pid, key, "tracker", &text);
            d.poke();
            return Err(text);
        }
        PollEvent::Backoff { why, .. } => {
            d.landing().raise(
                pid,
                key,
                "tracker",
                &format!(
                    "{}: the tracker is not answering ({why}); trying again",
                    p.id
                ),
            );
            return Err(why);
        }
        PollEvent::Changed | PollEvent::Quiet => d.landing().clear(pid, key),
    }
    let deps_due = |d: &Arc<Daemon>| {
        d.tracker
            .seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&pid)
            .is_some_and(|s| {
                s.deps_at
                    .is_none_or(|t| now.saturating_sub(t) >= DEPS_EVERY_INTERVALS * cfg.interval_s)
            })
    };
    let items = match items {
        Some(items) => Some(items),
        None if deps_due(d) && adapter.capabilities().dependencies => d
            .tracker
            .seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&pid)
            .map(|s| s.items.clone()),
        None => None,
    };
    if let Some(items) = items {
        let native = adapter.capabilities().dependencies.then(|| {
            let mut m = NativeDeps::new();
            for it in items.iter().filter(|i| !i.is_pr && i.open) {
                let by_owner = d.cfg.owner_names.iter().any(|o| same(o, &it.author));
                let labeled = it.labels.iter().any(|l| same(l, &cfg.label));
                if !(by_owner || labeled) {
                    continue;
                }
                match adapter.blocked_by(it.number) {
                    Ok(b) => {
                        m.insert(it.number, b);
                    }
                    Err(e) => tracing::warn!(
                        issue = it.number,
                        "the tracker would not say what the issue is blocked by: {e}"
                    ),
                }
            }
            m
        });
        summary = sync_with_deps(d, pid, &cfg.label, &cfg.repo, &items, native.as_ref());
        d.tracker
            .seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                pid,
                Seen {
                    items,
                    deps_at: Some(now),
                },
            );
    }
    mirror_out(d, pid, adapter);
    d.poke();
    Ok(summary)
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn priority(s: &str) -> Priority {
    match s {
        "urgent" => Priority::Urgent,
        "high" => Priority::High,
        "low" => Priority::Low,
        _ => Priority::Normal,
    }
}

/// What the tracker said each issue is blocked by, by its own relationship.
/// An issue absent from the map was not asked, or the question failed: its
/// dependencies are left as they are.
pub type NativeDeps = HashMap<u64, Vec<Blocker>>;

/// Apply a listing with no dependency relationship from the tracker (body
/// lines still count; see [`sync_with_deps`]).
pub fn sync_items(d: &Arc<Daemon>, pid: ProjectId, label: &str, items: &[Item]) -> Summary {
    sync_with_deps(d, pid, label, "", items, None)
}

/// Where a blocker of an issue stands.
enum Blocked {
    /// A task (already imported, maybe landed or canceled).
    Task(u64, TaskId),
    /// An open issue that is not a task (yet): the dependent waits to be imported.
    Pending,
    /// Closed before it was ever a task, or not visible at all: nothing to wait for.
    Ignored,
}

fn ids(v: &[u64]) -> String {
    v.iter()
        .map(|n| format!("#{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A cycle in `edges` (issue to the issues it waits for), as a path that
/// starts and ends at the same issue.
fn find_cycle(edges: &BTreeMap<u64, Vec<u64>>) -> Option<Vec<u64>> {
    fn walk(
        n: u64,
        edges: &BTreeMap<u64, Vec<u64>>,
        path: &mut Vec<u64>,
        done: &mut BTreeSet<u64>,
    ) -> Option<Vec<u64>> {
        if let Some(i) = path.iter().position(|x| *x == n) {
            let mut c = path[i..].to_vec();
            c.push(n);
            return Some(c);
        }
        if !done.insert(n) {
            return None;
        }
        path.push(n);
        for m in edges.get(&n).into_iter().flatten() {
            if let Some(c) = walk(*m, edges, path, done) {
                return Some(c);
            }
        }
        path.pop();
        None
    }
    let mut done = BTreeSet::new();
    for n in edges.keys() {
        if let Some(c) = walk(*n, edges, &mut vec![], &mut done) {
            return Some(c);
        }
    }
    None
}

/// Apply a listing: import, edit, close, and carry dependencies. (The rules in
/// the module docs.)
///
/// Dependencies are the tracker's own relationship (`native`, `None` when the
/// tracker has none) joined with `Depends on #N` / `Blocked by #N` body lines
/// of `repo`. An issue is imported only once every open blocker is a task, so
/// import order follows dependency order, within one listing too.
pub fn sync_with_deps(
    d: &Arc<Daemon>,
    pid: ProjectId,
    label: &str,
    repo: &str,
    items: &[Item],
    native: Option<&NativeDeps>,
) -> Summary {
    let mut sum = Summary::default();
    let Some(p) = d.project(pid) else { return sum };
    let mut rows: HashMap<u64, TrackerItemRow> = d
        .store
        .tracker_items(&p.id)
        .unwrap_or_default()
        .into_iter()
        .map(|r| (r.number, r))
        .collect();
    let mut alerts: Vec<String> = vec![];
    let by_number: HashMap<u64, &Item> = items
        .iter()
        .filter(|i| !i.is_pr)
        .map(|i| (i.number, i))
        .collect();
    let native_open: HashMap<u64, bool> = native
        .into_iter()
        .flatten()
        .flat_map(|(_, bs)| bs.iter().map(|b| (b.number, b.open)))
        .collect();
    // What an issue says it waits for: the relationship, then its body.
    let wants = |it: &Item| -> Vec<u64> {
        let mut v: Vec<u64> = native
            .and_then(|m| m.get(&it.number))
            .into_iter()
            .flatten()
            .map(|b| b.number)
            .collect();
        for n in mason_definition::filing::dependencies(&it.body, repo) {
            if !v.contains(&n) {
                v.push(n);
            }
        }
        v.retain(|n| *n != it.number);
        v
    };
    let resolve = |rows: &HashMap<u64, TrackerItemRow>, n: u64| -> Blocked {
        if let Some(r) = rows.get(&n) {
            return Blocked::Task(n, TaskId(r.task));
        }
        match by_number
            .get(&n)
            .map(|i| i.open)
            .or(native_open.get(&n).copied())
        {
            Some(true) => Blocked::Pending,
            _ => Blocked::Ignored,
        }
    };

    // Imported issues: edits and closures.
    for it in items.iter().filter(|i| !i.is_pr) {
        let Some(row) = rows.get(&it.number) else {
            continue;
        };
        let mut row = row.clone();
        let changed = row.title != it.title || row.body != it.body || row.priority != it.priority;
        if changed {
            // The tracker owns these; the task takes them if it can still change.
            let _ = d.emit(EventKind::TrackerEdited {
                task: TaskId(row.task),
                title: it.title.clone(),
                body: it.body.clone(),
                priority: priority(&it.priority),
            });
            row.title = it.title.clone();
            row.body = it.body.clone();
            row.priority = it.priority.clone();
            sum.updated += 1;
        }
        if row.open && !it.open {
            // Closed there: the task stops, unless it is already over.
            let live = d
                .world()
                .ok()
                .and_then(|w| w.tasks.get(&TaskId(row.task)).map(|t| t.state));
            if live.is_some_and(|s| !matches!(s, State::Landed | State::Canceled | State::Failed)) {
                let _ = d.emit(EventKind::Canceled {
                    task: TaskId(row.task),
                    delete_branch: true,
                });
            }
            sum.closed += 1;
        }
        if changed || row.open != it.open {
            row.open = it.open;
            let _ = d.store.put_tracker_item(&row);
        }
        rows.insert(it.number, row);
    }

    // New issues, in dependency order: repeat until a pass imports nothing.
    let mut waiting: Vec<&Item> = vec![];
    let mut candidates: Vec<&Item> = vec![];
    for it in items.iter().filter(|i| !i.is_pr) {
        if rows.contains_key(&it.number) {
            continue;
        }
        let by_owner = d.cfg.owner_names.iter().any(|o| same(o, &it.author));
        let labeled = it.labels.iter().any(|l| same(l, label));
        if !it.open || !(by_owner || labeled) {
            sum.ignored += 1;
            continue;
        }
        candidates.push(it);
    }
    loop {
        let mut progress = false;
        waiting.clear();
        for it in &candidates {
            if rows.contains_key(&it.number) {
                continue;
            }
            let by_owner = d.cfg.owner_names.iter().any(|o| same(o, &it.author));
            let want = wants(it);
            let resolved: Vec<Blocked> = want.iter().map(|n| resolve(&rows, *n)).collect();
            if resolved.iter().any(|b| matches!(b, Blocked::Pending)) {
                waiting.push(it);
                continue;
            }
            // MASON's own repository takes tasks only within its limits; the item stays
            // unimported and is looked at again next time.
            let trust = if by_owner {
                TrustSource::Owner
            } else {
                TrustSource::Outside
            };
            if crate::selfguard::check_filing_as(d, pid, trust).is_err() {
                continue;
            }
            let held: Vec<(u64, TaskId)> = resolved
                .iter()
                .filter_map(|b| match b {
                    Blocked::Task(n, t) => Some((*n, *t)),
                    _ => None,
                })
                .collect();
            let appended = d.emit(EventKind::TaskFiled {
                project: pid,
                title: it.title.clone(),
                body: it.body.clone(),
                priority: priority(&it.priority),
                // Someone else's words stay untrusted, however they got in.
                trust,
                explored: false,
                proposed: false,
                blockers: held.iter().map(|(_, t)| *t).collect(),
                not_before: None,
            });
            let task = appended.ok().and_then(|a| {
                a.effects.iter().find_map(|e| match e {
                    Effect::TaskCreated { task } => Some(task.0),
                    _ => None,
                })
            });
            if let Some(task) = task {
                let row = TrackerItemRow {
                    project: p.id.clone(),
                    number: it.number,
                    task,
                    title: it.title.clone(),
                    body: it.body.clone(),
                    priority: it.priority.clone(),
                    open: true,
                    mirrored: String::new(),
                };
                let _ = d.store.put_tracker_item(&row);
                let nums: Vec<u64> = held.iter().map(|(n, _)| *n).collect();
                let _ = d.store.set_tracker_deps(&p.id, it.number, &nums);
                rows.insert(it.number, row);
                sum.imported += 1;
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }
    sum.ignored += waiting.len();
    // What is still waiting: a cycle among the waiting issues is named, and so
    // is an open blocker that will never be imported.
    let waiting_nums: BTreeSet<u64> = waiting.iter().map(|i| i.number).collect();
    let edges: BTreeMap<u64, Vec<u64>> = waiting
        .iter()
        .map(|i| {
            (
                i.number,
                wants(i)
                    .into_iter()
                    .filter(|n| waiting_nums.contains(n))
                    .collect(),
            )
        })
        .collect();
    if let Some(c) = find_cycle(&edges) {
        alerts.push(format!(
            "issues {} depend on each other in a cycle ({}); none of them is imported until the cycle is broken",
            ids(&c[..c.len() - 1]),
            c.iter().map(|n| format!("#{n}")).collect::<Vec<_>>().join(" -> ")
        ));
    }
    for it in &waiting {
        let outside: Vec<u64> = wants(it)
            .into_iter()
            .filter(|n| matches!(resolve(&rows, *n), Blocked::Pending) && !waiting_nums.contains(n))
            .collect();
        if !outside.is_empty() {
            alerts.push(format!(
                "issue #{} is not imported: it is blocked by open {} which MASON has not imported (not labeled for MASON, or outside this tracker); it is looked at again next poll",
                it.number,
                ids(&outside)
            ));
        }
    }

    // Imported issues: what the tracker now says they wait for reaches the task.
    let stored = d.store.tracker_deps(&p.id).unwrap_or_default();
    let w = d.world().ok();
    for it in items.iter().filter(|i| !i.is_pr && i.open) {
        let Some(row) = rows.get(&it.number) else {
            continue;
        };
        // Not asked, or the question failed: leave the dependencies as they were.
        if native.is_some_and(|m| !m.contains_key(&it.number)) {
            continue;
        }
        let desired: BTreeSet<u64> = wants(it)
            .into_iter()
            .filter(|n| matches!(resolve(&rows, *n), Blocked::Task(..)))
            .collect();
        let prev = stored.get(&it.number).cloned().unwrap_or_default();
        if desired == prev {
            continue;
        }
        let task = TaskId(row.task);
        let state = w.as_ref().and_then(|w| w.tasks.get(&task).map(|t| t.state));
        if !matches!(state, Some(State::Proposed | State::Queued)) {
            alerts.push(format!(
                "the dependencies of issue #{} changed on the tracker (now {}) but task #{} is already {}, so it is not pulled back; the change is not applied",
                it.number,
                if desired.is_empty() { "none".to_string() } else { ids(&desired.iter().copied().collect::<Vec<_>>()) },
                row.task,
                state.map_or("gone".to_string(), |s| state_name(s).to_string()),
            ));
            continue;
        }
        let mut recorded = prev.clone();
        for n in desired.difference(&prev) {
            let Blocked::Task(_, bt) = resolve(&rows, *n) else {
                continue;
            };
            match d.emit(EventKind::BlockerAdded { task, blocker: bt }) {
                Ok(_) => {
                    recorded.insert(*n);
                }
                Err(mason_store::StoreError::Rejected(
                    mason_engine::ReduceError::BlockerCycle(path),
                )) => alerts.push(format!(
                    "issue #{} cannot wait for #{n}: that would be a cycle ({})",
                    it.number,
                    path.iter()
                        .map(|t| format!("task #{}", t.0))
                        .collect::<Vec<_>>()
                        .join(" -> ")
                )),
                Err(e) => tracing::warn!(issue = it.number, "a dependency was not applied: {e}"),
            }
        }
        for n in prev.difference(&desired) {
            let bt = rows.get(n).map(|r| TaskId(r.task));
            if let Some(bt) = bt {
                if d.emit(EventKind::BlockerRemoved { task, blocker: bt })
                    .is_ok()
                {
                    recorded.remove(n);
                }
            } else {
                recorded.remove(n);
            }
        }
        if recorded != prev {
            let v: Vec<u64> = recorded.iter().copied().collect();
            let _ = d.store.set_tracker_deps(&p.id, it.number, &v);
            sum.updated += 1;
        }
    }

    // A canceled blocker never lands: the dependent stays held, and someone is told.
    if let Ok(w) = d.world() {
        for (n, row) in &rows {
            let Some(t) = w.tasks.get(&TaskId(row.task)) else {
                continue;
            };
            if !matches!(t.state, State::Proposed | State::Queued) {
                continue;
            }
            let dead: Vec<u64> = t
                .blockers
                .iter()
                .filter(|b| w.tasks.get(b).is_some_and(|x| x.state == State::Canceled))
                .map(|b| b.0)
                .collect();
            if !dead.is_empty() {
                alerts.push(format!(
                    "task #{} (issue #{n}) is held: its blocker {} was canceled and will never land; remove the dependency to release it",
                    t.id.0,
                    dead.iter().map(|b| format!("task #{b}")).collect::<Vec<_>>().join(", ")
                ));
            }
        }
    }
    if alerts.is_empty() {
        d.landing().clear(pid, "tracker-deps");
    } else {
        alerts.sort();
        d.landing().raise(
            pid,
            "tracker-deps",
            "tracker",
            &format!("{}: {}", p.id, alerts.join("; ")),
        );
    }
    sum
}

/// State back out: a comment, or a close when the work landed. Best effort,
/// and what was mirrored is remembered only when it worked.
pub fn mirror_out(d: &Arc<Daemon>, pid: ProjectId, adapter: &dyn TrackerAdapter) {
    let Some(p) = d.project(pid) else { return };
    let Ok(w) = d.world() else { return };
    for row in d.store.tracker_items(&p.id).unwrap_or_default() {
        let Some(t) = w.tasks.get(&TaskId(row.task)) else {
            continue;
        };
        let (tag, note, close) = match t.state {
            State::Landed => ("landed", "MASON landed this.".to_string(), true),
            State::Failed => (
                "failed",
                format!(
                    "MASON could not finish this: {}",
                    t.failure.as_deref().unwrap_or("no reason recorded")
                ),
                false,
            ),
            _ => continue,
        };
        if row.mirrored == tag {
            continue;
        }
        let item = Item {
            id: String::new(),
            number: row.number,
            title: row.title.clone(),
            body: row.body.clone(),
            priority: row.priority.clone(),
            labels: vec![],
            author: String::new(),
            open: row.open,
            is_pr: false,
        };
        let r = if close && row.open {
            adapter.close(&item, &note)
        } else {
            adapter.mirror(&item, &note)
        };
        match r {
            Ok(()) => {
                let mut r2 = row.clone();
                r2.mirrored = tag.into();
                if close {
                    r2.open = false;
                }
                let _ = d.store.put_tracker_item(&r2);
            }
            Err(e) => tracing::warn!(
                task = row.task,
                "mirroring to the tracker failed (the work is not held up): {e}"
            ),
        }
    }
}
