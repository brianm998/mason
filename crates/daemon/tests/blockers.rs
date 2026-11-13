// SPDX-License-Identifier: GPL-3.0-or-later

//! Releasing, adding and reviving blockers: owner only over HTTP and the ws command
//! path, a cycle named, a started task refused, and the alert that follows the condition.

mod support;
use mason_core::*;
use mason_daemon::blockers::{self, Which};
use mason_daemon::Daemon;
use serde_json::{json, Value};
use std::sync::Arc;
use support::*;

fn world() -> (Scratch, Arc<Daemon>) {
    let s = Scratch::new("true", json!({"steps": {}}));
    s.write_project("scratch", "true", "");
    let d = s.start();
    mason_daemon::attention::pause(&d, "", "claims", "test", 3600).unwrap();
    (s, d)
}

fn post(d: &Arc<Daemon>, who: &str, path: String, body: Value) -> (u16, Value) {
    let d = d.clone();
    let who = who.to_string();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            use tower::ServiceExt;
            let req = axum::http::Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .header("x-mason-as", who)
                .body(axum::body::Body::from(body.to_string()))
                .unwrap();
            let resp = mason_daemon::http::api_router(d)
                .oneshot(req)
                .await
                .unwrap();
            let status = resp.status().as_u16();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            )
        })
}

fn file_after(d: &Arc<Daemon>, title: &str, after: Vec<TaskId>) -> TaskId {
    let pid = d.project_id_by_name("scratch").unwrap();
    let a = d
        .emit(EventKind::TaskFiled {
            project: pid,
            title: title.into(),
            body: String::new(),
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            explored: false,
            proposed: false,
            blockers: after,
            not_before: None,
        })
        .unwrap();
    a.effects
        .iter()
        .find_map(|e| match e {
            Effect::TaskCreated { task } => Some(*task),
            _ => None,
        })
        .unwrap()
}

fn cancel(d: &Arc<Daemon>, t: TaskId, delete_branch: bool) {
    d.emit(EventKind::Canceled {
        task: t,
        delete_branch,
    })
    .unwrap();
}

fn blocked_alerts(d: &Arc<Daemon>) -> Vec<String> {
    mason_daemon::attention::alerts_now(d)
        .into_iter()
        .filter(|(_, k, _)| k == "blocked")
        .map(|(_, _, t)| t)
        .collect()
}

#[test]
fn a_task_held_by_a_canceled_blocker_is_shown_as_held_and_the_alert_clears_on_release() {
    let (_s, d) = world();
    let owner = d.cfg.owner_names[0].clone();
    let x = file_task(&d, "x");
    let y = file_after(&d, "y", vec![x]);
    cancel(&d, x, false);
    let w = d.world().unwrap();
    let line = blockers::held_line(&w, &w.tasks[&y]).unwrap();
    assert!(
        line.contains("held: blocked by #") && line.contains("(cancelled)"),
        "{line}"
    );
    assert!(
        line.contains("mason task unblock") && line.contains("mason task revive"),
        "{line}"
    );
    let a = blocked_alerts(&d);
    assert_eq!(a.len(), 1);
    assert!(a[0].contains("mason task unblock") && a[0].contains("mason task revive"));
    // Reading the alerts again does not add another.
    assert_eq!(blocked_alerts(&d).len(), 1);
    blockers::release(&d, &owner, y, Which::All).unwrap();
    assert!(blocked_alerts(&d).is_empty());
    assert_eq!(task_of(&d, y).activity, Activity::Waiting(WaitReason::Slot));
}

#[test]
fn reviving_the_blocker_clears_the_alert_and_retry_on_a_canceled_task_is_refused() {
    let (_s, d) = world();
    let owner = d.cfg.owner_names[0].clone();
    let x = file_task(&d, "x");
    let y = file_after(&d, "y", vec![x]);
    cancel(&d, x, false);
    let (code, _) = post(&d, &owner, format!("/tasks/{}/retry", x.0), json!({}));
    assert_eq!(code, 409);
    assert_eq!(blocked_alerts(&d).len(), 1);
    blockers::revive(&d, &owner, x).unwrap();
    assert_eq!(state_of(&d, x), State::Queued);
    assert!(blocked_alerts(&d).is_empty());
    assert!(matches!(task_of(&d, y).activity, Activity::Held(_)));
}

#[test]
fn a_failed_blocker_is_retried_not_revived() {
    let (_s, d) = world();
    let owner = d.cfg.owner_names[0].clone();
    let x = file_task(&d, "x");
    let y = file_after(&d, "y", vec![x]);
    d.emit(EventKind::Admitted {
        task: x,
        entry: None,
    })
    .unwrap();
    d.emit(EventKind::StepFailed {
        task: x,
        reason: "r".into(),
        class: None,
    })
    .unwrap();
    let w = d.world().unwrap();
    let line = blockers::held_line(&w, &w.tasks[&y]).unwrap();
    assert!(
        line.contains("(failed)") && line.contains("mason task retry"),
        "{line}"
    );
    let links = blockers::links(&d, &w, &w.tasks[&y]);
    assert_eq!(links[0].actions, vec!["release", "retry"]);
    let e = blockers::revive(&d, &owner, x).unwrap_err();
    assert!(e.1.contains("mason task retry"), "{e:?}");
    // A failed task cannot be added as a blocker either.
    let z = file_task(&d, "z");
    assert_eq!(blockers::block(&d, &owner, z, &[x]).unwrap_err().0, 409);
}

#[test]
fn revive_refuses_a_task_whose_branch_was_deleted() {
    let (_s, d) = world();
    let owner = d.cfg.owner_names[0].clone();
    let x = file_task(&d, "x");
    cancel(&d, x, true);
    let e = blockers::revive(&d, &owner, x).unwrap_err();
    assert_eq!(e.0, 409);
}

#[test]
fn only_the_owner_may_change_blockers_over_http_and_ws() {
    let (_s, d) = world();
    let owner = d.cfg.owner_names[0].clone();
    let a = file_task(&d, "a");
    let b = file_task(&d, "b");
    let c = file_after(&d, "c", vec![a]);
    cancel(&d, b, false);
    for who in ["stranger", ""] {
        assert_eq!(
            post(
                &d,
                who,
                format!("/tasks/{}/unblock", c.0),
                json!({"all": true})
            )
            .0,
            403
        );
        assert_eq!(
            post(
                &d,
                who,
                format!("/tasks/{}/block", c.0),
                json!({"after": [b.0]})
            )
            .0,
            403
        );
        assert_eq!(
            post(&d, who, format!("/tasks/{}/revive", b.0), json!({})).0,
            403
        );
    }
    assert_eq!(task_of(&d, c).blockers.len(), 1);
    assert_eq!(state_of(&d, b), State::Canceled);
    // The ws path asks for the same identity.
    let cmd = mason_protocol::Command {
        id: "1".into(),
        actor: "stranger".into(),
        body: Some(mason_protocol::command::Body::Unblock(
            mason_protocol::Unblock {
                task: c.0,
                from: vec![],
                all: true,
            },
        )),
        ..Default::default()
    };
    let r = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(mason_daemon::ws::run_command(&d, cmd.clone()));
    assert!(!r.ok);
    assert_eq!(r.status, 403, "{}", r.error);
    let mut ok = cmd;
    ok.actor = owner.clone();
    let r = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(mason_daemon::ws::run_command(&d, ok));
    assert!(r.ok, "{}", r.error);
    assert!(task_of(&d, c).blockers.is_empty());
    // And the owner's revive works over HTTP.
    assert_eq!(
        post(&d, &owner, format!("/tasks/{}/revive", b.0), json!({})).0,
        200
    );
}

#[test]
fn a_cycle_is_refused_naming_it_and_a_started_task_is_refused() {
    let (_s, d) = world();
    let owner = d.cfg.owner_names[0].clone();
    let a = file_task(&d, "a");
    let b = file_after(&d, "b", vec![a]);
    let (code, v) = post(
        &d,
        &owner,
        format!("/tasks/{}/block", a.0),
        json!({"after": [b.0]}),
    );
    assert_eq!(code, 422);
    let text = v["error"].as_str().unwrap();
    assert!(
        text.contains("cycle")
            && text.contains(&format!("#{}", a.0))
            && text.contains(&format!("#{}", b.0)),
        "{text}"
    );
    let c = file_task(&d, "c");
    d.emit(EventKind::Admitted {
        task: c,
        entry: None,
    })
    .unwrap();
    let (code, _) = post(
        &d,
        &owner,
        format!("/tasks/{}/block", c.0),
        json!({"after": [a.0]}),
    );
    assert_eq!(code, 409);
    let (code, _) = post(
        &d,
        &owner,
        format!("/tasks/{}/unblock", c.0),
        json!({"all": true}),
    );
    assert_eq!(code, 409);
    // Releasing something that is not a blocker is refused, and says so.
    let (code, v) = post(
        &d,
        &owner,
        format!("/tasks/{}/unblock", b.0),
        json!({"from": [c.0]}),
    );
    assert_eq!(code, 409);
    assert!(v["error"].as_str().unwrap().contains("not a blocker"));
}

#[test]
fn block_adds_a_second_blocker_and_the_task_detail_lists_each_blocker_with_its_origin() {
    let (_s, d) = world();
    let owner = d.cfg.owner_names[0].clone();
    let p1 = file_task(&d, "p1");
    let p2 = file_task(&d, "p2");
    let t = file_after(&d, "t", vec![p1]);
    let (code, v) = post(
        &d,
        &owner,
        format!("/tasks/{}/block", t.0),
        json!({"after": [p2.0]}),
    );
    assert_eq!(code, 200, "{v}");
    let w = d.world().unwrap();
    let links = blockers::links(&d, &w, &w.tasks[&t]);
    assert_eq!(links.len(), 2);
    assert!(links
        .iter()
        .all(|l| l.origin == "set by you" && l.actions.contains(&"release")));
    let (code, _) = post(
        &d,
        &owner,
        format!("/tasks/{}/unblock", t.0),
        json!({"from": [p1.0]}),
    );
    assert_eq!(code, 200);
    assert_eq!(task_of(&d, t).blockers.len(), 1);
}
