// SPDX-License-Identifier: GPL-3.0-or-later

//! The loopback HTTP API the CLI talks to. Every handler that touches the
//! store, git or the filesystem runs on the bounded blocking pool, so the
//! event loop never waits on them. (Stage 11 moves the CLI and the monitors
//! onto the protobuf WebSocket; the shapes here are deliberately small.)

use crate::daemon::{now_ms, Daemon};
use crate::pool::PoolError;
use axum::extract::{Path, Query, State as Shared};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use mason_core::*;
use mason_definition::Definition;
use mason_engine::World;
use mason_store::{state_name, StoreError};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

type Res = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn fail(code: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    (code, Json(json!({"error": msg.into()})))
}

fn pool_err(e: PoolError) -> (StatusCode, Json<Value>) {
    match e {
        PoolError::Busy => fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "the daemon is busy; try again",
        ),
        PoolError::Lost => fail(StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
    }
}

fn store_err(e: StoreError) -> (StatusCode, Json<Value>) {
    match e {
        StoreError::Rejected(mason_engine::ReduceError::BlockerCycle(path)) => fail(
            StatusCode::CONFLICT,
            format!(
                "not allowed: that would make a dependency cycle: {}",
                path.iter()
                    .map(|t| format!("#{}", t.0))
                    .collect::<Vec<_>>()
                    .join(" -> ")
            ),
        ),
        StoreError::Rejected(r) => fail(StatusCode::CONFLICT, format!("not allowed: {r:?}")),
        other => fail(StatusCode::INTERNAL_SERVER_ERROR, other.to_string()),
    }
}

pub fn parse_state(s: &str) -> Option<State> {
    State::ALL.into_iter().find(|x| state_name(*x) == s)
}

pub fn activity_text(a: &Activity) -> String {
    match a {
        Activity::Idle => "idle".into(),
        Activity::Running => "running".into(),
        Activity::Paused => "paused".into(),
        Activity::Held(by) => {
            format!(
                "held, blocked by {}",
                by.iter()
                    .map(|t| format!("#{}", t.0))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        Activity::Waiting(WaitReason::Slot) => "waiting for a free slot".into(),
        Activity::Waiting(WaitReason::NotBefore(t)) => format!("waiting until {}", t.0),
        Activity::Waiting(WaitReason::Owner) => "waiting for an answer from the owner".into(),
        Activity::Waiting(WaitReason::On(w)) => format!("waiting on {w}"),
    }
}

fn task_json(d: &Daemon, w: &World, t: &Task, detail: bool) -> Value {
    let project = d.project(t.project);
    let mut v = json!({
        "id": t.id.0,
        "project": project.map_or_else(
            || w.projects.get(&t.project).map_or("?".to_string(), |p| p.name.clone()),
            |p| p.id.clone(),
        ),
        "title": t.title,
        "state": state_name(t.state),
        "activity": activity_text(&t.activity),
        "landed": t.landed.map(|l| match l { LandedOutcome::Merged => "merged", LandedOutcome::ClosedNoChange => "closed with no change" }),
        "enqueued_at": t.enqueued_at.0,
        "failures": t.failures,
        "step": t.step,
        "route": t.route.map(Route::name),
        "skipped_exploring": t.route.is_some_and(Route::skipped_exploring),
        "explore_returns": t.explore_returns,
    });
    if t.state == State::Failed {
        v["retry_to"] = json!(retry_default(&task_def(d, t), t));
    }
    if detail {
        v["body"] = json!(t.body);
        v["priority"] = json!(format!("{:?}", t.priority).to_lowercase());
        v["attempts"] = json!(t
            .attempts
            .iter()
            .map(|(s, n)| (state_name(*s).to_string(), json!(n)))
            .collect::<serde_json::Map<_, _>>());
        if let Some(p) = project {
            v["branch"] = json!(p.branch(t.id.0));
            v["worktree"] = json!(p.worktree(&d.cfg, t.id.0).display().to_string());
        }
        v["definition"] = json!(t.def);
        v["cpu_permille"] = json!(d
            .cpu_reading()
            .by_task
            .get(&t.id.0)
            .map_or(-1, |p| *p as i32));
        v["progress"] = json!({"now_ms": crate::daemon::now_ms(), "runs": progress_runs(d, t.id)});
        v["numbers"] = json!(d
            .number_lines(t)
            .iter()
            .map(|n| json!({
                "kind": n.kind,
                "number": n.number,
                "released": n.released,
                "path": n.path,
                "source": n.source,
            }))
            .collect::<Vec<_>>());
        // The tier and the real model in use (or, before it starts, expected).
        v["model"] = crate::launchview::task_use(d, w, t)
            .map_or(Value::Null, |m| crate::launchview::json_of(&m));
        v["blocked_by"] = json!(t.blockers.iter().map(|b| b.0).collect::<Vec<_>>());
        v["held_by"] = crate::blockers::links_json(d, w, t);
        v["held_line"] = crate::blockers::held_line(w, t).map_or(Value::Null, |s| json!(s));
        if t.state == State::Canceled && t.cancel_keeps_branch {
            v["revive"] = json!(format!(
                "`mason task revive {}` puts it back in the queue with its branch and links; `retry` does not revive a canceled task",
                t.id.0
            ));
        }
        v["blocks"] = json!(w
            .tasks
            .values()
            .filter(|o| o.blockers.contains(&t.id))
            .map(|o| o.id.0)
            .collect::<Vec<_>>());
        v["wait"] = d
            .broker
            .explain_task(t.id)
            .map_or(Value::Null, |w| json!(w));
        v["closing"] = d.store.closing(t.id.0).ok().flatten().map_or(
            Value::Null,
            |(verdict, note)| json!({"verdict": verdict, "note": note}),
        );
        v["debts"] = json!(d
            .store
            .debts(Some(t.id.0), false)
            .unwrap_or_default()
            .iter()
            .map(debt_json)
            .collect::<Vec<_>>());
        v["self_hold"] = d.store.hold(t.id.0).ok().flatten().map_or(
            Value::Null,
            |h| json!({"reasons": h.reasons, "approved_by": h.approved_by, "since": h.at}),
        );
        v["failure"] = t.failure.as_ref().map_or(Value::Null, |f| json!(f));
        v["failure_class"] = t.failure_class.map_or(Value::Null, |c| json!(c.name()));
        v["excused"] = json!(crate::basecheck::recorded(&d.task_dir(t.id)));
        v["host_losses"] = json!(d
            .store
            .host_losses_for_task(t.id.0)
            .unwrap_or_default()
            .iter()
            .map(|l| json!({
                "host": l.host_id, "run": l.run, "step": l.step, "lost_at_ms": l.lost_at_ms,
                "detail": l.detail, "rerun": l.retried,
            }))
            .collect::<Vec<_>>());
        v["question"] = crate::attention::card_of(d, t).map_or(Value::Null, |(c, _)| json!(c.ask));
        v["card"] = crate::attention::card_of(d, t)
            .map_or(Value::Null, |(c, h)| crate::attention::card_json(&c, &h));
        v["steps"] = json!(w
            .step_runs
            .values()
            .filter(|r| r.task == t.id)
            .map(|r| json!({
                "run": r.id.0, "step": state_name(r.step), "name": r.name, "attempt": r.attempt,
                "started_at": r.started_at.0,
                "outcome": r.outcome.map(|o| format!("{o:?}").to_lowercase()),
                "launches": crate::launchview::history(d, r.id.0).iter().map(crate::launchview::json_of).collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>());
    }
    v
}

async fn status(Shared(d): Shared<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    // The view pool: `mason status` is how you find out the main pool is stuck.
    let v = d
        .view_pool
        .run(move || {
            let w = d2.world().map_err(store_err)?;
            let mut counts = serde_json::Map::new();
            for s in State::ALL {
                counts.insert(
                    state_name(s).into(),
                    json!(w.tasks.values().filter(|t| t.state == s).count()),
                );
            }
            let (stalls, worst) = d2.stall_stats();
            let hosts = mason_step_host::registry::Registry::new(d2.cfg.hosts_dir())
                .scan()
                .unwrap_or_default();
            let running = hosts
                .iter()
                .filter(|h| h.state == mason_step_host::registry::HostState::Running)
                .count();
            Ok::<_, (StatusCode, Json<Value>)>(json!({
                "pid": std::process::id(),
                "uptime_s": d2.started.elapsed().as_secs(),
                "seq": d2.store.last_seq().unwrap_or(0),
                "tasks": counts,
                "jobs": d2.job_count(),
                "max_concurrent": d2.cfg.max_concurrent,
                "active": w.tasks.values().filter(|t| t.state.is_step()).filter_map(|t| {
                    crate::launchview::task_use(&d2, &w, t).map(|m| json!({
                        "task": t.id.0, "step": t.step, "display": m.display,
                        "model": crate::launchview::json_of(&m),
                    }))
                }).collect::<Vec<_>>(),
                "event_loop_stalls": stalls,
                "pools": {"blocking": d2.pool.stats(), "views": d2.view_pool.stats()},
                "worst_stall_ms": worst,
                "bugs": d2.store.bugs().map_or(0, |b| b.len()),
                "thread_panics": d2.store.bugs().map_or(json!(0), |b| {
                    let p: Vec<_> = b.iter().filter(|x| x.1 == "thread_panic").collect();
                    json!({"count": p.len(), "last": p.last().map(|x| x.2.clone())})
                }),
                "stuck": d2.broker.disk_low().map(|g| json!({
                    "cause": "disk",
                    "free_mb": g.free_mb,
                    "floor_mb": g.floor_mb,
                    "tasks_waiting": d2.broker.disk_held(),
                })),
                "projects": d2.projects().values().map(|p| p.id.clone()).collect::<Vec<_>>(),
                "step_hosts_running": running,
                "trackers": d2.projects().keys().map(|pid| crate::tracker::state_json(&d2, *pid)).collect::<Vec<_>>(),
                // The base-branch CI watch, for each project that has it on.
                "ci_watch": d2.projects().iter().filter_map(|(pid, p)| {
                    let v = crate::ci_watch::view(&d2, *pid);
                    (v["enabled"] == true).then(|| (p.id.clone(), v))
                }).collect::<serde_json::Map<String, Value>>(),
                // Scheduled CI runs being followed.
                "ci_runs": d2.store.schedules().unwrap_or_default().iter().filter_map(|r| {
                    crate::ci_schedule::follow_view(r).map(|mut v| {
                        v["name"] = json!(r.name);
                        v
                    })
                }).collect::<Vec<_>>(),
                "definitions": d2.projects().iter().map(|(pid, p)| json!({
                    "project": p.id,
                    "hash": d2.current_def(*pid).hash,
                    "origin": d2.current_origin(*pid).label(),
                    "refused": d2.definition_error(*pid),
                })).collect::<Vec<_>>(),
                // The digest's numbers on skipping exploration: how many tasks started
                // by each route, and how many of those came back.
                "exploring": {
                    "explored": w.tasks.values().filter(|t| t.route == Some(Route::Explore)).count(),
                    "skipped_by_flag": w.tasks.values().filter(|t| t.route == Some(Route::Flag)).count(),
                    "skipped_by_clarity": w.tasks.values().filter(|t| t.route == Some(Route::Clarity)).count(),
                    "skipped_by_classifier": w.tasks.values().filter(|t| t.route == Some(Route::Classifier)).count(),
                    "explore_anyway": w.tasks.values().filter(|t| t.route == Some(Route::ExploreAnyway)).count(),
                    "sent_back_to_exploring": w.tasks.values().map(|t| t.explore_returns).sum::<u32>(),
                },
            }))
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(v))
}

async fn list_tasks(
    Shared(d): Shared<Arc<Daemon>>,
    Query(q): Query<HashMap<String, String>>,
) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let w = d2.world().map_err(store_err)?;
            let want = q.get("state").and_then(|s| parse_state(s));
            if q.contains_key("state") && want.is_none() {
                return Err(fail(StatusCode::BAD_REQUEST, "unknown state"));
            }
            let mut tasks: Vec<&Task> = w.tasks.values().filter(|t| want.is_none_or(|s| t.state == s)).collect();
            tasks.sort_by_key(|t| t.id);
            Ok(json!({"tasks": tasks.iter().map(|t| task_json(&d2, &w, t, false)).collect::<Vec<_>>()}))
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(v))
}

/// The `after` field of a filing: a list of task ids.
fn parse_after(v: &Value) -> Result<Vec<TaskId>, String> {
    let Some(list) = v.as_array() else {
        return if v.is_null() {
            Ok(vec![])
        } else {
            Err("after must be a list of task ids".into())
        };
    };
    let mut out: Vec<TaskId> = vec![];
    for x in list {
        let n = x
            .as_u64()
            .ok_or_else(|| format!("after: {x} is not a task id"))?;
        if !out.contains(&TaskId(n)) {
            out.push(TaskId(n));
        }
    }
    Ok(out)
}

/// Every id exists, and none is a task that can never land (so would hold the
/// new one forever). A landed one is already satisfied.
pub fn check_after(d: &Arc<Daemon>, ids: &[TaskId]) -> Result<(), String> {
    let w = d.world().map_err(|e| e.to_string())?;
    for id in ids {
        match w.tasks.get(id) {
            None => return Err(format!("after: no task #{}", id.0)),
            Some(t) if matches!(t.state, State::Canceled | State::Failed) => {
                return Err(format!(
                    "after: task #{} is {} and will never land, so a task after it would be held forever",
                    id.0,
                    state_name(t.state)
                ))
            }
            Some(_) => {}
        }
    }
    Ok(())
}

async fn add_task(Shared(d): Shared<Arc<Daemon>>, Json(b): Json<Value>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let project = b["project"].as_str().unwrap_or_default();
            let Some(pid) = d2.project_id_by_name(project) else {
                return Err(fail(
                    StatusCode::NOT_FOUND,
                    format!(
                        "no project {project:?}; configured: {}",
                        d2.projects()
                            .values()
                            .map(|p| p.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            };
            // A proposal spends nothing, so only the cooldown refuses it (decision 0046).
            let proposed = b["proposed"].as_bool().unwrap_or(false);
            if proposed {
                crate::selfguard::check_filing_proposal(&d2, pid)
            } else {
                crate::selfguard::check_filing_as(&d2, pid, TrustSource::Owner)
            }
            .map_err(|e| fail(StatusCode::TOO_MANY_REQUESTS, e))?;
            let title = b["title"].as_str().unwrap_or_default().trim().to_string();
            if title.is_empty() || title.chars().count() > 200 {
                return Err(fail(
                    StatusCode::BAD_REQUEST,
                    "title must be 1 to 200 characters",
                ));
            }
            let filing = mason_definition::filing::parse(b["body"].as_str().unwrap_or_default());
            let explored = b["explored"].as_bool().unwrap_or(false) || filing.explored;
            let body = filing.body;
            if body.len() > 100_000 {
                return Err(fail(
                    StatusCode::BAD_REQUEST,
                    "body is limited to 100,000 bytes",
                ));
            }
            if crate::numbers::stage_of_task(&title, &body).is_none() {
                let room = d2.project(pid).is_none_or(|p| d2.adhoc_exit_room(p));
                if !room {
                    return Err(fail(
                        StatusCode::CONFLICT,
                        format!(
                            "the range of stage and exit-script numbers for ad-hoc tasks (from {} to {}) is used up; \
                             widening the `s[0-9][0-9][0-9].sh` glob is a gate change that waits for the owner",
                            d2.cfg.adhoc_exit_start,
                            crate::numbers::EXIT_CEILING
                        ),
                    ));
                }
            }
            let priority = match b["priority"].as_str().unwrap_or("normal") {
                "urgent" => Priority::Urgent,
                "high" => Priority::High,
                "normal" => Priority::Normal,
                "low" => Priority::Low,
                o => {
                    return Err(fail(
                        StatusCode::BAD_REQUEST,
                        format!("unknown priority {o:?}"),
                    ))
                }
            };
            let after = parse_after(&b["after"])
                .and_then(|ids| check_after(&d2, &ids).map(|()| ids))
                .map_err(|e| fail(StatusCode::BAD_REQUEST, e))?;
            let appended = d2
                .emit(EventKind::TaskFiled {
                    project: pid,
                    title,
                    body,
                    priority,
                    trust: TrustSource::Owner, // filed from the owner's own CLI
                    explored,
                    proposed,
                    blockers: after,
                    not_before: None,
                })
                .map_err(store_err)?;
            let task = appended.effects.iter().find_map(|e| {
                if let Effect::TaskCreated { task } = e {
                    Some(task.0)
                } else {
                    None
                }
            });
            Ok(json!({"task": task, "seq": appended.seq}))
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(v))
}

// ---- proposals and self-improvement (Part 13) ----

fn owner_only(d: &Daemon, who: &str, what: &str) -> Result<(), (StatusCode, Json<Value>)> {
    if crate::attention::is_owner(d, who) {
        Ok(())
    } else {
        Err(fail(
            StatusCode::FORBIDDEN,
            format!("{who:?} is not the owner's identity; only the owner can {what}"),
        ))
    }
}

fn code(c: u16) -> StatusCode {
    StatusCode::from_u16(c).unwrap_or(StatusCode::BAD_REQUEST)
}

async fn proposals_list(
    Shared(d): Shared<Arc<Daemon>>,
    Query(q): Query<HashMap<String, String>>,
) -> Res {
    let open = q.get("open").is_some_and(|v| v != "0");
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || crate::proposals::list_json(&d2, open))
        .await
        .map_err(pool_err)?;
    Ok(Json(json!({"proposals": v})))
}

async fn proposal_show(Shared(d): Shared<Arc<Daemon>>, Path(task): Path<u64>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let w = d2.world().map_err(store_err)?;
            match d2.store.proposal(task).map_err(store_err)? {
                Some(r) => Ok(crate::proposals::row_json(&w, &r)),
                None => Err(fail(
                    StatusCode::NOT_FOUND,
                    format!("task #{task} is not a proposal"),
                )),
            }
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(v))
}

/// File a proposal as the agent of task `task` would: every rule applies, and a refusal is the
/// error. (An agent's own `file_proposal` calls are filed the same way when its step ends.)
async fn proposal_file(Shared(d): Shared<Arc<Daemon>>, Json(b): Json<Value>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let from = b["task"].as_u64().map(TaskId);
            let w = d2.world().map_err(store_err)?;
            let project = match from.and_then(|t| w.tasks.get(&t)) {
                Some(t) => t.project,
                None => b["project"]
                    .as_str()
                    .and_then(|n| d2.project_id_by_name(n))
                    .ok_or_else(|| fail(StatusCode::BAD_REQUEST, "name the task the proposal comes from (`task`), or a `project`"))?,
            };
            let text = |k: &str| b[k].as_str().unwrap_or_default().to_string();
            let f = crate::proposals::Filing {
                from_task: from,
                project,
                title: text("title"),
                body: text("body"),
                evidence: text("evidence"),
                blocks_task: b["blocks_task"].as_bool().unwrap_or(false),
                explored: b["explored"].as_bool().unwrap_or(false),
            };
            match crate::proposals::file(&d2, &f) {
                Ok(done) => Ok(json!({"task": done.task.0, "route": done.route, "proposed": done.proposed, "project": done.project})),
                Err(why) => {
                    crate::attention::intervene(&d2, "proposal_refused", from, "pipeline", &f.title, &why);
                    Err(fail(StatusCode::UNPROCESSABLE_ENTITY, why))
                }
            }
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(v))
}

async fn proposal_act(
    Shared(d): Shared<Arc<Daemon>>,
    headers: axum::http::HeaderMap,
    Path((task, act)): Path<(u64, String)>,
    body: Option<Json<Value>>,
) -> Res {
    let who = who_of(&headers);
    let b = body.map_or(json!({}), |b| b.0);
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            owner_only(&d2, &who, "decide on or publish a proposal")?;
            if act != "edit" && d2.store.proposal(task).map_err(store_err)?.is_none() {
                return Err(fail(
                    StatusCode::NOT_FOUND,
                    format!("task #{task} is not a proposal"),
                ));
            }
            let s = |k: &str| b[k].as_str();
            match act.as_str() {
                "approve" | "reject" => {
                    let w = d2.world().map_err(store_err)?;
                    let st = w.tasks.get(&TaskId(task)).map(|t| t.state);
                    if st != Some(State::Proposed) {
                        return Err(fail(
                            StatusCode::CONFLICT,
                            format!("task #{task} is not waiting for you ({st:?})"),
                        ));
                    }
                    let approve = act == "approve";
                    let reason = s("reason").unwrap_or("no reason given").to_string();
                    d2.emit(if approve {
                        EventKind::Approved { task: TaskId(task) }
                    } else {
                        EventKind::Rejected { task: TaskId(task) }
                    })
                    .map_err(store_err)?;
                    crate::proposals::note_decision(&d2, task, approve, &reason);
                    crate::attention::intervene(
                        &d2,
                        &format!("proposal_{act}"),
                        Some(TaskId(task)),
                        &who,
                        &reason,
                        &reason,
                    );
                    Ok(json!({"ok": true}))
                }
                "edit" => {
                    crate::proposals::edit(&d2, task, s("title"), s("body"), s("evidence"))
                        .map_err(|(c, m)| fail(code(c), m))?;
                    Ok(json!({"ok": true}))
                }
                "publish" => {
                    let confirm = b["confirm"].as_bool().unwrap_or(false);
                    crate::improve::publish(&d2, task, confirm).map_err(|(c, m)| fail(code(c), m))
                }
                other => Err(fail(StatusCode::NOT_FOUND, format!("no action {other:?}"))),
            }
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(v))
}

async fn proposals_poll(Shared(d): Shared<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    d.pool
        .run(move || crate::improve::poll(&d2))
        .await
        .map_err(pool_err)?;
    Ok(Json(json!({"ok": true})))
}

async fn improve_start(
    Shared(d): Shared<Arc<Daemon>>,
    headers: axum::http::HeaderMap,
    Path(name): Path<String>,
    Json(b): Json<Value>,
) -> Res {
    let who = who_of(&headers);
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            owner_only(&d2, &who, "ask MASON to change itself")?;
            let text = b["text"].as_str().unwrap_or_default();
            let (task, branch) = crate::improve::improve(&d2, &name, text).map_err(|(c, m)| fail(code(c), m))?;
            crate::attention::intervene(&d2, "improve", Some(task), &who, text, "");
            let access = d2.store.proposal(task.0).ok().flatten().map(|r| r.access).unwrap_or_default();
            Ok(json!({"task": task.0, "branch": branch, "access": access, "channel": crate::improve::switched(&d2)}))
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(v))
}

async fn task_detail(Shared(d): Shared<Arc<Daemon>>, Path(id): Path<u64>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let w = d2.world().map_err(store_err)?;
            match w.tasks.get(&TaskId(id)) {
                Some(t) => Ok(task_json(&d2, &w, t, true)),
                None => Err(fail(StatusCode::NOT_FOUND, format!("no task #{id}"))),
            }
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(v))
}

async fn cancel(
    Shared(d): Shared<Arc<Daemon>>,
    Path(id): Path<u64>,
    body: Option<Json<Value>>,
) -> Res {
    let delete = body.is_some_and(|b| b["delete_branch"].as_bool().unwrap_or(false));
    let d2 = d.clone();
    d.pool
        .run(move || {
            d2.emit(EventKind::Canceled {
                task: TaskId(id),
                delete_branch: delete,
            })
            .map_err(store_err)
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(json!({"ok": true})))
}

/// The definition a task runs under.
fn task_def(d: &Daemon, t: &Task) -> Arc<Definition> {
    d.pinned_def(&t.def)
        .filter(|_| !t.def.is_empty())
        .unwrap_or_else(|| d.current_def(t.project))
}

/// Where a retry that names no step goes: the failed step when the failure was not
/// about the code (s168), otherwise the first developing step.
fn retry_default(def: &Definition, t: &Task) -> String {
    use crate::hostloss::retry_step;
    if let Some(step) = retry_step(t.failure.as_deref(), t.failure_class, &t.step) {
        return step;
    }
    def.first_in(State::Developing)
        .map_or("develop".into(), |s| s.name.clone())
}

/// The task's latest step run (0: it has had none).
pub fn latest_run(w: &World, id: TaskId) -> u64 {
    w.step_runs
        .iter()
        .rev()
        .find(|(_, r)| r.task == id)
        .map_or(0, |(r, _)| r.0)
}

/// Check and emit happen under this, so two retries sent together cannot both pass.
static RETRY_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

async fn retry(
    Shared(d): Shared<Arc<Daemon>>,
    Path(id): Path<u64>,
    body: Option<Json<Value>>,
) -> Res {
    let want = body
        .as_ref()
        .and_then(|b| b["to"].as_str())
        .map(String::from);
    let seen = body.as_ref().and_then(|b| b["seen"].as_u64());
    let restart = body
        .as_ref()
        .and_then(|b| b["restart"].as_bool())
        .unwrap_or(false);
    let d2 = d.clone();
    let (to, step) =
        d.pool
            .run(move || {
                let _gate = RETRY_GATE.lock().unwrap_or_else(|e| e.into_inner());
                let w = d2.world().map_err(store_err)?;
                let t = w
                    .tasks
                    .get(&TaskId(id))
                    .ok_or_else(|| fail(StatusCode::NOT_FOUND, format!("no task #{id}")))?;
                let latest = latest_run(&w, TaskId(id));
                if let Some(seen) = seen {
                    if seen != latest {
                        let now = if t.state.is_step() {
                            "running".to_string()
                        } else {
                            state_name(t.state).to_string()
                        };
                        return Err(fail(
                            StatusCode::CONFLICT,
                            format!("already retried (run {latest} is {now})"),
                        ));
                    }
                } else if t.state.is_step() && !restart {
                    return Err(fail(
                        StatusCode::CONFLICT,
                        format!(
                            "task #{id} is not failed (run {latest} is running); send \"restart\": true (`--restart`) to restart the step"
                        ),
                    ));
                }
                let def = task_def(&d2, t);
                // `to` is a step in the task's definition, or a category (the first step in it).
                let name = want.unwrap_or_else(|| retry_default(&def, t));
                let found = match def.step(&name) {
                    Some(s) => Some((s.category, s.name.clone())),
                    None => parse_state(&name)
                        .and_then(|c| def.first_in(c).map(|s| (c, s.name.clone()))),
                };
                let (to, step) = found.ok_or_else(|| {
                    let names: Vec<&str> = def.steps.iter().map(|s| s.name.as_str()).collect();
                    fail(
                        StatusCode::BAD_REQUEST,
                        format!(
                            "unknown step {name:?}; this task's steps are {}",
                            names.join(", ")
                        ),
                    )
                })?;
                d2.emit(EventKind::Retried {
                    task: TaskId(id),
                    to,
                    step: step.clone(),
                })
                .map_err(store_err)?;
                Ok::<_, (StatusCode, Json<Value>)>((to, step))
            })
            .await
            .map_err(pool_err)??;
    Ok(Json(
        json!({"ok": true, "to": state_name(to), "step": step}),
    ))
}

/// Approve: a proposed task (let it start) or an approval step (let it go on).
async fn approve(
    Shared(d): Shared<Arc<Daemon>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<u64>,
    body: Option<Json<Value>>,
) -> Res {
    // The diff the owner was shown, if the client says (a monitor does): a hold for
    // another diff is refused, as a stale card is.
    let shown = body
        .as_ref()
        .and_then(|b| b["diff_hash"].as_str())
        .filter(|h| !h.is_empty())
        .map(String::from);
    let who = headers
        .get("x-mason-as")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let d2 = d.clone();
    d.pool
        .run(move || {
            if let (Some(shown), Ok(Some(h))) = (&shown, d2.store.hold(id)) {
                if h.approved_by.is_none() && &h.diff_hash != shown {
                    return Err(fail(
                        StatusCode::CONFLICT,
                        format!("the change on task #{id} is not the diff you were shown (it is {}); look again before approving", h.diff_hash),
                    ));
                }
            }
            // A self-change held for the owner is approved by the owner's identity alone.
            match crate::selfguard::approve(&d2, TaskId(id), &who) {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err((code, msg)) => {
                    return Err(fail(
                        StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST),
                        msg,
                    ))
                }
            }
            let w = d2.world().map_err(store_err)?;
            let t = w
                .tasks
                .get(&TaskId(id))
                .ok_or_else(|| fail(StatusCode::NOT_FOUND, format!("no task #{id}")))?;
            if t.state == State::Proposed {
                d2.emit(EventKind::Approved { task: TaskId(id) })
                    .map_err(store_err)?;
                crate::proposals::note_decision(&d2, id, true, "");
                Ok(())
            } else {
                crate::jobs::conclude_approval(
                    &d2,
                    TaskId(id),
                    mason_definition::graph::Outcome::Green,
                    "",
                )
                .map_err(|e| fail(StatusCode::CONFLICT, e))
            }
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(json!({"ok": true})))
}

async fn reject(
    Shared(d): Shared<Arc<Daemon>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<u64>,
    body: Option<Json<Value>>,
) -> Res {
    let who = headers
        .get("x-mason-as")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let reason = body
        .as_ref()
        .and_then(|b| b["reason"].as_str())
        .unwrap_or("no reason given")
        .to_string();
    let d2 = d.clone();
    d.pool
        .run(move || {
            let w = d2.world().map_err(store_err)?;
            let t = w
                .tasks
                .get(&TaskId(id))
                .ok_or_else(|| fail(StatusCode::NOT_FOUND, format!("no task #{id}")))?;
            if t.state == State::Proposed {
                d2.emit(EventKind::Rejected { task: TaskId(id) })
                    .map_err(store_err)?;
                crate::proposals::note_decision(&d2, id, false, &reason);
                Ok(())
            } else if d2
                .store
                .hold(id)
                .ok()
                .flatten()
                .is_some_and(|h| h.approved_by.is_none())
            {
                // Rejecting a held self-change cancels the task: the hold loop then
                // sees the run gone and stops waiting.
                if !crate::attention::is_owner(&d2, &who) {
                    return Err(fail(
                        StatusCode::FORBIDDEN,
                        format!("{who:?} is not the owner's identity; only the owner can reject a change to MASON's own rules"),
                    ));
                }
                crate::attention::intervene(&d2, "self_reject", Some(TaskId(id)), &who, &reason, &reason);
                d2.emit(EventKind::Canceled {
                    task: TaskId(id),
                    delete_branch: false,
                })
                .map_err(store_err)?;
                Ok(())
            } else {
                crate::jobs::conclude_approval(
                    &d2,
                    TaskId(id),
                    mason_definition::graph::Outcome::Red,
                    &reason,
                )
                .map_err(|e| fail(StatusCode::CONFLICT, e))
            }
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(json!({"ok": true})))
}

/// "Explore anyway": exploring after all, before the task starts or after it skipped it.
async fn explore_anyway(Shared(d): Shared<Arc<Daemon>>, Path(id): Path<u64>) -> Res {
    let d2 = d.clone();
    let step = d
        .pool
        .run(move || {
            let w = d2.world().map_err(store_err)?;
            let t = w
                .tasks
                .get(&TaskId(id))
                .ok_or_else(|| fail(StatusCode::NOT_FOUND, format!("no task #{id}")))?;
            let def = d2
                .pinned_def(&t.def)
                .filter(|_| !t.def.is_empty())
                .unwrap_or_else(|| d2.current_def(t.project));
            let first = def.first_in(State::Exploring).ok_or_else(|| {
                fail(
                    StatusCode::CONFLICT,
                    "this definition has no exploring step",
                )
            })?;
            d2.emit(EventKind::ExploreAnyway {
                task: TaskId(id),
                step: first.name.clone(),
            })
            .map_err(store_err)?;
            Ok::<_, (StatusCode, Json<Value>)>(first.name.clone())
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(json!({"ok": true, "step": step})))
}

/// An override for one task only.
async fn task_override(
    Shared(d): Shared<Arc<Daemon>>,
    Path(id): Path<u64>,
    Json(b): Json<Value>,
) -> Res {
    let d2 = d.clone();
    d.pool
        .run(move || {
            let w = d2.world().map_err(store_err)?;
            let t = w
                .tasks
                .get(&TaskId(id))
                .ok_or_else(|| fail(StatusCode::NOT_FOUND, format!("no task #{id}")))?;
            let def = d2
                .pinned_def(&t.def)
                .filter(|_| !t.def.is_empty())
                .unwrap_or_else(|| d2.current_def(t.project));
            let (key, value) = (
                b["key"].as_str().unwrap_or_default(),
                b["value"].as_str().unwrap_or_default(),
            );
            mason_definition::layers::check_override(&def, key, value)
                .map_err(|e| fail(StatusCode::BAD_REQUEST, e))?;
            d2.store
                .set_task_override(
                    id,
                    key,
                    value,
                    b["owner"].as_str().unwrap_or("owner"),
                    now_ms(),
                )
                .map_err(store_err)?;
            Ok::<_, (StatusCode, Json<Value>)>(())
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(json!({"ok": true})))
}

/// `mason task priority <id> urgent|high|normal|low` (owner only): the task's
/// priority, and with it the place of its waiting resource requests, which keep
/// their arrival time. For a task imported from a tracker the tracker's own
/// `priority:*` label is set too (on its next look), so the next sync does not
/// take the priority back.
async fn task_priority(
    Shared(d): Shared<Arc<Daemon>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<u64>,
    Json(b): Json<Value>,
) -> Res {
    let who = who_of(&headers);
    let name = b["priority"].as_str().unwrap_or_default().to_string();
    let (prio, level) = match name.as_str() {
        "urgent" => (Priority::Urgent, 0u8),
        "high" => (Priority::High, 1),
        "normal" => (Priority::Normal, 2),
        "low" => (Priority::Low, 3),
        other => {
            return Err(fail(
                StatusCode::BAD_REQUEST,
                format!("priority must be urgent, high, normal or low, not {other:?}"),
            ))
        }
    };
    let d2 = d.clone();
    let reranked = d
        .pool
        .run(move || {
            owner_only(&d2, &who, "change a task's priority")?;
            let w = d2.world().map_err(store_err)?;
            let t = w
                .tasks
                .get(&TaskId(id))
                .ok_or_else(|| fail(StatusCode::NOT_FOUND, format!("no task #{id}")))?;
            if t.state.is_terminal() {
                return Err(fail(
                    StatusCode::BAD_REQUEST,
                    format!("task #{id} is over; its priority cannot change"),
                ));
            }
            d2.emit(EventKind::TrackerEdited {
                task: TaskId(id),
                title: t.title.clone(),
                body: t.body.clone(),
                priority: prio,
            })
            .map_err(store_err)?;
            if let Some(p) = d2.project(t.project) {
                let row = d2
                    .store
                    .tracker_items(&p.id)
                    .unwrap_or_default()
                    .into_iter()
                    .find(|r| r.task == id);
                if let Some(mut row) = row {
                    row.priority = name.clone();
                    d2.store.put_tracker_item(&row).map_err(store_err)?;
                    d2.tracker.set_owner_priority(t.project, row.number, &name);
                }
            }
            Ok::<_, (StatusCode, Json<Value>)>(d2.broker.set_task_priority(TaskId(id), level))
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(json!({"ok": true, "reranked": reranked})))
}

fn project_of(d: &Daemon, name: &str) -> Result<ProjectId, (StatusCode, Json<Value>)> {
    d.project_id_by_name(name)
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, format!("no project {name:?}")))
}

async fn project_definition(Shared(d): Shared<Arc<Daemon>>, Path(name): Path<String>) -> Res {
    let pid = project_of(&d, &name)?;
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let def = d2.current_def(pid);
            json!({
                "hash": def.hash,
                "origin": d2.current_origin(pid).label(),
                "start": def.start,
                "explored_entry": def.explored_entry,
                "steps": def.steps.iter().map(|s| json!({
                    "name": s.name, "type": s.kind.name(), "category": mason_definition::model::category_name(s.category),
                    "next": s.next, "on_red": s.on_red, "max_rounds": s.max_rounds,
                })).collect::<Vec<_>>(),
                "error": d2.definition_error(pid),
            })
        })
        .await
        .map_err(pool_err)?;
    Ok(Json(v))
}

// ---- the graphical editor (Part 18) -----------------------------------------------------------

fn who_of(h: &axum::http::HeaderMap) -> String {
    h.get("x-mason-as")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// The canvas model of a project's draft (`?step=NAME` selects a step for the inspector).
async fn editor_get(
    Shared(d): Shared<Arc<Daemon>>,
    Path(name): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Res {
    let pid = project_of(&d, &name)?;
    let step = q.get("step").cloned().unwrap_or_default();
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || crate::editor::model(&d2, pid, &step))
        .await
        .map_err(pool_err)?;
    Ok(Json(v))
}

/// One operation on the draft. Refused edits answer 422 with the reasons.
async fn editor_op(
    Shared(d): Shared<Arc<Daemon>>,
    Path(name): Path<String>,
    Json(b): Json<Value>,
) -> Res {
    let pid = project_of(&d, &name)?;
    let d2 = d.clone();
    let r = d
        .pool
        .run(move || crate::editor::operate(&d2, pid, &b))
        .await
        .map_err(pool_err)?;
    d.poke();
    match r {
        Ok(()) => Ok(Json(json!({"ok": true}))),
        Err(reasons) => Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": reasons.join("; "), "reasons": reasons})),
        )),
    }
}

async fn editor_history(
    Shared(d): Shared<Arc<Daemon>>,
    Path((name, what)): Path<(String, String)>,
) -> Res {
    let pid = project_of(&d, &name)?;
    let d2 = d.clone();
    let changed = d
        .pool
        .run(move || match what.as_str() {
            "undo" => Ok(crate::editor::undo(&d2, pid)),
            "redo" => Ok(crate::editor::redo(&d2, pid)),
            "discard" => {
                crate::editor::discard(&d2, pid);
                Ok(true)
            }
            _ => Err(()),
        })
        .await
        .map_err(pool_err)?
        .map_err(|()| fail(StatusCode::NOT_FOUND, "not found"))?;
    d.poke();
    Ok(Json(json!({"ok": true, "changed": changed})))
}

/// Walk scripted tasks through the draft (the standard set, or the one in the body).
async fn editor_simulate(
    Shared(d): Shared<Arc<Daemon>>,
    Path(name): Path<String>,
    body: Option<Json<Value>>,
) -> Res {
    let pid = project_of(&d, &name)?;
    let d2 = d.clone();
    let custom = body.map(|b| b.0).filter(|b| {
        b.get("results").is_some() || b.get("explored").is_some() || b.get("clear").is_some()
    });
    let v = d
        .pool
        .run(move || crate::editor::simulate(&d2, pid, custom.as_ref()))
        .await
        .map_err(pool_err)?
        .map_err(|e| fail(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    d.poke();
    Ok(Json(v))
}

/// Commit the draft to the base branch (or open a pull request). Owner only.
async fn editor_apply(
    Shared(d): Shared<Arc<Daemon>>,
    headers: axum::http::HeaderMap,
    Path(name): Path<String>,
    body: Option<Json<Value>>,
) -> Res {
    let pid = project_of(&d, &name)?;
    let who = who_of(&headers);
    let message = body
        .as_ref()
        .and_then(|b| b["message"].as_str())
        .unwrap_or_default()
        .to_string();
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || crate::editor::apply(&d2, pid, &who, &message))
        .await
        .map_err(pool_err)?
        .map_err(|(code, msg)| {
            fail(
                StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST),
                msg,
            )
        })?;
    Ok(Json(v))
}

/// The project's extensions and whether each may run (Part 12).
async fn extensions_list(Shared(d): Shared<Arc<Daemon>>, Path(name): Path<String>) -> Res {
    let pid = project_of(&d, &name)?;
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            d2.project(pid)
                .map(|p| crate::plugins::list(&d2, p))
                .unwrap_or_default()
        })
        .await
        .map_err(pool_err)?;
    Ok(Json(v))
}

/// Approve the extension's current content. Owner only.
async fn extension_approve(
    Shared(d): Shared<Arc<Daemon>>,
    headers: axum::http::HeaderMap,
    Path((name, ext)): Path<(String, String)>,
) -> Res {
    let pid = project_of(&d, &name)?;
    let who = who_of(&headers);
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || match d2.project(pid) {
            Some(p) => crate::plugins::approve(&d2, p, &ext, &who),
            None => Err((404, "no such project".to_string())),
        })
        .await
        .map_err(pool_err)?
        .map_err(|(code, msg)| {
            fail(
                StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST),
                msg,
            )
        })?;
    Ok(Json(v))
}

async fn project_reload(Shared(d): Shared<Arc<Daemon>>, Path(name): Path<String>) -> Res {
    let pid = project_of(&d, &name)?;
    let d2 = d.clone();
    let r = d
        .pool
        .run(move || d2.reload_definition(pid))
        .await
        .map_err(pool_err)?;
    Ok(Json(match r {
        crate::defs::Reload::Unchanged => json!({"result": "unchanged"}),
        crate::defs::Reload::Applied { hash, origin } => {
            json!({"result": "applied", "hash": hash, "origin": origin})
        }
        crate::defs::Reload::Refused(errors) => json!({"result": "refused", "errors": errors}),
    }))
}

/// Re-read the project's registration file for tasks admitted from now on.
async fn registration_reload(Shared(d): Shared<Arc<Daemon>>, Path(name): Path<String>) -> Res {
    project_of(&d, &name)?;
    let d2 = d.clone();
    let r = d
        .pool
        .run(move || d2.reload_registration(&name))
        .await
        .map_err(pool_err)?;
    Ok(Json(match r {
        Ok(changed) if changed.is_empty() => json!({"result": "unchanged"}),
        Ok(changed) => json!({"result": "applied", "changed": changed}),
        Err(errors) => json!({"result": "refused", "errors": errors}),
    }))
}

/// Every step's settings after the five layers, and where each came from.
async fn project_config(Shared(d): Shared<Arc<Daemon>>, Path(name): Path<String>) -> Res {
    let pid = project_of(&d, &name)?;
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let def = d2.current_def(pid);
            let overlay = d2.overlay(pid);
            let now = now_ms();
            let runtime: Vec<mason_definition::layers::RuntimeOverride> = d2
                .store
                .overrides(&name, now)
                .unwrap_or_default()
                .into_iter()
                .map(|o| mason_definition::layers::RuntimeOverride { key: o.key, value: o.value, owner: o.owner, expires_at: o.expires_at })
                .collect();
            let attr = |a: Option<(String, mason_definition::layers::Source)>| a.map_or(Value::Null, |(v, s)| json!({"value": v, "from": s.label()}));
            let steps: Vec<Value> = def
                .steps
                .iter()
                .map(|s| match mason_definition::layers::resolve(&def, s, &overlay, &runtime, &[], now) {
                    Ok(r) => json!({
                        "step": s.name, "type": s.kind.name(),
                        "tier": attr(r.tier.map(|a| (a.value.name().to_string(), a.source))),
                        "level": attr(r.level.map(|a| (a.value.name().to_string(), a.source))),
                        "model": attr(r.model.map(|a| (a.value, a.source))),
                        "effort": attr(r.effort.map(|a| (a.value, a.source))),
                        "budget": attr(r.budget.map(|a| (format!("{}s", a.value.as_secs()), a.source))),
                        "max_rounds": attr(r.max_rounds.map(|a| (a.value.to_string(), a.source))),
                        "command": attr(r.command.map(|a| (a.value, a.source))),
                        "build": attr(r.build.map(|a| (a.value, a.source))),
                    }),
                    Err(e) => json!({"step": s.name, "error": e.iter().map(|p| p.to_string()).collect::<Vec<_>>()}),
                })
                .collect();
            json!({"definition": def.hash, "steps": steps})
        })
        .await
        .map_err(pool_err)?;
    Ok(Json(v))
}

async fn overrides_list(Shared(d): Shared<Arc<Daemon>>, Path(name): Path<String>) -> Res {
    project_of(&d, &name)?;
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            d2.store.overrides(&name, now_ms()).map(|os| {
                json!({"overrides": os.iter().map(|o| json!({"key": o.key, "value": o.value, "owner": o.owner, "expires_at": o.expires_at})).collect::<Vec<_>>()})
            })
        })
        .await
        .map_err(pool_err)?
        .map_err(store_err)?;
    Ok(Json(v))
}

/// Set a runtime override. It needs an owner and an expiry, and it can only
/// set a step attribute, never change the graph.
async fn overrides_add(
    Shared(d): Shared<Arc<Daemon>>,
    Path(name): Path<String>,
    Json(b): Json<Value>,
) -> Res {
    let pid = project_of(&d, &name)?;
    let d2 = d.clone();
    d.pool
        .run(move || {
            let (key, value) = (
                b["key"].as_str().unwrap_or_default(),
                b["value"].as_str().unwrap_or_default(),
            );
            let owner = b["owner"].as_str().unwrap_or_default();
            let secs = b["expires_in_s"].as_u64().ok_or_else(|| {
                fail(
                    StatusCode::BAD_REQUEST,
                    "an override must say when it expires (expires_in_s)",
                )
            })?;
            mason_definition::layers::check_override(&d2.current_def(pid), key, value)
                .map_err(|e| fail(StatusCode::BAD_REQUEST, e))?;
            let now = now_ms();
            d2.store
                .add_override(&name, key, value, owner, now, now + secs * 1000)
                .map_err(|e| fail(StatusCode::BAD_REQUEST, e.to_string()))?;
            Ok::<_, (StatusCode, Json<Value>)>(())
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(json!({"ok": true})))
}

async fn overrides_remove(
    Shared(d): Shared<Arc<Daemon>>,
    Path(name): Path<String>,
    Json(b): Json<Value>,
) -> Res {
    project_of(&d, &name)?;
    let d2 = d.clone();
    let n = d
        .pool
        .run(move || {
            d2.store
                .remove_override(&name, b["key"].as_str().unwrap_or_default())
        })
        .await
        .map_err(pool_err)?
        .map_err(store_err)?;
    Ok(Json(json!({"removed": n})))
}

/// Every successful override leaves a record, whichever client made it: the
/// "interventions per landed task" metric counts these (Part 10).
async fn record_interventions(
    Shared(d): Shared<Arc<Daemon>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let post = req.method() == axum::http::Method::POST;
    let path = req.uri().path().to_string();
    let (who, reason) = {
        let get = |h: &axum::http::HeaderMap, name: &str| {
            h.get(name).and_then(|v| v.to_str().ok()).map(String::from)
        };
        let h = req.headers();
        (
            get(h, "x-mason-as")
                .unwrap_or_else(|| d.cfg.owner_names.first().cloned().unwrap_or_default()),
            get(h, "x-mason-reason").unwrap_or_default(),
        )
    };
    let resp = next.run(req).await;
    if post && resp.status().is_success() {
        if let Some((kind, task)) = override_kind(&path) {
            let d2 = d.clone();
            let _ = d.pool.spawn(move || {
                crate::attention::intervene(&d2, kind, task.map(TaskId), &who, &path, &reason)
            });
        }
    }
    resp
}

/// Which override a POST is, and its task. (Answers record themselves.)
fn override_kind(path: &str) -> Option<(&'static str, Option<u64>)> {
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    match parts.as_slice() {
        ["tasks", id, what] => {
            let kind = match *what {
                "cancel" => "cancel",
                "retry" => "retry",
                "approve" => "approve",
                "reject" => "reject",
                "explore-anyway" => "explore_anyway",
                "overrides" => "task_override",
                "priority" => "task_priority",
                "skip" => "skip_test",
                "land-now" => "land_now",
                "force" => "force",
                _ => return None,
            };
            Some((kind, id.parse().ok()))
        }
        ["pause"] => Some(("pause", None)),
        ["resources", "gate"] => Some(("gate", None)),
        ["resume"] => Some(("resume", None)),
        ["projects", _, "overrides"] => Some(("override", None)),
        _ => None,
    }
}

fn debt_json(x: &mason_store::DebtRow) -> Value {
    json!({
        "id": x.id, "task": x.task, "project": x.project, "scope": x.scope,
        "reason": x.reason, "owner": x.owner, "deferred": x.deferred,
        "status": x.status, "at": x.at,
    })
}

/// Force-skip a suite or a step, now or ahead of time. Records a debt.
async fn skip_task(
    Shared(d): Shared<Arc<Daemon>>,
    Path(id): Path<u64>,
    Json(b): Json<Value>,
) -> Res {
    let scope = match (b["suite"].as_str(), b["step"].as_str()) {
        (Some(s), None) => format!("suite:{s}"),
        (None, Some(s)) => format!("step:{s}"),
        _ => {
            return Err(fail(
                StatusCode::BAD_REQUEST,
                "name one `suite` or one `step` to skip",
            ))
        }
    };
    let (reason, owner) = (
        b["reason"].as_str().unwrap_or_default().to_string(),
        b["owner"].as_str().unwrap_or_default().to_string(),
    );
    let deferred = b["deferred"].as_bool().unwrap_or(false);
    let d2 = d.clone();
    let debt = d
        .pool
        .run(move || crate::suites::skip(&d2, TaskId(id), &scope, &reason, &owner, deferred))
        .await
        .map_err(pool_err)?
        .map_err(|e| fail(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({"debt": debt})))
}

/// Batches, freezes, queue idle time and what is waiting, per project.
async fn landing_status(Shared(d): Shared<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let projects: Vec<Value> = d2
                .projects()
                .iter()
                .map(|(pid, p)| {
                    json!({
                        "project": p.id,
                        "config": match crate::landing::load_config(&d2, *pid) {
                            Ok(c) => json!({
                                "mode": c.mode.name(),
                                "window_s": c.window.as_secs(),
                                "max_size": c.max_size,
                                "parallel": c.parallel,
                            }),
                            Err(e) => json!({"error": e}),
                        },
                        "queued": d2.landing().queued(*pid).iter().map(|t| t.0).collect::<Vec<_>>(),
                        "running_batches": d2.landing().running_batches(*pid),
                        "queue_idle_ms": d2.landing().idle_ms(*pid),
                    })
                })
                .collect();
            let batches: Vec<Value> = d2
                .store
                .batches(None, 50)
                .unwrap_or_default()
                .iter()
                .map(|b| {
                    json!({
                        "id": b.id, "project": b.project, "phase": b.phase, "tasks": b.tasks,
                        "landed": b.landed, "parked": b.parked, "note": b.note, "head": b.head,
                        "at": b.at, "finished_at": b.finished_at,
                    })
                })
                .collect();
            let freezes: Vec<Value> = d2
                .store
                .freezes()
                .unwrap_or_default()
                .iter()
                .map(|f| json!({"project": f.project, "reason": f.reason, "fix_task": f.fix_task, "since": f.since, "source": f.source}))
                .collect();
            json!({"projects": projects, "batches": batches, "freezes": freezes})
        })
        .await
        .map_err(pool_err)?;
    Ok(Json(v))
}

/// Where the daemon's own code stands: the runtime it runs from, the runtimes it
/// has, any rollback alert, and the update in progress.
async fn update_status(Shared(d): Shared<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let rt = crate::update::Runtimes::new(&d2.cfg.data_dir);
            let s = d2.update_status.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let c = d2.update_check.lock().unwrap_or_else(|e| e.into_inner()).clone();
            json!({
                "latest": c.latest, "behind": c.behind, "outdated": c.outdated,
                "checked_at_ms": c.at_ms, "check_error": c.error,
                "running": d2.cfg.runtime_sha,
                "current": rt.current(),
                "previous": rt.previous(),
                "bad": rt.bad(),
                "alert": rt.alert(),
                "channel": crate::update::UpdateCfg::load(&d2.cfg.config_dir).ok().map(|c| c.channel.name()),
                "phase": s.phase, "target": s.target, "error": s.error, "log": s.log,
            })
        })
        .await
        .map_err(pool_err)?;
    Ok(Json(v))
}

/// Start an update: it builds, proves itself and takes over in the background.
/// `{"channel": .., "sha": .., "rollback": bool, "check": bool}`.
async fn monitor_update_start(Shared(d): Shared<Arc<Daemon>>, Json(b): Json<Value>) -> Res {
    let kind =
        crate::update::monitor_kind(b["kind"].as_str().unwrap_or_default()).ok_or_else(|| {
            fail(
                StatusCode::BAD_REQUEST,
                "a monitor kind is `swift` or `compose`",
            )
        })?;
    crate::update::start_monitor_build(&d, kind).map_err(|e| fail(StatusCode::CONFLICT, e))?;
    Ok(Json(json!({"started": true})))
}

async fn update_start(Shared(d): Shared<Arc<Daemon>>, Json(b): Json<Value>) -> Res {
    let channel = match b["channel"].as_str() {
        Some(c) => {
            Some(crate::update::Channel::parse(c).map_err(|e| fail(StatusCode::BAD_REQUEST, e))?)
        }
        None => None,
    };
    let sha = b["sha"].as_str().map(String::from);
    if sha.as_deref().is_some_and(|s| {
        s.is_empty()
            || s.starts_with('-')
            || !s
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "/_.-".contains(c))
    }) {
        return Err(fail(StatusCode::BAD_REQUEST, "that is not a commit name"));
    }
    let req = crate::update::Request {
        channel,
        sha,
        rollback: b["rollback"].as_bool().unwrap_or(false),
        check_only: b["check"].as_bool().unwrap_or(false),
        now: b["now"].as_bool().unwrap_or(false),
    };
    if matches!(
        d.update_status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .phase
            .as_str(),
        "fetching" | "building" | "testing" | "waiting for landing" | "taking over"
    ) {
        return Err(fail(
            StatusCode::CONFLICT,
            "an update is already in progress",
        ));
    }
    let d2 = d.clone();
    let check = req.check_only;
    let now = req.now;
    let _ = std::thread::Builder::new()
        .name("update".into())
        .spawn(move || {
            if let Ok(sha) = crate::update::run(&d2, &req) {
                if !check {
                    // Let the answer to `mason update` and any open request leave first.
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    crate::update::drain_and_take_over(&d2, &sha, now);
                }
            }
        });
    Ok(Json(json!({"started": true})))
}

/// The owner has seen the rollback alert.
async fn update_ack(Shared(d): Shared<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    d.pool
        .run(move || {
            crate::update::Runtimes::new(&d2.cfg.data_dir).take_alert();
            d2.poke();
        })
        .await
        .map_err(pool_err)?;
    Ok(Json(json!({"acknowledged": true})))
}

/// MASON working on itself: the limits and where they stand, and the changes held for you.
async fn self_status(Shared(d): Shared<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let now = now_ms();
            let projects: Vec<Value> = d2
                .projects()
                .iter()
                .filter_map(|(pid, p)| p.self_cfg.as_ref().map(|c| (pid, p, c)))
                .map(|(pid, p, c)| {
                    let u = crate::selfguard::usage(&d2, *pid, c, now);
                    let s = u.status.clone();
                    json!({
                        "project": p.id,
                        "open": s.open, "held": s.held, "max_open": c.max_open,
                        "week": s.week, "per_week": c.per_week, "yours_not_counted": u.yours, "yours_open": u.yours_open,
                        "steps": s.steps, "yours_steps": u.yours_steps, "max_steps": c.max_steps, "period_h": c.period_h,
                        "cooldown_until_ms": s.cooldown_until_ms,
                        "blocked": crate::selfguard::check_as(&s, u.yours, c, TrustSource::Owner).err(),
                        "protect_extra": c.protect,
                    })
                })
                .collect();
            let holds: Vec<Value> = d2
                .store
                .holds()
                .unwrap_or_default()
                .iter()
                .map(|h| json!({"task": h.task, "reasons": h.reasons, "since": h.at, "approved_by": h.approved_by}))
                .collect();
            json!({"projects": projects, "holds": holds})
        })
        .await
        .map_err(pool_err)?;
    Ok(Json(v))
}

/// A webhook from GitHub (directly, or through a relay such as `gh webhook
/// forward`). Only a delivery whose signature verifies, under the secret the
/// project names, is acted on, and acting is only to look again now.
async fn github_hook(
    Shared(d): Shared<Arc<Daemon>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Res {
    let sig = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let repo = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|v| v["repository"]["full_name"].as_str().map(String::from))
        .unwrap_or_default();
    let d2 = d.clone();
    let hit = d
        .pool
        .run(move || {
            for (pid, p) in &d2.projects() {
                let Some(t) = &p.services.tracker else {
                    continue;
                };
                if t.repo != repo {
                    continue;
                }
                let Some(secret) = t
                    .webhook_secret
                    .as_deref()
                    .and_then(|r| crate::services::token(r).ok())
                    .and_then(|r| r.read().ok())
                else {
                    continue;
                };
                if mason_adapter_api::webhook::verify(&secret, &body, &sig) {
                    d2.tracker.refresh(*pid);
                    return true;
                }
            }
            false
        })
        .await
        .map_err(pool_err)?;
    if hit {
        Ok(Json(json!({"refreshing": true})))
    } else {
        // Unknown repository, no secret, or a signature that does not verify: all the same answer.
        Err(fail(
            StatusCode::UNAUTHORIZED,
            "the delivery was not verified",
        ))
    }
}

/// Ask the tracker now (what opening a task in a monitor does), and say what it changed.
async fn refresh_tracker(Shared(d): Shared<Arc<Daemon>>, Path(id): Path<u64>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let w = d2.world().map_err(store_err)?;
            let t = w.tasks.get(&TaskId(id)).ok_or_else(|| fail(StatusCode::NOT_FOUND, format!("no task #{id}")))?;
            let pid = t.project;
            let p = d2.project(pid).ok_or_else(|| fail(StatusCode::NOT_FOUND, "no such project"))?;
            let Some((adapter, cfg)) = crate::services::tracker(p).map_err(|e| fail(StatusCode::BAD_REQUEST, e))? else {
                let cfg = d2.cfg.config_dir.join("projects").join(format!("{}.toml", p.id));
                return Ok(json!({
                    "tracker": false,
                    "config": cfg.display().to_string(),
                    "note": format!("{} has no tracker: nothing is imported from its issues until a [tracker] section is added to {} (see docs/self-hosting.md)", p.id, cfg.display()),
                }));
            };
            d2.tracker.refresh(pid);
            match crate::tracker::poll_once(&d2, pid, adapter.as_ref(), &cfg) {
                // A look that found nothing new (the background poll it woke may just have
                // fetched the listing) says how things stand, not zeros.
                Ok(s) => {
                    let s = if s == crate::tracker::Summary::default() {
                        d2.tracker.standing(pid).unwrap_or(s)
                    } else {
                        s
                    };
                    Ok(json!({"tracker": true, "imported": s.imported, "updated": s.updated, "closed": s.closed, "ignored": s.ignored, "unchanged": s.unchanged, "waiting_slot": s.refused, "adopted": s.adopted}))
                }
                Err(e) => Ok(json!({"tracker": true, "error": e})),
            }
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(v))
}

/// Open issues the tracker has not imported (or whose task was canceled), with why.
async fn pending_issues(
    Shared(d): Shared<Arc<Daemon>>,
    Query(q): Query<HashMap<String, String>>,
) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let mut out = vec![];
            for (pid, p) in &d2.projects() {
                if q.get("project").is_some_and(|n| *n != p.id) {
                    continue;
                }
                for w in d2.tracker.pending(*pid) {
                    out.push(json!({"project": p.id, "number": w.number, "title": w.title, "reason": w.reason}));
                }
            }
            json!({"pending": out})
        })
        .await
        .map_err(pool_err)?;
    Ok(Json(v))
}

/// Tie a live task to an open issue whose task was canceled. Owner only.
async fn relink_task(
    Shared(d): Shared<Arc<Daemon>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<u64>,
    Json(b): Json<Value>,
) -> Res {
    let who = headers
        .get("x-mason-as")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let d2 = d.clone();
    d.pool
        .run(move || {
            if !crate::attention::is_owner(&d2, &who) {
                return Err(fail(
                    StatusCode::FORBIDDEN,
                    format!("{who:?} is not the owner's identity; only the owner can link a task to an issue"),
                ));
            }
            let issue = b["issue"].as_u64().filter(|n| *n > 0).ok_or_else(|| {
                fail(StatusCode::BAD_REQUEST, "issue must be an issue number")
            })?;
            let w = d2.world().map_err(store_err)?;
            let t = w
                .tasks
                .get(&TaskId(id))
                .ok_or_else(|| fail(StatusCode::NOT_FOUND, format!("no task #{id}")))?;
            crate::tracker::relink(&d2, t.project, issue, TaskId(id))
                .map_err(|e| fail(StatusCode::CONFLICT, e))?;
            crate::attention::intervene(&d2, "relink", Some(TaskId(id)), &who, "", &format!("issue #{issue}"));
            Ok(())
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(json!({"ok": true})))
}

fn who_header(headers: &axum::http::HeaderMap) -> String {
    headers
        .get("x-mason-as")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

fn blocker_ids(v: &Value, what: &str) -> Result<Vec<TaskId>, (StatusCode, Json<Value>)> {
    match v {
        Value::Null => Ok(vec![]),
        Value::Array(a) => a
            .iter()
            .map(|x| {
                x.as_u64().map(TaskId).ok_or_else(|| {
                    fail(
                        StatusCode::BAD_REQUEST,
                        format!("{what}: {x} is not a task id"),
                    )
                })
            })
            .collect(),
        _ => Err(fail(
            StatusCode::BAD_REQUEST,
            format!("{what} must be a list of task ids"),
        )),
    }
}

fn refusal(r: crate::attention::Refusal) -> (StatusCode, Json<Value>) {
    fail(
        StatusCode::from_u16(r.0).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        r.1,
    )
}

/// Release a task from some or all of its blockers. Owner only.
async fn unblock_task(
    Shared(d): Shared<Arc<Daemon>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<u64>,
    body: Option<Json<Value>>,
) -> Res {
    let who = who_header(&headers);
    let b = body.map_or(Value::Null, |j| j.0);
    let from = blocker_ids(&b["from"], "from")?;
    let which = if b["all"].as_bool().unwrap_or(false) || from.is_empty() {
        crate::blockers::Which::All
    } else {
        crate::blockers::Which::From(from)
    };
    let d2 = d.clone();
    let msg = d
        .pool
        .run(move || crate::blockers::release(&d2, &who, TaskId(id), which))
        .await
        .map_err(pool_err)?
        .map_err(refusal)?;
    Ok(Json(json!({"ok": true, "message": msg})))
}

/// Add blockers to a task that has not started. Owner only.
async fn block_task(
    Shared(d): Shared<Arc<Daemon>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<u64>,
    Json(b): Json<Value>,
) -> Res {
    let who = who_header(&headers);
    let after = blocker_ids(&b["after"], "after")?;
    let d2 = d.clone();
    let msg = d
        .pool
        .run(move || crate::blockers::block(&d2, &who, TaskId(id), &after))
        .await
        .map_err(pool_err)?
        .map_err(refusal)?;
    Ok(Json(json!({"ok": true, "message": msg})))
}

/// Put a canceled task whose branch was kept back in the queue. Owner only.
async fn revive_task(
    Shared(d): Shared<Arc<Daemon>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<u64>,
) -> Res {
    let who = who_header(&headers);
    let d2 = d.clone();
    let msg = d
        .pool
        .run(move || crate::blockers::revive(&d2, &who, TaskId(id)))
        .await
        .map_err(pool_err)?
        .map_err(refusal)?;
    Ok(Json(json!({"ok": true, "message": msg})))
}

async fn debts_list(
    Shared(d): Shared<Arc<Daemon>>,
    Query(q): Query<HashMap<String, String>>,
) -> Res {
    let open = q.get("open").is_some_and(|v| v != "0");
    let d2 = d.clone();
    let rows = d
        .pool
        .run(move || d2.store.debts(None, open))
        .await
        .map_err(pool_err)?
        .map_err(store_err)?;
    Ok(Json(
        json!({"debts": rows.iter().map(debt_json).collect::<Vec<_>>()}),
    ))
}

/// Pay a debt by running the skipped suite on the base branch now.
async fn debt_run(Shared(d): Shared<Arc<Daemon>>, Path(id): Path<u64>) -> Res {
    let d2 = d.clone();
    let r = d
        .pool
        .run(move || crate::suites::pay(&d2, id))
        .await
        .map_err(pool_err)?;
    match r {
        Ok(m) => Ok(Json(json!({"paid": true, "detail": m}))),
        Err(e) => Ok(Json(json!({"paid": false, "detail": e}))),
    }
}

async fn quarantine_list(Shared(d): Shared<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    let (rows, flaky) = d
        .pool
        .run(move || {
            let now = now_ms();
            (
                d2.store.quarantined(now).unwrap_or_default(),
                d2.store
                    .flaky_since(now.saturating_sub(7 * 86_400_000))
                    .unwrap_or_default(),
            )
        })
        .await
        .map_err(pool_err)?;
    let candidates =
        mason_testing::gates::quarantine_candidates(&flaky, now_ms(), 7 * 86_400_000, 3);
    Ok(Json(json!({
        "quarantined": rows.iter().map(|(s, t, o, e, r)| json!({"suite": s, "test": t, "owner": o, "expires_at": e, "reason": r})).collect::<Vec<_>>(),
        "candidates": candidates.iter().map(|(s, t)| json!({"suite": s, "test": t})).collect::<Vec<_>>(),
    })))
}

/// Quarantine a test: always with an owner and an expiry.
async fn quarantine_add(Shared(d): Shared<Arc<Daemon>>, Json(b): Json<Value>) -> Res {
    let now = now_ms();
    let q = mason_testing::gates::Quarantine {
        suite: b["suite"].as_str().unwrap_or_default().to_string(),
        test: b["test"].as_str().unwrap_or_default().to_string(),
        owner: b["owner"].as_str().unwrap_or_default().to_string(),
        expires_at_ms: now + b["days"].as_u64().unwrap_or(0) * 86_400_000,
        reason: b["reason"].as_str().unwrap_or_default().to_string(),
    };
    if q.suite.is_empty() || q.test.is_empty() {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "name the `suite` and the `test`",
        ));
    }
    mason_testing::gates::check_quarantine(&q, now, 90)
        .map_err(|e| fail(StatusCode::BAD_REQUEST, e))?;
    let d2 = d.clone();
    d.pool
        .run(move || {
            d2.store
                .put_quarantine(&q.suite, &q.test, &q.owner, q.expires_at_ms, &q.reason, now)
        })
        .await
        .map_err(pool_err)?
        .map_err(store_err)?;
    Ok(Json(json!({"ok": true})))
}

async fn quarantine_remove(Shared(d): Shared<Arc<Daemon>>, Json(b): Json<Value>) -> Res {
    let (s, t) = (
        b["suite"].as_str().unwrap_or_default().to_string(),
        b["test"].as_str().unwrap_or_default().to_string(),
    );
    let d2 = d.clone();
    let n = d
        .pool
        .run(move || d2.store.remove_quarantine(&s, &t))
        .await
        .map_err(pool_err)?
        .map_err(store_err)?;
    Ok(Json(json!({"removed": n})))
}

/// Answer a task's open question. The answer carries the hash of the card the
/// answerer was looking at, and counts only from the owner's identity.
async fn answer_task(
    Shared(d): Shared<Arc<Daemon>>,
    Path(id): Path<u64>,
    Json(b): Json<Value>,
) -> Res {
    let (answer, hash, who) = (
        b["answer"].as_str().unwrap_or_default().to_string(),
        b["hash"].as_str().unwrap_or_default().to_string(),
        b["as"].as_str().unwrap_or_default().to_string(),
    );
    let d2 = d.clone();
    let r = d
        .pool
        .run(move || crate::attention::answer(&d2, TaskId(id), &answer, &hash, &who))
        .await
        .map_err(pool_err)?;
    match r {
        Ok(text) => Ok(Json(json!({"ok": true, "answer": text}))),
        Err((code, msg)) => Err(fail(
            StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST),
            msg,
        )),
    }
}

/// Land now (its own batch, without waiting for the window) and force (no
/// verification, past a freeze). Each is recorded as an override and leaves a
/// debt; the landing engine reads the mark when the task is in the queue.
async fn land_override(
    Shared(d): Shared<Arc<Daemon>>,
    Path((id, kind)): Path<(u64, String)>,
    body: Option<Json<Value>>,
) -> Res {
    let kind = match kind.as_str() {
        "land-now" | "force" => kind,
        _ => return Err(fail(StatusCode::NOT_FOUND, "not found")),
    };
    let reason = body
        .as_ref()
        .and_then(|b| b["reason"].as_str())
        .unwrap_or_default()
        .to_string();
    let who = body
        .as_ref()
        .and_then(|b| b["as"].as_str())
        .map(String::from)
        .unwrap_or_else(|| d.cfg.owner_names.first().cloned().unwrap_or_default());
    let d2 = d.clone();
    let debt = d
        .pool
        .run(move || {
            let w = d2.world().map_err(store_err)?;
            let t = w
                .tasks
                .get(&TaskId(id))
                .ok_or_else(|| fail(StatusCode::NOT_FOUND, format!("no task #{id}")))?;
            let project = d2
                .project(t.project)
                .map_or(String::new(), |p| p.id.clone());
            let flag = if kind == "force" { "force" } else { "now" };
            // A mark is for a task that has yet to land.
            if !matches!(t.state, State::Landed | State::Canceled) {
                d2.store
                    .set_landing_flag(id, flag, &who, now_ms())
                    .map_err(store_err)?;
            }
            let debt = d2
                .store
                .add_debt(
                    id,
                    &project,
                    &format!("landing:{}", kind.replace('-', "_")),
                    &reason,
                    &who,
                    false,
                    now_ms(),
                )
                .map_err(store_err)?;
            d2.landing.wake();
            d2.poke();
            Ok(debt)
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(json!({
        "debt": debt,
        "applied": true,
        "note": "marked; the landing engine acts on it when the task is in the landing step, and the debt stays open",
    })))
}

async fn open_session(Shared(d): Shared<Arc<Daemon>>, Path(id): Path<u64>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let v = crate::attention::session_for(&d2, TaskId(id));
            if v.is_ok() {
                let who = d2.cfg.owner_names.first().cloned().unwrap_or_default();
                crate::attention::intervene(&d2, "open_session", Some(TaskId(id)), &who, "", "");
            }
            v
        })
        .await
        .map_err(pool_err)?
        .map_err(|e| fail(StatusCode::NOT_FOUND, e))?;
    Ok(Json(v))
}

/// The keys that could be used for the project's remote (owner only).
async fn ssh_keys_list(
    Shared(d): Shared<Arc<Daemon>>,
    headers: axum::http::HeaderMap,
    Path(name): Path<String>,
) -> Res {
    owner_only(&d, &who_of(&headers), "list SSH keys")?;
    project_of(&d, &name)?;
    let d2 = d.clone();
    let keys = d
        .pool
        .run(move || crate::ssh_keys::list(&d2, &name))
        .await
        .map_err(pool_err)?
        .map_err(|e| fail(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({
        "keys": keys.iter().map(|k| k.to_json()).collect::<Vec<_>>(),
        "host": hostname(),
    })))
}

fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Use a key for the project's remote: write the repository's `core.sshCommand`, unlock the key
/// (a dialog on this host), probe. A probe that gets through lifts the hold. No passphrase is
/// ever a field of this request.
async fn ssh_key_choose(
    Shared(d): Shared<Arc<Daemon>>,
    headers: axum::http::HeaderMap,
    Path(name): Path<String>,
    Json(b): Json<Value>,
) -> Res {
    let who = who_of(&headers);
    owner_only(&d, &who, "choose an SSH key")?;
    project_of(&d, &name)?;
    let path = b["path"].as_str().unwrap_or_default().to_string();
    if path.is_empty() {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "path is the private key's file",
        ));
    }
    let d2 = d.clone();
    let n2 = name.clone();
    let p2 = path.clone();
    let r = d
        .pool
        .run(move || crate::ssh_keys::choose(&d2, &n2, &p2))
        .await
        .map_err(pool_err)?;
    match r {
        Ok(v) => {
            crate::attention::intervene(&d, "ssh_key", None, &who, &format!("{name}: {path}"), "");
            Ok(Json(v))
        }
        Err(e) => Err(fail(StatusCode::BAD_GATEWAY, e)),
    }
}

async fn pause_set(Shared(d): Shared<Arc<Daemon>>, Json(b): Json<Value>) -> Res {
    let project = b["project"].as_str().unwrap_or_default().to_string();
    let mode = b["mode"].as_str().unwrap_or("all").to_string();
    let owner = b["owner"].as_str().unwrap_or_default().to_string();
    let secs = b["for_s"].as_u64().unwrap_or(0);
    let d2 = d.clone();
    d.pool
        .run(move || crate::attention::pause(&d2, &project, &mode, &owner, secs))
        .await
        .map_err(pool_err)?
        .map_err(|e| fail(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({"ok": true})))
}

async fn pause_clear(Shared(d): Shared<Arc<Daemon>>, Json(b): Json<Value>) -> Res {
    let project = b["project"].as_str().unwrap_or_default().to_string();
    let mode = b["mode"].as_str().unwrap_or_default().to_string();
    let d2 = d.clone();
    let n = d
        .pool
        .run(move || crate::attention::resume(&d2, &project, &mode))
        .await
        .map_err(pool_err)?
        .map_err(|e| fail(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({"lifted": n})))
}

fn account_actor(d: &Daemon, b: &Value) -> String {
    b["by"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| d.cfg.owner_names.first().cloned().unwrap_or_default())
}

/// Hold an account by hand (`mason account hold`).
async fn account_hold(Shared(d): Shared<Arc<Daemon>>, Json(b): Json<Value>) -> Res {
    let account = b["account"].as_str().unwrap_or_default().to_string();
    let reason = b["reason"].as_str().unwrap_or_default().to_string();
    let for_s = b["for_s"].as_u64();
    let who = account_actor(&d, &b);
    let d2 = d.clone();
    d.pool
        .run(move || crate::holds::hold(&d2, &account, &who, for_s, &reason))
        .await
        .map_err(pool_err)?
        .map_err(|e| fail(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({"ok": true})))
}

/// Release a hold now: try again at once (`mason account release`).
async fn account_release(Shared(d): Shared<Arc<Daemon>>, Json(b): Json<Value>) -> Res {
    let account = b["account"].as_str().unwrap_or_default().to_string();
    let who = account_actor(&d, &b);
    let d2 = d.clone();
    let was = d
        .pool
        .run(move || crate::holds::release(&d2, &account, &who))
        .await
        .map_err(pool_err)?
        .map_err(|e| fail(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({"released": was})))
}

/// The LLM usage view, as the monitors get it (proto3 JSON).
async fn usage_status(Shared(d): Shared<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let w = d2.world().map_err(store_err)?;
            let u = crate::views::Ctx::new(&d2, &w).usage();
            serde_json::to_value(&u)
                .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(v))
}

async fn pauses_list(Shared(d): Shared<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    let rows = d
        .pool
        .run(move || d2.store.pauses(now_ms()))
        .await
        .map_err(pool_err)?
        .map_err(store_err)?;
    Ok(Json(json!({"pauses": rows.iter().map(|p| json!({
        "project": p.project, "mode": p.mode, "owner": p.owner,
        "since": p.created_at, "expires_at": p.expires_at,
    })).collect::<Vec<_>>()})))
}

async fn alerts_list(
    Shared(d): Shared<Arc<Daemon>>,
    Query(q): Query<HashMap<String, String>>,
) -> Res {
    let all = q.get("all").is_some_and(|v| v != "0");
    let d2 = d.clone();
    let rows = d
        .pool
        .run(move || {
            crate::attention::sync_alerts(&d2);
            crate::notify::live_alerts(&d2, !all)
        })
        .await
        .map_err(pool_err)?
        .map_err(store_err)?;
    let unwritable = crate::notify::store_unwritable(&d);
    let mut out = json!({"alerts": rows.iter().map(|a| json!({
        "key": a.key, "kind": a.kind, "text": a.text, "since": a.since, "cleared_at": a.cleared_at,
        "acked_at": a.acked_at, "reminded_at": a.reminded_at,
    })).collect::<Vec<_>>()});
    if unwritable {
        out["store_writable"] = json!(false);
        out["note"] = json!(crate::notify::UNWRITABLE_NOTE);
    }
    Ok(Json(out))
}

/// Acknowledge an alert (owner only): its noise stops, it stays visible while its condition holds.
async fn ack_alert_key(d: Arc<Daemon>, key: String, who: Option<String>) -> Res {
    let who = who
        .filter(|w| !w.is_empty())
        .unwrap_or_else(|| d.cfg.owner_names.first().cloned().unwrap_or_default());
    owner_only(&d, &who, "acknowledge an alert")?;
    let (d2, k2) = (d.clone(), key.clone());
    let done = d
        .pool
        .run(move || crate::notify::ack(&d2, &k2))
        .await
        .map_err(pool_err)?
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    if !done {
        return Err(fail(
            StatusCode::NOT_FOUND,
            format!("no live alert {key:?} (it may have cleared)"),
        ));
    }
    let d3 = d.clone();
    let _ = d
        .pool
        .spawn(move || crate::attention::intervene(&d3, "acknowledged", None, &who, &key, ""));
    d.poke();
    Ok(Json(json!({"acknowledged": true})))
}

async fn alert_ack(
    Shared(d): Shared<Arc<Daemon>>,
    Path(key): Path<String>,
    body: Option<Json<Value>>,
) -> Res {
    let who = body.and_then(|b| b["as"].as_str().map(String::from));
    ack_alert_key(d, key, who).await
}

async fn alert_ack_body(Shared(d): Shared<Arc<Daemon>>, Json(b): Json<Value>) -> Res {
    let key = b["key"].as_str().unwrap_or_default().to_string();
    ack_alert_key(d, key, b["as"].as_str().map(String::from)).await
}

/// A monitor asks whether it is the one that speaks, and is given what is waiting if so.
async fn announce_claim(Shared(d): Shared<Arc<Daemon>>, Json(b): Json<Value>) -> Res {
    let client = b["client"].as_str().unwrap_or_default().to_string();
    if client.is_empty() {
        return Err(fail(StatusCode::BAD_REQUEST, "a claim names its client"));
    }
    let muted = b["muted"].as_bool().unwrap_or(false);
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || crate::notify::claim(&d2, &client, muted))
        .await
        .map_err(pool_err)?;
    Ok(Json(v))
}

async fn notify_status(Shared(d): Shared<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || crate::notify::status(&d2))
        .await
        .map_err(pool_err)?;
    Ok(Json(v))
}

async fn notify_test(Shared(d): Shared<Arc<Daemon>>, Json(b): Json<Value>) -> Res {
    owner_only(
        &d,
        b["as"]
            .as_str()
            .filter(|w| !w.is_empty())
            .unwrap_or(d.cfg.owner_names.first().map_or("", String::as_str)),
        "send a test notification",
    )?;
    let channel = b["channel"].as_str().unwrap_or_default().to_string();
    let d2 = d.clone();
    let r = d
        .pool
        .run(move || crate::notify::test(&d2, &channel))
        .await
        .map_err(pool_err)?;
    r.map(Json).map_err(|e| fail(StatusCode::BAD_REQUEST, e))
}

async fn metrics(Shared(d): Shared<Arc<Daemon>>, Query(q): Query<HashMap<String, String>>) -> Res {
    let days: u64 = q.get("days").and_then(|s| s.parse().ok()).unwrap_or(0);
    let since = if days == 0 {
        0
    } else {
        now_ms().saturating_sub(days * 86_400_000)
    };
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || crate::attention::metrics(&d2, since))
        .await
        .map_err(pool_err)?;
    Ok(Json(v))
}

async fn health(Shared(d): Shared<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || crate::introspect::health(&d2))
        .await
        .map_err(pool_err)?;
    Ok(Json(v))
}

async fn digest(Shared(d): Shared<Arc<Daemon>>, Query(q): Query<HashMap<String, String>>) -> Res {
    let days: u64 = q
        .get("days")
        .and_then(|s| s.parse().ok())
        .unwrap_or(7)
        .max(1);
    let project = q.get("project").cloned();
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let now = now_ms();
            crate::introspect::digest(
                &d2,
                project.as_deref(),
                now.saturating_sub(days * 86_400_000),
                now,
            )
        })
        .await
        .map_err(pool_err)?;
    Ok(Json(v))
}

async fn schedules_list(Shared(d): Shared<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || crate::introspect::health(&d2)["schedules"].clone())
        .await
        .map_err(pool_err)?;
    Ok(Json(json!({"schedules": v})))
}

/// Run a recurring job now: `introspect`, `analyst:<project>` or `<project>/<schedule>`.
async fn schedule_run(
    Shared(d): Shared<Arc<Daemon>>,
    headers: axum::http::HeaderMap,
    Json(b): Json<Value>,
) -> Res {
    let who = who_of(&headers);
    let name = b["name"].as_str().unwrap_or_default().to_string();
    let d2 = d.clone();
    let said = d
        .pool
        .run(move || {
            owner_only(&d2, &who, "run a recurring job")?;
            crate::introspect::run_now(&d2, &name).map_err(|e| fail(StatusCode::CONFLICT, e))
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(json!({"said": said})))
}

async fn triage_list(Shared(d): Shared<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    let rows = d
        .pool
        .run(move || d2.store.triage().unwrap_or_default())
        .await
        .map_err(pool_err)?;
    let v: Vec<Value> = rows
        .iter()
        .map(|t| {
            json!({"id": t.id, "task": t.task, "question": t.question, "action": t.action,
                        "answer": t.answer, "evidence": t.evidence, "status": t.status,
                        "verdict": t.verdict, "at": t.at})
        })
        .collect();
    Ok(Json(json!({"triage": v})))
}

async fn triage_review(
    Shared(d): Shared<Arc<Daemon>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<u64>,
    Json(b): Json<Value>,
) -> Res {
    let who = who_of(&headers);
    let agree = b["agree"].as_bool().unwrap_or(false);
    let d2 = d.clone();
    d.pool
        .run(move || {
            owner_only(&d2, &who, "review a triage answer")?;
            match d2.store.review_triage(id, agree).map_err(store_err)? {
                true => Ok(()),
                false => Err(fail(
                    StatusCode::CONFLICT,
                    format!("sample {id} is not waiting for review"),
                )),
            }
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(
        json!({"reviewed": id, "verdict": if agree { "agree" } else { "disagree" }}),
    ))
}

/// Where the machine's CPU goes, by task (proto3 JSON, as the monitors get it).
async fn cpu_usage(Shared(d): Shared<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let w = d2.world().map_err(store_err)?;
            let u = crate::views::Ctx::new(&d2, &w).cpu_usage();
            serde_json::to_value(&u)
                .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(v))
}

/// The home view, as the monitors get it (proto3 JSON).
async fn home_view(
    Shared(d): Shared<Arc<Daemon>>,
    Query(q): Query<HashMap<String, String>>,
) -> Res {
    let project = q.get("project").cloned().unwrap_or_default();
    let d2 = d.clone();
    let v = d
        .view_pool
        .run(move || {
            let w = d2.world().map_err(store_err)?;
            let h = crate::views::Ctx::new(&d2, &w).home(&project);
            serde_json::to_value(&h)
                .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(v))
}

async fn resources(Shared(d): Shared<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || d2.broker.snapshot())
        .await
        .map_err(pool_err)?;
    Ok(Json(v))
}

/// Devices the provider lists, who holds each, and the size of each device pool.
async fn targets(Shared(d): Shared<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || d2.targets.view(&d2))
        .await
        .map_err(pool_err)?;
    Ok(Json(v))
}

async fn resource_limit(Shared(d): Shared<Arc<Daemon>>, Json(b): Json<Value>) -> Res {
    let (name, units) = (
        b["resource"].as_str().unwrap_or_default().to_string(),
        b["units"].as_u64(),
    );
    let Some(units) = units.filter(|_| !name.is_empty()) else {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "give a `resource` and `units`",
        ));
    };
    let d2 = d.clone();
    d.pool
        .run(move || {
            let r = d2.broker.set_limit(&name, units as u32);
            // A raised limit may free a session for a task that is queued.
            d2.admit();
            // The home page shows the limits.
            d2.poke();
            r
        })
        .await
        .map_err(pool_err)?
        .map_err(|e| fail(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({"ok": true})))
}

/// Change a compile gate while masond runs (`value: null` goes back to resources.toml).
/// Owner only. Kept across restarts.
async fn resource_gate(
    Shared(d): Shared<Arc<Daemon>>,
    headers: axum::http::HeaderMap,
    Json(b): Json<Value>,
) -> Res {
    owner_only(&d, &who_of(&headers), "change a compile gate")?;
    let name = b["name"].as_str().unwrap_or_default().to_string();
    let value = match &b["value"] {
        Value::Null => None,
        v => match v.as_i64() {
            Some(n) => Some(n),
            None => {
                return Err(fail(
                    StatusCode::BAD_REQUEST,
                    "`value` must be a whole number, or null to go back to resources.toml",
                ))
            }
        },
    };
    if name.is_empty() {
        return Err(fail(StatusCode::BAD_REQUEST, "give a gate `name`"));
    }
    let d2 = d.clone();
    d.pool
        .run(move || {
            let r = d2.broker.set_gate(&name, value);
            // A looser gate may free a session for a task that is queued.
            d2.admit();
            d2.poke();
            r
        })
        .await
        .map_err(pool_err)?
        .map_err(|e| fail(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({"ok": true})))
}

/// An interactive caller asks for resources (`mason run --needs`).
async fn lease_open(Shared(d): Shared<Arc<Daemon>>, Json(b): Json<Value>) -> Res {
    let mut uses = std::collections::BTreeMap::new();
    for (k, v) in b["uses"].as_object().into_iter().flatten() {
        match v.as_u64() {
            Some(n) => drop(uses.insert(k.clone(), n as u32)),
            None => {
                return Err(fail(
                    StatusCode::BAD_REQUEST,
                    format!("`{k}` needs a count"),
                ))
            }
        }
    }
    if uses.is_empty() {
        return Err(fail(StatusCode::BAD_REQUEST, "name what it needs"));
    }
    let label = b["label"].as_str().unwrap_or("interactive").to_string();
    // On behalf of a task: ranked as that task's pipeline work. An id that is
    // not a live task is treated as no task at all (a person's request).
    let task = b["task"].as_u64();
    let pid = b["pid"].as_u64().unwrap_or(0) as u32;
    // Another pipeline's work: ranked as pipeline work, never a person's, and
    // `task` is ignored (it names a MASON task).
    if let Some(name) = b["pipeline"].as_str() {
        let o = crate::outside::parse_request(name, b["priority"].as_str(), b["progress"].as_u64())
            .map_err(|e| fail(StatusCode::BAD_REQUEST, e))?;
        let d2 = d.clone();
        let id = d
            .pool
            .run(move || {
                let id = d2.broker.lease_open_outside(&label, &uses, &o);
                d2.broker.lease_set_pid(id, pid);
                id
            })
            .await
            .map_err(pool_err)?;
        return Ok(Json(json!({"id": id})));
    }
    let d2 = d.clone();
    let (id, th) = d
        .pool
        .run(move || {
            let standing = task.and_then(|t| d2.lease_standing(TaskId(t)));
            let id = d2.broker.lease_open_as(&label, &uses, standing);
            d2.broker.lease_set_pid(id, pid);
            if let (Some(t), Some(_)) = (task, standing) {
                d2.broker.lease_set_task(id, TaskId(t));
            }
            let th = task
                .map(|t| d2.stall_thresholds(TaskId(t)))
                .unwrap_or_default();
            (id, th)
        })
        .await
        .map_err(pool_err)?;
    Ok(Json(json!({
        "id": id,
        "cont_after_s": th.cont_after_ms / 1000, "kill_after_s": th.kill_after_ms / 1000,
    })))
}

/// The processes of lease requests still waiting, for a `mason run` watching its own tree.
async fn stall_waiting(Shared(d): Shared<Arc<Daemon>>) -> Res {
    let pids: Vec<u32> = d.broker.waiting_pids().into_iter().collect();
    Ok(Json(json!({"pids": pids})))
}

/// A `mason run` child's watcher says what it did, so `mason health` counts it.
async fn stall_event(Shared(d): Shared<Arc<Daemon>>, Json(b): Json<Value>) -> Res {
    use std::sync::atomic::Ordering;
    match b["event"].as_str() {
        Some("cont") => d.stall_conts.fetch_add(1, Ordering::SeqCst),
        Some("kill") => d.stall_kills.fetch_add(1, Ordering::SeqCst),
        _ => return Err(fail(StatusCode::BAD_REQUEST, "event is cont or kill")),
    };
    Ok(Json(json!({"ok": true})))
}

async fn lease_poll(Shared(d): Shared<Arc<Daemon>>, Path(id): Path<u64>) -> Res {
    use crate::sched::LeaseState;
    let d2 = d.clone();
    let s = d
        .pool
        .run(move || d2.broker.lease_poll(id))
        .await
        .map_err(pool_err)?;
    Ok(Json(match s {
        LeaseState::Waiting { explain } => json!({"state": "waiting", "explain": explain}),
        LeaseState::Granted {
            classes,
            exclusive,
            capacity,
            locks_dir,
        } => json!({
            "state": "granted", "classes": classes, "exclusive": exclusive,
            "capacity": capacity, "locks_dir": locks_dir.display().to_string(),
        }),
        LeaseState::Gone => json!({"state": "gone"}),
    }))
}

async fn lease_release(Shared(d): Shared<Arc<Daemon>>, Path(id): Path<u64>) -> Res {
    let d2 = d.clone();
    let had = d
        .pool
        .run(move || d2.broker.lease_close(id))
        .await
        .map_err(pool_err)?;
    Ok(Json(json!({"released": had})))
}

/// Render an agent's stream-json journal as readable lines.
fn render_agent(text: &str) -> String {
    let mut out = String::new();
    for l in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(l) else {
            out.push_str(l);
            out.push('\n');
            continue;
        };
        match v["type"].as_str() {
            Some("assistant") => {
                for b in v["message"]["content"].as_array().into_iter().flatten() {
                    match b["type"].as_str() {
                        Some("text") => out.push_str(&format!(
                            "agent: {}\n",
                            b["text"].as_str().unwrap_or_default()
                        )),
                        Some("tool_use") => out.push_str(&format!(
                            "tool: {} {}\n",
                            b["name"].as_str().unwrap_or("?"),
                            b["input"]
                        )),
                        _ => {}
                    }
                }
            }
            Some("user") => {
                for b in v["message"]["content"].as_array().into_iter().flatten() {
                    if b["type"] == "tool_result" {
                        let t = b["content"][0]["text"].as_str().unwrap_or_default();
                        let flat: String = t.lines().collect::<Vec<_>>().join(" | ");
                        let shown: String = flat.chars().take(500).collect();
                        out.push_str(&format!("result: {shown}\n"));
                    }
                }
            }
            Some("result") => out.push_str(&format!(
                "-- finished: {}\n",
                v["result"].as_str().unwrap_or("")
            )),
            _ => {}
        }
    }
    out
}

/// A host's id with its journal's size and modified time: if none of these
/// changed, `render_logs` would give the same text.
pub type LogStamp = (String, u64, Option<std::time::SystemTime>);

pub fn log_signature(d: &Daemon, task: u64) -> Vec<LogStamp> {
    let reg = mason_step_host::registry::Registry::new(d.cfg.hosts_dir());
    let mut v: Vec<LogStamp> = reg
        .scan()
        .unwrap_or_default()
        .into_iter()
        .filter(|h| crate::sweep::parse_host_id(&h.info.id).is_some_and(|(t, _)| t == task))
        .map(|h| {
            let m = std::fs::metadata(h.info.dir.join("journal")).ok();
            (
                h.info.id.clone(),
                m.as_ref().map_or(0, |m| m.len()),
                m.and_then(|m| m.modified().ok()),
            )
        })
        .collect();
    v.sort();
    v
}

pub fn render_logs(d: &Daemon, task: u64) -> String {
    // Live hosts and the ones the sweep moved to `t/<task>/hosts/`.
    let mut hosts: Vec<_> = crate::hostarchive::task_hosts(&d.cfg, task)
        .into_iter()
        .filter_map(|h| {
            crate::sweep::parse_host_id(&h.info.id)
                .filter(|(t, _)| *t == task)
                .map(|(_, r)| (r, h))
        })
        .collect();
    hosts.sort_by(|a, b| (a.0, &a.1.info.id).cmp(&(b.0, &b.1.info.id)));
    let mut out = String::new();
    for (run, h) in hosts {
        // `t<task>r<run>-test`, `-cmd`, a red test run again (`-test-again<k>`),
        // and the build a test step with `build` runs first (`-build`).
        let what = h.info.id.split_once('-').map_or("", |(_, w)| w);
        let kind =
            if what == "build" || what.starts_with("build-") || what.starts_with("checkbuild") {
                "build"
            } else if what == "test"
                || what == "cmd"
                || what.starts_with("test-again")
                || what.starts_with("cmd-again")
                || what.starts_with("check")
            {
                "test"
            } else {
                "agent"
            };
        let again = what
            .split_once("-again")
            .map_or(String::new(), |(_, k)| format!(", run again ({k})"));
        out.push_str(&format!(
            "=== step run {run} ({kind}{again}) [{}] ===\n",
            h.info.id
        ));
        let raw =
            String::from_utf8_lossy(&std::fs::read(h.info.dir.join("journal")).unwrap_or_default())
                .to_string();
        out.push_str(&if kind != "agent" {
            raw
        } else {
            render_agent(&raw)
        });
        if !out.ends_with('\n') {
            out.push('\n');
        }
        if out.len() > 400_000 {
            out.push_str("(truncated)\n");
            break;
        }
    }
    out
}

async fn logs(
    Shared(d): Shared<Arc<Daemon>>,
    Path(id): Path<u64>,
    Query(q): Query<HashMap<String, String>>,
) -> Res {
    let offset: usize = q.get("offset").and_then(|s| s.parse().ok()).unwrap_or(0);
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let w = d2.world().map_err(store_err)?;
            if !w.tasks.contains_key(&TaskId(id)) {
                return Err(fail(StatusCode::NOT_FOUND, format!("no task #{id}")));
            }
            let text = render_logs(&d2, id);
            let start = offset.min(text.len());
            let start = (start..=text.len())
                .find(|i| text.is_char_boundary(*i))
                .unwrap_or(text.len());
            Ok(json!({"text": &text[start..], "next": text.len()}))
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(v))
}

/// The live progress records of a task's open step runs, as JSON: the same figures
/// as the `StepProgress` a monitor is pushed.
fn progress_runs(d: &Daemon, id: TaskId) -> Vec<Value> {
    let fig = |f: &mason_core::progress::Figure| json!({"value": f.value, "source": format!("{:?}", f.source).to_lowercase()});
    d.progress_of_task(id)
        .iter()
        .map(|p| {
            json!({
                "run": p.run, "step": p.step, "unit": p.unit,
                "done": fig(&p.done), "total": fig(&p.total),
                "total_low": p.total_low, "history_samples": p.history_samples,
                "percent": p.percent(),
                "stale_after_ms": p.stale_after_ms,
                "message": p.message,
                "output": {"lines": p.output.lines, "bytes": p.output.bytes,
                           "last_output_ms": p.output.last_output_ms},
                "started_ms": p.started_ms, "updated_at_ms": p.updated_at_ms,
                "subs": p.subs.iter().map(|s| json!({
                    "name": s.name, "unit": s.unit,
                    "done": fig(&s.done), "total": fig(&s.total)})).collect::<Vec<_>>(),
                "todo": p.todo.iter().map(|t| json!({"text": t.text, "status": t.status})).collect::<Vec<_>>(),
                // The tokens the step's sessions have used so far; null until one reported.
                "tokens": p.token_total().map(|t| json!({
                    "input": t.input, "output": t.output,
                    "cache_read": t.cache_read, "cache_write": t.cache_write,
                    "cost_usd": t.cost_nano_usd.map(|n| n as f64 / 1e9)})),
            })
        })
        .collect()
}

/// The live progress records of a task's open step runs.
async fn task_progress(Shared(d): Shared<Arc<Daemon>>, Path(id): Path<u64>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let w = d2.world().map_err(store_err)?;
            if !w.tasks.contains_key(&TaskId(id)) {
                return Err(fail(StatusCode::NOT_FOUND, format!("no task #{id}")));
            }
            let runs = progress_runs(&d2, TaskId(id));
            Ok(json!({"task": id, "now_ms": crate::daemon::now_ms(), "runs": runs}))
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(v))
}

/// Deliberately blocks the event loop, to prove the watchdog notices.
async fn debug_stall(
    Shared(d): Shared<Arc<Daemon>>,
    Query(q): Query<HashMap<String, String>>,
) -> Res {
    if !d.cfg.debug {
        return Err(fail(StatusCode::NOT_FOUND, "not found"));
    }
    let ms: u64 = q.get("ms").and_then(|s| s.parse().ok()).unwrap_or(800);
    std::thread::sleep(std::time::Duration::from_millis(ms));
    Ok(Json(json!({"blocked_ms": ms})))
}

/// Makes the alert and delivery writes fail, or work again (`?refuse=1` / `?refuse=0`), to show
/// the alerts surviving a store that cannot be written.
async fn debug_store_writes(
    Shared(d): Shared<Arc<Daemon>>,
    Query(q): Query<HashMap<String, String>>,
) -> Res {
    if !d.cfg.debug {
        return Err(fail(StatusCode::NOT_FOUND, "not found"));
    }
    let on = q.get("refuse").is_none_or(|v| v != "0");
    crate::notify::refuse_writes(&d, on);
    Ok(Json(json!({"refuse": on})))
}

/// Holds a project's repository lock for `ms` milliseconds, to show that alerts go on while git
/// work is stuck. With `fetch=1` it holds the project's fetch lock too, as a hung fetch would
/// (since s81 the fetch has a lock of its own, so the repository lock alone stalls no fetch).
/// Returns at once; a thread holds the locks.
async fn debug_hold_repo_lock(
    Shared(d): Shared<Arc<Daemon>>,
    Query(q): Query<HashMap<String, String>>,
) -> Res {
    if !d.cfg.debug {
        return Err(fail(StatusCode::NOT_FOUND, "not found"));
    }
    let ms: u64 = q.get("ms").and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let pid = q
        .get("project")
        .and_then(|n| d.project_id_by_name(n))
        .or_else(|| d.projects().keys().next().copied())
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "no project"))?;
    let lock = d.repo_lock(pid);
    let fetch = (q.get("fetch").map(String::as_str) == Some("1")).then(|| d.fetch_lock(pid));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        let _f = fetch
            .as_ref()
            .map(|f| f.lock().unwrap_or_else(|e| e.into_inner()));
        let _ = tx.send(());
        std::thread::sleep(std::time::Duration::from_millis(ms));
    });
    let _ = rx.recv_timeout(std::time::Duration::from_millis(100));
    Ok(Json(json!({"holding_ms": ms})))
}

/// Runs on the blocking pool and waits on the repository lock (debug daemons
/// only): the shape of `reload_definition` and `jobs::cleanup`.
async fn debug_wait_repo(Shared(d): Shared<Arc<Daemon>>) -> Res {
    if !d.cfg.debug {
        return Err(fail(StatusCode::NOT_FOUND, "not found"));
    }
    let pid = d
        .projects()
        .keys()
        .next()
        .copied()
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "no project"))?;
    let d2 = d.clone();
    d.pool
        .run(move || {
            let lock = d2.repo_lock(pid);
            let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        })
        .await
        .map_err(pool_err)?;
    Ok(Json(json!({"waited": true})))
}

/// Every operation of the API, without the socket. The socket runs its
/// commands through this same router, so there is one implementation of each.
pub fn api_router(d: Arc<Daemon>) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/lan", get(crate::lan::status))
        .route("/pair/codes", post(crate::lan::issue_code))
        .route("/devices", get(crate::lan::devices))
        .route("/devices/{id}/revoke", post(crate::lan::revoke))
        .route("/tasks", get(list_tasks).post(add_task))
        .route("/tasks/{id}", get(task_detail))
        .route("/tasks/{id}/cancel", post(cancel))
        .route("/tasks/{id}/retry", post(retry))
        .route("/tasks/{id}/approve", post(approve))
        .route("/tasks/{id}/reject", post(reject))
        .route("/tasks/{id}/explore-anyway", post(explore_anyway))
        .route("/tasks/{id}/overrides", post(task_override))
        .route("/tasks/{id}/priority", post(task_priority))
        .route("/projects/{name}/definition", get(project_definition))
        .route("/projects/{name}/extensions", get(extensions_list))
        .route(
            "/projects/{name}/extensions/{ext}/approve",
            post(extension_approve),
        )
        .route("/projects/{name}/editor", get(editor_get))
        .route("/projects/{name}/editor/op", post(editor_op))
        .route("/projects/{name}/editor/simulate", post(editor_simulate))
        .route("/projects/{name}/editor/apply", post(editor_apply))
        .route("/projects/{name}/editor/{what}", post(editor_history))
        .route("/projects/{name}/reload", post(project_reload))
        .route(
            "/projects/{name}/registration/reload",
            post(registration_reload),
        )
        .route("/projects/{name}/config", get(project_config))
        .route(
            "/projects/{name}/overrides",
            get(overrides_list).post(overrides_add),
        )
        .route("/projects/{name}/overrides/remove", post(overrides_remove))
        .route("/tasks/{id}/logs", get(logs))
        .route("/tasks/{id}/progress", get(task_progress))
        .route("/resources", get(resources))
        .route("/cpu", get(cpu_usage))
        .route("/views/home", get(home_view))
        .route("/targets", get(targets))
        .route("/health", get(health))
        .route("/digest", get(digest))
        .route("/schedules", get(schedules_list))
        .route("/schedules/run", post(schedule_run))
        .route("/triage", get(triage_list))
        .route("/triage/{id}/review", post(triage_review))
        .route("/tasks/{id}/skip", post(skip_task))
        .route("/tasks/{id}/answer", post(answer_task))
        .route("/tasks/{id}/session", get(open_session))
        .route("/tasks/{id}/{kind}", post(land_override))
        .route("/account/hold", post(account_hold))
        .route("/account/release", post(account_release))
        .route("/projects/{name}/ssh-keys", get(ssh_keys_list))
        .route("/projects/{name}/ssh-key", post(ssh_key_choose))
        .route("/pause", post(pause_set))
        .route("/resume", post(pause_clear))
        .route("/pauses", get(pauses_list))
        .route("/usage", get(usage_status))
        .route("/alerts", get(alerts_list))
        .route("/alerts/ack", post(alert_ack_body))
        .route("/alerts/{key}/ack", post(alert_ack))
        .route("/announce/claim", post(announce_claim))
        .route("/notify", get(notify_status))
        .route("/notify/test", post(notify_test))
        .route("/metrics", get(metrics))
        .route("/demo", get(demo_status))
        .route("/demo/next", post(demo_next))
        .route("/landing", get(landing_status))
        .route("/self", get(self_status))
        .route("/hooks/github", post(github_hook))
        .route("/update", get(update_status).post(update_start))
        .route("/update/ack", post(update_ack))
        .route("/update/monitor", post(monitor_update_start))
        .route("/tasks/{id}/refresh", post(refresh_tracker))
        .route("/tasks/{id}/relink", post(relink_task))
        .route("/tasks/{id}/unblock", post(unblock_task))
        .route("/tasks/{id}/block", post(block_task))
        .route("/tasks/{id}/revive", post(revive_task))
        .route("/pending-issues", get(pending_issues))
        .route("/proposals", get(proposals_list).post(proposal_file))
        .route("/proposals/poll", post(proposals_poll))
        .route("/proposals/{task}", get(proposal_show))
        .route("/proposals/{task}/{act}", post(proposal_act))
        .route("/projects/{name}/improve", post(improve_start))
        .route("/debts", get(debts_list))
        .route("/debts/{id}/run", post(debt_run))
        .route("/quarantine", get(quarantine_list).post(quarantine_add))
        .route("/quarantine/remove", post(quarantine_remove))
        .route("/resources/limit", post(resource_limit))
        .route("/resources/gate", post(resource_gate))
        .route("/leases", post(lease_open))
        .route("/leases/{id}", get(lease_poll))
        .route("/stall/waiting", get(stall_waiting))
        .route("/stall/event", post(stall_event))
        .route("/leases/{id}/release", post(lease_release))
        .route("/debug/stall", post(debug_stall))
        .route("/debug/store-writes", post(debug_store_writes))
        .route("/debug/hold-repo-lock", post(debug_hold_repo_lock))
        .route("/debug/wait-repo", post(debug_wait_repo))
        .layer(axum::middleware::from_fn_with_state(
            d.clone(),
            record_interventions,
        ))
        .with_state(d)
}

/// The whole thing: the API and the client socket.
pub fn router(d: Arc<Daemon>) -> Router {
    api_router(d.clone()).merge(
        Router::new()
            .route("/ws", get(crate::ws::handler))
            .with_state(d),
    )
}

async fn demo_status(Shared(d): Shared<Arc<Daemon>>) -> Res {
    if !d.demo {
        return Err(fail(StatusCode::NOT_FOUND, "not a demo daemon"));
    }
    Ok(Json(json!({
        "position": d.demo_position(),
        "total": crate::demo::checkpoints(),
    })))
}

/// Play the next step of the demo script (demo daemons only).
async fn demo_next(Shared(d): Shared<Arc<Daemon>>) -> Res {
    if !d.demo {
        return Err(fail(StatusCode::NOT_FOUND, "not a demo daemon"));
    }
    let d2 = d.clone();
    let at = d.pool.run(move || d2.demo_next()).await.map_err(pool_err)?;
    Ok(Json(json!({
        "position": d.demo_position(),
        "total": crate::demo::checkpoints(),
        "advanced": at.is_some(),
    })))
}

/// The single-daemon lock, held for the life of the process. The file is
/// opened close-on-exec, so an `exec` (the managed loader, `take_over`) hands
/// it back; the new image takes it again and `ensure_single` retries briefly.
static SINGLE_LOCK: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);

/// Refuse to run if another daemon is already running on this data directory:
/// a kernel `flock` on `daemon.lock`, which a dead daemon can never leave held.
/// Call this *before* opening the store or resuming any work. A repeat call by
/// the holder does nothing.
pub fn ensure_single(cfg: &crate::config::DaemonConfig) -> std::io::Result<()> {
    use std::io::{Read, Seek, Write};
    let mut held = SINGLE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if held.is_some() {
        return Ok(());
    }
    std::fs::create_dir_all(&cfg.data_dir)?;
    let path = cfg.data_dir.join("daemon.lock");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let mut f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        match f.try_lock() {
            Ok(()) => {
                let _ = f.set_len(0);
                let _ = f.rewind();
                let _ = write!(f, "{}", std::process::id());
                *held = Some(f);
                return Ok(());
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                if std::time::Instant::now() < deadline {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    continue;
                }
                let mut text = String::new();
                let _ = f.read_to_string(&mut text);
                let pid = text.trim().parse::<u32>().ok().or_else(|| {
                    std::fs::read_to_string(cfg.info_path())
                        .ok()
                        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                        .and_then(|v| v["pid"].as_u64())
                        .map(|p| p as u32)
                });
                let who = pid.map(|p| format!(" (pid {p})")).unwrap_or_default();
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("a daemon is already running here{who}"),
                ));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
        }
    }
}

/// Who holds a loopback port, as `pid N (command)`, if `lsof` can say.
fn port_holder(port: u16) -> Option<String> {
    let out = std::process::Command::new("lsof")
        .args(["-nP", "-t", &format!("-iTCP:{port}"), "-sTCP:LISTEN"])
        .output()
        .ok()?;
    let pid = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .to_string();
    let comm = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|c| !c.is_empty());
    Some(match comm {
        Some(c) => format!("pid {pid} ({c})"),
        None => format!("pid {pid}"),
    })
}

/// The message when the port is held by something else: who, and how to change it.
pub fn port_taken_message(port: u16, config_dir: &std::path::Path) -> String {
    let owner = port_holder(port).unwrap_or_else(|| "another process".into());
    format!(
        "port {port} on 127.0.0.1 is taken by {owner}; masond does not pick another port. \
         Stop that process, or choose a different port with --port N or with `[daemon] port = N` in {}",
        config_dir.join("daemon.toml").display()
    )
}

/// Bind loopback, announce ourselves in `daemon.json`, run the watchdog, serve.
pub async fn serve(d: Arc<Daemon>) -> std::io::Result<()> {
    serve_until(d, std::future::pending()).await
}

/// `serve`, stopping gracefully when `shutdown` completes: no new connections, and
/// it returns once the open ones are done (the caller bounds the wait).
pub async fn serve_until(
    d: Arc<Daemon>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    ensure_single(&d.cfg)?;
    let info = d.cfg.info_path();
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", d.cfg.port)).await {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            return Err(std::io::Error::new(
                e.kind(),
                port_taken_message(d.cfg.port, &d.cfg.config_dir),
            ))
        }
        Err(e) => return Err(e),
    };
    let port = listener.local_addr()?.port();
    let lan_port = if d.cfg.lan.enabled {
        Some(crate::lan::start(d.clone()).await?)
    } else {
        None
    };
    let tmp = info.with_extension("tmp");
    let mut announce = json!({"pid": std::process::id(), "port": port, "started_ms": now_ms()});
    if let (Some(p), Some(i)) = (lan_port, d.lan.info()) {
        announce["lan_port"] = json!(p);
        announce["lan_fingerprint"] = json!(i.fingerprint);
    }
    std::fs::write(&tmp, announce.to_string())?;
    std::fs::rename(&tmp, &info)?;
    tokio::spawn(crate::watchdog::run(d.clone()));
    tracing::info!(port, "masond listening on loopback");
    axum::serve(listener, router(d))
        .with_graceful_shutdown(shutdown)
        .await
}
