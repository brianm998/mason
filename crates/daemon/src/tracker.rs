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
use mason_store::TrackerItemRow;
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

#[derive(Default)]
pub struct Hub {
    pollers: Mutex<HashMap<ProjectId, Poller>>,
    cv: Condvar,
}

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
        if let Some(poller) = self.pollers.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&p) {
            poller.refresh();
        }
        self.cv.notify_all();
    }

    pub fn refresh_all(&self) {
        for p in self.pollers.lock().unwrap_or_else(|e| e.into_inner()).values_mut() {
            p.refresh();
        }
        self.cv.notify_all();
    }

    pub fn stopped(&self, p: ProjectId) -> Option<String> {
        self.pollers.lock().unwrap_or_else(|e| e.into_inner()).get(&p).and_then(|x| x.stopped().map(String::from))
    }
}

/// Start the polling thread if any project has a tracker.
pub fn spawn(d: &Arc<Daemon>) {
    if !d.projects.values().any(|p| p.services.tracker.is_some()) {
        return;
    }
    let weak: Weak<Daemon> = Arc::downgrade(d);
    let _ = std::thread::Builder::new().name("tracker".into()).spawn(move || loop {
        let Some(d) = weak.upgrade() else { return };
        for (pid, p) in &d.projects {
            let Ok(Some((adapter, cfg))) = crate::services::tracker(p) else { continue };
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
pub fn poll_once(d: &Arc<Daemon>, pid: ProjectId, adapter: &dyn TrackerAdapter, cfg: &TrackerCfg) -> Result<Summary, String> {
    let p = d.project(pid).ok_or("no such project")?;
    let now = d.started.elapsed().as_secs();
    let etag = {
        let mut m = d.tracker.pollers.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(pid).or_insert_with(|| Poller::new(cfg.interval_s)).etag.clone()
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
            d.landing().raise(pid, key, "tracker", &format!("{}: the tracker is not answering ({why}); trying again", p.id));
            return Err(why);
        }
        PollEvent::Changed | PollEvent::Quiet => d.landing().clear(pid, key),
    }
    if let Some(items) = items {
        summary = sync_items(d, pid, &cfg.label, &items);
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

/// Apply a listing: import, edit, close. (The rules in the module docs.)
pub fn sync_items(d: &Arc<Daemon>, pid: ProjectId, label: &str, items: &[Item]) -> Summary {
    let mut sum = Summary::default();
    let Some(p) = d.project(pid) else { return sum };
    let known: HashMap<u64, TrackerItemRow> = d
        .store
        .tracker_items(&p.id)
        .unwrap_or_default()
        .into_iter()
        .map(|r| (r.number, r))
        .collect();
    for it in items.iter().filter(|i| !i.is_pr) {
        let by_owner = d.cfg.owner_names.iter().any(|o| same(o, &it.author));
        match known.get(&it.number) {
            None => {
                let labeled = it.labels.iter().any(|l| same(l, label));
                if !it.open || !(by_owner || labeled) {
                    sum.ignored += 1;
                    continue;
                }
                let appended = d.emit(EventKind::TaskFiled {
                    project: pid,
                    title: it.title.clone(),
                    body: it.body.clone(),
                    priority: priority(&it.priority),
                    // Someone else's words stay untrusted, however they got in.
                    trust: if by_owner { TrustSource::Owner } else { TrustSource::Outside },
                    explored: false,
                    proposed: false,
                    blockers: vec![],
                    not_before: None,
                });
                let task = appended.ok().and_then(|a| {
                    a.effects.iter().find_map(|e| match e {
                        Effect::TaskCreated { task } => Some(task.0),
                        _ => None,
                    })
                });
                if let Some(task) = task {
                    let _ = d.store.put_tracker_item(&TrackerItemRow {
                        project: p.id.clone(),
                        number: it.number,
                        task,
                        title: it.title.clone(),
                        body: it.body.clone(),
                        priority: it.priority.clone(),
                        open: true,
                        mirrored: String::new(),
                    });
                    sum.imported += 1;
                }
            }
            Some(row) => {
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
                    let live = d.world().ok().and_then(|w| w.tasks.get(&TaskId(row.task)).map(|t| t.state));
                    if live.is_some_and(|s| !matches!(s, State::Landed | State::Canceled | State::Failed)) {
                        let _ = d.emit(EventKind::Canceled { task: TaskId(row.task), delete_branch: true });
                    }
                    sum.closed += 1;
                }
                if changed || row.open != it.open {
                    row.open = it.open;
                    let _ = d.store.put_tracker_item(&row);
                }
            }
        }
    }
    sum
}

/// State back out: a comment, or a close when the work landed. Best effort,
/// and what was mirrored is remembered only when it worked.
pub fn mirror_out(d: &Arc<Daemon>, pid: ProjectId, adapter: &dyn TrackerAdapter) {
    let Some(p) = d.project(pid) else { return };
    let Ok(w) = d.world() else { return };
    for row in d.store.tracker_items(&p.id).unwrap_or_default() {
        let Some(t) = w.tasks.get(&TaskId(row.task)) else { continue };
        let (tag, note, close) = match t.state {
            State::Landed => ("landed", "MASON landed this.".to_string(), true),
            State::Failed => ("failed", format!("MASON could not finish this: {}", t.failure.as_deref().unwrap_or("no reason recorded")), false),
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
        let r = if close && row.open { adapter.close(&item, &note) } else { adapter.mirror(&item, &note) };
        match r {
            Ok(()) => {
                let mut r2 = row.clone();
                r2.mirrored = tag.into();
                if close {
                    r2.open = false;
                }
                let _ = d.store.put_tracker_item(&r2);
            }
            Err(e) => tracing::warn!(task = row.task, "mirroring to the tracker failed (the work is not held up): {e}"),
        }
    }
}
