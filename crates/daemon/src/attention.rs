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
    let held = d.broker.disk_held();
    if held > 0 {
        text.push_str(&format!(
            "; {held} task{} waiting",
            if held == 1 { "" } else { "s" }
        ));
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

/// How long a launch may wait before it is called frozen. `MASON_LAUNCH_FROZEN_MS` overrides it.
const LAUNCH_FROZEN_MS: u64 = 2 * 60 * 1000;

/// The task a host id belongs to: ids start `t<task>r<run>`.
fn launch_task(id: &str) -> Option<u64> {
    id.strip_prefix('t')?.split('r').next()?.parse().ok()
}

/// Whether `syspolicyd` is running, asked of `/usr/bin/pgrep` (Apple's tools do not freeze with
/// it) for at most 2 seconds. None off macOS or when the answer did not come.
fn syspolicyd_running() -> Option<bool> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let mut c = std::process::Command::new("/usr/bin/pgrep")
        .args(["-x", "syspolicyd"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let end = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match c.try_wait() {
            Ok(Some(st)) => return Some(st.success()),
            Ok(None) if std::time::Instant::now() < end => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            _ => {
                let _ = c.kill();
                let _ = c.wait();
                return None;
            }
        }
    }
}

/// The alert for step hosts whose launch has waited longer than `threshold_ms` at `now_ms`:
/// how many, the longest and which tasks. `syspolicyd` is the injected answer to "is it running"
/// (None: not known). None when no launch is that old.
pub fn launch_frozen_alert(
    list: &[mason_step_host::launcher::Launching],
    now_ms: u64,
    threshold_ms: u64,
    syspolicyd: impl FnOnce() -> Option<bool>,
) -> Option<(String, String, String)> {
    let old: Vec<_> = list
        .iter()
        .filter(|l| now_ms.saturating_sub(l.since_ms) > threshold_ms)
        .collect();
    if old.is_empty() {
        return None;
    }
    let longest = old
        .iter()
        .map(|l| now_ms.saturating_sub(l.since_ms))
        .max()
        .unwrap_or(0)
        / 1000;
    let mut tasks: Vec<u64> = old.iter().filter_map(|l| launch_task(&l.id)).collect();
    tasks.sort();
    tasks.dedup();
    let which = if tasks.is_empty() {
        String::new()
    } else {
        format!(
            " (tasks {})",
            tasks
                .iter()
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let mut text = format!(
        "{} step host launch(es) frozen{which}: the longest has waited {longest}s without the host coming up",
        old.len()
    );
    match syspolicyd() {
        Some(true) => text.push_str("; syspolicyd is running"),
        Some(false) => text.push_str("; syspolicyd is NOT running"),
        None => {}
    }
    Some(("launch-frozen".into(), "environment".into(), text))
}

/// Bug kinds that alert while one was filed in the last `BUG_WINDOW_MS`.
const ALERT_BUGS: &[&str] = &["thread_panic", "job_panic", "job_spawn", "projection_drift"];
/// Repairs that alert only when the same task needs them again and again.
const REPEAT_BUGS: &[&str] = &["missed_job", "missed_admit"];
pub const BUG_WINDOW_MS: u64 = 24 * 3600 * 1000;
pub const REPEAT_WINDOW_MS: u64 = 3600 * 1000;
pub const REPEAT_COUNT: usize = 3;
/// An event-loop stall this long (ms) alerts, for `STALL_WINDOW_MS`.
pub const STALL_ALERT_MS: u64 = 30_000;
pub const STALL_WINDOW_MS: u64 = 3600 * 1000;

/// The `daemon_bug` alerts for the bug rows `(at_ms, kind, detail)` at time `now`. Pure.
/// `orphan_host` and `leftover_branch` are housekeeping the sweep fixed: no alert.
pub fn bug_alerts(rows: &[(u64, String, String)], now: u64) -> Vec<(String, String, String)> {
    let mut out = vec![];
    for kind in ALERT_BUGS {
        let mine: Vec<_> = rows
            .iter()
            .filter(|(at, k, _)| k == kind && *at + BUG_WINDOW_MS > now && *at <= now)
            .collect();
        if let Some((_, _, detail)) = mine.iter().max_by_key(|(at, _, _)| *at) {
            out.push((
                format!("daemon_bug:{kind}"),
                "daemon_bug".into(),
                format!(
                    "The daemon filed {} {kind} bug(s) in the last 24 hours; latest: {detail}",
                    mine.len()
                ),
            ));
        }
    }
    for kind in REPEAT_BUGS {
        // The task is the number after "task " in the detail.
        let mut by_task: std::collections::BTreeMap<String, Vec<&(u64, String, String)>> =
            Default::default();
        for r in rows
            .iter()
            .filter(|(at, k, _)| k == kind && *at + REPEAT_WINDOW_MS > now && *at <= now)
        {
            let task =
                r.2.strip_prefix("task ")
                    .and_then(|t| t.split_whitespace().next())
                    .unwrap_or("?")
                    .to_string();
            by_task.entry(task).or_default().push(r);
        }
        for (task, hits) in by_task {
            if hits.len() >= REPEAT_COUNT {
                let latest = hits.iter().max_by_key(|r| r.0).map_or("", |r| r.2.as_str());
                out.push((
                    format!("daemon_bug:{kind}:{task}"),
                    "daemon_bug".into(),
                    format!(
                        "The sweep has had to repair task {task} {} times in the last hour ({kind}); the repair is not holding. Latest: {latest}",
                        hits.len()
                    ),
                ));
            }
        }
    }
    out
}

/// The alert for an event-loop stall of `late_ms` that ended at `at_ms`, while it is recent.
pub fn stall_alert(last: Option<(u64, u64)>, now: u64) -> Option<(String, String, String)> {
    let (at, late_ms) = last?;
    (late_ms >= STALL_ALERT_MS && at + STALL_WINDOW_MS > now).then(|| {
        (
            "daemon_bug:stall".into(),
            "daemon_bug".into(),
            format!(
                "The daemon's event loop stalled for {} seconds",
                late_ms / 1000
            ),
        )
    })
}

/// An agent that has been silent past its alert limit, one per running step. Exists
/// until output resumes, the step is stopped for it, or the step ends.
fn silent_alerts(d: &Daemon) -> Vec<(String, String, String)> {
    let silent = d.silent.lock().unwrap_or_else(|e| e.into_inner());
    let mut v: Vec<_> = silent.iter().collect();
    v.sort_by_key(|(run, _)| **run);
    v.into_iter()
        .map(|(run, a)| {
            (
                format!("silent:{run}"),
                "host_lost".to_string(),
                crate::hostloss::silent_alert(a.task, &a.step, a.silent_ms),
            )
        })
        .collect()
}

/// Waits that are not moving, or never can: one per resource and cause.
fn wait_alerts(d: &Daemon) -> Vec<(String, String, String)> {
    let long =
        crate::notify::load(d).map_or(crate::notify::DEFAULT_LONG_WAIT_MS, |s| s.long_wait_ms);
    d.broker
        .wait_alerts(long)
        .into_iter()
        .map(|a| {
            if a.never {
                (
                    format!("never-fits:{}", a.resource),
                    "never_fits".into(),
                    a.text,
                )
            } else {
                (
                    format!("long-wait:{}", a.resource),
                    "long_wait".into(),
                    a.text,
                )
            }
        })
        .collect()
}

/// A project's base is frozen: its post-land tier went red and only the fix task may land.
/// Names the reason, how long, and the fix task with its state; when that task has failed or
/// been canceled nothing will lift the freeze, so it says so and gives the commands.
fn freeze_alerts(d: &Daemon, w: &mason_engine::World) -> Vec<(String, String, String)> {
    let now = now_ms();
    d.store
        .freezes()
        .unwrap_or_default()
        .into_iter()
        .map(|f| {
            let mins = now.saturating_sub(f.since) / 60_000;
            let mut text = format!(
                "{}: the base is frozen, only the fix may land ({}; frozen {mins} min)",
                f.project, f.reason
            );
            let fix = f.fix_task.and_then(|n| w.tasks.get(&TaskId(n)));
            match (f.fix_task, fix) {
                (Some(n), Some(t)) => {
                    text.push_str(&format!(
                        ". Fix task #{n} is {}",
                        format!("{:?}", t.state).to_lowercase()
                    ));
                    if matches!(t.state, State::Failed | State::Canceled) {
                        text.push_str(&format!(
                            ". The freeze will not lift by itself: run `mason task retry {n}`, \
                             or `mason task force {n}` to land past it"
                        ));
                    }
                }
                _ => text.push_str(
                    ". No fix task exists, so the freeze will not lift by itself: file one and \
                     `mason task force` it past the freeze",
                ),
            }
            (format!("frozen:{}", f.project), "landing".into(), text)
        })
        .collect()
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
    out.extend(silent_alerts(d));
    out.extend(d.landing().attention());
    out.extend(freeze_alerts(d, &w));
    out.extend(wait_alerts(d));
    if let Some((ms, source)) = d.broker.cpu_missing_ms() {
        out.push((
            "cpu-reading".into(),
            "environment".into(),
            format!(
                "The CPU reading has been missing for {} seconds; compiles are gated on {source} meanwhile",
                ms / 1000
            ),
        ));
    }
    if let Some(text) = d.exit_numbers_warning() {
        out.push(("numbers".into(), "numbers".into(), text));
    }
    out.extend(disk_alert(d, &w));
    out.extend(bug_alerts(
        &d.store
            .bugs_since(now_ms().saturating_sub(BUG_WINDOW_MS))
            .unwrap_or_default(),
        now_ms(),
    ));
    out.extend(stall_alert(d.last_big_stall(), now_ms()));
    let threshold = std::env::var("MASON_LAUNCH_FROZEN_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(LAUNCH_FROZEN_MS);
    out.extend(launch_frozen_alert(
        &mason_step_host::launcher::launching(),
        now_ms(),
        threshold,
        syspolicyd_running,
    ));
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
    out.extend(fetch_alerts(d));
    out.extend(git_stuck_alert(d));
    out.extend(stalled_alert(d, &w));
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

/// `fetch:<project>`: the project's fetches have been failing for `fetch_alert_after`. Reads only the
/// failure record; touches no git.
fn fetch_alerts(d: &Daemon) -> Vec<(String, String, String)> {
    let now = now_ms();
    let limit = d.cfg.fetch_alert_after.as_millis() as u64;
    d.fetch_failing()
        .into_iter()
        .filter_map(|(pid, since, err)| {
            let age = now.saturating_sub(since);
            if age < limit {
                return None;
            }
            let p = d.projects.get(&pid)?;
            let first = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("no error text");
            Some((
                format!("fetch:{}", p.id),
                "tracker".into(),
                format!(
                    "{}: git fetch has been failing for {}: {first}. Tasks are working from the base as it was {} ago",
                    p.id,
                    span_text(age),
                    span_text(age)
                ),
            ))
        })
        .collect()
}

/// The definitions thread (watch, fetch, admit) has not finished a pass for too long: git or the
/// repository lock is stuck. Reads an atomic and the lock labels; never waits on git or a repository
/// lock itself.
fn git_stuck_alert(d: &Daemon) -> Option<(String, String, String)> {
    let now = now_ms();
    let ended = d
        .defs_pass_ended_ms
        .load(std::sync::atomic::Ordering::SeqCst);
    let limit = (d.cfg.stuck_after + d.cfg.reload_interval).as_millis() as u64;
    let age = now.saturating_sub(ended);
    if age <= limit {
        return None;
    }
    let mut text = format!(
        "Git work is stuck: the last pass over the repositories ended {} ago",
        span_text(age)
    );
    if let Some((label, since)) = d.repo_lock_holders().into_iter().min_by_key(|h| h.1) {
        text.push_str(&format!(
            "; the repository lock has been held by {label} for {}",
            span_text(now.saturating_sub(since))
        ));
    }
    Some(("git-stuck".into(), "git_stuck".into(), text))
}

/// The pipeline as a whole has stopped getting anywhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stall {
    /// How long nothing has moved: no event appended and no step output.
    pub quiet_ms: u64,
    /// The tasks that are outstanding and could be moving.
    pub tasks: Vec<TaskId>,
}

/// Is this task work the pipeline owes, as opposed to something waiting on a person, a blocker, a
/// pause or a clock?
fn outstanding(t: &Task, now_ms: u64) -> bool {
    if matches!(
        t.state,
        State::Proposed | State::NeedsAttention | State::Failed
    ) || t.state.is_terminal()
    {
        return false;
    }
    match &t.activity {
        Activity::Idle | Activity::Held(_) | Activity::Paused => false,
        Activity::Waiting(WaitReason::Owner) => false,
        Activity::Waiting(WaitReason::NotBefore(at)) => at.0 <= now_ms,
        Activity::Waiting(_) | Activity::Running => true,
    }
}

/// Decide whether the pipeline is stalled: some task is outstanding, and neither an event
/// (`last_event_ms`, 0 if there is none) nor any running step's output (`outputs`) is newer than
/// `stall_after_ms` ago. `stall_after_ms == 0` is off. Pure: the clock is an argument.
pub fn stall_check(
    tasks: &[&Task],
    last_event_ms: u64,
    outputs: &[u64],
    now_ms: u64,
    stall_after_ms: u64,
) -> Option<Stall> {
    if stall_after_ms == 0 || last_event_ms == 0 {
        return None;
    }
    let owed: Vec<TaskId> = tasks
        .iter()
        .filter(|t| outstanding(t, now_ms))
        .map(|t| t.id)
        .collect();
    if owed.is_empty() {
        return None;
    }
    let last = outputs.iter().copied().fold(last_event_ms, u64::max);
    let quiet_ms = now_ms.saturating_sub(last);
    (quiet_ms >= stall_after_ms).then_some(Stall {
        quiet_ms,
        tasks: owed,
    })
}

/// The words for a stall: how long, how many tasks, and the reasons already known.
pub fn stall_text(s: &Stall, reasons: &[String]) -> String {
    let n = s.tasks.len();
    let mut text = format!(
        "The pipeline has stopped moving: no event or step output for {}, with {n} task{} unfinished",
        span_text(s.quiet_ms),
        if n == 1 { "" } else { "s" }
    );
    if reasons.is_empty() {
        text.push_str(". Nothing known explains it; look at what is running");
    } else {
        text.push_str(&format!(". Likely: {}", reasons.join("; ")));
    }
    text
}

/// `stalled`: raised from the event log's last timestamp, the live progress records, the task
/// list and the scheduler's own answers. It touches no git and no repository lock.
fn stalled_alert(d: &Daemon, w: &mason_engine::World) -> Option<(String, String, String)> {
    let after = crate::notify::load(d).unwrap_or_default().stall_ms;
    if after == 0 {
        return None;
    }
    let last = d.store.last_seq().ok().filter(|s| *s > 0)?;
    let last_event = d
        .store
        .read_since(last - 1, 1)
        .ok()?
        .into_iter()
        .next()?
        .event
        .at
        .0;
    let tasks: Vec<&Task> = w.tasks.values().collect();
    let stall = stall_check(
        &tasks,
        last_event,
        &d.progress_last_outputs(),
        now_ms(),
        after,
    )?;
    let mut reasons: Vec<String> = vec![];
    if let Some((_, _, text)) = disk_alert(d, w) {
        reasons.push(text);
    }
    let now = now_ms();
    for h in d.store.account_holds().unwrap_or_default() {
        if h.until_ms.is_some_and(|u| u <= now) {
            continue;
        }
        reasons.push(format!(
            "{}: {}",
            h.account,
            crate::holds::describe(&h, now)
        ));
    }
    let snap = d.broker.snapshot();
    let mut waits: Vec<String> = snap["waiting"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x["explain"].as_str())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    waits.sort();
    waits.dedup();
    reasons.extend(waits.into_iter().take(3));
    Some((
        "stalled".into(),
        "stalled".into(),
        stall_text(&stall, &reasons),
    ))
}

fn span_text(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 120 {
        format!("{} minutes", s / 60)
    } else {
        format!("{s} seconds")
    }
}

/// Raise new alerts and clear the ones whose condition is gone.
pub fn sync_alerts(d: &Daemon) -> (Vec<String>, Vec<String>) {
    // One at a time: two callers (the watcher and a request for the list) that each work out what
    // is true and then write it can cross, and the older answer would clear an alert the newer one
    // had just raised, to be raised again as "new".
    let _one = d.notify.syncing.lock().unwrap_or_else(|e| e.into_inner());
    crate::notify::sync_table(d, &alerts_now(d))
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
                        && !h.info.id.contains("-check")
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

#[cfg(test)]
mod launch_frozen_tests {
    use super::*;
    use mason_step_host::launcher::Launching;

    fn l(id: &str, since_ms: u64) -> Launching {
        Launching {
            id: id.into(),
            since_ms,
        }
    }

    #[test]
    fn nothing_waiting_longer_than_the_threshold_raises_nothing() {
        assert!(launch_frozen_alert(&[], 1_000_000, 120_000, || panic!("asked")).is_none());
        let fresh = [l("t1r1-a", 950_000)];
        assert!(launch_frozen_alert(&fresh, 1_000_000, 120_000, || panic!("asked")).is_none());
    }

    #[test]
    fn old_launches_are_counted_with_the_longest_and_their_tasks() {
        let list = [
            l("t4r1-a", 700_000),
            l("t9r2-b", 600_000),
            l("t5r1-c", 990_000),
        ];
        let (key, kind, text) =
            launch_frozen_alert(&list, 1_000_000, 120_000, || Some(false)).unwrap();
        assert_eq!(
            (key.as_str(), kind.as_str()),
            ("launch-frozen", "environment")
        );
        assert!(text.starts_with("2 step host launch(es) frozen"), "{text}");
        assert!(text.contains("#4, #9") && !text.contains("#5"), "{text}");
        assert!(text.contains("longest has waited 400s"), "{text}");
        assert!(text.contains("syspolicyd is NOT running"), "{text}");
    }

    #[test]
    fn syspolicyd_running_and_unknown() {
        let list = [l("t4r1-a", 0)];
        let t = launch_frozen_alert(&list, 1_000_000, 120_000, || Some(true))
            .unwrap()
            .2;
        assert!(t.contains("syspolicyd is running"), "{t}");
        let t = launch_frozen_alert(&list, 1_000_000, 120_000, || None)
            .unwrap()
            .2;
        assert!(!t.contains("syspolicyd"), "{t}");
    }

    #[test]
    fn task_from_host_id() {
        assert_eq!(launch_task("t12r3-test"), Some(12));
        assert_eq!(launch_task("other"), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn task(id: u64, state: State, activity: Activity) -> Task {
        Task {
            id: TaskId(id),
            project: ProjectId(1),
            title: String::new(),
            body: String::new(),
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            enqueued_at: Timestamp(0),
            state_entered_at: Timestamp(0),
            state,
            activity,
            landed: None,
            explored: false,
            blockers: BTreeSet::new(),
            not_before: None,
            paused: false,
            failures: 0,
            attempts: BTreeMap::new(),
            decision: None,
            cancel_keeps_branch: false,
            failure: None,
            def: String::new(),
            step: String::new(),
            step_attempts: BTreeMap::new(),
            route: None,
            explore_returns: 0,
            approved: false,
            force_explore: false,
        }
    }

    const MIN: u64 = 60_000;
    const NOW: u64 = 100 * MIN;

    fn check(tasks: &[Task], last_event: u64, outputs: &[u64], after: u64) -> Option<Stall> {
        let refs: Vec<&Task> = tasks.iter().collect();
        stall_check(&refs, last_event, outputs, NOW, after)
    }

    #[test]
    fn quiet_with_work_outstanding_is_a_stall() {
        let t = [
            task(1, State::Developing, Activity::Running),
            task(2, State::Queued, Activity::Waiting(WaitReason::Slot)),
        ];
        let s = check(&t, NOW - 40 * MIN, &[], 30 * MIN).unwrap();
        assert_eq!(s.quiet_ms, 40 * MIN);
        assert_eq!(s.tasks, vec![TaskId(1), TaskId(2)]);
        let text = stall_text(&s, &["disk has 12 GB free".into()]);
        assert!(
            text.contains("40 minutes") && text.contains("2 tasks"),
            "{text}"
        );
        assert!(text.contains("disk has 12 GB free"), "{text}");
    }

    #[test]
    fn recent_event_or_off_is_no_stall() {
        let t = [task(1, State::Developing, Activity::Running)];
        assert!(check(&t, NOW - 29 * MIN, &[], 30 * MIN).is_none());
        assert!(check(&t, NOW - 29 * MIN, &[], 0).is_none());
        assert!(check(&t, NOW - 90 * MIN, &[], 0).is_none());
        assert!(
            check(&t, 0, &[], 30 * MIN).is_none(),
            "no log, nothing to compare"
        );
    }

    #[test]
    fn output_still_arriving_is_no_stall() {
        let t = [task(1, State::Developing, Activity::Running)];
        assert!(check(&t, NOW - 90 * MIN, &[NOW - 90 * MIN, NOW - MIN], 30 * MIN).is_none());
        assert!(check(&t, NOW - 90 * MIN, &[NOW - 80 * MIN], 30 * MIN).is_some());
    }

    #[test]
    fn only_tasks_waiting_on_someone_else_is_no_stall() {
        let t = [
            task(
                1,
                State::NeedsAttention,
                Activity::Waiting(WaitReason::Owner),
            ),
            task(
                2,
                State::Queued,
                Activity::Held(BTreeSet::from([TaskId(1)])),
            ),
            task(3, State::Proposed, Activity::Idle),
            task(4, State::Queued, Activity::Paused),
            task(5, State::Failed, Activity::Idle),
            task(6, State::Landed, Activity::Idle),
            task(
                7,
                State::Queued,
                Activity::Waiting(WaitReason::NotBefore(Timestamp(NOW + MIN))),
            ),
        ];
        assert!(check(&t, NOW - 90 * MIN, &[], 30 * MIN).is_none());
        let mut more = t.to_vec();
        more.push(task(
            8,
            State::Testing,
            Activity::Waiting(WaitReason::On("compile slot".into())),
        ));
        let s = check(&more, NOW - 90 * MIN, &[], 30 * MIN).unwrap();
        assert_eq!(s.tasks, vec![TaskId(8)]);
    }
}
