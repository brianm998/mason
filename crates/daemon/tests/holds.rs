// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 67 end to end: MASON acts on provider usage. A real daemon runs tasks
//! with the fake agent. A usage limit with a reset holds the whole account
//! (nobody starts a session until the reset, then everything resumes with no
//! failure), a limit with no readable reset backs off with one probe at a
//! time, an outage is told apart from a limit, `overage` decides whether extra
//! billing is held or carried on, `warn_at` warns once per window per reset,
//! and an api account's budget holds it until the owner releases it.

mod support;
use mason_adapter_api::{Overage, OverageState, UsageSnapshot, Window, WindowState};
use mason_core::*;
use mason_daemon::holds;
use mason_daemon::views::Ctx;
use mason_daemon::Daemon;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::*;

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// How long the usage-limit hold in the kill test lasts. It is released by hand, never waited out.
const HOLD_S: u64 = 3600;

/// The reset is `resets_in` seconds after the fake agent emits the event, never an
/// absolute time fixed at test start (the daemon drops resets already past).
fn limit_turn(resets_in: u64) -> Value {
    json!([
        {"rate_limit": {"status": "rejected", "window": "five_hour", "resets_in": resets_in,
            "windows": {"five_hour": {"utilization": 1.0, "status": "rejected"}}}},
        {"result": {"is_error": true, "text": "You've hit your limit", "api_error_status": 429}}
    ])
}

/// A refusal whose reset the daemon cannot resolve (a named zone).
fn limit_unknown_turn() -> Value {
    json!([{"result": {"is_error": true,
        "text": "You've hit your limit \u{b7} resets 3pm (America/Los_Angeles)", "api_error_status": 429}}])
}

fn outage_turn() -> Value {
    json!([{"result": {"is_error": true, "text": "overloaded", "api_error_status": 529}}])
}

fn script(develop: Vec<Value>) -> Value {
    json!({"steps": {"explore": [explore_turn()], "develop": develop}})
}

fn alerts(d: &Arc<Daemon>) -> Vec<(String, String, String)> {
    mason_daemon::attention::sync_alerts(d);
    d.store
        .alerts(true)
        .unwrap()
        .into_iter()
        .map(|a| (a.key, a.kind, a.text))
        .collect()
}

fn banners(d: &Arc<Daemon>) -> Vec<(String, String)> {
    let w = d.world().unwrap();
    Ctx::new(d, &w)
        .header("")
        .banners
        .into_iter()
        .map(|b| (b.kind, b.text))
        .collect()
}

fn waiting_on(d: &Arc<Daemon>, t: TaskId) -> Option<String> {
    match d.world().unwrap().tasks[&t].activity {
        Activity::Waiting(WaitReason::On(ref on)) => Some(on.clone()),
        _ => None,
    }
}

fn launches(d: &Arc<Daemon>, tasks: &[TaskId]) -> usize {
    tasks
        .iter()
        .map(|t| d.store.launches_for_task(t.0).unwrap().len())
        .sum()
}

fn full_window(reset: u64) -> Window {
    Window {
        id: "five_hour".into(),
        label: "5-hour".into(),
        length_secs: Some(18000),
        utilization: Some(1.0),
        resets_at: Some(reset),
        state: WindowState::Blocked,
    }
}

#[test]
fn a_limit_with_a_reset_holds_the_account_until_the_reset() {
    let t0 = now();
    let s = Scratch::new(
        "test -f hello.txt",
        script(vec![limit_turn(14), develop_turn()]),
    );
    let d = s.start();
    let tasks: Vec<TaskId> = (1..=3)
        .map(|i| file_task(&d, &format!("Task {i}")))
        .collect();
    wait_for("the account to be held", 40, || {
        !d.store.account_holds().unwrap().is_empty()
    });
    let h = holds::hold_of(&d, "claude").unwrap();
    assert_eq!(h.kind, "limit");
    let reset = h.until_ms.expect("a reset") / 1000;
    assert!((t0 + 14..=now() + 14).contains(&reset), "{reset} vs {t0}");
    // The daemon runs two tasks at once: two wait on the account, the third is queued.
    wait_for("the running tasks to wait on the account", 12, || {
        let w = d.world().unwrap();
        let waiting = tasks
            .iter()
            .filter(|t| waiting_on(&d, **t).is_some_and(|x| x.starts_with("waiting for claude:")))
            .count();
        waiting >= 2
            && tasks
                .iter()
                .all(|t| waiting_on(&d, *t).is_some() || w.tasks[t].state == State::Queued)
    });
    let text = waiting_on(
        &d,
        *tasks
            .iter()
            .find(|t| waiting_on(&d, **t).is_some())
            .unwrap(),
    )
    .unwrap();
    assert!(text.contains("resets") && text.contains("(in "), "{text}");
    // One item and one banner for the whole account, not one per task.
    let held: Vec<_> = alerts(&d)
        .into_iter()
        .filter(|a| a.1 == "provider_limit")
        .collect();
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(
        banners(&d)
            .iter()
            .filter(|b| b.0 == "provider_limit")
            .count(),
        1
    );
    let n = launches(&d, &tasks);
    while now() + 2 < reset {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(launches(&d, &tasks), n, "no session starts while held");
    for t in &tasks {
        wait_state(&d, *t, State::Landed, 90);
        assert_eq!(d.world().unwrap().tasks[t].failures, 0);
    }
    assert!(
        holds::hold_of(&d, "claude").is_none(),
        "released at the reset"
    );
    assert!(
        alerts(&d).iter().all(|a| a.1 != "provider_limit"),
        "cleared when it ends"
    );
}

#[test]
fn no_readable_reset_backs_off_with_one_probe_at_a_time() {
    let s = Scratch::new(
        "test -f hello.txt",
        script(vec![
            limit_unknown_turn(),
            limit_unknown_turn(),
            develop_turn(),
        ]),
    );
    let d = s.start();
    let tasks: Vec<TaskId> = (1..=3)
        .map(|i| file_task(&d, &format!("Task {i}")))
        .collect();
    let (mut most, mut backoffs) = (0, std::collections::BTreeSet::new());
    let end = Instant::now() + Duration::from_secs(120);
    loop {
        most = most.max(d.probes.in_flight());
        if let Some(h) = holds::hold_of(&d, "claude") {
            assert!(h.until_ms.is_none());
            backoffs.insert(h.backoff_ms);
        }
        let w = d.world().unwrap();
        if tasks.iter().all(|t| w.tasks[t].state == State::Landed) {
            break;
        }
        assert!(Instant::now() < end, "tasks never landed");
        std::thread::sleep(Duration::from_millis(15));
    }
    assert!(most <= 1, "more than one probe at once: {most}");
    assert!(backoffs.contains(&300), "{backoffs:?}");
    assert!(backoffs.contains(&600), "doubles: {backoffs:?}");
    for t in &tasks {
        assert_eq!(d.world().unwrap().tasks[t].failures, 0);
    }
}

#[test]
fn an_outage_holds_and_is_told_apart_from_a_limit() {
    let s = Scratch::new(
        "test -f hello.txt",
        script(vec![outage_turn(), develop_turn()]),
    );
    let d = s.start();
    let t = file_task(&d, "Outage");
    let mut saw = false;
    wait_for("the task to land", 60, || {
        if let Some(h) = holds::hold_of(&d, "claude") {
            assert_eq!(h.kind, "outage");
            let v = Ctx::new(&d, &d.world().unwrap()).usage();
            let a = v.providers.iter().find(|p| p.name == "claude").unwrap();
            let (al, ba) = (alerts(&d), banners(&d));
            // The hold may be released between `hold_of` above and any of these
            // reads (a probe succeeds); only a hold still in force must be shown.
            // CI on 2026-11-08 read `doing` as "nothing" in that gap.
            if holds::hold_of(&d, "claude").is_none() {
                return d.world().unwrap().tasks[&t].state == State::Landed;
            }
            assert_eq!(a.status, "outage");
            assert!(a.doing.contains("provider outage"), "{}", a.doing);
            assert!(al.iter().any(|a| a.1 == "provider_outage"));
            assert!(al.iter().all(|a| a.1 != "provider_limit"));
            assert!(ba.iter().any(|b| b.0 == "provider_outage"));
            saw = true;
        }
        d.world().unwrap().tasks[&t].state == State::Landed
    });
    assert!(saw);
    assert_eq!(d.world().unwrap().tasks[&t].failures, 0);
}

#[test]
fn overage_hold_holds_at_the_plan_limit_and_allow_carries_on() {
    let in_use = |reset| UsageSnapshot {
        windows: vec![full_window(reset)],
        overage: Overage {
            state: OverageState::InUse,
            resets_at: None,
        },
        spend_usd: Some(3.5),
        ..Default::default()
    };
    // The default is hold.
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    mason_daemon::usage::record_snapshot(&d, "claude", &in_use(now() + 3600));
    let h = holds::hold_of(&d, "claude").expect("held at the plan's own limit");
    assert_eq!(h.kind, "overage");
    assert!(matches!(holds::gate(&d, "claude", 1), holds::Gate::Wait(_)));
    assert!(alerts(&d).iter().any(|a| a.1 == "provider_overage"));

    // `allow`: work carries on and the monitors show extra billing and its cost.
    let s = Scratch::new("true", happy_script());
    std::fs::write(
        s.config_dir.join("providers.toml"),
        "[provider.claude]\nadapter = \"claude\"\nbilling = \"subscription\"\noverage = \"allow\"\n",
    )
    .unwrap();
    let d = s.start();
    mason_daemon::usage::record_snapshot(&d, "claude", &in_use(now() + 3600));
    assert!(holds::hold_of(&d, "claude").is_none());
    assert_eq!(holds::gate(&d, "claude", 1), holds::Gate::Open);
    let extra: Vec<_> = alerts(&d)
        .into_iter()
        .filter(|a| a.1 == "provider_extra_billing")
        .collect();
    assert_eq!(extra.len(), 1);
    assert!(extra[0].2.contains("$3.50"), "{}", extra[0].2);
    assert!(banners(&d).iter().any(|b| b.0 == "provider_extra_billing"));
    let v = Ctx::new(&d, &d.world().unwrap()).usage();
    let a = v.providers.iter().find(|p| p.name == "claude").unwrap();
    assert!(a.overage_text.contains("in use") && a.overage_text.contains("extra billing"));
}

#[test]
fn warn_at_warns_once_per_window_per_reset_and_projects_only_with_two_readings() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let t0 = now() - 1200;
    let reset = now() + 10_000;
    let snap = |u: f64, at: u64, reset: u64| UsageSnapshot {
        windows: vec![Window {
            id: "seven_day".into(),
            label: "Weekly".into(),
            length_secs: Some(7 * 86400),
            utilization: Some(u),
            resets_at: Some(reset),
            state: WindowState::Warning,
        }],
        observed_at: Some(at),
        ..Default::default()
    };
    let warns = |d: &Arc<Daemon>| -> Vec<(String, String, String)> {
        alerts(d)
            .into_iter()
            .filter(|a| a.1 == "provider_warning")
            .collect()
    };
    let rec = |u, at, r| mason_daemon::usage::record_snapshot(&d, "claude", &snap(u, at, r));
    rec(0.5, t0, reset);
    assert!(warns(&d).is_empty(), "below warn_at");
    rec(0.82, t0 + 600, reset);
    let w = warns(&d);
    assert_eq!(w.len(), 1);
    assert!(
        w[0].2.contains("82%") && w[0].2.contains("resets"),
        "{}",
        w[0].2
    );
    rec(0.84, t0 + 1200, reset);
    assert_eq!(warns(&d).len(), 1, "once per window per reset");
    assert!(warns(&d)[0].2.contains("recent rate"), "{}", warns(&d)[0].2);
    assert!(
        holds::hold_of(&d, "claude").is_none(),
        "a warning holds nothing"
    );
    // Another reset is another window, and warns again.
    rec(0.9, t0 + 1300, reset + 7 * 86400);
    assert_eq!(d.store.warnings().unwrap().len(), 2);
    let w = warns(&d);
    assert_eq!(w.len(), 1, "the earlier window is over");
    assert!(
        w[0].0.ends_with(&(reset + 7 * 86400).to_string()),
        "{}",
        w[0].0
    );

    // The first warning of a fresh window carries no projection.
    let s = Scratch::new("true", happy_script());
    let d2 = s.start();
    mason_daemon::usage::record_snapshot(&d2, "claude", &snap(0.9, now(), reset));
    let w = alerts(&d2)
        .into_iter()
        .find(|a| a.1 == "provider_warning")
        .unwrap();
    assert!(!w.2.contains("recent rate"), "{}", w.2);
}

#[test]
fn an_api_budget_reached_holds_the_account_until_the_owner_releases_it() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    std::fs::write(
        s.config_dir.join("providers.toml"),
        "[provider.metered]\nadapter = \"claude\"\nbilling = \"api\"\nmonthly_budget_usd = 1\n",
    )
    .unwrap();
    s.write_project("scratch", "test -f hello.txt", "account = \"metered\"");
    let d = s.start();
    let who = |d: &Arc<Daemon>| {
        d.store
            .interventions(0)
            .unwrap()
            .into_iter()
            .filter(|i| i.kind.starts_with("account_"))
            .map(|i| (i.kind, i.who))
            .collect::<Vec<_>>()
    };
    // Spend reaches the budget before the task starts.
    let tokens = mason_adapters::runner::TokenTotals {
        cost_usd: Some(1.25),
        ..Default::default()
    };
    let attempt = mason_daemon::usage::Attempt {
        key: "x",
        task: 99,
        project: "scratch",
        step: "develop",
        account: "metered",
        model: None,
    };
    mason_daemon::usage::record_tokens(&d, &attempt, &tokens);
    let h = holds::hold_of(&d, "metered").expect("budget held");
    assert_eq!(h.kind, "budget");
    assert!(alerts(&d).iter().any(|a| a.1 == "provider_budget"));
    let t = file_task(&d, "Over budget");
    wait_for("the task to wait", 30, || {
        waiting_on(&d, t).is_some_and(|w| w.contains("budget"))
    });
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(d.store.launches_for_task(t.0).unwrap().len(), 0);
    assert!(holds::release(&d, "nope", "ann").is_err());
    assert!(holds::release(&d, "metered", "ann").unwrap());
    assert_eq!(who(&d), vec![("account_release".into(), "ann".into())]);
    wait_state(&d, t, State::Landed, 60);
    assert!(
        holds::hold_of(&d, "metered").is_none(),
        "released with the spend still over: not held again this month"
    );
    assert!(alerts(&d).iter().all(|a| a.1 != "provider_budget"));
}

#[test]
fn the_owner_can_hold_an_account_by_hand_and_release_it() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    let d = s.start();
    holds::hold(&d, "claude", "ann", None, "maintenance").unwrap();
    let t = file_task(&d, "Held by hand");
    wait_for("the task to wait", 30, || {
        waiting_on(&d, t).is_some_and(|w| w.contains("held by ann"))
    });
    assert!(alerts(&d).iter().any(|a| a.1 == "provider_hold"));
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(d.store.launches_for_task(t.0).unwrap().len(), 0);
    assert!(holds::release(&d, "claude", "bob").unwrap());
    assert!(
        !holds::release(&d, "claude", "bob").unwrap(),
        "already released"
    );
    wait_state(&d, t, State::Landed, 60);
    let kinds: Vec<_> = d
        .store
        .interventions(0)
        .unwrap()
        .into_iter()
        .filter(|i| i.kind.starts_with("account_"))
        .map(|i| (i.kind, i.who))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("account_hold".into(), "ann".into()),
            ("account_release".into(), "bob".into())
        ]
    );
}

fn usage_of(s: &Scratch, account: &str) -> Value {
    let v: Value = serde_json::from_str(&s.mason_ok(&["usage", "--json"])).unwrap();
    v["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == account)
        .unwrap()
        .clone()
}

#[test]
fn a_real_daemon_keeps_a_hold_across_a_kill_and_the_cli_holds_and_releases() {
    let t0 = now();
    let s = Scratch::new(
        "test -f hello.txt",
        script(vec![limit_turn(HOLD_S), develop_turn()]),
    );
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    s.mason_ok(&["task", "add", "First"]);
    s.mason_ok(&["task", "add", "Second"]);
    wait_for("the account to be held", 60, || {
        usage_of(&s, "claude")["status"] == "blocked"
    });
    let p = usage_of(&s, "claude");
    assert!(
        p["doing"].as_str().unwrap().contains("usage limit")
            || p["doing"].as_str().unwrap().contains("limit"),
        "{p}"
    );
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(m.child.id() as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    let _ = m.child.wait();
    let store = mason_store::Store::open(s.data_dir.join("store.db")).unwrap();
    let held = store.account_holds().unwrap();
    let before: usize = (0..2)
        .map(|t| store.launches_for_task(t).unwrap().len())
        .sum();
    drop(store);
    assert_eq!(held.len(), 1, "the hold is in the store");
    let reset = held[0].until_ms.expect("a reset") / 1000;
    assert!(
        (t0 + HOLD_S..=now() + HOLD_S).contains(&reset),
        "{reset} vs {t0}"
    );

    let mut m2 = s.masond(&[], &[]);
    s.wait_daemon(&mut m2);
    assert_eq!(
        usage_of(&s, "claude")["status"],
        "blocked",
        "still held after the restart"
    );
    std::thread::sleep(Duration::from_secs(3));
    let store = mason_store::Store::open(s.data_dir.join("store.db")).unwrap();
    let during: usize = (0..2)
        .map(|t| store.launches_for_task(t).unwrap().len())
        .sum();
    drop(store);
    assert_eq!(during, before, "no session starts during the hold");
    // The hold lasts far longer than this test needs, so no machine stall can end it
    // early; it is released by hand, which is recorded like any release.
    assert!(s
        .mason_ok(&["account", "release", "claude"])
        .contains("Released"));
    s.wait_task(0, "landed", 300);
    s.wait_task(1, "landed", 300);
    assert_eq!(usage_of(&s, "claude")["status"], "ok");

    // By hand, from the CLI.
    s.mason_ok(&["account", "hold", "claude", "--reason", "testing"]);
    assert_eq!(usage_of(&s, "claude")["status"], "blocked");
    let o = s.mason(&["account", "release", "nope"]);
    assert!(!o.status.success());
    assert!(s
        .mason_ok(&["account", "release", "claude"])
        .contains("Released"));
    assert_eq!(usage_of(&s, "claude")["status"], "ok");
    let metrics: Value = serde_json::from_str(&s.mason_ok(&["metrics", "--json"])).unwrap();
    assert_eq!(metrics["by_kind"]["account_hold"], 1, "{metrics}");
    // One release ends the usage-limit hold, one ends the hand-made hold.
    assert_eq!(metrics["by_kind"]["account_release"], 2, "{metrics}");
}
