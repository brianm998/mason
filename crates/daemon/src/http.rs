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
        StoreError::Rejected(r) => fail(StatusCode::CONFLICT, format!("not allowed: {r:?}")),
        other => fail(StatusCode::INTERNAL_SERVER_ERROR, other.to_string()),
    }
}

pub fn parse_state(s: &str) -> Option<State> {
    State::ALL.into_iter().find(|x| state_name(*x) == s)
}

fn activity_text(a: &Activity) -> String {
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
        "project": project.map_or("?".to_string(), |p| p.id.clone()),
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
        v["failure"] = t.failure.as_ref().map_or(Value::Null, |f| json!(f));
        v["question"] = t
            .decision
            .and_then(|id| w.decisions.get(&id))
            .map_or(Value::Null, |q| json!(q.question));
        v["steps"] = json!(w
            .step_runs
            .values()
            .filter(|r| r.task == t.id)
            .map(|r| json!({
                "run": r.id.0, "step": state_name(r.step), "name": r.name, "attempt": r.attempt,
                "started_at": r.started_at.0,
                "outcome": r.outcome.map(|o| format!("{o:?}").to_lowercase()),
            }))
            .collect::<Vec<_>>());
    }
    v
}

async fn status(Shared(d): Shared<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
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
                "event_loop_stalls": stalls,
                "worst_stall_ms": worst,
                "bugs": d2.store.bugs().map_or(0, |b| b.len()),
                "projects": d2.projects.values().map(|p| p.id.clone()).collect::<Vec<_>>(),
                "step_hosts_running": running,
                "definitions": d2.projects.iter().map(|(pid, p)| json!({
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
                        d2.projects
                            .values()
                            .map(|p| p.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            };
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
            let appended = d2
                .emit(EventKind::TaskFiled {
                    project: pid,
                    title,
                    body,
                    priority,
                    trust: TrustSource::Owner, // filed from the owner's own CLI
                    explored,
                    proposed: b["proposed"].as_bool().unwrap_or(false),
                    blockers: vec![],
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

async fn retry(
    Shared(d): Shared<Arc<Daemon>>,
    Path(id): Path<u64>,
    body: Option<Json<Value>>,
) -> Res {
    let want = body
        .as_ref()
        .and_then(|b| b["to"].as_str())
        .map(String::from);
    let d2 = d.clone();
    let (to, step) =
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
                // `to` is a step in the task's definition, or a category (the first step in it).
                let name = want.unwrap_or_else(|| {
                    def.first_in(State::Developing)
                        .map_or("develop".into(), |s| s.name.clone())
                });
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
async fn approve(Shared(d): Shared<Arc<Daemon>>, Path(id): Path<u64>) -> Res {
    let d2 = d.clone();
    d.pool
        .run(move || {
            let w = d2.world().map_err(store_err)?;
            let t = w
                .tasks
                .get(&TaskId(id))
                .ok_or_else(|| fail(StatusCode::NOT_FOUND, format!("no task #{id}")))?;
            if t.state == State::Proposed {
                d2.emit(EventKind::Approved { task: TaskId(id) })
                    .map_err(store_err)?;
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
    Path(id): Path<u64>,
    body: Option<Json<Value>>,
) -> Res {
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
                        "model": attr(r.model.map(|a| (a.value, a.source))),
                        "effort": attr(r.effort.map(|a| (a.value, a.source))),
                        "budget": attr(r.budget.map(|a| (format!("{}s", a.value.as_secs()), a.source))),
                        "max_rounds": attr(r.max_rounds.map(|a| (a.value.to_string(), a.source))),
                        "command": attr(r.command.map(|a| (a.value, a.source))),
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

async fn resources(Shared(d): Shared<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || d2.broker.snapshot())
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
    let d2 = d.clone();
    let id = d
        .pool
        .run(move || d2.broker.lease_open(&label, &uses))
        .await
        .map_err(pool_err)?;
    Ok(Json(json!({"id": id})))
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

pub fn render_logs(d: &Daemon, task: u64) -> String {
    let reg = mason_step_host::registry::Registry::new(d.cfg.hosts_dir());
    let mut hosts: Vec<_> = reg
        .scan()
        .unwrap_or_default()
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
        let kind = if h.info.id.ends_with("-test") || h.info.id.ends_with("-cmd") {
            "test"
        } else {
            "agent"
        };
        out.push_str(&format!(
            "=== step run {run} ({kind}) [{}] ===\n",
            h.info.id
        ));
        let raw =
            String::from_utf8_lossy(&std::fs::read(h.info.dir.join("journal")).unwrap_or_default())
                .to_string();
        out.push_str(&if kind == "test" {
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

pub fn router(d: Arc<Daemon>) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/tasks", get(list_tasks).post(add_task))
        .route("/tasks/{id}", get(task_detail))
        .route("/tasks/{id}/cancel", post(cancel))
        .route("/tasks/{id}/retry", post(retry))
        .route("/tasks/{id}/approve", post(approve))
        .route("/tasks/{id}/reject", post(reject))
        .route("/tasks/{id}/explore-anyway", post(explore_anyway))
        .route("/tasks/{id}/overrides", post(task_override))
        .route("/projects/{name}/definition", get(project_definition))
        .route("/projects/{name}/reload", post(project_reload))
        .route("/projects/{name}/config", get(project_config))
        .route(
            "/projects/{name}/overrides",
            get(overrides_list).post(overrides_add),
        )
        .route("/projects/{name}/overrides/remove", post(overrides_remove))
        .route("/tasks/{id}/logs", get(logs))
        .route("/resources", get(resources))
        .route("/tasks/{id}/skip", post(skip_task))
        .route("/debts", get(debts_list))
        .route("/debts/{id}/run", post(debt_run))
        .route("/quarantine", get(quarantine_list).post(quarantine_add))
        .route("/quarantine/remove", post(quarantine_remove))
        .route("/resources/limit", post(resource_limit))
        .route("/leases", post(lease_open))
        .route("/leases/{id}", get(lease_poll))
        .route("/leases/{id}/release", post(lease_release))
        .route("/debug/stall", post(debug_stall))
        .with_state(d)
}

/// Refuse to run if another daemon is already running on this data directory.
/// Call this *before* opening the store or resuming any work.
pub fn ensure_single(cfg: &crate::config::DaemonConfig) -> std::io::Result<()> {
    if let Ok(text) = std::fs::read_to_string(cfg.info_path()) {
        if let Some(pid) = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["pid"].as_u64())
        {
            if pid as u32 != std::process::id() && mason_step_host::registry::pid_alive(pid as u32)
            {
                return Err(std::io::Error::other(format!(
                    "a daemon is already running here (pid {pid})"
                )));
            }
        }
    }
    Ok(())
}

/// Bind loopback, announce ourselves in `daemon.json`, run the watchdog, serve.
pub async fn serve(d: Arc<Daemon>) -> std::io::Result<()> {
    ensure_single(&d.cfg)?;
    let info = d.cfg.info_path();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", d.cfg.port)).await?;
    let port = listener.local_addr()?.port();
    let tmp = info.with_extension("tmp");
    std::fs::write(
        &tmp,
        json!({"pid": std::process::id(), "port": port, "started_ms": now_ms()}).to_string(),
    )?;
    std::fs::rename(&tmp, &info)?;
    tokio::spawn(crate::watchdog::run(d.clone()));
    tracing::info!(port, "masond listening on loopback");
    axum::serve(listener, router(d)).await
}
