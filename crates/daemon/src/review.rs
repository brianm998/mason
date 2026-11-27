// SPDX-License-Identifier: GPL-3.0-or-later

//! The pipeline review (Part 14, decision 1082): one recurring job for the whole pipeline
//! that asks an agent the owner's six questions and files what it finds as proposals.
//!
//! - The settings live in `<config>/review.toml`, read on every scheduler tick. A missing
//!   file means the defaults: on, every night at midnight, `opus` at `medium`.
//! - The review is one Low-priority task in MASON's own project. It covers every project
//!   whose `[introspect]` has `analyst` on. Its facts are written into its worktree
//!   (`review-facts/`) when its develop step starts.
//! - The schedule row `review` records the run: when it was filed, and (in `follow`, as
//!   JSON) the window of the running review and what the last completed one filed.
//!
//! Reading the agent's result file is `introspect::ingest_analyst`.

use crate::config::ProjectCfg;
use crate::daemon::{now_ms, Daemon};
use mason_core::ci_schedule::{parse_at, Daily};
use mason_core::*;
use mason_definition::tiers::{Entry, Launch};
use mason_store::ScheduleRow;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use toml_edit::{DocumentMut, Item};

/// The name of the schedule row, and what `mason schedule run` takes.
pub const ROW: &str = "review";
/// Where the facts are written, in the review task's worktree. Never committed.
pub const FACTS_DIR: &str = "review-facts";

const DAY_MS: u64 = 24 * 3_600_000;
const HOUR_MS: u64 = 3_600_000;
/// The window of a first review.
const FIRST_WINDOW_MS: u64 = 7 * DAY_MS;
/// How long the notice of a finished review stays among the alerts.
const NOTICE_MS: u64 = DAY_MS;
/// Retries within one hour that make a burst.
const BURST: u64 = 3;

/// One edit of `review.toml` at a time.
static EDITING: Mutex<()> = Mutex::new(());
/// One look at the schedule, or one filing, at a time: two must not file two reviews.
static FILING: Mutex<()> = Mutex::new(());

// ---- settings ----------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub enabled: bool,
    /// Local minutes after midnight.
    pub at_min: u32,
    pub every_days: u32,
    /// `None`: the built-in model of the provider (as for a diagnosis).
    pub model: Option<String>,
    pub effort: Option<String>,
    pub max_proposals: u32,
    pub changed_by: Option<String>,
    pub changed_at: Option<u64>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            enabled: true,
            at_min: 0,
            every_days: 1,
            model: None,
            effort: None,
            max_proposals: 10,
            changed_by: None,
            changed_at: None,
        }
    }
}

pub fn path(d: &Daemon) -> PathBuf {
    d.cfg.config_dir.join("review.toml")
}

fn at_text(min: u32) -> String {
    format!("{:02}:{:02}", min / 60, min % 60)
}

fn effort_known(e: &str) -> bool {
    use crate::config::Provider;
    [Provider::Claude, Provider::Codex]
        .into_iter()
        .any(|p| crate::tiermap::effort_levels(p).iter().any(|x| x == e))
}

/// `review.toml`. An unknown key or a bad value is an error.
pub fn parse(text: &str) -> Result<Settings, String> {
    let doc: DocumentMut = text.parse().map_err(|e| format!("review.toml: {e}"))?;
    let mut s = Settings::default();
    for (k, v) in doc.iter() {
        let bad = |what: &str| format!("review.toml: `{k}` must be {what}");
        let text = |what: &str| {
            v.as_str()
                .filter(|t| !t.trim().is_empty())
                .map(String::from)
                .ok_or_else(|| bad(what))
        };
        let whole = |lo: i64, hi: i64| {
            v.as_integer()
                .filter(|n| (lo..=hi).contains(n))
                .map(|n| n as u32)
                .ok_or_else(|| bad(&format!("a whole number from {lo} to {hi}")))
        };
        match k {
            "enabled" => s.enabled = v.as_bool().ok_or_else(|| bad("true or false"))?,
            "at" => {
                s.at_min = v
                    .as_str()
                    .and_then(parse_at)
                    .ok_or_else(|| bad("a time of day like \"00:00\""))?
            }
            "every_days" => s.every_days = whole(1, 30)?,
            "max_proposals" => s.max_proposals = whole(1, 100)?,
            "model" => s.model = Some(text("a model name")?),
            "effort" => {
                let e = text("an effort")?;
                if !effort_known(&e) {
                    return Err(bad("an effort a provider has, such as \"medium\""));
                }
                s.effort = Some(e);
            }
            "changed_by" => s.changed_by = v.as_str().map(String::from),
            "changed_at" => s.changed_at = v.as_integer().map(|n| n.max(0) as u64),
            other => return Err(format!("review.toml: unknown key `{other}`")),
        }
    }
    Ok(s)
}

/// What `review.toml` says now. A missing file is the defaults. A file that cannot be read
/// turns the review off, with the problem in the second part.
pub fn read(d: &Daemon) -> (Settings, Option<String>) {
    let off = |why: String| {
        let s = Settings {
            enabled: false,
            ..Settings::default()
        };
        (s, Some(why))
    };
    match std::fs::read_to_string(path(d)) {
        Ok(t) => match parse(&t) {
            Ok(s) => (s, None),
            Err(e) => off(e),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
        Err(e) => off(format!("review.toml: {e}")),
    }
}

/// Replace a value, keeping the comments around the old one.
fn put(doc: &mut DocumentMut, key: &str, new: toml_edit::Value) {
    if let Some(old) = doc.get_mut(key).and_then(Item::as_value_mut) {
        let mut new = new;
        *new.decor_mut() = old.decor().clone();
        *old = new;
        return;
    }
    doc.insert(key, Item::Value(new));
}

/// Change the settings. The file's comments stay; who changed it and when are written in it.
/// Nothing is written unless the whole change is good.
pub fn set(d: &Daemon, change: &Value, who: &str) -> Result<(), (u16, String)> {
    let bad = |m: String| (400u16, m);
    let change = change
        .as_object()
        .ok_or_else(|| bad("the settings must be an object".into()))?;
    let _one = EDITING.lock().unwrap_or_else(|e| e.into_inner());
    let file = path(d);
    let old = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err((500, format!("review.toml: {e}"))),
    };
    let mut doc: DocumentMut = old.parse().map_err(|e| bad(format!("review.toml: {e}")))?;
    let provider = crate::proposals::self_project(d)
        .map(|(_, p)| p.agent().provider)
        .unwrap_or(crate::config::Provider::Claude);
    let mut changed = false;
    for (k, v) in change {
        let wrong = |what: &str| bad(format!("`{k}` must be {what}"));
        let new: Option<toml_edit::Value> = match k.as_str() {
            "as" => continue,
            "enabled" => Some(v.as_bool().ok_or_else(|| wrong("true or false"))?.into()),
            "at" => {
                let min = v
                    .as_str()
                    .and_then(parse_at)
                    .ok_or_else(|| wrong("a time of day like \"00:00\""))?;
                Some(at_text(min).into())
            }
            "every_days" => Some(
                v.as_i64()
                    .filter(|n| (1..=30).contains(n))
                    .ok_or_else(|| wrong("a whole number from 1 to 30"))?
                    .into(),
            ),
            "max_proposals" => Some(
                v.as_i64()
                    .filter(|n| (1..=100).contains(n))
                    .ok_or_else(|| wrong("a whole number from 1 to 100"))?
                    .into(),
            ),
            // `null` goes back to the built-in.
            "model" | "effort" if v.is_null() => None,
            "model" => Some(
                v.as_str()
                    .filter(|t| !t.trim().is_empty())
                    .ok_or_else(|| wrong("a model name"))?
                    .into(),
            ),
            "effort" => {
                let efforts = crate::tiermap::effort_levels(provider);
                let e = v
                    .as_str()
                    .filter(|e| efforts.iter().any(|x| x == e))
                    .ok_or_else(|| wrong(&format!("one of {}", efforts.join(", "))))?;
                Some(e.into())
            }
            other => return Err(bad(format!("unknown setting `{other}`"))),
        };
        match new {
            Some(new) => put(&mut doc, k, new),
            None => {
                doc.remove(k);
            }
        }
        changed = true;
    }
    if !changed {
        return Ok(());
    }
    put(&mut doc, "changed_by", who.into());
    put(
        &mut doc,
        "changed_at",
        i64::try_from(now_ms()).unwrap_or(i64::MAX).into(),
    );
    let text = doc.to_string();
    parse(&text).map_err(bad)?;
    let tmp = file.with_extension("toml.tmp");
    std::fs::create_dir_all(&d.cfg.config_dir)
        .and_then(|()| std::fs::write(&tmp, &text))
        .and_then(|()| std::fs::rename(&tmp, &file))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            (500, format!("{}: {e}", file.display()))
        })?;
    d.poke();
    Ok(())
}

/// The model and effort the review session runs on: what the settings name, over the
/// provider's built-in diagnosis pair (`opus` at `medium` for Claude).
pub fn model_and_effort(d: &Daemon, s: &Settings) -> Entry {
    use mason_definition::tiers::{builtin_diagnose, resolve_diagnose};
    let own = crate::proposals::self_project(d);
    let provider = own.map_or(crate::config::Provider::Claude, |(_, p)| p.agent().provider);
    let text = own.map_or("", |(_, p)| p.overlay_text.as_str());
    let eff = crate::tiermap::effective(&d.cfg.config_dir, provider, text);
    resolve_diagnose(
        builtin_diagnose(crate::tiermap::name(provider), &eff.mapping),
        None,
        Entry {
            model: s.model.clone(),
            effort: s.effort.clone(),
        },
    )
    .0
}

/// Make the effort the review task carries the one its session gets. As an override, `low`,
/// `medium` and `high` are abstract levels, which pick a row of the tier's mapping and so may
/// launch another effort or none; the review's settings name the real one.
pub fn pin_effort(d: &Daemon, task: TaskId, step: &str, launched: &mut Launch) {
    let key = format!("step.{step}.effort");
    let carried = d
        .store
        .task_overrides(task.0)
        .unwrap_or_default()
        .into_iter()
        .find(|(k, _)| *k == key);
    if let Some((_, effort)) = carried {
        launched.effort = Some(effort);
        launched.effort_from = "the review's settings".into();
        launched.overridden = true;
    }
}

// ---- the record --------------------------------------------------------------------------

/// The projects a review covers: those whose `[introspect]` has `enabled` and `analyst` on.
pub fn included(d: &Daemon) -> BTreeSet<String> {
    d.projects()
        .values()
        .filter(|p| {
            let c = p.introspect();
            c.enabled && c.analyst
        })
        .map(|p| p.id.clone())
        .collect()
}

/// The JSON the row keeps: `seen` (when the review was first looked at), `current` (the
/// review that was filed and has not completed) and `last` (the last completed one).
pub(crate) fn record(row: &ScheduleRow) -> Value {
    serde_json::from_str(&row.follow)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

/// The row, made if there is none.
fn ensure_row(d: &Daemon, project: &str, s: &Settings, now: u64) -> ScheduleRow {
    let every_ms = u64::from(s.every_days) * DAY_MS;
    let found = d.store.schedule(ROW).ok().flatten();
    if let Some(r) = &found {
        if r.every_ms == every_ms && r.project == project {
            return r.clone();
        }
    }
    let mut row = found.unwrap_or_else(|| ScheduleRow {
        name: ROW.into(),
        kind: ROW.into(),
        follow: json!({ "seen": now }).to_string(),
        ..Default::default()
    });
    row.project = project.into();
    row.every_ms = every_ms;
    let _ = d.store.put_schedule(&row);
    row
}

fn is_open(d: &Daemon, task: u64) -> bool {
    d.world()
        .ok()
        .and_then(|w| w.tasks.get(&TaskId(task)).map(|t| t.state))
        .is_some_and(|s| !matches!(s, State::Landed | State::Canceled | State::Failed))
}

fn daily(s: &Settings) -> Daily {
    Daily {
        at_min: s.at_min,
        every_days: s.every_days,
    }
}

/// When the last completed review started, or when the review was first looked at.
fn base_of(rec: &Value, now: u64) -> u64 {
    rec["last"]["started"]
        .as_u64()
        .or_else(|| rec["seen"].as_u64())
        .unwrap_or(now)
}

/// When the next review is due (`now` if it is due already); `None` when it is off.
pub fn next_run(d: &Daemon, now: u64) -> Option<u64> {
    let (s, _) = read(d);
    if !s.enabled || crate::proposals::self_project(d).is_none() {
        return None;
    }
    let row = d.store.schedule(ROW).ok().flatten().unwrap_or_default();
    Some(daily(&s).next(
        row.last_run,
        base_of(&record(&row), now),
        now,
        crate::ci_schedule::local_offset_min(),
    ))
}

// ---- the tick ----------------------------------------------------------------------------

/// One look at whether a review is due.
pub fn tick(d: &Arc<Daemon>, now: u64) {
    tick_at(d, now, crate::ci_schedule::local_offset_min());
}

/// [`tick`] at a given local offset from UTC in minutes. Returns the task filed, if any.
/// A review that is due but cannot start (the account is held, the previous one is open) is
/// looked at again on the next tick.
pub fn tick_at(d: &Arc<Daemon>, now: u64, tz_min: i32) -> Option<TaskId> {
    let _one = FILING.lock().unwrap_or_else(|e| e.into_inner());
    let (_, own) = crate::proposals::self_project(d)?;
    drop_old_rows(d);
    let (s, _) = read(d);
    let row = ensure_row(d, &own.id, &s, now);
    finish_filing(d, &row);
    if !s.enabled || !daily(&s).due(row.last_run, base_of(&record(&row), now), now, tz_min) {
        return None;
    }
    if let Some(why) = held(d, own, now) {
        say(d, &format!("waiting: {why}"));
        return None;
    }
    match file_locked(d, now) {
        Ok(t) => Some(t),
        Err(why) => {
            say(d, &format!("waiting: {why}"));
            None
        }
    }
}

/// The rows of the per-project analyst this replaced.
fn drop_old_rows(d: &Daemon) {
    for r in d.store.schedules().unwrap_or_default() {
        if r.name.starts_with("analyst:") {
            let _ = d.store.delete_schedule(&r.name);
        }
    }
}

fn held(d: &Daemon, own: &ProjectCfg, now: u64) -> Option<String> {
    crate::holds::hold_of(d, &own.account)
        .filter(|h| h.until_ms.is_none_or(|u| u > now))
        .map(|h| {
            format!(
                "account {} is held: {}",
                h.account,
                crate::holds::describe(&h, now)
            )
        })
}

fn say(d: &Daemon, status: &str) {
    if let Ok(Some(mut row)) = d.store.schedule(ROW) {
        if row.last_status != status {
            row.last_status = status.into();
            let _ = d.store.put_schedule(&row);
            d.poke();
        }
    }
}

/// A daemon that stopped between filing the task and letting it start lets it start now.
fn finish_filing(d: &Arc<Daemon>, row: &ScheduleRow) {
    let mut rec = record(row);
    if rec["current"]["approving"] != json!(true) {
        return;
    }
    let Some(task) = rec["current"]["task"].as_u64() else {
        return;
    };
    let proposed = d
        .world()
        .ok()
        .and_then(|w| w.tasks.get(&TaskId(task)).map(|t| t.state))
        == Some(State::Proposed);
    if proposed && d.emit(EventKind::Approved { task: TaskId(task) }).is_err() {
        return;
    }
    rec["current"]["approving"] = json!(false);
    let mut row = row.clone();
    row.follow = rec.to_string();
    let _ = d.store.put_schedule(&row);
}

/// File a review now, whatever the schedule says. Refused while the previous one is open.
pub fn file(d: &Arc<Daemon>, now: u64) -> Result<TaskId, String> {
    let _one = FILING.lock().unwrap_or_else(|e| e.into_inner());
    file_locked(d, now)
}

fn file_locked(d: &Arc<Daemon>, now: u64) -> Result<TaskId, String> {
    let (pid, own) = crate::proposals::self_project(d).ok_or(
        "MASON's own project is not registered (a project file with [self]); the review is filed there",
    )?;
    let (s, _) = read(d);
    let mut row = ensure_row(d, &own.id, &s, now);
    if let Some(prev) = row.last_task.filter(|t| is_open(d, *t)) {
        return Err(format!("the last review task, #{prev}, is still open"));
    }
    let mut rec = record(&row);
    let since = rec["last"]["started"]
        .as_u64()
        .unwrap_or_else(|| now.saturating_sub(FIRST_WINDOW_MS));
    let body = crate::introspect::review_body(d, since, now, s.max_proposals);
    // Filed as proposed and approved below: the model and effort, and the row that makes it
    // the review task, are in place before anything can claim it.
    let appended = d
        .emit(EventKind::TaskFiled {
            project: pid,
            title: crate::introspect::ANALYST_TITLE.into(),
            body,
            priority: Priority::Low,
            trust: TrustSource::OwnerAgent,
            explored: true,
            proposed: true,
            blockers: vec![],
            not_before: None,
        })
        .map_err(|e| e.to_string())?;
    let task = appended
        .effects
        .iter()
        .find_map(|e| match e {
            Effect::TaskCreated { task } => Some(*task),
            _ => None,
        })
        .ok_or("the task was not created")?;
    let entry = model_and_effort(d, &s);
    if let Some(step) = d.current_def(pid).first_in(State::Developing) {
        for (attr, value) in [("model", &entry.model), ("effort", &entry.effort)] {
            if let Some(v) = value {
                d.store
                    .set_task_override(task.0, &format!("step.{}.{attr}", step.name), v, ROW, now)
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    row.last_task = Some(task.0);
    row.last_run = now;
    row.runs += 1;
    row.last_status = format!("review task #{} filed", task.0);
    rec["current"] = json!({"task": task.0, "started": now, "since": since, "approving": true});
    row.follow = rec.to_string();
    d.store.put_schedule(&row).map_err(|e| e.to_string())?;
    finish_filing(d, &row);
    d.poke();
    Ok(task)
}

// ---- the result, the notice and the status -----------------------------------------------

/// Keep what a completed review came to. `last` gets its `task`, `started` and `completed`.
pub(crate) fn complete(d: &Daemon, task: TaskId, mut last: Value, summary: &str) {
    let Ok(Some(mut row)) = d.store.schedule(ROW) else {
        return;
    };
    let mut rec = record(&row);
    let now = now_ms();
    let started = rec["current"]["started"].as_u64().unwrap_or(row.last_run);
    last["task"] = json!(task.0);
    last["started"] = json!(started);
    last["completed"] = json!(now);
    last["summary"] = json!(summary);
    rec["last"] = last;
    if let Some(o) = rec.as_object_mut() {
        o.remove("current");
    }
    row.follow = rec.to_string();
    row.last_status = summary.into();
    let _ = d.store.put_schedule(&row);
}

/// The one line about a completed review: "Pipeline review: 4 proposed, 3 duplicates dropped".
fn notice_line(last: &Value) -> String {
    let n = |k: &str| last[k].as_array().map_or(0, Vec::len);
    let dups = n("duplicates");
    let mut line = format!(
        "Pipeline review: {} proposed, {dups} duplicate{} dropped",
        n("filed"),
        if dups == 1 { "" } else { "s" }
    );
    if n("capped") > 0 {
        line.push_str(&format!(", {} over the cap", n("capped")));
    }
    line
}

/// The notice of the last completed review, as a live alert for a day.
pub(crate) fn alert(d: &Daemon, now: u64) -> Option<(String, String, String)> {
    let row = d.store.schedule(ROW).ok().flatten()?;
    let rec = record(&row);
    let last = &rec["last"];
    let completed = last["completed"].as_u64()?;
    (now.saturating_sub(completed) < NOTICE_MS).then(|| {
        (
            format!("review:{}", last["task"].as_u64().unwrap_or(0)),
            ROW.to_string(),
            notice_line(last),
        )
    })
}

/// `GET /v1/review`.
pub fn status(d: &Daemon) -> Value {
    let now = now_ms();
    let (s, problem) = read(d);
    let entry = model_and_effort(d, &s);
    let row = d.store.schedule(ROW).ok().flatten().unwrap_or_default();
    let rec = record(&row);
    let running = row.last_task.filter(|t| is_open(d, *t));
    let next = next_run(d, now);
    json!({
        "settings": {
            "enabled": s.enabled, "at": at_text(s.at_min), "every_days": s.every_days,
            "model": entry.model, "effort": entry.effort, "max_proposals": s.max_proposals,
            "changed_by": s.changed_by, "changed_at": s.changed_at,
        },
        "problem": problem,
        "project": crate::proposals::self_project(d).map(|(_, p)| p.id.clone()),
        "includes": included(d),
        "running": running.is_some(),
        "running_task": running,
        "next_run": next,
        "next_run_text": next.map(|n| {
            if n <= now { "due now".to_string() } else { crate::ci_schedule::when_text(n, now) }
        }),
        "last": rec.get("last").map(|l| {
            let mut l = l.clone();
            if let Some(at) = l["completed"].as_u64() {
                l["completed_text"] = json!(crate::ci_schedule::when_text(at, now));
            }
            l
        }),
        "last_status": row.last_status,
    })
}

// ---- the facts ---------------------------------------------------------------------------

/// What the event log says of the window, beyond the digest.
#[derive(Default)]
struct Walk {
    failed: Vec<Value>,
    retries: Vec<Value>,
    runs: Vec<(String, u64, Value)>,
    questions: Vec<Value>,
    answers: Vec<Value>,
}

fn walk(d: &Daemon, from: u64, to: u64, mine: &dyn Fn(&str) -> bool) -> Walk {
    let mut out = Walk::default();
    let mut w = mason_engine::World::new();
    let mut after = 0u64;
    loop {
        let Ok(batch) = d.store.read_since(after, 5000) else {
            break;
        };
        if batch.is_empty() {
            break;
        }
        for se in batch {
            after = se.seq;
            let at = se.event.at.0;
            if at >= from && at < to {
                note(&mut out, &w, &se.event.kind, at, mine);
            }
            if let Ok((next, _)) = mason_engine::reduce(&w, &se.event) {
                w = next;
            }
        }
    }
    out
}

fn note(
    out: &mut Walk,
    w: &mason_engine::World,
    kind: &EventKind,
    at: u64,
    mine: &dyn Fn(&str) -> bool,
) {
    let of = |t: &TaskId| {
        let task = w.tasks.get(t)?;
        let project = w.projects.get(&task.project)?.name.clone();
        mine(&project).then_some((task, project))
    };
    match kind {
        EventKind::StepFailed {
            task,
            reason,
            class,
        } => {
            if let Some((t, project)) = of(task) {
                out.failed.push(json!({
                    "task": task.0, "project": project, "title": t.title, "at": at,
                    "step": t.step, "reason": reason, "class": class.map(|c| format!("{c:?}")),
                }));
            }
        }
        EventKind::Retried {
            task, auto, step, ..
        } => {
            if let Some((_, project)) = of(task) {
                out.retries.push(json!({
                    "task": task.0, "project": project, "at": at, "auto": auto, "step": step,
                }));
            }
        }
        EventKind::QuestionAsked { task, question } => {
            if let Some((_, project)) = of(task) {
                out.questions.push(
                    json!({"task": task.0, "project": project, "at": at, "question": question}),
                );
            }
        }
        EventKind::Answered { task, answer, .. } => {
            if let Some((_, project)) = of(task) {
                out.answers
                    .push(json!({"task": task.0, "project": project, "at": at, "answer": answer}));
            }
        }
        _ => {}
    }
    let (task, outcome) = match kind {
        EventKind::StepSucceeded { task, .. } => (task, "green"),
        EventKind::SentBack { task, .. } => (task, "red"),
        EventKind::StepFailed { task, .. } => (task, "failed"),
        _ => return,
    };
    let Some((t, project)) = of(task) else { return };
    if let Some(run) = w.open_run(*task).and_then(|id| w.step_runs.get(&id)) {
        let ms = at.saturating_sub(run.started_at.0);
        let state = mason_store::state_name(t.state).to_string();
        out.runs.push((
            state.clone(),
            ms,
            json!({
                "task": task.0, "project": project, "state": state, "step": t.step,
                "ms": ms, "outcome": outcome, "ended_at": at,
            }),
        ));
    }
}

/// Add lines to the repository's `info/exclude`, so the facts and the result never show as
/// work to commit.
fn exclude(wt: &Path) -> Result<(), String> {
    let rel = mason_landing::git::run(wt, &["rev-parse", "--git-path", "info/exclude"])
        .map_err(|e| e.to_string())?;
    let file = wt.join(rel.trim());
    let mut text = std::fs::read_to_string(&file).unwrap_or_default();
    for line in [
        format!("/{FACTS_DIR}/"),
        format!("/{}", crate::introspect::ANALYST_FILE),
    ] {
        if !text.lines().any(|l| l.trim() == line) {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&line);
            text.push('\n');
        }
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&file, text).map_err(|e| format!("{}: {e}", file.display()))
}

const FACTS_README: &str = "Facts for the pipeline review, computed from recorded data only.

- digest-7d.json: the digest of the last 7 days, per project (trends).
- digest-window.json: the same since the last completed review.
- failed-tasks.json: step failures with their reasons and classes.
- diagnoses.json: diagnosis sessions, their verdicts and what became of them.
- retries.json: every retry (auto or the owner's) and the hours with a burst of them.
- interventions.json: what a person did (overrides, skips), and the debts.
- health.json: the health log (findings, health sessions), alerts and the stall watch.
- durations.json: step run times, and slot waits by kind (compile, test-suite, gha).
- tests.json: tests run per successful test step, what each task's last test step selected,
  narrowed or left out, and the suites run whole (full tests).
- batches.json: landing batches, their phases, rounds (a round above 0 is a rerun or bisect).
- daemon.json: daemon starts and takeovers, job panics and other recorded bugs.
- ci-watch.json: the base-branch CI watch of each project.
- owner.json: sessions opened by hand, questions and the owner's answers.
- tasks.json: every task (id, project, state, title, the first 400 characters of its body).
  Search it before proposing anything.
";

/// Write the facts of the review `task` into its worktree.
pub fn write_facts(d: &Daemon, task: TaskId, wt: &Path) -> Result<(), String> {
    let now = now_ms();
    let row = d.store.schedule(ROW).ok().flatten().unwrap_or_default();
    let rec = record(&row);
    let since = rec["current"]["since"]
        .as_u64()
        .filter(|_| rec["current"]["task"].as_u64() == Some(task.0))
        .unwrap_or_else(|| now.saturating_sub(FIRST_WINDOW_MS));
    let projects = included(d);
    let mine = |p: &str| projects.contains(p);
    let dir = wt.join(FACTS_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    exclude(wt)?;
    let write = |name: &str, v: &Value| {
        let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(name), text).map_err(|e| format!("{name}: {e}"))
    };
    std::fs::write(
        dir.join("README.md"),
        format!(
            "{FACTS_README}\nThe window is from {since} to {now} (milliseconds since 1970). Projects covered: {}.\n",
            projects.iter().cloned().collect::<Vec<_>>().join(", ")
        ),
    )
    .map_err(|e| e.to_string())?;

    let week = now.saturating_sub(7 * DAY_MS);
    let digests = |from: u64| {
        let rep = crate::introspect::replay(d, from, now);
        let mut m = Map::new();
        for p in &projects {
            m.insert(
                p.clone(),
                crate::introspect::digest_from(d, &rep, Some(p), from, now),
            );
        }
        Value::Object(m)
    };
    write("digest-7d.json", &digests(week))?;
    let window = digests(since);
    write("digest-window.json", &window)?;

    let w = d.world().map_err(|e| e.to_string())?;
    let project_of = |t: u64| {
        w.tasks
            .get(&TaskId(t))
            .and_then(|t| w.projects.get(&t.project))
            .map(|p| p.name.clone())
            .unwrap_or_default()
    };
    // A row that names no project of its own (the daemon's) is always included.
    let about = |p: &str| p.is_empty() || mine(p);
    let log = walk(d, since, now, &mine);

    write("failed-tasks.json", &json!(log.failed))?;
    let diagnoses: Vec<Value> = d
        .store
        .diagnoses(None)
        .unwrap_or_default()
        .iter()
        .filter(|r| r.at_ms >= since && about(&project_of(r.task)))
        .map(|r| {
            json!({"task": r.task, "project": project_of(r.task), "at": r.at_ms,
                   "verdict": r.verdict, "status": r.status, "decided_at": r.decided_ms,
                   "submitted": serde_json::from_str::<Value>(&r.args).unwrap_or(Value::Null)})
        })
        .collect();
    write("diagnoses.json", &json!(diagnoses))?;

    let mut hours: BTreeMap<u64, (u64, u64)> = BTreeMap::new();
    for r in &log.retries {
        let e = hours
            .entry(r["at"].as_u64().unwrap_or(0) / HOUR_MS * HOUR_MS)
            .or_default();
        if r["auto"] == json!(true) {
            e.0 += 1;
        } else {
            e.1 += 1;
        }
    }
    let bursts: Vec<Value> = hours
        .iter()
        .filter(|(_, (a, m))| a + m >= BURST)
        .map(|(h, (a, m))| json!({"hour_from": h, "auto": a, "owner": m}))
        .collect();
    write(
        "retries.json",
        &json!({"retries": log.retries, "bursts": bursts}),
    )?;

    let interventions: Vec<Value> = d
        .store
        .interventions(since)
        .unwrap_or_default()
        .iter()
        .filter(|r| about(&r.project))
        .map(|r| {
            json!({"at": r.at, "kind": r.kind, "task": r.task, "project": r.project,
                   "who": r.who, "detail": r.detail, "reason": r.reason})
        })
        .collect();
    let debts: Vec<Value> = d
        .store
        .debts(None, false)
        .unwrap_or_default()
        .iter()
        .filter(|x| about(&x.project) && (x.at >= since || x.status == "open"))
        .map(|x| {
            json!({"id": x.id, "at": x.at, "task": x.task, "project": x.project,
                   "scope": x.scope, "reason": x.reason, "status": x.status, "how": x.how})
        })
        .collect();
    write(
        "interventions.json",
        &json!({"interventions": interventions, "debts": debts}),
    )?;

    let health: Vec<Value> = d
        .store
        .health_since(&[], since)
        .unwrap_or_default()
        .iter()
        .map(|h| {
            json!({"at": h.at_ms, "kind": h.kind, "key": h.key, "session": h.session,
                   "detail": h.detail})
        })
        .collect();
    let alerts: Vec<Value> = d
        .store
        .alerts(false)
        .unwrap_or_default()
        .iter()
        .filter(|a| a.cleared_at.is_none_or(|c| c >= since))
        .map(|a| {
            json!({"key": a.key, "kind": a.kind, "text": a.text, "since": a.since,
                   "cleared_at": a.cleared_at})
        })
        .collect();
    write(
        "health.json",
        &json!({
            "log": health,
            "alerts": alerts,
            "stall_watch_since_start": {
                "sigconts": d.stall_conts.load(std::sync::atomic::Ordering::SeqCst),
                "kills": d.stall_kills.load(std::sync::atomic::Ordering::SeqCst),
            },
        }),
    )?;

    let mut by_state: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
    for (state, ms, _) in &log.runs {
        by_state.entry(state).or_default().push(*ms);
    }
    let by_state: Map<String, Value> = by_state
        .into_iter()
        .map(|(s, v)| {
            let total: u64 = v.iter().sum();
            (
                s.to_string(),
                json!({"runs": v.len(), "total_ms": total,
                       "median_ms": mason_core::introspect::median(v)}),
            )
        })
        .collect();
    let slot_waits: Map<String, Value> = projects
        .iter()
        .map(|p| (p.clone(), window[p]["wait_by_class"].clone()))
        .collect();
    write(
        "durations.json",
        &json!({
            "by_state": by_state,
            "slot_waits": slot_waits,
            "step_runs": log.runs.iter().map(|r| r.2.clone()).collect::<Vec<_>>(),
        }),
    )?;

    let steps = d.store.step_history_between(since, now).unwrap_or_default();
    let test_runs: Vec<Value> = steps
        .iter()
        .filter(|h| about(&h.project) && h.done > 0)
        .map(|h| {
            json!({"task": h.task, "project": h.project, "step": h.step,
                   "tests_run": h.done, "ended_at": h.at})
        })
        .collect();
    let tested: BTreeSet<u64> = steps
        .iter()
        .filter(|h| h.done > 0)
        .map(|h| h.task)
        .collect();
    let notes: Map<String, Value> = tested
        .iter()
        .filter_map(|t| {
            let text =
                std::fs::read_to_string(d.task_dir(TaskId(*t)).join("test-notes.txt")).ok()?;
            Some((t.to_string(), json!(text)))
        })
        .collect();
    let mut full = vec![];
    for p in d.projects().values().filter(|p| mine(&p.id)) {
        for spec in p.schedules() {
            for r in d
                .store
                .full_tests(&p.id, &spec.name)
                .unwrap_or_default()
                .iter()
                .filter(|r| r.started_ms >= since)
            {
                full.push(json!({
                    "project": p.id, "schedule": spec.name, "commit": r.commit,
                    "trigger": r.trigger, "started_at": r.started_ms,
                    "finished_at": r.finished_ms, "verdict": r.verdict, "link": r.link,
                }));
            }
        }
    }
    write(
        "tests.json",
        &json!({
            "test_step_runs": test_runs,
            "last_test_step_notes_by_task": notes,
            "whole_suite_runs": full,
        }),
    )?;

    let mut batches = vec![];
    for p in &projects {
        let phases = d.store.batch_phases(p, 300).unwrap_or_default();
        for b in d
            .store
            .batches(Some(p), 300)
            .unwrap_or_default()
            .iter()
            .filter(|b| b.finished_at.unwrap_or(b.at) >= since)
        {
            let seen: Vec<Value> = phases
                .iter()
                .filter(|x| x.batch == b.id)
                .map(|x| json!({"phase": x.phase, "at": x.at}))
                .collect();
            batches.push(json!({
                "id": b.id, "project": p, "at": b.at, "finished_at": b.finished_at,
                "phase": b.phase, "tasks": b.tasks, "landed": b.landed, "parked": b.parked,
                "note": b.note, "round": b.round, "phases": seen,
            }));
        }
    }
    write("batches.json", &json!(batches))?;

    let rt = crate::update::Runtimes::new(&d.cfg.data_dir);
    let boots: Vec<Value> = crate::update::Boots::new(&rt)
        .all()
        .iter()
        .filter(|b| b.at_ms >= since)
        .map(|b| json!({"at": b.at_ms, "build": b.sha, "event": b.event}))
        .collect();
    let bugs: Vec<Value> = d
        .store
        .bugs_since(since)
        .unwrap_or_default()
        .into_iter()
        .map(|(at, kind, detail)| json!({"at": at, "kind": kind, "detail": detail}))
        .collect();
    write("daemon.json", &json!({"starts": boots, "bugs": bugs}))?;

    let ci: Map<String, Value> = projects
        .iter()
        .filter_map(|p| {
            let t = d.store.ci_watch_state(p).ok().flatten()?;
            let v = serde_json::from_str(&t).unwrap_or(Value::String(t));
            Some((p.clone(), v))
        })
        .collect();
    write("ci-watch.json", &Value::Object(ci))?;

    let sessions: Vec<&Value> = interventions
        .iter()
        .filter(|r| r["kind"] == json!("open_session"))
        .collect();
    write(
        "owner.json",
        &json!({
            "sessions_opened_by_hand": sessions,
            "questions": log.questions,
            "answers": log.answers,
        }),
    )?;

    let index: Vec<Value> = {
        let mut all: Vec<&Task> = w.tasks.values().collect();
        all.sort_by_key(|t| t.id.0);
        all.iter()
            .map(|t| {
                json!({
                    "id": t.id.0,
                    "project": w.projects.get(&t.project).map(|p| p.name.clone()),
                    "state": mason_store::state_name(t.state),
                    "title": t.title,
                    "body": t.body.chars().take(400).collect::<String>(),
                })
            })
            .collect()
    };
    write("tasks.json", &json!(index))
}

/// What a review step's submission must show: the head it names is the tip, and the result
/// file is there and complete. Nothing is committed, so the usual change checks do not apply
/// (the branch of a review task is never landed).
pub fn validator(wt: PathBuf) -> mason_adapter_api::contract::Validator {
    Box::new(move |tool, args| {
        if tool != "submit_change" {
            return Ok(());
        }
        let head = args["head"].as_str().unwrap_or_default();
        let tip = mason_landing::git::rev_parse(&wt, "HEAD").unwrap_or_default();
        let mut problems = vec![];
        if mason_landing::git::rev_parse(&wt, head).ok().as_deref() != Some(tip.as_str()) {
            problems.push(format!(
                "head: submit the hash from `git rev-parse HEAD` ({}); commit nothing",
                tip.chars().take(12).collect::<String>()
            ));
        }
        match std::fs::read_to_string(wt.join(crate::introspect::ANALYST_FILE)) {
            Ok(t) => {
                if let Err(e) = crate::introspect::parse_result(&t) {
                    problems.push(format!("{}: {e}", crate::introspect::ANALYST_FILE));
                }
            }
            Err(_) => problems.push(format!(
                "there is no {} in the root of your worktree; write it as the task says",
                crate::introspect::ANALYST_FILE
            )),
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_settings_file_is_read_whole_or_not_at_all() {
        assert_eq!(parse("").unwrap(), Settings::default());
        let s = parse(
            "# mine\nenabled = false\nat = \"3:05\"\nevery_days = 7\nmodel = \"sonnet\"\neffort = \"high\"\nmax_proposals = 3\n",
        )
        .unwrap();
        assert_eq!(
            (s.enabled, s.at_min, s.every_days, s.max_proposals),
            (false, 185, 7, 3)
        );
        assert_eq!(s.model.as_deref(), Some("sonnet"));
        for bad in [
            "at = \"25:00\"",
            "at = 3",
            "every_days = 0",
            "every_days = 31",
            "max_proposals = 0",
            "effort = \"ultra\"",
            "model = \"\"",
            "enabled = \"yes\"",
            "when = \"night\"",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_notice_counts_what_was_filed_and_dropped() {
        let last = json!({"filed": [1, 2, 3, 4], "duplicates": [{}, {}, {}], "capped": []});
        assert_eq!(
            notice_line(&last),
            "Pipeline review: 4 proposed, 3 duplicates dropped"
        );
        let last = json!({"filed": [], "duplicates": [{}], "capped": ["x"]});
        assert_eq!(
            notice_line(&last),
            "Pipeline review: 0 proposed, 1 duplicate dropped, 1 over the cap"
        );
    }
}
