// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 155: another pipeline's work queues in the broker. It is ranked as
//! pipeline work (never a person's), shares the `llm:claude` count, and shows
//! with its pipeline's name. Nothing here asserts on timing: every wait is a
//! bounded poll on state.

use mason_daemon::cpu::FakeCpu;
use mason_daemon::outside::{parse_request, Outside};
use mason_daemon::sched::{Broker, LeaseState, ResourceCfg};
use mason_scheduler::{Needs, Rank};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn broker(dir: &Path, cfg: &ResourceCfg) -> Arc<Broker> {
    Broker::new(
        dir.join("locks"),
        dir.join("limits.kv"),
        cfg,
        10,
        FakeCpu::new(0),
        &mason_daemon::providers::default_accounts(),
    )
    .unwrap()
}

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("mason-outside-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn uses(r: &str) -> BTreeMap<String, u32> {
    [(r.to_string(), 1)].into()
}

fn until(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(30);
    while !f() {
        assert!(Instant::now() < end, "gave up waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn outside(priority: &str, progress: Option<u64>) -> Outside {
    parse_request("other", Some(priority), progress).unwrap()
}

/// A MASON task's step waiting for `resource`, on its own thread.
struct Step {
    alive: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Step {
    fn spawn(
        b: &Arc<Broker>,
        task: u64,
        label: &str,
        resource: &str,
        steps: u32,
        prio: u8,
    ) -> Step {
        let alive = Arc::new(AtomicBool::new(true));
        let (b, alive2) = (b.clone(), alive.clone());
        let (label, resource) = (label.to_string(), resource.to_string());
        let thread = std::thread::spawn(move || {
            let rank = Rank::new(false, steps, prio, 1, 1000 + task, false);
            let _ = b.acquire(
                mason_core::TaskId(task),
                &label,
                rank,
                Needs::of(&[(&resource, 1)]),
                &|| alive2.load(Ordering::SeqCst),
                &mut |_| {},
            );
        });
        Step {
            alive,
            thread: Some(thread),
        }
    }
    fn stop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
        }
    }
}

fn pending(b: &Broker) -> Vec<String> {
    b.compilation()
        .pending
        .into_iter()
        .map(|p| p.label)
        .collect()
}

/// Line order of one outside request among MASON steps (all queued first, so
/// at a tie MASON's arrive earlier) at equal steps remaining.
fn line(mason: &[(&str, u8)], o: Outside) -> Vec<String> {
    let dir = scratch("line");
    let b = broker(&dir, &ResourceCfg::default());
    b.declare("compile", 1);
    let held = b.lease_open("holder", &uses("compile"));
    assert!(matches!(b.lease_poll(held), LeaseState::Granted { .. }));
    let mut steps = vec![];
    for (i, (label, prio)) in mason.iter().enumerate() {
        steps.push(Step::spawn(&b, i as u64 + 1, label, "compile", 3, *prio));
        until("a MASON step to queue", || pending(&b).len() == i + 1);
    }
    let id = b.lease_open_outside("req", &uses("compile"), &o);
    until("the outside request to queue", || {
        pending(&b).len() == mason.len() + 1
    });
    let order = pending(&b);
    assert!(matches!(b.lease_poll(id), LeaseState::Waiting { .. }));
    b.lease_close(id);
    for s in &mut steps {
        s.stop();
    }
    order
}

fn at(order: &[String], label: &str) -> usize {
    order
        .iter()
        .position(|l| l.contains(label))
        .unwrap_or_else(|| panic!("{label} not in {order:?}"))
}

#[test]
fn outside_work_is_interleaved_with_mason_tasks_by_priority_at_equal_steps() {
    // MASON tasks of every priority, queued before the outside request.
    let mason = [
        ("m-urgent", 0),
        ("m-high", 1),
        ("m-normal", 2),
        ("m-low", 3),
    ];
    // Each outside request is ranked as the same task priority would be.
    let cases = [
        // (outside priority, behind these, ahead of these)
        ("urgent", vec![], vec!["m-high", "m-normal", "m-low"]),
        ("high", vec!["m-urgent"], vec!["m-normal", "m-low"]),
        ("normal", vec!["m-urgent", "m-high"], vec!["m-low"]),
        (
            "low",
            vec!["m-urgent", "m-high", "m-normal", "m-low"],
            vec![],
        ),
    ];
    for (prio, behind, ahead) in cases {
        let order = line(&mason, outside(prio, Some(3)));
        let me = at(&order, "other req");
        for b in &behind {
            assert!(
                at(&order, b) < me,
                "outside {prio} waits behind {b}: {order:?}"
            );
        }
        for a in &ahead {
            assert!(me < at(&order, a), "outside {prio} passes {a}: {order:?}");
        }
    }
}

#[test]
fn fewer_steps_remaining_win_by_default_and_priority_wins_when_asked() {
    let dir = scratch("steps");
    let b = broker(&dir, &ResourceCfg::default());
    b.declare("compile", 1);
    let held = b.lease_open("holder", &uses("compile"));
    assert!(matches!(b.lease_poll(held), LeaseState::Granted { .. }));
    let mut near = Step::spawn(&b, 1, "m-near-low", "compile", 1, 3);
    until("the step to queue", || pending(&b).len() == 1);
    // Outside urgent, six steps out (progress 0): it does not pass a task one
    // step from landing.
    let id = b.lease_open_outside("far", &uses("compile"), &outside("urgent", None));
    until("both to queue", || pending(&b).len() == 2);
    let order = pending(&b);
    assert!(
        at(&order, "m-near-low") < at(&order, "other far"),
        "{order:?}"
    );
    // Progress 6 is zero steps remaining: it passes.
    b.lease_close(id);
    let id = b.lease_open_outside("done", &uses("compile"), &outside("low", Some(6)));
    until("both to queue", || pending(&b).len() == 2);
    let order = pending(&b);
    assert!(
        at(&order, "other done") < at(&order, "m-near-low"),
        "{order:?}"
    );
    b.lease_close(id);
    near.stop();

    // With [order] priority_first the priority decides first.
    let dir = scratch("steps-pf");
    let cfg = ResourceCfg {
        priority_first: true,
        ..ResourceCfg::default()
    };
    let b = broker(&dir, &cfg);
    b.declare("compile", 1);
    let held = b.lease_open("holder", &uses("compile"));
    assert!(matches!(b.lease_poll(held), LeaseState::Granted { .. }));
    let mut near = Step::spawn(&b, 1, "m-near-low", "compile", 1, 3);
    until("the step to queue", || pending(&b).len() == 1);
    let id = b.lease_open_outside("far", &uses("compile"), &outside("urgent", None));
    until("both to queue", || pending(&b).len() == 2);
    let order = pending(&b);
    assert!(
        at(&order, "other far") < at(&order, "m-near-low"),
        "{order:?}"
    );
    b.lease_close(id);
    near.stop();
}

#[test]
fn a_person_is_granted_before_an_outside_request_that_queued_first() {
    let dir = scratch("person");
    let b = broker(&dir, &ResourceCfg::default());
    b.declare("compile", 1);
    let held = b.lease_open("holder", &uses("compile"));
    assert!(matches!(b.lease_poll(held), LeaseState::Granted { .. }));
    // Most urgent and nearest landing an outside request can be.
    let o = b.lease_open_outside("first", &uses("compile"), &outside("urgent", Some(6)));
    let person = b.lease_open("person", &uses("compile"));
    assert!(matches!(b.lease_poll(o), LeaseState::Waiting { .. }));
    assert!(matches!(b.lease_poll(person), LeaseState::Waiting { .. }));
    assert_eq!(pending(&b), vec!["person", "other first"]);
    b.lease_close(held);
    until("the person to be granted", || {
        matches!(b.lease_poll(person), LeaseState::Granted { .. })
    });
    assert!(matches!(b.lease_poll(o), LeaseState::Waiting { .. }));
    let snap = b.snapshot();
    let waiting = snap["waiting"].as_array().unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0]["pipeline"], "other");
    assert_eq!(waiting[0]["interactive"], false);
    b.lease_close(person);
    until("the outside request to be granted", || {
        matches!(b.lease_poll(o), LeaseState::Granted { .. })
    });
    let snap = b.snapshot();
    let holders = snap["holders"].as_array().unwrap();
    assert_eq!(holders[0]["pipeline"], "other");
    assert_eq!(holders[0]["label"], "other first");
}

#[test]
fn an_outside_waiter_is_explained_with_how_many_are_ahead() {
    let dir = scratch("explain");
    let b = broker(&dir, &ResourceCfg::default());
    b.declare("compile", 1);
    let held = b.lease_open("holder", &uses("compile"));
    assert!(matches!(b.lease_poll(held), LeaseState::Granted { .. }));
    let _a = b.lease_open("a", &uses("compile"));
    let _b = b.lease_open("b", &uses("compile"));
    let o = b.lease_open_outside("#483 develop", &uses("compile"), &outside("normal", None));
    match b.lease_poll(o) {
        LeaseState::Waiting { explain } => {
            assert_eq!(explain, "waiting for compile (2 ahead)")
        }
        other => panic!("{other:?}"),
    }
    let snap = b.snapshot();
    let w = snap["waiting"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["pipeline"] == "other")
        .unwrap();
    assert_eq!(w["label"], "other #483 develop");
    assert_eq!(w["explain"], "waiting for compile (2 ahead)");
}

#[test]
fn an_outside_llm_hold_counts_against_the_same_claude_limit() {
    let dir = scratch("llm");
    let cfg = ResourceCfg {
        llm: [("claude".to_string(), 2)].into(),
        ..ResourceCfg::default()
    };
    let b = broker(&dir, &cfg);
    assert_eq!(b.limit("llm:claude"), Some(2));
    // One MASON agent step holds a session.
    let Ok(_agent) = b.acquire(
        mason_core::TaskId(1),
        "agent one",
        Rank::new(false, 3, 2, 1, 1001, false),
        Needs::of(&[("llm:claude", 1)]),
        &|| true,
        &mut |_| {},
    ) else {
        panic!("the first agent was not granted")
    };
    // So does one outside pipeline.
    let hold = b.lease_open_outside("session", &uses("llm:claude"), &outside("normal", None));
    assert!(matches!(b.lease_poll(hold), LeaseState::Granted { .. }));
    assert_eq!(b.used("llm:claude"), 2);
    // A second MASON agent step waits: there is no separate pool for outsiders.
    let mut second = Step::spawn(&b, 2, "agent two", "llm:claude", 3, 2);
    until("the second agent to queue", || {
        b.snapshot()["waiting"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["label"] == "agent two")
    });
    assert_eq!(b.used("llm:claude"), 2);
    // The hold shows with its pipeline's name.
    let snap = b.snapshot();
    assert!(snap["holders"]
        .as_array()
        .unwrap()
        .iter()
        .any(|h| h["pipeline"] == "other" && h["label"] == "other session"));
    second.stop();
    b.lease_close(hold);
}
