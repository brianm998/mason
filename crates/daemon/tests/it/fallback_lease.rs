// SPDX-License-Identifier: GPL-3.0-or-later

//! A run meant for GitHub that `fallback = "local"` sends here takes the machine slots it would
//! hold when placed here, through the broker, so this machine's limits (and with them the CPU
//! and thermal holds) apply to it. Before, such a run had skipped the local queue on its way to
//! GitHub and started here at once, beside everything else. No duration is asserted.

use crate::support;
use std::collections::BTreeMap;
use support::lw::*;
use support::wait_for;

fn compile() -> BTreeMap<String, u32> {
    BTreeMap::from([("compile".to_string(), 1)])
}

#[test]
fn a_fallback_run_waits_for_the_slot_it_would_hold_here_and_gets_it_when_freed() {
    let w = world("window = \"20m\"\n", &[]);
    let d = w.d.clone();
    d.broker.set_limit("compile", 1).unwrap();

    let first = mason_daemon::remote::fallback_lease(&d, "scratch", None, &compile(), "first")
        .unwrap()
        .expect("a lease");
    assert_eq!(d.broker.used("compile"), 1);

    // A second fallback run queues behind the first: the limit of one holds.
    let got = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let h = {
        let (d, got) = (d.clone(), got.clone());
        std::thread::spawn(move || {
            let l = mason_daemon::remote::fallback_lease(&d, "scratch", None, &compile(), "second")
                .unwrap()
                .expect("a lease");
            got.store(true, std::sync::atomic::Ordering::SeqCst);
            l
        })
    };
    // While the first holds the only slot, the second cannot have it (this can only fail if the
    // limit is ignored, never because the machine is slow).
    assert!(!got.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(d.broker.used("compile"), 1);

    drop(first);
    wait_for("the second fallback run to get the freed slot", 60, || {
        got.load(std::sync::atomic::Ordering::SeqCst)
    });
    let second = h.join().unwrap();
    assert_eq!(d.broker.used("compile"), 1);
    drop(second);
    assert_eq!(d.broker.used("compile"), 0);
}

#[test]
fn nothing_to_hold_takes_nothing() {
    let w = world("window = \"20m\"\n", &[]);
    let none =
        mason_daemon::remote::fallback_lease(&w.d, "scratch", None, &BTreeMap::new(), "x").unwrap();
    assert!(none.is_none());
    assert_eq!(w.d.broker.used("compile"), 0);
}

/// #835: a fallback run with no task in a step (a debt payment's) is ranked in the debts band,
/// behind a live task's request at any progress and priority, never in the landing band.
#[test]
fn a_debts_fallback_run_goes_after_a_live_tasks_request() {
    use mason_daemon::sched::{LeaseState, Standing};
    let w = world("window = \"20m\"\n", &[]);
    let d = w.d.clone();
    d.broker.set_limit("compile", 1).unwrap();
    let held = mason_daemon::remote::fallback_lease(&d, "scratch", None, &compile(), "held")
        .unwrap()
        .expect("a lease");

    // The debt queues first, so only its rank can put it behind.
    let got = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let h = {
        let (d, got) = (d.clone(), got.clone());
        std::thread::spawn(move || {
            let l = mason_daemon::remote::fallback_lease(&d, "scratch", None, &compile(), "debt")
                .unwrap()
                .expect("a lease");
            got.store(true, std::sync::atomic::Ordering::SeqCst);
            l
        })
    };
    wait_for("the debt's run to queue", 60, || {
        d.broker.snapshot().to_string().contains("debt")
    });
    // A live task's check, far from landing and at the lowest priority.
    let check = d.broker.lease_open_as(
        "check",
        &compile(),
        Some(Standing {
            steps_remaining: 40,
            span: 1,
            priority: 9,
        }),
    );
    drop(held);
    wait_for("the check to be granted", 60, || {
        matches!(d.broker.lease_poll(check), LeaseState::Granted { .. })
    });
    assert!(
        !got.load(std::sync::atomic::Ordering::SeqCst),
        "the debt waits while the check holds the slot"
    );
    d.broker.lease_close(check);
    wait_for("the debt to get the slot", 60, || {
        got.load(std::sync::atomic::Ordering::SeqCst)
    });
    drop(h.join().unwrap());
}

fn suite_slot() -> BTreeMap<String, u32> {
    BTreeMap::from([("test-suite".to_string(), 1)])
}

/// s911: a landing batch holds the suite's slot for its whole run. An exclusive request queued
/// behind that hold waits for the machine to drain. A suite of the batch that falls back from
/// GitHub must not queue behind the exclusive request for the slot the batch already holds for
/// it: that was a cycle (batch lease, own fallback, exclusive request, batch lease).
#[test]
fn a_covered_fallback_runs_under_the_batchs_hold_beside_a_waiting_exclusive_request() {
    use mason_daemon::sched::LeaseState;
    let w = world("window = \"20m\"\n", &[]);
    let d = w.d.clone();
    d.broker.declare("test-suite", 1);
    d.broker.set_limit("test-suite", 1).unwrap();

    // The batch's lease, and another project's exclusive request behind it.
    let batch = d.broker.lease_open_as("batch #1", &suite_slot(), None);
    wait_for("the batch's lease to be granted", 60, || {
        matches!(d.broker.lease_poll(batch), LeaseState::Granted { .. })
    });
    let exclusive = d.broker.lease_open_as(
        "star #846",
        &BTreeMap::from([("exclusive".to_string(), 1)]),
        None,
    );
    assert!(!matches!(
        d.broker.lease_poll(exclusive),
        LeaseState::Granted { .. }
    ));

    // Covered: it goes on without asking the broker, so it cannot wait behind the exclusive
    // request. Run on a thread so that a regression fails the wait instead of hanging.
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let h = {
        let (d, done) = (d.clone(), done.clone());
        std::thread::spawn(move || {
            let l = mason_daemon::remote::fallback_lease_unless_covered(
                &d,
                "scratch",
                None,
                &suite_slot(),
                "covered",
                true,
            )
            .unwrap();
            done.store(true, std::sync::atomic::Ordering::SeqCst);
            l
        })
    };
    wait_for("the covered fallback to proceed", 60, || {
        done.load(std::sync::atomic::Ordering::SeqCst)
    });
    assert!(h.join().unwrap().is_none(), "it takes nothing");
    assert_eq!(d.broker.used("test-suite"), 1, "only the batch's own hold");

    // Not covered: it asks the broker as before and queues behind the exclusive request.
    let queued = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let h = {
        let (d, queued) = (d.clone(), queued.clone());
        std::thread::spawn(move || {
            let l = mason_daemon::remote::fallback_lease_unless_covered(
                &d,
                "scratch",
                None,
                &suite_slot(),
                "uncovered",
                false,
            )
            .unwrap()
            .expect("a lease");
            queued.store(true, std::sync::atomic::Ordering::SeqCst);
            l
        })
    };
    wait_for("the uncovered fallback to queue", 60, || {
        d.broker.snapshot().to_string().contains("uncovered")
    });
    assert!(!queued.load(std::sync::atomic::Ordering::SeqCst));
    d.broker.lease_close(batch);
    wait_for("the exclusive request to be granted", 60, || {
        matches!(d.broker.lease_poll(exclusive), LeaseState::Granted { .. })
    });
    assert!(!queued.load(std::sync::atomic::Ordering::SeqCst));
    d.broker.lease_close(exclusive);
    wait_for("the uncovered fallback to get the slot", 60, || {
        queued.load(std::sync::atomic::Ordering::SeqCst)
    });
    drop(h.join().unwrap());
    assert_eq!(d.broker.used("test-suite"), 0);
}

/// s924: the room check that gates a fallback reads the real broker. With the suite's slot held
/// (by a landing batch, say) an uncovered run has no room for it; a covered run, whose caller
/// holds that slot for it, is not counted against its own use. Free again, both have room.
#[test]
fn room_for_a_fallback_reads_the_broker_and_a_covered_run_is_not_blocked_by_its_own_hold() {
    use mason_daemon::sched::LeaseState;
    let w = world("window = \"20m\"\n", &[]);
    let d = w.d.clone();
    d.broker.declare("test-suite", 1);
    d.broker.set_limit("test-suite", 1).unwrap();
    let now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    };
    let room = |covered: bool| {
        mason_daemon::place::no_room_for(&d.broker.load_reading(now()), &suite_slot(), covered)
    };
    assert_eq!(room(false), None, "free: room");

    let batch = d.broker.lease_open_as("batch #1", &suite_slot(), None);
    wait_for("the batch's lease to be granted", 60, || {
        matches!(d.broker.lease_poll(batch), LeaseState::Granted { .. })
    });
    let why = room(false).expect("the slot is held: no room");
    assert!(why.contains("test-suite"), "{why}");
    assert_eq!(room(true), None, "covered: its own hold is not counted");

    d.broker.lease_close(batch);
    assert_eq!(room(false), None, "released: room");
}

/// s943: a landing batch holds nothing for a suite bound for GitHub, so one that comes back here
/// takes its slots then. It is ranked as the batch's own work, ahead of a debt's run that queued
/// first, not behind everything as a run no task owns is.
#[test]
fn a_batch_suite_falling_back_here_takes_its_slots_ranked_as_the_batch() {
    use mason_daemon::remote::BatchOwner;
    let w = world("window = \"20m\"\n", &[]);
    let d = w.d.clone();
    d.broker.set_limit("compile", 1).unwrap();
    let held = mason_daemon::remote::fallback_lease(&d, "scratch", None, &compile(), "held")
        .unwrap()
        .expect("a lease");

    // The debt queues first, so only its rank can put it behind the batch's suite.
    let order = std::sync::Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));
    let debt = {
        let (d, order) = (d.clone(), order.clone());
        std::thread::spawn(move || {
            let l = mason_daemon::remote::fallback_lease(&d, "scratch", None, &compile(), "debt")
                .unwrap()
                .expect("a lease");
            order.lock().unwrap().push("debt");
            l
        })
    };
    wait_for("the debt's run to queue", 60, || {
        d.broker.snapshot().to_string().contains("debt")
    });
    let owner = BatchOwner {
        task: mason_core::TaskId(7),
        rank: mason_scheduler::Rank::new(false, 1, 0, 1, 7, false),
        tasks: vec![mason_core::TaskId(7)],
        runs: vec![],
        watch: false,
    };
    let batch = {
        let (d, order) = (d.clone(), order.clone());
        std::thread::spawn(move || {
            let l = mason_daemon::remote::fallback_lease_for_batch(
                &d,
                "scratch",
                &owner,
                &compile(),
                "batch suite",
            )
            .unwrap()
            .expect("a lease");
            order.lock().unwrap().push("batch");
            l
        })
    };
    wait_for("the batch's suite to queue", 60, || {
        d.broker.snapshot().to_string().contains("batch suite")
    });
    drop(held);
    wait_for("the first of them to be granted", 60, || {
        !order.lock().unwrap().is_empty()
    });
    assert_eq!(order.lock().unwrap().clone(), vec!["batch"]);
    // The debt gets it once the batch's suite is done.
    drop(batch.join().unwrap());
    drop(debt.join().unwrap());
    assert_eq!(d.broker.used("compile"), 0);
}

#[test]
fn a_skip_ends_the_wait_for_this_machines_slots() {
    // #873: the owner skipped a task's testing while its check queued behind another here.
    let w = world("window = \"20m\"\n", &[]);
    let d = w.d.clone();
    d.broker.set_limit("compile", 1).unwrap();
    let first = mason_daemon::remote::fallback_lease(&d, "scratch", None, &compile(), "first")
        .unwrap()
        .expect("a lease");
    let skipped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let h = {
        let (d, skipped) = (d.clone(), skipped.clone());
        std::thread::spawn(move || {
            mason_daemon::remote::fallback_lease_until(
                &d,
                "scratch",
                None,
                &compile(),
                "second",
                &|| skipped.load(std::sync::atomic::Ordering::SeqCst),
            )
            .map(|l| l.is_some())
        })
    };
    skipped.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(h.join().unwrap().is_err(), "the wait ended without a slot");
    assert_eq!(d.broker.used("compile"), 1, "only the first holds one");
    drop(first);
    assert_eq!(d.broker.used("compile"), 0);
}

fn uses(names: &[&str]) -> BTreeMap<String, u32> {
    names.iter().map(|n| (n.to_string(), 1)).collect()
}

/// A lease as a step or a check holds one, ranked as ordinary pipeline work.
fn step_lease(
    d: &std::sync::Arc<mason_daemon::daemon::Daemon>,
    task: u64,
    amounts: &[(&str, u32)],
) -> mason_daemon::sched::Lease {
    d.broker
        .acquire(
            mason_core::TaskId(task),
            &format!("#{task}"),
            mason_scheduler::Rank::new(false, 1, 0, task, task, false),
            mason_scheduler::Needs::of(amounts),
            &|| true,
            &mut |_| {},
        )
        .expect("a lease")
}

/// #1149: a run whose caller holds the slots takes nothing more, and one that lacks some asks
/// for those only.
#[test]
fn a_run_borrows_what_its_caller_holds_and_asks_for_the_rest_only() {
    use mason_daemon::remote::{fallback_lease_under, Grant};
    let w = world("window = \"20m\"\n", &[]);
    let d = w.d.clone();
    d.broker.declare("test-suite", 1);
    d.broker.set_limit("compile", 1).unwrap();
    d.broker.set_limit("test-suite", 1).unwrap();
    let grant = Grant::of(step_lease(&d, 1, &[("compile", 1)]));

    let none =
        fallback_lease_under(&d, "scratch", None, &compile(), "check", &grant, &|| false).unwrap();
    assert!(none.is_none(), "the held slot is the run's");
    assert_eq!(d.broker.used("compile"), 1);

    let rest = fallback_lease_under(
        &d,
        "scratch",
        None,
        &uses(&["compile", "test-suite"]),
        "check",
        &grant,
        &|| false,
    )
    .unwrap()
    .expect("the slot it does not hold");
    assert_eq!(d.broker.used("compile"), 1, "not asked for again");
    assert_eq!(d.broker.used("test-suite"), 1);
    drop(rest);
    drop(grant);
    assert_eq!(d.broker.used("compile"), 0);
}

/// #1149: what a run still has to ask for while its caller holds a slot goes ahead of a step
/// start that queued first.
#[test]
fn a_request_made_while_holding_a_slot_goes_before_an_earlier_step_start() {
    use mason_daemon::remote::{fallback_lease_under, Grant};
    let w = world("window = \"20m\"\n", &[]);
    let d = w.d.clone();
    d.broker.declare("test-suite", 1);
    d.broker.set_limit("test-suite", 1).unwrap();
    let busy = step_lease(&d, 5, &[("test-suite", 1)]);
    let order = std::sync::Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));
    let start = {
        let (d, order) = (d.clone(), order.clone());
        std::thread::spawn(move || {
            let l = step_lease(&d, 2, &[("test-suite", 1)]);
            order.lock().unwrap().push("step start");
            l
        })
    };
    wait_for("the step start to queue", 60, || {
        d.broker.snapshot()["waiting"].as_array().unwrap().len() == 1
    });
    let check = {
        let (d, order) = (d.clone(), order.clone());
        std::thread::spawn(move || {
            let grant = Grant::of(step_lease(&d, 1, &[("compile", 1)]));
            let l = fallback_lease_under(
                &d,
                "scratch",
                None,
                &suite_slot(),
                "own check",
                &grant,
                &|| false,
            )
            .unwrap();
            order.lock().unwrap().push("check");
            (l, grant)
        })
    };
    wait_for("the check's request to queue", 60, || {
        d.broker.snapshot()["waiting"].as_array().unwrap().len() == 2
    });
    drop(busy);
    wait_for("one of them to be granted", 60, || {
        !order.lock().unwrap().is_empty()
    });
    assert_eq!(order.lock().unwrap().clone(), vec!["check"]);
    drop(check.join().unwrap());
    drop(start.join().unwrap());
}

/// #1149: every unit held by checks that each wait for one more is a deadlock: `mason alerts`
/// says so and `mason resources` explains the wait.
#[test]
fn checks_waiting_on_their_own_slots_are_alerted_and_explained() {
    use mason_daemon::remote::{fallback_lease_under, Grant};
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    let w = world("window = \"20m\"\n", &[]);
    let d = w.d.clone();
    d.broker.set_limit("compile", 2).unwrap();
    let grants = [
        Grant::of(step_lease(&d, 1, &[("compile", 1)])),
        Grant::of(step_lease(&d, 2, &[("compile", 1)])),
    ];
    assert!(d.broker.deadlock_alerts().is_empty(), "both are building");
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let two = BTreeMap::from([("compile".to_string(), 2)]);
    let checks: Vec<_> = grants
        .iter()
        .map(|g| {
            let (d, g, stop, two) = (d.clone(), g.clone(), stop.clone(), two.clone());
            std::thread::spawn(move || {
                fallback_lease_under(&d, "scratch", None, &two, "own check", &g, &|| {
                    stop.load(SeqCst)
                })
                .map(|l| l.is_some())
            })
        })
        .collect();
    wait_for("the deadlock to be seen", 60, || {
        !d.broker.deadlock_alerts().is_empty()
    });
    let alerts = d.broker.deadlock_alerts();
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    let text = &alerts[0].1;
    assert!(text.contains("#1, #2"), "{text}");
    assert!(text.contains("mason resources limit compile 3"), "{text}");
    let snap = d.broker.snapshot().to_string();
    assert!(
        snap.contains("Waiting for a slot its own agent holds"),
        "{snap}"
    );
    stop.store(true, SeqCst);
    for c in checks {
        assert!(c.join().unwrap().is_err());
    }
    assert!(d.broker.deadlock_alerts().is_empty());
}
