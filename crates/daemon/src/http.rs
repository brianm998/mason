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
                "run": r.id.0, "step": state_name(r.step), "attempt": r.attempt,
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
            let body = b["body"].as_str().unwrap_or_default().to_string();
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
                    explored: false,
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
    let to = match body.as_ref().and_then(|b| b["to"].as_str()) {
        None => State::Developing,
        Some(name) => parse_state(name)
            .ok_or_else(|| fail(StatusCode::BAD_REQUEST, format!("unknown step {name:?}")))?,
    };
    let d2 = d.clone();
    d.pool
        .run(move || {
            d2.emit(EventKind::Retried {
                task: TaskId(id),
                to,
                step: String::new(),
            })
            .map_err(store_err)
        })
        .await
        .map_err(pool_err)??;
    Ok(Json(json!({"ok": true, "to": state_name(to)})))
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
        let kind = if h.info.id.ends_with("-test") {
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
        .route("/tasks/{id}/logs", get(logs))
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
