// SPDX-License-Identifier: GPL-3.0-or-later

//! Owner sessions (s364, decision 0357): the owner's conversation with an agent, held by the
//! daemon and not by a terminal.
//!
//! A session has an id, an optional task (a later stage adds sessions about no task), a kind
//! (`task` now), who opened it and when, its last activity, its turn count and a state: `open`,
//! `working` while a turn is in flight, or `ended`. Its messages are stored in order, with the
//! role `owner`, `agent` or `tool` (a tool call as one line).
//!
//! Each owner message is one adapter turn, through [`AgentAdapter`] and never a provider's
//! binary by name: `resume` of the task agent's latest session when there is one (the same
//! lookup `mason task open` uses), else `start`, in the task's worktree. The session lives in the
//! store, so it survives the dialog closing, the client going away and a daemon restart (a turn in
//! flight at a restart is marked interrupted in the transcript). It ends when the owner ends it,
//! or after `[sessions] idle_end_h` hours (default 24; 0: never) with no message.
//!
//! The rule against the pipeline: while a session is `working`, the task's next agent step does
//! not launch and its wait reads [`HOLD_REASON`]; an open session that is idle holds nothing.
//! Opening while the task's step is running asks first (`step_running`, then `confirm`).

use crate::daemon::{now_ms, Daemon};
use mason_adapter_api::{AgentEvent, Outcome, SessionSpec, StepKind};
use mason_core::*;
use mason_store::owner_sessions::{OwnerMessageRow, OwnerSessionRow};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// The session kind of a conversation about a task.
pub const KIND_TASK: &str = "task";

/// Why a task's next agent step waits while a session is replying.
pub const HOLD_REASON: &str = "owner session replying";

/// A turn is cut off after this long.
const TURN_DEADLINE: Duration = Duration::from_secs(1800);

/// At start: a turn that was in flight did not survive, so say so in its transcript; and end the
/// sessions nobody has written to for too long, from now on.
pub fn on_start(d: &Arc<Daemon>) {
    let _ = d.store.interrupt_owner_sessions(now_ms());
    crate::bg::spawn_loop(d, "owner-sessions", move |weak| {
        std::thread::sleep(Duration::from_secs(60));
        let Some(d) = weak.upgrade() else {
            return false;
        };
        end_idle(&d, now_ms());
        true
    });
}

/// End every idle session with no message for `idle_end_h` hours. Returns the ids.
pub fn end_idle(d: &Arc<Daemon>, now: u64) -> Vec<u64> {
    let hours = d.cfg.idle_end_h;
    if hours == 0 {
        return vec![];
    }
    let limit = hours.saturating_mul(3_600_000);
    let mut ended = vec![];
    for s in d.store.open_owner_sessions().unwrap_or_default() {
        if s.state == "open" && now.saturating_sub(s.last_ms) >= limit {
            let _ = d.store.add_owner_message(
                s.id,
                "tool",
                &format!("the session ended: no message for {hours} h"),
                now,
            );
            let _ = d.store.set_owner_session_state(s.id, "ended", now);
            ended.push(s.id);
        }
    }
    if !ended.is_empty() {
        d.poke();
    }
    ended
}

/// Is an owner session replying about this task? Then its next agent step waits.
pub fn replying(d: &Daemon, task: TaskId) -> bool {
    d.store
        .open_owner_sessions()
        .unwrap_or_default()
        .iter()
        .any(|s| s.task == Some(task.0) && s.state == "working")
}

/// The first message, drafted from the task. Pure over its inputs.
#[allow(clippy::too_many_arguments)]
pub fn draft_prompt(
    number: u64,
    title: &str,
    body: &str,
    state: &str,
    reason: &str,
    history: &str,
    plan: Option<&Value>,
    logs: &str,
) -> String {
    let none = |s: &str, empty: &str| {
        if s.trim().is_empty() {
            empty.to_string()
        } else {
            s.trim().to_string()
        }
    };
    format!(
        "I want to talk about task #{number}, \"{title}\". You are in its worktree.\n\n\
         # The task\n{body}\n\n\
         # Where it stands\nState: {state}.\n\n\
         # The last failure\n{reason}\n\n\
         # What each round was told\n{history}\n\n\
         # The plan\n{plan}\n\n\
         # The end of the last test logs\n{logs}\n\n\
         # What I expect\nLook at the worktree and tell me what you find and what you would do \
         next. Change nothing until I say so.\n",
        body = none(body, "(no description)"),
        reason = none(reason, "(none: it has not failed)"),
        history = none(history, "(nothing was recorded)"),
        plan = plan.map_or("(none)".to_string(), |p| p.to_string()),
        logs = none(logs, "(no logs)"),
    )
}

/// The draft for a task, from what the daemon knows of it.
pub fn draft(d: &Arc<Daemon>, task: TaskId) -> Result<String, String> {
    let w = d.world().map_err(|e| e.to_string())?;
    let t = w.tasks.get(&task).ok_or("no such task")?;
    let dir = d.cfg.task_dir(task.0);
    let plan: Option<Value> = std::fs::read_to_string(dir.join("plan.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let history = std::fs::read_to_string(dir.join("feedback-history.txt")).unwrap_or_default();
    let first = draft_prompt(
        task.0,
        &t.title,
        &t.body,
        mason_store::state_name(t.state),
        t.failure.as_deref().unwrap_or(""),
        &history,
        plan.as_ref(),
        &crate::diagnose::log_tails(&dir, 2, 30),
    );
    // A task the recovery agent handed off brings its brief, fenced as data (task 1111).
    Ok(match d.store.recovery_brief(task.0).ok().flatten() {
        Some((brief, _)) => format!(
            "{first}\n# The recovery brief\n{}\n",
            crate::recovery::fenced_brief(&brief)
        ),
        None => first,
    })
}

/// The open session of a task, if there is one.
pub fn open_of(d: &Daemon, task: TaskId) -> Option<OwnerSessionRow> {
    d.store
        .open_owner_sessions()
        .unwrap_or_default()
        .into_iter()
        .find(|s| s.task == Some(task.0))
}

/// What opening a session answers.
pub struct Opened {
    pub session: OwnerSessionRow,
    /// The drafted first message, while the session has no message of the owner's yet.
    pub draft: Option<String>,
    pub created: bool,
}

/// Open the owner's session about a task, or return the one that is open. With the task's step
/// running, ask first: `Err` with `step_running` unless `confirm`.
pub fn open_for_task(
    d: &Arc<Daemon>,
    task: TaskId,
    who: &str,
    confirm: bool,
) -> Result<Opened, OpenError> {
    let w = d.world().map_err(|e| OpenError::Other(e.to_string()))?;
    let t = w
        .tasks
        .get(&task)
        .ok_or_else(|| OpenError::Other("no such task".into()))?;
    let account = d.account_of(t.project).to_string();
    let (session, created) = match open_of(d, task) {
        Some(s) => (s, false),
        None => {
            if !confirm && w.open_run(task).is_some_and(|r| d.has_job(r)) {
                return Err(OpenError::StepRunning);
            }
            let id = d
                .store
                .add_owner_session(Some(task.0), KIND_TASK, &account, who, now_ms())
                .map_err(|e| OpenError::Other(e.to_string()))?;
            crate::attention::intervene(d, "open_session", Some(task), who, "", "");
            d.poke();
            let s = d
                .store
                .owner_session(id)
                .ok()
                .flatten()
                .ok_or_else(|| OpenError::Other("the session was not stored".into()))?;
            (s, true)
        }
    };
    let owner_spoke = d
        .store
        .owner_messages(session.id, 0)
        .unwrap_or_default()
        .iter()
        .any(|m| m.role == "owner");
    let draft = if owner_spoke {
        None
    } else {
        Some(draft(d, task).map_err(OpenError::Other)?)
    };
    Ok(Opened {
        session,
        draft,
        created,
    })
}

pub enum OpenError {
    /// The task's step is running; send again with `confirm`.
    StepRunning,
    Other(String),
}

/// The owner's message: stored at once, answered by a turn on its own thread. `Err` if the
/// session is gone, ended or already replying.
pub fn post(d: &Arc<Daemon>, id: u64, text: &str, who: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("the message is empty".into());
    }
    let s = d
        .store
        .owner_session(id)
        .map_err(|e| e.to_string())?
        .ok_or("no such session")?;
    match s.state.as_str() {
        "ended" => return Err("the session has ended".into()),
        "working" => return Err("the agent is still replying".into()),
        _ => {}
    }
    let now = now_ms();
    d.store
        .add_owner_message(id, "owner", text, now)
        .map_err(|e| e.to_string())?;
    d.store
        .set_owner_session_state(id, "working", now)
        .map_err(|e| e.to_string())?;
    let _ = who;
    d.poke();
    let d2 = d.clone();
    let text = text.to_string();
    std::thread::Builder::new()
        .name(format!("owner-turn-{id}"))
        .spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| turn(&d2, id, &text)));
            let sid = r.as_ref().ok().cloned().flatten();
            if r.is_err() {
                let _ = d2.store.add_owner_message(
                    id,
                    "tool",
                    "the turn failed inside the daemon",
                    now_ms(),
                );
            }
            d2.owner_turns
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            let _ = d2.store.finish_owner_turn(id, sid.as_deref(), now_ms());
            d2.poke();
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// One turn: the adapter's events become stored messages, in order.
fn turn(d: &Arc<Daemon>, id: u64, text: &str) -> Option<String> {
    let say = |role: &str, t: &str| {
        let _ = d.store.add_owner_message(id, role, t, now_ms());
        d.poke();
    };
    let Ok(Some(s)) = d.store.owner_session(id) else {
        return None;
    };
    let task = s.task.map(TaskId);
    let found = match task {
        Some(task) => d.world().ok().and_then(|w| {
            let t = w.tasks.get(&task)?;
            let p = d.project(t.project)?.clone();
            if let Err(e) = crate::worktrees::restore(d, task) {
                tracing::warn!(task = task.0, "the worktree could not be added again: {e}");
            }
            let wt = p.worktree(&d.cfg, task.0);
            let before = if s.adapter_session.is_empty() {
                crate::attention::session_for(d, task)
                    .ok()
                    .and_then(|v| v["session"].as_str().map(String::from))
            } else {
                Some(s.adapter_session.clone())
            };
            Some((p, wt, before))
        }),
        // A health session works in its own checkout of MASON's project (s432).
        None if s.kind == crate::health::KIND => crate::proposals::self_project(d).map(|(_, p)| {
            let dir = crate::health::checkout_dir(d, id);
            let wt = if dir.exists() { dir } else { p.path.clone() };
            (
                p.clone(),
                wt,
                Some(s.adapter_session.clone()).filter(|a| !a.is_empty()),
            )
        }),
        None => None,
    };
    let Some((project, wt, before)) = found else {
        say("tool", "the task or its project is gone");
        return None;
    };
    let adapter = crate::jobs::adapter_for(d, &project, &wt);
    let mut spec = SessionSpec::new(StepKind::Chat, text, &wt);
    spec.env = project.agent_env.clone();
    spec.env.retain(|(k, _)| k != "MASON_TASK");
    if let Some(task) = task {
        spec.env.push(("MASON_TASK".into(), task.0.to_string()));
    }
    spec.tool_timeout = Duration::from_secs(120);
    spec.deadline = Some(TURN_DEADLINE);
    spec.run_key = Some(format!(
        "t{}r0chat{}x{}",
        task.map_or(0, |t| t.0),
        id,
        s.turns
    ));
    let started = match &before {
        Some(sid) => adapter.resume(sid, text.to_string(), spec),
        None => adapter.start(spec),
    };
    let mut handle = match started {
        Ok(h) => h,
        Err(e) => {
            say("tool", &format!("the agent could not be started: {e}"));
            return None;
        }
    };
    d.owner_turns
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, handle.canceller());
    let mut sid = None;
    while let Some(ev) = handle.next_event() {
        if let Some(x) = handle.session_id() {
            sid = Some(x);
        }
        match ev {
            AgentEvent::Text(t) if !t.trim().is_empty() => say("agent", &t),
            AgentEvent::ToolCall { name, summary } => say("tool", &tool_line(&name, &summary)),
            AgentEvent::Finished(o) => {
                match o {
                    Outcome::Accepted | Outcome::NoResult => {}
                    other => say("tool", &format!("the turn ended: {other:?}")),
                }
                break;
            }
            _ => {}
        }
    }
    handle.session_id().or(sid)
}

/// A tool call as one line.
pub fn tool_line(name: &str, summary: &str) -> String {
    let one: String = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    let one: String = one.chars().take(200).collect();
    if one.is_empty() {
        name.to_string()
    } else {
        format!("{name}: {one}")
    }
}

/// End a session. A turn in flight is stopped. Idempotent.
pub fn end(d: &Arc<Daemon>, id: u64, _who: &str) -> Result<(), String> {
    d.store
        .owner_session(id)
        .map_err(|e| e.to_string())?
        .ok_or("no such session")?;
    d.store
        .set_owner_session_state(id, "ended", now_ms())
        .map_err(|e| e.to_string())?;
    crate::health::remove_checkout(d, id);
    if let Some(c) = d
        .owner_turns
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .cloned()
    {
        c();
    }
    d.poke();
    Ok(())
}

pub fn session_json(s: &OwnerSessionRow) -> Value {
    json!({
        "id": s.id,
        "task": s.task,
        "kind": s.kind,
        "account": s.account,
        "opened_by": s.opened_by,
        "opened_ms": s.opened_ms,
        "last_activity_ms": s.last_ms,
        "turns": s.turns,
        "state": s.state,
        "working": s.state == "working",
    })
}

pub fn message_json(m: &OwnerMessageRow) -> Value {
    json!({"id": m.id, "role": m.role, "text": m.text, "at_ms": m.at_ms})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_draft_names_the_task_its_failure_and_what_is_expected() {
        let t = draft_prompt(7, "Fix it", "The body", "failed", "tests red", "", None, "");
        assert!(t.contains("task #7") && t.contains("Fix it") && t.contains("The body"));
        assert!(t.contains("State: failed") && t.contains("tests red"));
        assert!(t.contains("(nothing was recorded)") && t.contains("(no logs)"));
        assert!(t.contains("# What I expect"));
    }

    #[test]
    fn a_tool_call_is_one_short_line() {
        assert_eq!(tool_line("Read", ""), "Read");
        assert_eq!(tool_line("Bash", "ls\n  -la"), "Bash: ls -la");
        assert!(tool_line("Bash", &"x".repeat(900)).len() < 220);
    }
}
