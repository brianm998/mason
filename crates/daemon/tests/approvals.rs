// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 20: the owner decides by clicking. The monitor's approval view and its
//! Approve and Reject buttons: what a self-hosting hold shows, the owner flag the
//! welcome carries, the diff hash an approval is for, a stale one refused, and a
//! rejected hold that cancels the task.

mod support;
use mason_core::*;
use mason_daemon::views::Ctx;
use mason_protocol::client::Client;
use mason_protocol::*;
use std::sync::Arc;
use std::time::Duration;
use support::lw::*;
use support::*;

fn owner(w: &W) -> String {
    w.d.cfg.owner_names[0].clone()
}

fn serve(d: &Arc<mason_daemon::Daemon>) -> u16 {
    let d2 = d.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let _ = rt.block_on(mason_daemon::http::serve(d2));
    });
    let info = d.cfg.info_path();
    let mut port = 0;
    wait_for("the socket to be served", 20, || {
        std::fs::read_to_string(&info)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v["port"].as_u64())
            .map(|p| port = p as u16)
            .is_some()
    });
    port
}

fn held_world() -> (W, TaskId) {
    let w = world_with(
        "mode = \"direct\"\nwindow = \"0s\"\n",
        &|_| "[self]\n".to_string(),
        &[("crates/daemon/src/protect.rs", "// the rules\n")],
    );
    let t = w.task(
        "edit a verdict",
        &[("crates/testing/src/verdict.rs", "// weaker\n")],
    );
    wait_for("the hold", 30, || w.d.store.hold(t.0).unwrap().is_some());
    wait_for("the wait to show", 30, || {
        task_of(&w.d, t).activity != Activity::Running
    });
    (w, t)
}

fn as_owner(c: &mut Client, who: &str) -> bool {
    c.acting_as(who);
    c.hello(0, true).unwrap().owner
}

fn approve_hold(task: u64, hash: &str) -> Command {
    Command {
        id: String::new(),
        body: Some(command::Body::ApproveHold(ApproveHold {
            task,
            diff_hash: hash.into(),
        })),
        actor: String::new(),
        reason: String::new(),
    }
}

fn with_actor(mut c: Command, who: &str) -> Command {
    c.actor = who.into();
    c
}

#[test]
fn the_welcome_says_whether_the_connection_is_the_owner() {
    let (w, _t) = held_world();
    let port = serve(&w.d);
    let mut a = Client::connect(port, Encoding::Json).unwrap();
    assert!(as_owner(&mut a, &owner(&w)));
    let mut b = Client::connect(port, Encoding::Protobuf).unwrap();
    assert!(!as_owner(&mut b, "mallory"));
    let mut c = Client::connect(port, Encoding::Json).unwrap();
    assert!(!as_owner(&mut c, ""), "nobody is not the owner");
}

#[test]
fn a_hold_shows_what_it_waits_on_and_counts_as_needing_the_owner() {
    let (w, t) = held_world();
    let hold = w.d.store.hold(t.0).unwrap().unwrap();
    let world = w.d.world().unwrap();
    let ctx = Ctx::new(&w.d, &world);
    let detail = ctx.task_detail(t.0).unwrap();
    let a = detail.approval.expect("an approval view");
    assert_eq!(a.kind, "hold");
    assert_eq!(a.diff_hash, hold.diff_hash);
    assert!(
        a.lines
            .iter()
            .any(|l| l.contains("crates/testing/src/verdict.rs")),
        "{:?}",
        a.lines
    );
    assert!(a.diff_command.contains("git ") && a.diff_command.contains(&a.branch));
    let acts = detail.row.unwrap().allowed_actions;
    assert!(
        acts.contains(&"approve".to_string()) && acts.contains(&"reject".to_string()),
        "{acts:?}"
    );
    let header = ctx.header("");
    assert_eq!(header.attention, 1, "{header:?}");
    let home = ctx.home("");
    assert_eq!(home.attention.len(), 1);
    assert!(home.attention[0].approval.is_some() && home.attention[0].card.is_none());
}

#[test]
fn approving_sends_the_diff_shown_and_a_different_one_is_refused() {
    let (w, t) = held_world();
    let port = serve(&w.d);
    let mut c = Client::connect(port, Encoding::Json).unwrap();
    let who = owner(&w);
    assert!(as_owner(&mut c, &who));
    let hash = w.d.store.hold(t.0).unwrap().unwrap().diff_hash;
    let (r, _) = c
        .command(
            with_actor(approve_hold(t.0, "not-the-diff"), &who),
            Duration::from_secs(5),
        )
        .unwrap();
    assert!(!r.ok && r.status == 409, "{r:?}");
    assert!(w.d.store.hold(t.0).unwrap().unwrap().approved_by.is_none());
    // Not the owner: refused, however right the hash.
    let (r, _) = c
        .command(
            with_actor(approve_hold(t.0, &hash), "mallory"),
            Duration::from_secs(5),
        )
        .unwrap();
    assert!(!r.ok && r.status == 403, "{r:?}");
    let (r, _) = c
        .command(
            with_actor(approve_hold(t.0, &hash), &who),
            Duration::from_secs(5),
        )
        .unwrap();
    assert!(r.ok, "{r:?}");
    wait_for("it to land", 60, || w.landed(t));
}

#[test]
fn rejecting_a_hold_cancels_the_task_with_the_reason() {
    let (w, t) = held_world();
    let port = serve(&w.d);
    let mut c = Client::connect(port, Encoding::Json).unwrap();
    let who = owner(&w);
    let reject = |actor: &str| Command {
        id: String::new(),
        body: Some(command::Body::Reject(Reject {
            task: t.0,
            reason: "too broad".into(),
        })),
        actor: actor.into(),
        reason: String::new(),
    };
    assert!(as_owner(&mut c, &who));
    let (r, _) = c
        .command(reject("mallory"), Duration::from_secs(5))
        .unwrap();
    assert!(!r.ok && r.status == 403, "{r:?}");
    assert_ne!(task_of(&w.d, t).state, State::Canceled);
    let (r, _) = c.command(reject(&who), Duration::from_secs(5)).unwrap();
    assert!(r.ok, "{r:?}");
    wait_for("it to be canceled", 30, || {
        task_of(&w.d, t).state == State::Canceled
    });
    assert!(!w.landed(t));
}
