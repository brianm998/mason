// SPDX-License-Identifier: GPL-3.0-or-later

//! The client protocol over one WebSocket (Part 16). A client says hello with
//! the last event sequence number it saw and gets a snapshot (new, or too far
//! behind) or just the events it missed. After that the daemon pushes new
//! events and recomputed views as things change, and answers commands.
//!
//! Frames are binary protobuf, or text in the proto3 JSON mapping: the first
//! frame decides which, and the daemon answers in kind. Commands run through
//! the very same handlers as the HTTP API (the command line uses this socket
//! too), so there is one implementation of every operation.

use crate::daemon::Daemon;
use crate::views::{summarize, Ctx};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State as Shared;
use axum::response::Response;
use mason_core::*;
use mason_protocol::{
    client_message::Body as C, command::Body as Cmd, server_message::Body as S, *,
};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

/// Further behind than this and a client gets a snapshot instead of the events.
const RESUME_LIMIT: u64 = 2000;

/// `peer` is set only on the LAN listener, where the token check has already run.
pub async fn handler(
    Shared(d): Shared<Arc<Daemon>>,
    peer: Option<axum::Extension<crate::lan::Peer>>,
    ws: WebSocketUpgrade,
) -> Response {
    let peer = peer.map(|p| p.0);
    ws.on_upgrade(move |sock| session(d, sock, peer))
}

struct Sub {
    views: Vec<ViewKind>,
    project: String,
    task: u64,
    editor_step: String,
}

impl Default for Sub {
    fn default() -> Self {
        Sub {
            views: vec![ViewKind::Header, ViewKind::Sidebar, ViewKind::Home],
            project: String::new(),
            task: 0,
            editor_step: String::new(),
        }
    }
}

fn frame(m: &ServerMessage, e: Encoding) -> Message {
    match e {
        Encoding::Protobuf => Message::Binary(encode_server(m, e).into()),
        Encoding::Json => Message::Text(
            String::from_utf8(encode_server(m, e))
                .unwrap_or_default()
                .into(),
        ),
    }
}

fn server(body: S) -> ServerMessage {
    ServerMessage { body: Some(body) }
}

async fn send(sock: &mut WebSocket, m: ServerMessage, e: Encoding) -> bool {
    sock.send(frame(&m, e)).await.is_ok()
}

fn decode(msg: &Message) -> Option<(Encoding, Result<ClientMessage, String>)> {
    match msg {
        Message::Binary(b) => Some((Encoding::Protobuf, decode_client(b, Encoding::Protobuf))),
        Message::Text(t) => Some((Encoding::Json, decode_client(t.as_bytes(), Encoding::Json))),
        _ => None,
    }
}

/// The current views for a subscription, each stamped with the sequence
/// number they were computed at.
fn compute(d: &Daemon, sub: &Sub) -> (u64, Vec<View>) {
    let seq = d.store.last_seq().unwrap_or(0);
    let Ok(w) = d.world() else {
        return (seq, vec![]);
    };
    let ctx = Ctx::new(d, &w);
    let views = sub
        .views
        .iter()
        .filter_map(|k| ctx.view(*k, &sub.project, sub.task, &sub.editor_step))
        .collect();
    (seq, views)
}

fn records(d: &Daemon, after: u64, limit: usize) -> Vec<EventRecord> {
    d.store
        .read_since(after, limit)
        .unwrap_or_default()
        .iter()
        .map(|e| {
            let (kind, summary, task) = summarize(&e.event.kind);
            EventRecord {
                seq: e.seq,
                at_ms: e.event.at.0,
                kind,
                summary,
                task,
            }
        })
        .collect()
}

async fn session(d: Arc<Daemon>, mut sock: WebSocket, peer: Option<crate::lan::Peer>) {
    // The first frame must be a hello, and decides the encoding.
    let first = tokio::time::timeout(Duration::from_secs(10), sock.recv()).await;
    let Ok(Some(Ok(msg))) = first else { return };
    let Some((enc, decoded)) = decode(&msg) else {
        return;
    };
    let hello = match decoded {
        Ok(ClientMessage {
            body: Some(C::Hello(h)),
        }) => h,
        Ok(_) => {
            let _ = send(
                &mut sock,
                server(S::Error(Error {
                    message: "say hello first".into(),
                })),
                enc,
            )
            .await;
            return;
        }
        Err(e) => {
            let _ = send(
                &mut sock,
                server(S::Error(Error {
                    message: format!("not a message: {e}"),
                })),
                enc,
            )
            .await;
            return;
        }
    };
    if !mason_protocol::accepts(hello.protocol) {
        // Too old (or newer than this daemon): say what is needed, in a Welcome the
        // client can still read, so that it can stay read-only and tell its user why.
        let msg = format!(
            "this daemon serves protocols {} to {PROTOCOL}, not {}",
            mason_protocol::MIN_PROTOCOL,
            hello.protocol
        );
        let welcome = Welcome {
            protocol: PROTOCOL,
            seq: 0,
            resume: ResumeKind::UpToDate as i32,
            daemon: "masond".into(),
            demo: d.demo,
            version: d.cfg.runtime_sha.clone().unwrap_or_else(|| "dev".into()),
            min_protocol: mason_protocol::MIN_PROTOCOL,
            owner: false,
            role: String::new(),
            now_ms: crate::daemon::now_ms(),
        };
        let _ = send(&mut sock, server(S::Welcome(welcome)), enc).await;
        let _ = send(&mut sock, server(S::Error(Error { message: msg })), enc).await;
        return;
    }
    // Subscribe to the bus before reading, so nothing slips between.
    let mut bus = d.bus.subscribe();
    let mut progress_bus = d.progress_bus.subscribe();
    let mut revoked = d.lan.revoked.subscribe();
    let mut sub = Sub::default();
    if hello.commands_only {
        sub.views.clear();
    }
    let d2 = d.clone();
    let Ok(last) = on_view_pool(&d, move || d2.store.last_seq().unwrap_or(0)).await else {
        return;
    };
    let behind = last.saturating_sub(hello.last_seq);
    let resume = if hello.commands_only {
        ResumeKind::UpToDate
    } else if hello.last_seq == 0 || hello.last_seq > last || behind > RESUME_LIMIT {
        ResumeKind::Snapshot
    } else if behind == 0 {
        ResumeKind::UpToDate
    } else {
        ResumeKind::Events
    };
    let welcome = Welcome {
        protocol: PROTOCOL,
        seq: last,
        resume: resume as i32,
        daemon: "masond".into(),
        demo: d.demo,
        version: d.cfg.runtime_sha.clone().unwrap_or_else(|| "dev".into()),
        min_protocol: mason_protocol::MIN_PROTOCOL,
        owner: match &peer {
            Some(p) => p.role == mason_core::access::Role::Owner,
            None => crate::attention::is_owner(&d, &hello.actor),
        },
        role: peer.as_ref().map_or("", |p| p.role.as_str()).to_string(),
        now_ms: crate::daemon::now_ms(),
    };
    if !send(&mut sock, server(S::Welcome(welcome)), enc).await {
        return;
    }
    let mut sent = last;
    let mut owed = false;
    match resume {
        ResumeKind::Snapshot => {
            let (d2, sub_views) = (
                d.clone(),
                Sub {
                    views: sub.views.clone(),
                    project: sub.project.clone(),
                    task: sub.task,
                    editor_step: sub.editor_step.clone(),
                },
            );
            // Never an empty snapshot: wait for room, and failing that close, so
            // the client reconnects (and says so) rather than drawing nothing.
            let Ok((seq, views)) = on_view_pool(&d, move || compute(&d2, &sub_views)).await else {
                return;
            };
            sent = seq.max(last);
            if !send(&mut sock, server(S::Snapshot(Snapshot { seq, views })), enc).await {
                return;
            }
        }
        ResumeKind::Events => {
            // Only what was missed, then the views as they stand.
            let (d2, after) = (d.clone(), hello.last_seq);
            let Ok(events) =
                on_view_pool(&d, move || records(&d2, after, RESUME_LIMIT as usize + 1)).await
            else {
                return;
            };
            if let Some(l) = events.last() {
                sent = sent.max(l.seq);
            }
            if !send(&mut sock, server(S::Events(Events { events })), enc).await {
                return;
            }
            match push_views(&d, &mut sock, &sub, enc).await {
                Push::Closed => return,
                Push::Busy => owed = true,
                Push::Sent => {}
            }
        }
        _ => {}
    }

    loop {
        tokio::select! {
            // Views are owed (the pool was busy): try again shortly, no event needed.
            _ = tokio::time::sleep(VIEW_RETRY), if owed => {
                match catch_up(&d, &mut sock, &sub, enc, &mut sent).await {
                    Push::Closed => return,
                    Push::Busy => {}
                    Push::Sent => owed = false,
                }
            }
            incoming = sock.recv() => {
                let Some(Ok(msg)) = incoming else { return };
                if matches!(msg, Message::Close(_)) { return }
                let Some((_, decoded)) = decode(&msg) else { continue };
                match decoded {
                    Err(e) => {
                        if !send(&mut sock, server(S::Error(Error { message: format!("not a message: {e}") })), enc).await { return }
                    }
                    Ok(ClientMessage { body: Some(C::Ping(p)) }) => {
                        if !send(&mut sock, server(S::Pong(Pong { nonce: p.nonce })), enc).await { return }
                    }
                    Ok(ClientMessage { body: Some(C::Subscribe(s)) }) => {
                        sub = Sub {
                            views: s.views.iter().filter_map(|v| ViewKind::try_from(*v).ok()).collect(),
                            project: s.project,
                            task: s.task,
                            editor_step: s.editor_step,
                        };
                        match push_views(&d, &mut sock, &sub, enc).await {
                            Push::Closed => return,
                            Push::Busy => owed = true,
                            Push::Sent => owed = false,
                        }
                    }
                    Ok(ClientMessage { body: Some(C::Command(c)) }) => {
                        let r = run_command_as(&d, c, peer.as_ref()).await;
                        if !send(&mut sock, server(S::Result(r)), enc).await { return }
                    }
                    Ok(_) => {
                        if !send(&mut sock, server(S::Error(Error { message: "unexpected message".into() })), enc).await { return }
                    }
                }
            }
            gone = revoked.recv() => {
                use tokio::sync::broadcast::error::RecvError;
                match (gone, &peer) {
                    (Ok(id), Some(p)) if p.device == id => {
                        let _ = sock.send(Message::Close(None)).await;
                        return;
                    }
                    // Missed some: close, and the device must prove its token again.
                    (Err(RecvError::Lagged(_)), Some(_)) => return,
                    _ => {}
                }
            }
            update = progress_bus.recv() => {
                use tokio::sync::broadcast::error::RecvError;
                let u = match update {
                    Ok(u) => u,
                    // Missed some: the next one carries the whole record anyway.
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return,
                };
                if !wants_progress(&d, &sub, &u) { continue }
                let m = StepProgress {
                    seq: sent,
                    task: u.task,
                    run: u.run,
                    step: u.cell.step.clone(),
                    progress: Some(u.cell),
                    now_ms: u.now_ms,
                    log: u.log,
                };
                if !send(&mut sock, server(S::StepProgress(m)), enc).await { return }
            }
            tick = bus.recv() => {
                match tick {
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => return,
                }
                // A burst of events is one update.
                tokio::time::sleep(Duration::from_millis(10)).await;
                while bus.try_recv().is_ok() {}
                match catch_up(&d, &mut sock, &sub, enc, &mut sent).await {
                    Push::Closed => return,
                    Push::Busy => owed = true,
                    Push::Sent => owed = false,
                }
            }
        }
    }
}

/// Does a subscription show this run's row? The task it watches, or a list
/// (sidebar, home) of its project.
fn wants_progress(d: &Daemon, sub: &Sub, u: &crate::progress::Update) -> bool {
    if sub.views.is_empty() {
        return false;
    }
    if sub.task == u.task {
        return true;
    }
    let lists = sub
        .views
        .iter()
        .any(|v| matches!(v, ViewKind::Sidebar | ViewKind::Home));
    lists
        && (sub.project.is_empty()
            || d.project(ProjectId(u.project))
                .is_some_and(|p| p.id == sub.project))
}

/// How soon to try again when the view pool was full.
const VIEW_RETRY: std::time::Duration = std::time::Duration::from_millis(100);
/// How long a connecting client waits for the view pool before the socket is closed.
const VIEW_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Run `f` on the view pool, waiting a few seconds for room. `Err` means it
/// never ran (the pool stayed full, or the work was lost).
async fn on_view_pool<T: Send + 'static>(
    d: &Arc<Daemon>,
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, crate::pool::PoolError> {
    use crate::pool::PoolError;
    let end = tokio::time::Instant::now() + VIEW_WAIT;
    let f = Arc::new(std::sync::Mutex::new(Some(f)));
    loop {
        let g = f.clone();
        let job = move || g.lock().ok().and_then(|mut o| o.take()).map(|f| f());
        // Accepted but never started (every thread stuck) also runs out of time.
        let r = match tokio::time::timeout_at(end, d.view_pool.run(job)).await {
            Ok(r) => r,
            Err(_) => return Err(PoolError::Busy),
        };
        match r {
            Ok(Some(v)) => return Ok(v),
            Ok(None) | Err(PoolError::Lost) => return Err(PoolError::Lost),
            Err(PoolError::Busy) if tokio::time::Instant::now() < end => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await
            }
            Err(PoolError::Busy) => return Err(PoolError::Busy),
        }
    }
}

/// The events since `sent`, then the views as they stand. `Busy` if either
/// could not be computed yet; `sent` only moves past events that went out.
async fn catch_up(
    d: &Arc<Daemon>,
    sock: &mut WebSocket,
    sub: &Sub,
    e: Encoding,
    sent: &mut u64,
) -> Push {
    let (d2, after) = (d.clone(), *sent);
    let Ok(events) = d.view_pool.run(move || records(&d2, after, 1000)).await else {
        return Push::Busy;
    };
    // (A tick with no new event, such as a pause, still changes views.)
    if !events.is_empty() {
        *sent = events.last().map_or(*sent, |r| r.seq);
        if !send(sock, server(S::Events(Events { events })), e).await {
            return Push::Closed;
        }
    }
    push_views(d, sock, sub, e).await
}

enum Push {
    Sent,
    /// The pool was full or the work lost; the caller owes the views.
    Busy,
    Closed,
}

async fn push_views(d: &Arc<Daemon>, sock: &mut WebSocket, sub: &Sub, e: Encoding) -> Push {
    let (d2, s2) = (
        d.clone(),
        Sub {
            views: sub.views.clone(),
            project: sub.project.clone(),
            task: sub.task,
            editor_step: sub.editor_step.clone(),
        },
    );
    let Ok((seq, views)) = d.view_pool.run(move || compute(&d2, &s2)).await else {
        return Push::Busy;
    };
    for v in views {
        if !send(sock, server(S::View(ViewUpdate { seq, view: Some(v) })), e).await {
            return Push::Closed;
        }
    }
    Push::Sent
}

// ---- commands --------------------------------------------------------------

/// What an HTTP-style call looks like for a command.
struct Call {
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The action this is, if it acts on a task: it must be in the task's
    /// allowed actions.
    needs: Option<(u64, &'static str)>,
}

fn call_for(c: &Command) -> Result<Call, String> {
    let post = |path: String, body: Option<Value>, needs: Option<(u64, &'static str)>| Call {
        method: "POST",
        path,
        body,
        needs,
    };
    Ok(match c.body.as_ref().ok_or("an empty command")? {
        Cmd::AddTask(a) => post(
            "/tasks".into(),
            Some(json!({
                "project": a.project, "title": a.title, "body": a.body,
                "priority": if a.priority.is_empty() { "normal" } else { &a.priority },
                "proposed": a.proposed, "explored": a.explored,
                "after": a.after,
            })),
            None,
        ),
        Cmd::Cancel(t) => post(
            format!("/tasks/{}/cancel", t.task),
            None,
            Some((t.task, "cancel")),
        ),
        Cmd::Retry(r) => post(
            format!("/tasks/{}/retry", r.task),
            Some(if r.from.is_empty() {
                json!({})
            } else {
                json!({"to": r.from})
            }),
            Some((r.task, "retry")),
        ),
        Cmd::Approve(t) => post(
            format!("/tasks/{}/approve", t.task),
            None,
            Some((t.task, "approve")),
        ),
        Cmd::ApproveHold(h) => post(
            format!("/tasks/{}/approve", h.task),
            Some(json!({"diff_hash": h.diff_hash})),
            Some((h.task, "approve")),
        ),
        Cmd::Reject(r) => post(
            format!("/tasks/{}/reject", r.task),
            Some(json!({"reason": r.reason})),
            Some((r.task, "reject")),
        ),
        Cmd::ExploreAnyway(t) => post(
            format!("/tasks/{}/explore-anyway", t.task),
            None,
            Some((t.task, "explore_anyway")),
        ),
        Cmd::Answer(a) => post(
            format!("/tasks/{}/answer", a.task),
            Some(json!({"answer": a.answer, "hash": a.card_hash, "as": c.actor})),
            Some((a.task, "answer")),
        ),
        Cmd::Skip(s) => {
            let mut body = json!({"reason": c.reason, "owner": c.actor, "deferred": s.deferred});
            match &s.scope {
                Some(skip::Scope::Suite(x)) => body["suite"] = json!(x),
                Some(skip::Scope::Step(x)) => body["step"] = json!(x),
                None => return Err("name a suite or a step to skip".into()),
            }
            post(
                format!("/tasks/{}/skip", s.task),
                Some(body),
                Some((s.task, "skip_test")),
            )
        }
        Cmd::LandNow(t) => post(
            format!("/tasks/{}/land-now", t.task),
            Some(json!({"reason": c.reason, "as": c.actor})),
            Some((t.task, "land_now")),
        ),
        Cmd::Force(t) => post(
            format!("/tasks/{}/force", t.task),
            Some(json!({"reason": c.reason, "as": c.actor})),
            Some((t.task, "force")),
        ),
        Cmd::Unblock(u) => post(
            format!("/tasks/{}/unblock", u.task),
            Some(if u.all || u.from.is_empty() {
                json!({"all": true})
            } else {
                json!({"from": u.from})
            }),
            Some((u.task, "unblock")),
        ),
        Cmd::Block(b) => post(
            format!("/tasks/{}/block", b.task),
            Some(json!({"after": b.after})),
            Some((b.task, "block")),
        ),
        Cmd::Revive(t) => post(
            format!("/tasks/{}/revive", t.task),
            None,
            Some((t.task, "revive")),
        ),
        Cmd::Pause(p) => post(
            "/pause".into(),
            Some(
                json!({"project": p.project, "mode": p.mode, "for_s": p.for_seconds, "owner": c.actor}),
            ),
            None,
        ),
        Cmd::Resume(r) => post(
            "/resume".into(),
            Some(json!({"project": r.project, "mode": r.mode})),
            None,
        ),
        Cmd::SetLimit(l) => post(
            "/resources/limit".into(),
            Some(json!({"resource": l.resource, "units": l.units})),
            None,
        ),
        Cmd::Raw(r) => Call {
            method: match r.method.to_uppercase().as_str() {
                "GET" => "GET",
                "POST" => "POST",
                other => return Err(format!("method {other:?} is not part of the API")),
            },
            path: r.path.clone(),
            body: if r.json.is_empty() {
                None
            } else {
                Some(
                    serde_json::from_str(&r.json)
                        .map_err(|e| format!("the body is not JSON: {e}"))?,
                )
            },
            needs: None,
        },
    })
}

fn result(
    id: &str,
    ok: bool,
    status: u32,
    error: impl Into<String>,
    json: String,
) -> CommandResult {
    CommandResult {
        id: id.to_string(),
        ok,
        error: error.into(),
        status,
        json,
    }
}

/// Run a command through the same router the HTTP API uses.
pub async fn run_command(d: &Arc<Daemon>, c: Command) -> CommandResult {
    run_command_as(d, c, None).await
}

/// As `run_command`, for a LAN device: the actor is the token's, never the one the client
/// names, and a viewer's token changes nothing.
pub async fn run_command_as(
    d: &Arc<Daemon>,
    mut c: Command,
    peer: Option<&crate::lan::Peer>,
) -> CommandResult {
    let id = c.id.clone();
    let call = match call_for(&c) {
        Ok(x) => x,
        Err(e) => return result(&id, false, 400, e, String::new()),
    };
    if let Some(p) = peer {
        if let mason_core::access::Access::Refuse(code, why) =
            mason_core::access::decide(p.role, call.method, &call.path)
        {
            return result(&id, false, u32::from(code), why, String::new());
        }
        c.actor = match p.role {
            mason_core::access::Role::Owner => crate::lan::owner_identity(d),
            mason_core::access::Role::Viewer => format!("viewer:{}", p.device),
        };
    }
    // A command outside the task's allowed actions is refused: the list in
    // the view is the one rule.
    if let Some((task, action)) = call.needs {
        let d2 = d.clone();
        let verdict = d
            .pool
            .run(move || {
                let w = d2.world().map_err(|e| e.to_string())?;
                let t = w
                    .tasks
                    .get(&TaskId(task))
                    .ok_or(format!("no task #{task}"))?;
                let ctx = Ctx::new(&d2, &w);
                if ctx.allowed_actions(t).iter().any(|a| a == action) {
                    Ok(())
                } else {
                    Err(format!(
                        "{action} is not available for task #{task} while it is {}",
                        mason_store::state_name(t.state)
                    ))
                }
            })
            .await
            .unwrap_or_else(|_| Err("the daemon is busy".into()));
        if let Err(e) = verdict {
            let status = if e.starts_with("no task") { 404 } else { 409 };
            return result(&id, false, status, e, String::new());
        }
    }
    let mut req = axum::http::Request::builder()
        .method(call.method)
        .uri(&call.path);
    // (Only a request that has a body says it is JSON.)
    if call.body.is_some() {
        req = req.header("content-type", "application/json");
    }
    if !c.actor.is_empty() {
        req = req.header("x-mason-as", c.actor.replace(['\r', '\n'], " "));
    }
    if !c.reason.is_empty() {
        req = req.header("x-mason-reason", c.reason.replace(['\r', '\n'], " "));
    }
    let body = call.body.map_or_else(String::new, |b| b.to_string());
    let Ok(req) = req.body(axum::body::Body::from(body)) else {
        return result(&id, false, 400, "that is not a valid path", String::new());
    };
    let resp = match crate::http::api_router(d.clone()).oneshot(req).await {
        Ok(r) => r,
        Err(never) => match never {},
    };
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes).to_string();
    if status.is_success() {
        result(&id, true, u32::from(status.as_u16()), "", text)
    } else {
        let error = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["error"].as_str().map(String::from))
            .unwrap_or_else(|| format!("HTTP {}", status.as_u16()));
        result(&id, false, u32::from(status.as_u16()), error, text)
    }
}
