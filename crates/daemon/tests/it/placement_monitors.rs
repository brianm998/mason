// SPDX-License-Identifier: GPL-3.0-or-later

//! #834: the monitors show the placement mode and where it came from (task, project, global),
//! and the owner sets it from them. A non-owner's set is refused.

use crate::support;
use mason_core::*;
use mason_daemon::views::Ctx;
use serde_json::{json, Value};
use std::sync::Arc;
use support::*;

const REMOTE_RUNS: &str =
    "[remote_runs]\nkind = \"github-actions\"\nrepo = \"o/r\"\ntoken = \"env:MASON_T834_TOKEN\"\n";

fn file(d: &Arc<mason_daemon::Daemon>, title: &str) -> TaskId {
    let pid = d.project_id_by_name("scratch").unwrap();
    let a = d
        .emit(EventKind::TaskFiled {
            project: pid,
            title: title.into(),
            body: "b".into(),
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            explored: false,
            proposed: false,
            blockers: vec![],
            // Far ahead, so nothing is admitted while the test runs.
            not_before: Some(Timestamp(u64::MAX / 4)),
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

fn post(d: &Arc<mason_daemon::Daemon>, who: &str, path: String, body: Value) -> (u16, Value) {
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

/// The scratch comes back with the daemon: dropping it deletes the directory the daemon's
/// config and data live in, and every later write there fails with "No such file".
fn started() -> (Scratch, Arc<mason_daemon::Daemon>) {
    let s = Scratch::new("true", happy_script());
    s.write_project("scratch", "true", REMOTE_RUNS);
    let d = s.start();
    (s, d)
}

fn project_file(d: &Arc<mason_daemon::Daemon>) -> String {
    std::fs::read_to_string(d.cfg.config_dir.join("projects/scratch.toml")).unwrap()
}

#[test]
fn the_task_project_and_home_views_carry_the_mode_and_its_source() {
    use mason_daemon::place::{set_global, set_project_mode, Mode};
    let (_s, d) = started();
    let t = file(&d, "one");
    let other = file(&d, "two");
    let now = mason_daemon::daemon::now_ms();

    let views = |d: &Arc<mason_daemon::Daemon>, task: TaskId| {
        let w = d.world().unwrap();
        let ctx = Ctx::new(d, &w);
        (
            ctx.home("").placement.unwrap(),
            ctx.header("")
                .project_placements
                .into_iter()
                .find(|p| p.project == "scratch")
                .unwrap()
                .placement
                .unwrap(),
            ctx.task_detail(task.0).unwrap(),
        )
    };

    // Nothing set: everything follows the global switch.
    let (home, project, detail) = views(&d, t);
    assert_eq!(
        (home.mode.as_str(), home.source.as_str()),
        ("local", "global")
    );
    assert_eq!(
        (project.mode.as_str(), project.source.as_str(), project.own),
        ("local", "global", false)
    );
    let p = detail.placement.unwrap();
    assert_eq!(
        (p.mode.as_str(), p.source.as_str(), p.own),
        ("local", "global", false)
    );
    assert_eq!(detail.row.unwrap().run_on_override, "");

    // The global switch.
    set_global(&d, Mode::FavorLocal, "brian").unwrap();
    let (home, project, detail) = views(&d, t);
    assert_eq!(home.mode, "favor-local");
    assert_eq!(project.mode, "favor-local");
    assert_eq!(detail.placement.unwrap().source, "global");

    // The project's own mode.
    set_project_mode(&d, "scratch", Mode::FavorRemote).unwrap();
    let (home, project, detail) = views(&d, t);
    assert_eq!(home.mode, "favor-local", "home is the global switch");
    assert_eq!(
        (project.mode.as_str(), project.source.as_str(), project.own),
        ("favor-remote", "project", true)
    );
    let p = detail.placement.unwrap();
    assert_eq!(
        (p.mode.as_str(), p.source.as_str()),
        ("favor-remote", "project")
    );
    let w = d.world().unwrap();
    assert_eq!(
        Ctx::new(&d, &w)
            .home("scratch")
            .project_placement
            .unwrap()
            .mode,
        "favor-remote"
    );

    // The task's own choice, and the marker on its row only.
    d.store
        .set_task_override(t.0, "run_on", "local", "brian", now)
        .unwrap();
    let (_, _, detail) = views(&d, t);
    let p = detail.placement.unwrap();
    assert_eq!(
        (p.mode.as_str(), p.source.as_str(), p.own),
        ("local", "task", true)
    );
    assert_eq!(detail.row.unwrap().run_on_override, "local");
    let (_, _, detail) = views(&d, other);
    assert_eq!(detail.row.unwrap().run_on_override, "");
    assert_eq!(detail.placement.unwrap().source, "project");
}

#[test]
fn a_non_owners_set_is_refused_and_the_owners_project_set_shows_source_project() {
    let (_s, d) = started();
    let t = file(&d, "one");
    let owner = d.cfg.owner_names[0].clone();
    let before = project_file(&d);

    for who in ["", "someone-else"] {
        for (path, body) in [
            ("/remote".to_string(), json!({"mode": "remote"})),
            (
                "/remote".to_string(),
                json!({"mode": "remote", "project": "scratch"}),
            ),
            (format!("/tasks/{}/run_on", t.0), json!({"to": "remote"})),
        ] {
            let (code, _) = post(&d, who, path.clone(), body);
            assert_eq!(code, 403, "{who:?} {path}");
        }
    }
    assert_eq!(project_file(&d), before, "nothing was written");

    let (code, v) = post(
        &d,
        &owner,
        "/remote".into(),
        json!({"mode": "favor-remote", "project": "scratch"}),
    );
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["source"], "project", "{v}");
    let text = project_file(&d);
    assert!(text.contains("mode = \"favor-remote\""), "{text}");
    let w = d.world().unwrap();
    let h = Ctx::new(&d, &w).header("");
    let p = h
        .project_placements
        .iter()
        .find(|p| p.project == "scratch")
        .unwrap()
        .placement
        .clone()
        .unwrap();
    assert_eq!(
        (p.mode.as_str(), p.source.as_str()),
        ("favor-remote", "project")
    );

    // The owner sets a task; "Follow project" and "Follow MASON" clear each level.
    let (code, v) = post(
        &d,
        &owner,
        format!("/tasks/{}/run_on", t.0),
        json!({"to": "local"}),
    );
    assert_eq!(code, 200, "{v}");
    let (code, _) = post(
        &d,
        &owner,
        format!("/tasks/{}/run_on", t.0),
        json!({"to": "default"}),
    );
    assert_eq!(code, 200);
    let (code, v) = post(
        &d,
        &owner,
        "/remote".into(),
        json!({"mode": "default", "project": "scratch"}),
    );
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["source"], "global", "{v}");
    let text = project_file(&d);
    assert!(!text.contains("mode ="), "{text}");
}
