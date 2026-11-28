// SPDX-License-Identifier: GPL-3.0-or-later

//! Pipeline self-correction (s432, decision 0425): a seeded stuck task makes one health tick
//! start one health session; its verdicts file proposals, raise a banner or nothing; the direct
//! fix is gated on the daemon's own finding. Driven by calling `tick` with a time of its own and
//! by files the scripted agent waits for, never by sleeping.

use crate::support;
use mason_daemon::health;
use mason_daemon::views::Ctx;
use mason_daemon::Daemon;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use support::*;

const H: u64 = 3_600_000;

fn gate_dir() -> (tempfile::TempDir, PathBuf) {
    let g = tempfile::Builder::new()
        .prefix("gate")
        .tempdir_in("/tmp")
        .unwrap();
    let p = g.path().join("open");
    (g, p)
}

fn held(gate: &Path) -> Value {
    json!([
        {"sh": format!("while [ ! -f {} ]; do sleep 0.05; done", gate.display())},
        {"say": "done"}
    ])
}

fn submit(args: Value) -> Value {
    json!([{"call": {"tool": "submit_health", "args": args}}])
}

/// A world whose only project is MASON's own, with a developing task held at a gate.
fn world(health_turns: Value) -> (Scratch, Arc<Daemon>, mason_core::TaskId, tempfile::TempDir) {
    world_with(
        health_turns,
        "[self]\nmax_open = 50\nper_week = 50\nmax_steps = 500\n",
    )
}

fn world_with(
    health_turns: Value,
    self_cfg: &str,
) -> (Scratch, Arc<Daemon>, mason_core::TaskId, tempfile::TempDir) {
    let (g, gate) = gate_dir();
    let s = Scratch::new(
        "test -f hello.txt",
        json!({"steps": {"explore": [explore_turn()], "develop": [held(&gate)], "health": health_turns}}),
    );
    s.write_project("scratch", "test -f hello.txt", self_cfg);
    let d = s.start_with(|c| c.owner_names = vec!["brian".to_string()]);
    let t = file_task_as(
        &d,
        "Slow",
        "Takes long.",
        true,
        mason_core::TrustSource::Owner,
        false,
    );
    wait_state(&d, t, mason_core::State::Developing, 60);
    (s, d, t, g)
}

fn wait_verdict(d: &Arc<Daemon>, sid: u64) {
    wait_for("the verdict", 60, || {
        !d.store.health_since(&["verdict"], 0).unwrap().is_empty()
            && d.store
                .owner_session(sid)
                .unwrap()
                .is_some_and(|s| s.state != "working")
    });
}

fn later() -> u64 {
    mason_daemon::daemon::now_ms() + 10 * H
}

#[test]
fn a_stuck_task_starts_one_session_and_a_second_tick_starts_none() {
    let (_s, d, t, _g) = world(json!([submit(json!({
        "summary": "the task waits on a gate",
        "needs_owner": {"question": "Cancel the slow task?", "recommendation": "Cancel it"}
    }))]));
    let now = later();
    let tick = health::tick(&d, now);
    assert!(
        tick.fresh.contains(&format!("stuck:task:{}", t.0)),
        "{:?}",
        tick.findings
    );
    let sid = tick.session.expect("a session starts");
    let s = d.store.owner_session(sid).unwrap().unwrap();
    assert_eq!((s.kind.as_str(), s.task), ("health", None));
    wait_verdict(&d, sid);
    // Not again: the same findings are within their cooldown and a session is open.
    let again = health::tick(&d, now + 60_000);
    assert!(again.fresh.is_empty(), "{:?}", again.fresh);
    assert!(again.session.is_none());
    assert_eq!(
        d.store
            .open_owner_sessions()
            .unwrap()
            .iter()
            .filter(|s| s.kind == "health")
            .count(),
        1
    );
    // The history says what happened.
    let h = health::history_json(&d, 20);
    let kinds: Vec<&str> = h["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["kind"].as_str().unwrap())
        .collect();
    assert!(
        kinds.contains(&"check") && kinds.contains(&"session") && kinds.contains(&"verdict"),
        "{kinds:?}"
    );
}

#[test]
fn needs_owner_raises_a_banner_with_the_session_and_ending_it_removes_it() {
    let (_s, d, _t, _g) = world(json!([submit(json!({
        "summary": "stuck",
        "needs_owner": {"question": "Cancel the slow task?", "recommendation": "Cancel it"}
    }))]));
    let sid = health::tick(&d, later()).session.unwrap();
    wait_verdict(&d, sid);
    let w = d.world().unwrap();
    let ctx = Ctx::new(&d, &w);
    let h = ctx.header("");
    let b = h
        .banners
        .iter()
        .find(|b| b.kind == "health")
        .expect("a health banner");
    assert_eq!(b.session, sid);
    assert!(b.text.contains("Cancel the slow task?"), "{}", b.text);
    // The sidebar panel lists it, waiting for the owner.
    let a = h.active_sessions.iter().find(|a| a.id == sid).unwrap();
    assert_eq!(
        (a.kind.as_str(), a.label.as_str(), a.waiting),
        ("health", "Pipeline health", true)
    );
    mason_daemon::usersession::end(&d, sid, "brian").unwrap();
    let w = d.world().unwrap();
    let h = Ctx::new(&d, &w).header("");
    assert!(h.banners.iter().all(|b| b.kind != "health"));
    assert!(h.active_sessions.iter().all(|a| a.id != sid));
}

#[test]
fn a_false_alarm_raises_no_banner_and_ends_the_session() {
    let (_s, d, _t, _g) = world(json!([submit(json!({
        "summary": "the gate is a test's",
        "false_alarm": {"finding": "stuck:task", "why": "a test holds it"}
    }))]));
    let sid = health::tick(&d, later()).session.unwrap();
    wait_for("the session to end", 60, || {
        d.store
            .owner_session(sid)
            .unwrap()
            .is_some_and(|s| s.state == "ended")
    });
    let w = d.world().unwrap();
    let h = Ctx::new(&d, &w).header("");
    assert!(h.banners.iter().all(|b| b.kind != "health"));
}

#[test]
fn proposed_files_one_proposal_per_solution_and_a_repeat_files_none() {
    let one = json!({"finding": "stuck:queue", "title": "Let the queue start work after a stall",
        "body": "The queue held work while slots were free.",
        "evidence": "README:1 shows the scratch repository the finding was seen in."});
    let turn = submit(json!({"summary": "queue", "proposed": [one.clone()]}));
    let (_s, d, _t, _g) = world(json!([turn.clone(), turn]));
    let pid = d.project_id_by_name("scratch").unwrap();
    let id = d.project(pid).unwrap().id.clone();
    let before = d.store.proposals_in(&id).unwrap().len();
    let sid = health::tick(&d, later()).session.unwrap();
    wait_for("the session to end", 60, || {
        d.store
            .owner_session(sid)
            .unwrap()
            .is_some_and(|s| s.state == "ended")
    });
    let rows = d.store.proposals_in(&id).unwrap();
    assert_eq!(rows.len(), before + 1, "{rows:?}");
    // The same proposal again, as a later session would submit it: not filed twice.
    let sid2 = d
        .store
        .add_owner_session(None, "health", "default", "health", 1)
        .unwrap();
    health::apply(
        &d,
        sid2,
        pid,
        &json!({"summary": "again", "proposed": [one]}),
        later(),
    );
    assert_eq!(d.store.proposals_in(&id).unwrap().len(), before + 1);
}

#[test]
fn acting_is_logged_and_only_owner_safe_actions_pass_the_contract() {
    use mason_adapter_api::contract::validate;
    assert!(validate(
        "submit_health",
        &json!({"summary": "s", "acted": [{"action": "delete_branch", "task": 1, "reason": "x"}]})
    )
    .is_err());
    let (_s, d, t, _g) = world(json!([submit(
        json!({"summary": "x", "false_alarm": {"why": "w"}})
    )]));
    let pid = d.project_id_by_name("scratch").unwrap();
    let sid = d
        .store
        .add_owner_session(None, "health", "default", "health", 1)
        .unwrap();
    // A retry of a task that is not failed is refused by the daemon, not done; both are logged.
    health::apply(
        &d,
        sid,
        pid,
        &json!({"summary": "s", "acted": [{"action": "retry", "task": t.0, "reason": "try"}]}),
        later(),
    );
    let msgs = d.store.owner_messages(sid, 0).unwrap();
    assert!(msgs.iter().any(|m| m.text.contains("refused")), "{msgs:?}");
    assert!(d
        .store
        .interventions(0)
        .unwrap()
        .iter()
        .any(|i| i.kind == "health_retry" && i.who == "health"));
}

#[test]
fn a_direct_fix_needs_the_daemons_own_nothing_can_land_finding() {
    let (_s, d, _t, _g) = world(json!([submit(
        json!({"summary": "x", "false_alarm": {"why": "w"}})
    )]));
    let pid = d.project_id_by_name("scratch").unwrap();
    let sid = d
        .store
        .add_owner_session(None, "health", "default", "health", 1)
        .unwrap();
    // Landing works (nothing has failed to land): the session's word is not enough.
    let needs = health::apply(
        &d,
        sid,
        pid,
        &json!({"summary": "s", "direct_fix": {"summary": "change a thing"}}),
        later(),
    );
    assert!(needs, "a refused direct fix asks the owner");
    let msgs = d.store.owner_messages(sid, 0).unwrap();
    assert!(
        msgs.iter()
            .any(|m| m.text.contains("can still take in a task")),
        "{msgs:?}"
    );
    assert!(d.store.health_since(&["direct_fix"], 0).unwrap().is_empty());
}

fn fix_world(change: &str) -> (Scratch, Arc<Daemon>, tempfile::TempDir) {
    let (g, gate) = gate_dir();
    let s = Scratch::new(
        "test -f hello.txt",
        json!({"steps": {
            "explore": [explore_turn()], "develop": [held(&gate)],
            "chat": [[{"sh": format!(
                "mkdir -p $(dirname {change}) && echo fix > {change} && git add -A && \
                 git -c user.name=t -c user.email=t@t commit -qm fix")}, {"say": "committed"}]]}}),
    );
    s.write_project(
        "scratch",
        "test -f hello.txt",
        "[self]\nmax_open = 50\nper_week = 50\nmax_steps = 500\n",
    );
    let d = s.start_with(|c| c.owner_names = vec!["brian".to_string()]);
    (s, d, g)
}

/// The self-update has failed three checks in a row and does not take over: nothing can land.
fn break_the_update(d: &Arc<Daemon>) {
    d.update_status.lock().unwrap().phase = "failed".into();
    let now = mason_daemon::daemon::now_ms();
    for _ in 0..3 {
        d.store
            .add_health("update_failed", "", None, "", now)
            .unwrap();
    }
}

fn direct_fix(d: &Arc<Daemon>) -> (bool, Vec<String>) {
    let pid = d.project_id_by_name("scratch").unwrap();
    let sid = d
        .store
        .add_owner_session(None, "health", "default", "health", 1)
        .unwrap();
    let needs = health::apply(
        d,
        sid,
        pid,
        &json!({"summary": "s", "direct_fix": {"summary": "fix the update"}}),
        later(),
    );
    let msgs = d.store.owner_messages(sid, 0).unwrap();
    (needs, msgs.into_iter().map(|m| m.text).collect())
}

#[test]
fn a_direct_fix_is_allowed_when_nothing_can_land_and_hands_the_branch_to_the_owner() {
    let (_s, d, _g) = fix_world("src/fix.txt");
    break_the_update(&d);
    let (needs, msgs) = direct_fix(&d);
    assert!(needs);
    let line = msgs
        .iter()
        .find(|m| m.contains("claude/health-"))
        .expect("a branch");
    assert!(line.contains("did not push or land"), "{line}");
    assert!(!line.contains("protected"), "{line}");
    assert_eq!(
        d.store.health_since(&["direct_fix_done"], 0).unwrap().len(),
        1
    );
    // One at a time, and the daemon's finding is read afresh each time.
    let (_, again) = direct_fix(&d);
    assert!(
        again
            .iter()
            .any(|m| m.contains("already under way") || m.contains("claude/health-")),
        "{again:?}"
    );
}

#[test]
fn a_direct_fix_to_a_protected_path_still_waits_for_the_owner() {
    let (_s, d, _g) = fix_world("scripts/exit/s999.sh");
    break_the_update(&d);
    let (needs, msgs) = direct_fix(&d);
    assert!(needs);
    assert!(
        msgs.iter()
            .any(|m| m.contains("protected paths and waits for your approval")),
        "{msgs:?}"
    );
}

/// The stage's exit flow, as `scripts/exit/s432.sh` runs it: a stuck task, one tick, a session,
/// `needs_owner` with its banner and count, `proposed` with its proposal, the end of the session.
#[test]
fn exit_flow_stuck_task_session_banner_proposal_end() {
    let (_s, d, t, _g) = world(json!([submit(json!({
        "summary": "the task is stuck",
        "needs_owner": {"question": "Cancel the stuck task?", "recommendation": "Cancel it"}
    }))]));
    let pid = d.project_id_by_name("scratch").unwrap();
    let id = d.project(pid).unwrap().id.clone();
    let tick = health::tick(&d, later());
    assert!(tick.fresh.contains(&format!("stuck:task:{}", t.0)));
    let sid = tick.session.expect("a health session starts");
    wait_verdict(&d, sid);
    let view = |d: &Arc<Daemon>| {
        let w = d.world().unwrap();
        Ctx::new(d, &w).header("")
    };
    let h = view(&d);
    assert!(h
        .banners
        .iter()
        .any(|b| b.kind == "health" && b.session == sid));
    assert_eq!(h.active_sessions.len(), 1, "Active sessions (1)");
    // The session proposes a fix for the finding.
    let before = d.store.proposals_in(&id).unwrap().len();
    health::apply(
        &d,
        sid,
        pid,
        &json!({"summary": "p", "proposed": [{"finding": format!("stuck:task:{}", t.0),
            "title": "Stop a stuck step from holding its task",
            "body": "A step that makes no progress should be ended.",
            "evidence": "README:1 is the repository the finding was seen in."}]}),
        later(),
    );
    assert_eq!(d.store.proposals_in(&id).unwrap().len(), before + 1);
    mason_daemon::usersession::end(&d, sid, "brian").unwrap();
    let h = view(&d);
    assert!(h.banners.iter().all(|b| b.kind != "health"));
    assert!(h.active_sessions.is_empty());
}

#[test]
fn a_busy_task_is_stuck_long_and_the_prompt_names_the_restart() {
    let (_s, d, t, _g) = world(json!([submit(json!({
        "summary": "long", "false_alarm": {"finding": "stuck:long", "why": "x"}
    }))]));
    // Seven full rounds in the current run, whatever the time.
    for i in 0..7u64 {
        d.store
            .put_check_run(t.0, 1, 1_000 + i, false, &[])
            .unwrap();
    }
    let (_x, findings) = health::report(&d, mason_daemon::daemon::now_ms());
    let f = findings
        .iter()
        .find(|f| f.key == format!("stuck:long:{}", t.0))
        .unwrap_or_else(|| panic!("{findings:?}"));
    assert!(
        f.evidence.iter().any(|e| e.contains("7 full check rounds")),
        "{f:?}"
    );
    let text = health::prompt(
        &d,
        &findings,
        std::slice::from_ref(f),
        mason_daemon::daemon::now_ms(),
    );
    assert!(text.contains(&format!("stuck:long:{}", t.0)) && text.contains("restart_round"));
}

#[test]
fn a_needs_owner_session_does_not_block_a_new_finding_and_times_out() {
    let ask = submit(json!({"summary": "s",
        "needs_owner": {"question": "Cancel?", "recommendation": "Cancel it"}}));
    let (_s, d, t, _g) = world(json!([ask.clone(), ask]));
    let now = later();
    let sid = health::tick(&d, now).session.unwrap();
    wait_verdict(&d, sid);
    // A second task stalls: a different key, so a new session starts although #sid is open.
    let t2 = file_task_as(
        &d,
        "Slow two",
        "Takes long.",
        true,
        mason_core::TrustSource::Owner,
        false,
    );
    wait_state(&d, t2, mason_core::State::Developing, 60);
    let tick = health::tick(&d, now + 60_000);
    assert!(
        tick.fresh.contains(&format!("stuck:task:{}", t2.0)),
        "{:?}",
        tick.fresh
    );
    assert!(!tick.fresh.contains(&format!("stuck:task:{}", t.0)));
    let sid2 = tick.session.expect("a new session starts");
    assert_ne!(sid, sid2);
    wait_for("the second verdict", 60, || {
        d.store
            .owner_session(sid2)
            .unwrap()
            .is_some_and(|s| s.state == "open")
    });
    // Past the TTL both waiting sessions end as timed_out and their keys are fresh again.
    let late = now + 3 * H;
    let tick = health::tick(&d, late);
    for id in [sid, sid2] {
        assert_eq!(d.store.owner_session(id).unwrap().unwrap().state, "ended");
    }
    assert!(!d.store.health_since(&["timed_out"], 0).unwrap().is_empty());
    assert!(
        tick.fresh.contains(&format!("stuck:task:{}", t.0)),
        "{:?}",
        tick.fresh
    );
}

#[test]
fn a_health_proposal_passes_the_weekly_self_task_cap_once() {
    let (_s, d, _t, _g) = world_with(
        json!([submit(json!({"summary": "x", "false_alarm": {"why": "w"}}))]),
        "[self]\nmax_open = 50\nper_week = 1\nmax_steps = 500\n",
    );
    let pid = d.project_id_by_name("scratch").unwrap();
    let id = d.project(pid).unwrap().id.clone();
    // One agent-filed self-task uses up the week's cap of 1.
    file_task_as(
        &d,
        "Agent filed",
        "Work.",
        true,
        mason_core::TrustSource::OwnerAgent,
        false,
    );
    assert!(
        mason_daemon::selfguard::check_filing_as(&d, pid, mason_core::TrustSource::OwnerAgent)
            .is_err()
    );
    let one = json!({"finding": "stuck:queue", "title": "Let the queue start work after a stall",
        "body": "The queue held work while slots were free.",
        "evidence": "README:1 shows the scratch repository the finding was seen in."});
    let sid = d
        .store
        .add_owner_session(None, "health", "default", "health", 1)
        .unwrap();
    let before = d.store.proposals_in(&id).unwrap().len();
    health::apply(
        &d,
        sid,
        pid,
        &json!({"summary": "p", "proposed": [one.clone()]}),
        later(),
    );
    assert_eq!(d.store.proposals_in(&id).unwrap().len(), before + 1);
    health::apply(
        &d,
        sid,
        pid,
        &json!({"summary": "p", "proposed": [one]}),
        later(),
    );
    assert_eq!(d.store.proposals_in(&id).unwrap().len(), before + 1);
}

#[test]
fn a_failed_update_is_logged_with_its_target_and_error_and_its_recovery_with_the_target() {
    let (_s, d, _t, _g) = world(json!([submit(
        json!({"summary": "s", "false_alarm": "seen"})
    )]));
    let long = format!("selftest red: s22 {}", "x".repeat(600));
    {
        let mut st = d.update_status.lock().unwrap();
        st.phase = "failed".into();
        st.target = Some("abc".into());
        st.error = Some(long.clone());
        st.log = vec![
            "building abc into runtimes/abc/".into(),
            "backing up the database, then the new build's self-test".into(),
            format!("failed: {long}"),
        ];
    }
    let rows = |kind: &str| -> Vec<Value> {
        d.store
            .health_since(&[kind], 0)
            .unwrap()
            .iter()
            .map(|r| serde_json::from_str(&r.detail).unwrap())
            .collect()
    };
    let now = mason_daemon::daemon::now_ms();
    health::tick(&d, now);
    let first = rows("update_failed");
    assert_eq!(first.len(), 1);
    assert_eq!(first[0]["target"], "abc");
    assert_eq!(first[0]["phase"], "testing");
    let error = first[0]["error"].as_str().unwrap();
    assert_eq!(error.chars().count(), 500);
    assert!(error.starts_with("selftest red: s22"), "{error}");

    // The same failure at later checks still counts towards the limit.
    let limit = d.cfg.health.limits.update_failures;
    for i in 1..limit {
        health::tick(&d, now + u64::from(i));
    }
    let (facts, findings) = health::report(&d, now + u64::from(limit));
    assert_eq!(facts.update_failures, limit);
    assert!(facts.update_blocked);
    assert!(findings.iter().any(|f| f.key == "failures:update"));

    d.update_status.lock().unwrap().phase = "idle".into();
    health::tick(&d, now + 60_000);
    health::tick(&d, now + 120_000);
    let ok = rows("update_ok");
    assert_eq!(ok, vec![json!({"target": "abc"})]);

    // `mason health history` shows both, detail and all.
    let h = health::history_json(&d, 50);
    for kind in ["update_failed", "update_ok"] {
        assert!(
            h["history"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| { r["kind"] == kind && r["detail"].as_str().unwrap().contains("abc") }),
            "{kind}: {h}"
        );
    }
}
