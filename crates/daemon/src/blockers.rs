// SPDX-License-Identifier: GPL-3.0-or-later

//! Releasing, adding and reviving a task's blockers (owner only). A blocker can
//! encode an ordering the owner wanted, so only the owner's identity may change
//! it. For a task imported from an issue, GitHub's relationship stays the source
//! of truth, with two owner overrides the tracker sync respects: a release (kept
//! until the relationship on GitHub changes) and an addition (kept always).

use crate::attention::{intervene, is_owner, Refusal};
use crate::daemon::{now_ms, Daemon};
use mason_core::*;
use mason_engine::World;
use mason_store::{state_name, StoreError};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub enum Which {
    All,
    From(Vec<TaskId>),
}

/// A blocker that will not land by itself: canceled or failed.
pub fn is_dead(s: State) -> bool {
    matches!(s, State::Canceled | State::Failed)
}

/// The blockers of a proposed or queued task that are canceled or failed.
pub fn dead_blockers(w: &World, t: &Task) -> Vec<(TaskId, State)> {
    if !matches!(t.state, State::Proposed | State::Queued) {
        return vec![];
    }
    t.blockers
        .iter()
        .filter_map(|b| w.tasks.get(b).map(|x| (*b, x.state)))
        .filter(|(_, s)| is_dead(*s))
        .collect()
}

/// The commands that free a task from its dead blockers.
pub fn fix_commands(t: TaskId, dead: &[(TaskId, State)]) -> String {
    let mut v = vec![format!(
        "`mason task unblock {} --from {}`",
        t.0,
        dead.iter()
            .map(|(b, _)| b.0.to_string())
            .collect::<Vec<_>>()
            .join(",")
    )];
    for (b, s) in dead {
        v.push(match s {
            State::Canceled => format!("`mason task revive {}`", b.0),
            _ => format!("`mason task retry {}`", b.0),
        });
    }
    v.join(" or ")
}

fn names(dead: &[(TaskId, State)]) -> String {
    dead.iter()
        .map(|(b, s)| {
            format!(
                "#{} ({})",
                b.0,
                if *s == State::Canceled {
                    "cancelled"
                } else {
                    "failed"
                }
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// "held: blocked by #8 (cancelled)" with the commands, for a task held by dead blockers.
pub fn held_line(w: &World, t: &Task) -> Option<String> {
    let dead = dead_blockers(w, t);
    if dead.is_empty() {
        return None;
    }
    Some(format!(
        "held: blocked by {}; free it with {}",
        names(&dead),
        fix_commands(t.id, &dead)
    ))
}

/// The alert for a task held by a blocker that can no longer land.
pub fn alert_text(w: &World, t: &Task) -> Option<String> {
    let dead = dead_blockers(w, t);
    if dead.is_empty() {
        return None;
    }
    Some(format!(
        "Task #{} is held: its blocker {} was canceled or failed and will not land by itself. Free it with {}",
        t.id.0,
        names(&dead),
        fix_commands(t.id, &dead)
    ))
}

/// One blocker as shown: its state, where it came from, and what can be done.
pub struct Link {
    pub task: TaskId,
    pub state: State,
    pub origin: &'static str,
    pub actions: Vec<&'static str>,
}

/// Where each of a task's blockers came from. A task imported from an issue has
/// the blockers GitHub lists "from GitHub"; every other one is "set by you".
pub fn links(d: &Daemon, w: &World, t: &Task) -> Vec<Link> {
    if t.blockers.is_empty() {
        return vec![];
    }
    let from_github = github_blockers(d, t);
    let open = matches!(t.state, State::Proposed | State::Queued);
    t.blockers
        .iter()
        .map(|b| {
            let state = w.tasks.get(b).map_or(State::Canceled, |x| x.state);
            let mut actions = vec![];
            if open {
                actions.push("release");
            }
            match state {
                State::Canceled if w.tasks.get(b).is_some_and(|x| x.cancel_keeps_branch) => {
                    actions.push("revive")
                }
                State::Failed => actions.push("retry"),
                _ => {}
            }
            Link {
                task: *b,
                state,
                origin: if from_github.contains(b) {
                    "from GitHub"
                } else {
                    "set by you"
                },
                actions,
            }
        })
        .collect()
}

/// The blocker tasks GitHub's relationship put on this task.
fn github_blockers(d: &Daemon, t: &Task) -> BTreeSet<TaskId> {
    let Some(p) = d.project(t.project) else {
        return BTreeSet::new();
    };
    let rows = d.store.tracker_items(&p.id).unwrap_or_default();
    let Some(row) = rows.iter().find(|r| r.task == t.id.0) else {
        return BTreeSet::new();
    };
    let deps = d.store.tracker_deps(&p.id).unwrap_or_default();
    let nums = deps.get(&row.number).cloned().unwrap_or_default();
    rows.iter()
        .filter(|r| nums.contains(&r.number))
        .map(|r| TaskId(r.task))
        .collect()
}

fn owner_only(d: &Daemon, who: &str, what: &str) -> Result<(), Refusal> {
    if is_owner(d, who) {
        Ok(())
    } else {
        Err((
            403,
            format!("{who:?} is not the owner's identity; only the owner can {what}, because a blocker can encode an ordering the owner wanted"),
        ))
    }
}

fn refuse(e: StoreError, task: TaskId) -> Refusal {
    use mason_engine::ReduceError as R;
    match e {
        StoreError::Rejected(R::BlockerCycle(path)) => (
            422,
            format!(
                "that would be a cycle: {}",
                path.iter()
                    .map(|t| format!("#{}", t.0))
                    .collect::<Vec<_>>()
                    .join(" -> ")
            ),
        ),
        StoreError::Rejected(R::NotABlocker(b)) => {
            (409, format!("#{} is not a blocker of #{}", b.0, task.0))
        }
        StoreError::Rejected(R::UnknownTask(t)) => (404, format!("no task #{}", t.0)),
        StoreError::Rejected(R::Illegal { from, .. }) => (
            409,
            format!(
                "task #{} is {}; blockers can change only while it is queued (or proposed)",
                task.0,
                state_name(from)
            ),
        ),
        other => (500, other.to_string()),
    }
}

fn task_of(w: &World, id: TaskId) -> Result<&Task, Refusal> {
    w.tasks.get(&id).ok_or((404, format!("no task #{}", id.0)))
}

fn describe(w: &World, t: TaskId) -> String {
    match w.tasks.get(&t) {
        Some(x) if x.blockers.is_empty() => {
            format!("task #{} has no blockers left and can be claimed", t.0)
        }
        Some(x) => format!(
            "task #{} is still held by {}",
            t.0,
            x.blockers
                .iter()
                .map(|b| format!("#{}", b.0))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        None => String::new(),
    }
}

fn issue_of(d: &Daemon, t: &Task) -> Option<(String, u64)> {
    let p = d.project(t.project)?;
    d.store
        .tracker_items(&p.id)
        .ok()?
        .into_iter()
        .find(|r| r.task == t.id.0)
        .map(|r| (p.id.clone(), r.number))
}

/// Release a task from some or all of its blockers.
pub fn release(d: &Arc<Daemon>, who: &str, task: TaskId, which: Which) -> Result<String, Refusal> {
    owner_only(d, who, "release a blocker")?;
    let _quiet = d.tracker.quiet();
    let w = d.world().map_err(|e| (500, e.to_string()))?;
    let t = task_of(&w, task)?;
    if !matches!(t.state, State::Proposed | State::Queued) {
        return Err((
            409,
            format!(
                "task #{} is {}; blockers can change only while it is queued (or proposed)",
                task.0,
                state_name(t.state)
            ),
        ));
    }
    let released: Vec<TaskId> = match which {
        Which::All => t.blockers.iter().copied().collect(),
        Which::From(v) => v,
    };
    if released.is_empty() {
        return Err((409, format!("task #{} has no blockers to release", task.0)));
    }
    d.emit(EventKind::BlockersChanged {
        task,
        released: released.clone(),
        added: vec![],
    })
    .map_err(|e| refuse(e, task))?;
    // GitHub still lists what was released: remember that the owner released it.
    if let Some((project, number)) = issue_of(d, t) {
        let rows = d.store.tracker_items(&project).unwrap_or_default();
        let mut deps = d.store.tracker_deps(&project).unwrap_or_default();
        let mut mine = deps.remove(&number).unwrap_or_default();
        for r in &released {
            let _ = d
                .store
                .drop_tracker_overrides(&project, number, Some("add"), Some(r.0));
            if let Some(n) = rows.iter().find(|x| x.task == r.0).map(|x| x.number) {
                if mine.remove(&n) {
                    let _ =
                        d.store
                            .put_tracker_override(&project, number, r.0, "release", now_ms());
                }
            }
        }
        let v: Vec<u64> = mine.into_iter().collect();
        let _ = d.store.set_tracker_deps(&project, number, &v);
    }
    let ids = released
        .iter()
        .map(|b| format!("#{}", b.0))
        .collect::<Vec<_>>()
        .join(", ");
    intervene(d, "unblock", Some(task), who, &ids, "");
    let w = d.world().map_err(|e| (500, e.to_string()))?;
    Ok(format!(
        "released task #{} from {ids}; {}",
        task.0,
        describe(&w, task)
    ))
}

/// Add blockers to a task that has not started.
pub fn block(
    d: &Arc<Daemon>,
    who: &str,
    task: TaskId,
    after: &[TaskId],
) -> Result<String, Refusal> {
    owner_only(d, who, "add a blocker")?;
    let _quiet = d.tracker.quiet();
    let w = d.world().map_err(|e| (500, e.to_string()))?;
    let t = task_of(&w, task)?;
    if after.is_empty() {
        return Err((400, "name the tasks to wait for".into()));
    }
    crate::http::check_after(d, after).map_err(|e| (409, e))?;
    if after.contains(&task) {
        return Err((422, format!("that would be a cycle: #{0} -> #{0}", task.0)));
    }
    let fresh: Vec<TaskId> = after
        .iter()
        .copied()
        .filter(|b| !t.blockers.contains(b))
        .collect();
    if fresh.is_empty() {
        return Err((409, format!("task #{} is already blocked by those", task.0)));
    }
    d.emit(EventKind::BlockersChanged {
        task,
        released: vec![],
        added: fresh.clone(),
    })
    .map_err(|e| refuse(e, task))?;
    if let Some((project, number)) = issue_of(d, t) {
        for b in &fresh {
            let _ = d
                .store
                .put_tracker_override(&project, number, b.0, "add", now_ms());
        }
    }
    let ids = fresh
        .iter()
        .map(|b| format!("#{}", b.0))
        .collect::<Vec<_>>()
        .join(", ");
    intervene(d, "block", Some(task), who, &ids, "");
    let w = d.world().map_err(|e| (500, e.to_string()))?;
    Ok(format!(
        "task #{} now waits for {ids}; {}",
        task.0,
        describe(&w, task)
    ))
}

/// Put a canceled task whose branch was kept back in the queue.
pub fn revive(d: &Arc<Daemon>, who: &str, task: TaskId) -> Result<String, Refusal> {
    owner_only(d, who, "revive a task")?;
    let _quiet = d.tracker.quiet();
    let w = d.world().map_err(|e| (500, e.to_string()))?;
    let t = task_of(&w, task)?;
    if t.state != State::Canceled {
        return Err((
            409,
            format!(
                "task #{} is {}; only a canceled task can be revived (a failed one is retried with `mason task retry {}`)",
                task.0,
                state_name(t.state),
                task.0
            ),
        ));
    }
    if !t.cancel_keeps_branch {
        return Err((
            409,
            format!(
                "task #{} was canceled with its branch deleted; there is nothing to revive",
                task.0
            ),
        ));
    }
    if let Some((project, number)) = issue_of(d, t) {
        let open = d
            .store
            .tracker_items(&project)
            .unwrap_or_default()
            .iter()
            .any(|r| r.number == number && r.open);
        if !open {
            return Err((409, format!("issue #{number} is closed on the tracker; reopen it first, or the next poll would cancel the task again")));
        }
    }
    d.emit(EventKind::Revived { task })
        .map_err(|e| refuse(e, task))?;
    intervene(d, "revive", Some(task), who, "", "");
    Ok(format!(
        "task #{} is queued again, with its branch and links kept",
        task.0
    ))
}

/// For `mason task show`, the JSON of a task's blockers.
pub fn links_json(d: &Daemon, w: &World, t: &Task) -> serde_json::Value {
    serde_json::json!(links(d, w, t)
        .iter()
        .map(|l| serde_json::json!({
            "task": l.task.0,
            "state": state_name(l.state),
            "origin": l.origin,
            "actions": l.actions,
        }))
        .collect::<Vec<_>>())
}

/// The overrides for a project, keyed by issue: released tasks and added tasks.
pub fn overrides(
    d: &Daemon,
    project: &str,
) -> (
    BTreeMap<u64, BTreeSet<TaskId>>,
    BTreeMap<u64, BTreeSet<TaskId>>,
) {
    let mut rel: BTreeMap<u64, BTreeSet<TaskId>> = BTreeMap::new();
    let mut add: BTreeMap<u64, BTreeSet<TaskId>> = BTreeMap::new();
    for (n, b, kind) in d.store.tracker_overrides(project).unwrap_or_default() {
        let m = if kind == "release" {
            &mut rel
        } else {
            &mut add
        };
        m.entry(n).or_default().insert(TaskId(b));
    }
    (rel, add)
}
