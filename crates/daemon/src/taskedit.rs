// SPDX-License-Identifier: GPL-3.0-or-later

//! The owner edits a started or failed task's title and body (task 989). The text changes in
//! place (`TrackerEdited`), so `task show`, the monitor and the next step's prompt all see the
//! new scope. What it was is kept in the intervention row (`task_edit`), where `task show
//! --original` reads it. A step already running keeps the prompt it started with.

use crate::daemon::Daemon;
use mason_core::*;
use serde_json::{json, Value};
use std::sync::Arc;

pub const KIND: &str = "task_edit";

/// Edit a task's text. The caller has checked that `who` is the owner.
pub fn edit(
    d: &Arc<Daemon>,
    who: &str,
    id: u64,
    title: Option<&str>,
    body: Option<&str>,
    reason: &str,
) -> Result<Value, (u16, String)> {
    if !crate::attention::is_owner(d, who) {
        return Err((
            403,
            format!("{who:?} is not the owner's identity; only the owner can edit a task's text"),
        ));
    }
    let w = d.world().map_err(|e| (500, e.to_string()))?;
    let t = w
        .tasks
        .get(&TaskId(id))
        .ok_or((404, format!("no task #{id}")))?;
    if t.state.is_terminal() {
        return Err((
            409,
            format!("task #{id} is over ({:?}); its text is frozen", t.state),
        ));
    }
    if title.is_none() && body.is_none() {
        return Err((400, "give --title, --body or --body-file".into()));
    }
    // The tracker owns an imported task's text and writes it back over an edit made here.
    if let Some(p) = d.project(t.project) {
        if let Some(r) = d
            .store
            .tracker_items(&p.id)
            .unwrap_or_default()
            .into_iter()
            .find(|r| r.task == id)
        {
            return Err((
                409,
                format!(
                    "task #{id} comes from issue #{}; edit the issue instead, or the next sync overwrites this",
                    r.number
                ),
            ));
        }
    }
    let new_title = title.map_or_else(|| t.title.clone(), |s| s.trim().to_string());
    if new_title.is_empty() || new_title.chars().count() > 120 {
        return Err((400, "title must be 1 to 120 characters".into()));
    }
    let new_body = body.map_or_else(|| t.body.clone(), |s| s.trim().to_string());
    if new_body.is_empty() {
        return Err((400, "body must not be empty".into()));
    }
    d.emit(EventKind::TrackerEdited {
        task: TaskId(id),
        title: new_title.clone(),
        body: new_body.clone(),
        priority: t.priority,
    })
    .map_err(|e| (409, e.to_string()))?;
    let was = json!({"title": t.title, "body": t.body}).to_string();
    crate::attention::intervene(d, KIND, Some(TaskId(id)), who, &was, reason);
    Ok(
        json!({"ok": true, "state": format!("{:?}", t.state).to_lowercase(),
        "note": "a step already running keeps the prompt it started with; the next step reads the new text"}),
    )
}

/// Every edit of a task, oldest first: `{at, who, reason, title, body}` where title and body
/// are what the text was before that edit.
pub fn history(d: &Daemon, id: u64) -> Vec<Value> {
    d.store
        .interventions(0)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.kind == KIND && r.task == Some(id))
        .map(|r| {
            let was: Value = serde_json::from_str(&r.detail).unwrap_or(Value::Null);
            json!({"at": r.at, "who": r.who, "reason": r.reason,
                "title": was["title"], "body": was["body"]})
        })
        .collect()
}
