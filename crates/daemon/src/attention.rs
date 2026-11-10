// SPDX-License-Identifier: GPL-3.0-or-later

//! Human attention and overrides (Part 10): decision cards and their answers,
//! pauses, the record of every intervention, alerts tied to live conditions,
//! and the "interventions per landed task" metric.

use crate::daemon::{now_ms, Daemon};
use mason_adapter_api::card::{self, Card};
use mason_core::*;
use mason_store::{PauseRow, StoreError};
use serde_json::{json, Value};
use std::sync::Arc;

/// An HTTP-shaped error: a status and what to tell the caller.
pub type Refusal = (u16, String);

/// Only the owner's identity counts as an answer.
pub fn is_owner(d: &Daemon, who: &str) -> bool {
    let who = who.trim();
    !who.is_empty() && d.cfg.owner_names.iter().any(|n| n == who)
}

/// Record one thing a person did to the pipeline. These are what the
/// "interventions per landed task" metric counts. Giving a reason is
/// optional: asking for one adds friction exactly when someone is in a hurry.
pub fn intervene(
    d: &Daemon,
    kind: &str,
    task: Option<TaskId>,
    who: &str,
    detail: &str,
    reason: &str,
) {
    let project = task
        .and_then(|t| {
            d.world()
                .ok()
                .and_then(|w| w.tasks.get(&t).map(|x| x.project))
        })
        .and_then(|p| d.project(p).map(|p| p.id.clone()))
        .unwrap_or_default();
    let _ = d.store.record_intervention(
        kind,
        task.map(|t| t.0),
        &project,
        who,
        detail,
        reason,
        now_ms(),
    );
}

/// The card a task is waiting on, with its hash. Older logs stored the bare
/// question; it is shown as a card with no options.
pub fn card_of(d: &Daemon, t: &Task) -> Option<(Card, String)> {
    let w = d.world().ok()?;
    let id = t.decision?;
    let text = &w.decisions.get(&id)?.question;
    let card = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| Card::from_json(&v))
        .unwrap_or(Card {
            ask: text.clone(),
            why: String::new(),
            options: vec![],
            evidence: vec![],
            links: vec![],
            key: String::new(),
            origin: card::Origin::Agent,
            problems: vec![],
        });
    Some((card, card::hash(text)))
}

/// What the owner sees of a task's open question.
pub fn card_json(card: &Card, hash: &str) -> Value {
    let mut v = card.to_json();
    v["hash"] = json!(hash);
    // The monitor always adds "Other" with a text box.
    v["other"] = json!(true);
    v
}

/// Answer a task's open question. `hash` is the hash of the card the answerer
/// was looking at: a late tap on a stale card is refused. `answer` is an
/// option number, an option's label, or free text (the "Other" box).
pub fn answer(
    d: &Arc<Daemon>,
    task: TaskId,
    answer: &str,
    hash: &str,
    who: &str,
) -> Result<String, Refusal> {
    let w = d.world().map_err(|e| (500, e.to_string()))?;
    let t = w
        .tasks
        .get(&task)
        .ok_or((404, format!("no task #{}", task.0)))?;
    let Some((card, current)) = card_of(d, t).filter(|_| t.state == State::NeedsAttention) else {
        return Err((409, format!("task #{} has no open question", task.0)));
    };
    if !is_owner(d, who) {
        intervene(
            d,
            "refused_answer",
            Some(task),
            who,
            "not the owner's identity",
            "",
        );
        return Err((
            403,
            format!("{who:?} is not the owner's identity; only the owner's answer counts"),
        ));
    }
    if hash != current {
        return Err((
            409,
            "stale: that card has changed since you looked (it was answered, replaced or asked again). Look at the current one.".into(),
        ));
    }
    let text = match card.pick(answer) {
        Some(o) => o.label.clone(),
        None if answer.trim().is_empty() => return Err((400, "an answer needs text".into())),
        None if answer.trim().parse::<usize>().is_ok() => {
            return Err((400, format!("there is no option {}", answer.trim())))
        }
        None => format!("Other: {}", answer.trim()),
    };
    let def = d
        .pinned_def(&t.def)
        .unwrap_or_else(|| d.current_def(t.project));
    let step = def
        .first_in(State::Developing)
        .map(|s| s.name.clone())
        .unwrap_or_default();
    let decision = t.decision.expect("checked by card_of");
    remember(d, task, &card.ask, &text);
    d.emit(EventKind::Answered {
        task,
        decision,
        answer: text.clone(),
        step,
    })
    .map_err(|e| match e {
        StoreError::Rejected(r) => (409, format!("not allowed: {r:?}")),
        other => (500, other.to_string()),
    })?;
    intervene(d, "answer", Some(task), who, &text, "");
    Ok(text)
}

/// Keep what the owner decided where the resumed agent will read it.
pub fn remember(d: &Daemon, task: TaskId, ask: &str, answer: &str) {
    use std::io::Write;
    let dir = d.cfg.task_dir(task.0);
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("decisions.txt"))
    {
        let _ = writeln!(f, "You asked: {ask}\nThe owner answered: {answer}\n");
    }
}

/// An earlier answer to a question about the same thing (same `key`), if the
/// task has had one. A gate that fails again after the owner answered is
/// acknowledged with that answer rather than asked again, so there are no loops.
pub fn earlier_answer(d: &Daemon, task: TaskId, key: &str) -> Option<String> {
    if key.is_empty() {
        return None;
    }
    let w = d.world().ok()?;
    w.decisions
        .values()
        .filter(|x| x.task == task)
        .filter_map(|x| {
            let v: Value = serde_json::from_str(&x.question).ok()?;
            (Card::from_json(&v)?.key == key)
                .then(|| x.answer.clone())
                .flatten()
        })
        .next_back()
}

// ---- pauses ----------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    /// Claiming a queued task.
    Claim,
    /// Starting a step of an admitted task.
    Step,
    /// A landing step.
    Landing,
}

fn applies(mode: &str, what: What) -> bool {
    matches!(
        (mode, what),
        ("all", _) | ("claims", What::Claim) | ("landing", What::Landing)
    )
}

/// The pause that stops `what` for `project`, if one is in force.
pub fn paused(d: &Daemon, project: &str, what: What) -> Option<PauseRow> {
    d.store
        .pauses(now_ms())
        .ok()?
        .into_iter()
        .find(|p| (p.project.is_empty() || p.project == project) && applies(&p.mode, what))
}

pub const MODES: &[&str] = &["all", "claims", "landing"];

/// Pause for a time: all work (it stops at the next step boundary), new
/// claims only, or landing only; one project or all. A pause is a runtime
/// override, so it carries an owner and an expiry.
pub fn pause(
    d: &Arc<Daemon>,
    project: &str,
    mode: &str,
    owner: &str,
    secs: u64,
) -> Result<(), String> {
    if !MODES.contains(&mode) {
        return Err(format!("mode is one of {}", MODES.join(", ")));
    }
    if secs == 0 {
        return Err("a pause needs an expiry".into());
    }
    if !project.is_empty() && d.project_id_by_name(project).is_none() {
        return Err(format!("no project {project:?}"));
    }
    let now = now_ms();
    d.store
        .add_pause(project, mode, owner, now, now + secs * 1000)
        .map_err(|e| e.to_string())?;
    d.poke();
    Ok(())
}

pub fn resume(d: &Arc<Daemon>, project: &str, mode: &str) -> Result<usize, String> {
    let n = d
        .store
        .remove_pauses(project, mode)
        .map_err(|e| e.to_string())?;
    // Claims that were held back can go now.
    d.admit();
    d.poke();
    Ok(n)
}

// ---- alerts ----------------------------------------------------------------

/// How long a lost step host keeps its alert.
const HOST_LOSS_ALERT_MS: u64 = 3_600_000;
/// Hosts of different tasks lost within this long of each other were lost together.
const TOGETHER_MS: u64 = 5_000;

/// Alerts for lost step hosts (oldest first in `rows`): one for each cluster of
/// two or more tasks' hosts lost within five seconds of each other ("something
/// killed them"), and one for a task whose host was lost again after its step
/// was already re-run once. A single loss that the re-run answered is only in
/// the task's history.
pub fn host_loss_alerts(rows: &[mason_store::HostLossRow]) -> Vec<(String, String, String)> {
    let mut rows: Vec<&mason_store::HostLossRow> = rows.iter().collect();
    rows.sort_by(|a, b| (a.lost_at_ms, &a.host_id).cmp(&(b.lost_at_ms, &b.host_id)));
    let mut clusters: Vec<Vec<&mason_store::HostLossRow>> = vec![];
    for r in &rows {
        match clusters.last_mut() {
            Some(c)
                if r.lost_at_ms
                    .saturating_sub(c.last().map_or(0, |l| l.lost_at_ms))
                    <= TOGETHER_MS =>
            {
                c.push(r)
            }
            _ => clusters.push(vec![r]),
        }
    }
    let mut out = vec![];
    let mut together = std::collections::HashSet::new();
    for c in &clusters {
        let mut tasks: Vec<u64> = c.iter().map(|r| r.task).collect();
        tasks.sort_unstable();
        tasks.dedup();
        if tasks.len() < 2 {
            continue;
        }
        together.extend(c.iter().map(|r| r.host_id.clone()));
        let list = tasks
            .iter()
            .map(|t| format!("#{t}"))
            .collect::<Vec<_>>()
            .join(", ");
        let details = c
            .iter()
            .map(|r| format!("task #{} {}: {}", r.task, r.step, r.detail))
            .collect::<Vec<_>>()
            .join("; ");
        out.push((
            format!("hosts_lost_together:{}", c[0].host_id),
            "host_lost".into(),
            format!(
                "{} step hosts were lost at the same time (tasks {list}): something killed them. {details}",
                c.len()
            ),
        ));
    }
    for r in &rows {
        let earlier = rows
            .iter()
            .any(|e| e.task == r.task && e.lost_at_ms < r.lost_at_ms && e.host_id != r.host_id);
        if earlier && !together.contains(&r.host_id) {
            out.push((
                format!("host_lost:{}", r.host_id),
                "host_lost".into(),
                format!(
                    "Task #{}: the step host of `{}` was lost again: {}",
                    r.task, r.step, r.detail
                ),
            ));
        }
    }
    out
}

/// How old a measurement of the directories may be before it is made again.
const DISK_SIZES_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(300);

/// Free disk is below the floor, or a store write was just refused for want of it. Names the
/// free space, the floor and what MASON knows takes room. Exists only while it is true.
fn disk_alert(d: &Daemon, w: &mason_engine::World) -> Option<(String, String, String)> {
    let low = d.broker.disk_low();
    let full = d.store_full_since();
    if low.is_none() && full.is_none() {
        return None;
    }
    let mut text = match low {
        Some(g) => format!(
            "Disk nearly full: {} GB free, floor {} GB; no new compile or agent session starts until there is room",
            crate::disk::gb(g.free_mb),
            crate::disk::gb(g.floor_mb)
        ),
        None => "Disk full: MASON could not write to its store".to_string(),
    };
    if low.is_some() && full.is_some() {
        text.push_str("; a write to the store was refused");
    }
    let sizes = d.disk_sizes.get(
        vec![
            ("task_cache".into(), crate::taskcache::dirs(d, w)),
            ("worktrees".into(), vec![d.cfg.worktrees_dir()]),
        ],
        DISK_SIZES_MAX_AGE,
    );
    let known: Vec<String> = sizes
        .iter()
        .filter(|(_, b)| **b > 0)
        .map(|(n, b)| format!("{n} {:.1} GB", *b as f64 / 1e9))
        .collect();
    if !known.is_empty() {
        text.push_str(&format!(". MASON knows of: {}", known.join(", ")));
    }
    Some(("disk".into(), "disk".into(), text))
}

/// The conditions that are true right now, as `(key, kind, text)`. An alert
/// exists exactly as long as its condition does.
pub fn alerts_now(d: &Daemon) -> Vec<(String, String, String)> {
    let mut out = vec![];
    let Ok(w) = d.world() else { return out };
    for t in w.tasks.values() {
        match t.state {
            State::NeedsAttention => {
                let ask = card_of(d, t).map_or_else(String::new, |(c, _)| c.ask);
                out.push((
                    format!("question:{}", t.id.0),
                    "needs_you".into(),
                    format!("Task #{} needs a decision: {ask}", t.id.0),
                ));
            }
            State::Failed => out.push((
                format!("failed:{}", t.id.0),
                "failed".into(),
                format!(
                    "Task #{} failed: {}",
                    t.id.0,
                    t.failure.as_deref().unwrap_or("no reason recorded")
                ),
            )),
            _ => {}
        }
        if let Some(text) = crate::blockers::alert_text(&w, t) {
            // One per task, from the world as it is: it clears when the task is released
            // or the blocker is revived or retried.
            out.push((format!("held-blocker:{}", t.id.0), "blocked".into(), text));
        }
        if let Activity::Waiting(WaitReason::On(why)) = &t.activity {
            if why.starts_with("environment problem") {
                out.push((
                    format!("environment:{}", t.id.0),
                    "environment".into(),
                    format!("Task #{}: {why}", t.id.0),
                ));
            }
        }
    }
    out.extend(d.landing().attention());
    if let Some(text) = d.exit_numbers_warning() {
        out.push(("numbers".into(), "numbers".into(), text));
    }
    out.extend(disk_alert(d, &w));
    out.extend(host_loss_alerts(
        &d.store
            .host_losses_since(now_ms().saturating_sub(HOST_LOSS_ALERT_MS))
            .unwrap_or_default(),
    ));
    if let Some(text) = crate::update::Runtimes::new(&d.cfg.data_dir).alert() {
        out.push(("runtime".into(), "runtime".into(), text));
    }
    for (pid, p) in &d.projects {
        if let Some(problems) = d.definition_error(*pid) {
            out.push((
                format!("definition:{}", p.id),
                "definition".into(),
                format!(
                    "{}: the definition is refused and the last good one is running ({})",
                    p.id,
                    problems.first().cloned().unwrap_or_default()
                ),
            ));
        }
    }
    for p in d.store.pauses(now_ms()).unwrap_or_default() {
        let scope = if p.project.is_empty() {
            "everything"
        } else {
            &p.project
        };
        out.push((
            format!("paused:{}:{}", p.project, p.mode),
            "paused".into(),
            format!("Paused ({}) for {scope} by {}", p.mode, p.owner),
        ));
    }
    // A provider event is one item, whatever number of tasks wait on it.
    out.extend(crate::holds::alerts(d));
    let owed = d.store.debts(None, true).unwrap_or_default().len();
    if owed > 0 {
        out.push((
            "debts".into(),
            "debt".into(),
            format!("{owed} skipped check(s) still owed"),
        ));
    }
    out
}

/// Raise new alerts and clear the ones whose condition is gone.
pub fn sync_alerts(d: &Daemon) -> (Vec<String>, Vec<String>) {
    // One at a time: two callers (the watcher and a request for the list) that each work out what
    // is true and then write it can cross, and the older answer would clear an alert the newer one
    // had just raised, to be raised again as "new".
    let _one = d.notify.syncing.lock().unwrap_or_else(|e| e.into_inner());
    d.store
        .sync_alerts(&alerts_now(d), now_ms())
        .unwrap_or_default()
}

// ---- the metric ------------------------------------------------------------

/// Interventions per landed task: how much the pipeline needs you.
/// `since_ms` limits the window (0: everything). Only a person's actions count;
/// the pipeline acknowledging an answer it already had does not, and neither
/// does an answer refused for coming from someone else.
pub fn metrics(d: &Daemon, since_ms: u64) -> Value {
    let rows = d.store.interventions(since_ms).unwrap_or_default();
    let counted: Vec<_> = rows
        .iter()
        .filter(|r| r.who != "pipeline" && r.kind != "refused_answer")
        .collect();
    let landed = d
        .world()
        .map(|w| w.tasks.values().filter(|t| t.landed.is_some()).count())
        .unwrap_or(0);
    let mut by_kind = serde_json::Map::new();
    for r in &counted {
        let n = by_kind.get(&r.kind).and_then(Value::as_u64).unwrap_or(0);
        by_kind.insert(r.kind.clone(), json!(n + 1));
    }
    let idle: u64 = d.projects.keys().map(|p| d.landing().idle_ms(*p)).sum();
    json!({
        "queue_idle_ms": idle,
        "interventions": counted.len(),
        "landed": landed,
        "per_landed": if landed == 0 { Value::Null } else { json!(counted.len() as f64 / landed as f64) },
        "by_kind": by_kind,
        "refused_answers": rows.iter().filter(|r| r.kind == "refused_answer").count(),
        "acknowledged": rows.iter().filter(|r| r.kind == "acknowledged").count(),
    })
}

// ---- open session ----------------------------------------------------------

/// What `mason task open` needs: the worktree, the agent's session if there is
/// one, and whether the step is idle (so resuming it cannot collide with a
/// running agent).
pub fn session_for(d: &Daemon, task: TaskId) -> Result<Value, String> {
    let w = d.world().map_err(|e| e.to_string())?;
    let t = w.tasks.get(&task).ok_or("no such task")?;
    let p = d.project(t.project).ok_or("no such project")?;
    let reg = mason_step_host::registry::Registry::new(d.cfg.hosts_dir());
    let mut hosts: Vec<_> = reg
        .scan()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|h| {
            crate::sweep::parse_host_id(&h.info.id)
                .filter(|(tk, _)| *tk == task.0)
                .filter(|_| {
                    !h.info.id.ends_with("-test")
                        && !h.info.id.ends_with("-cmd")
                        && !h.info.id.contains("-u-")
                        && !h.info.id.contains("-r-")
                        && !h.info.id.contains("-b-")
                })
                .map(|(_, r)| (r, h))
        })
        .collect();
    hosts.sort_by_key(|(r, _)| *r);
    let session = hosts.iter().rev().find_map(|(_, h)| {
        let raw = std::fs::read_to_string(h.info.dir.join("journal")).ok()?;
        raw.lines().rev().find_map(|l| {
            serde_json::from_str::<Value>(l)
                .ok()
                .and_then(|v| v["session_id"].as_str().map(String::from))
        })
    });
    let running = w.open_run(task).is_some_and(|r| d.has_job(r));
    Ok(json!({
        "worktree": p.worktree(&d.cfg, task.0).display().to_string(),
        "branch": p.branch(task.0),
        "session": session,
        "idle": !running,
        "claude": p.claude_bin.as_ref().map(|b| b.display().to_string()),
    }))
}
