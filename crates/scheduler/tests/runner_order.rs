// SPDX-License-Identifier: GPL-3.0-or-later

//! The order of GitHub runner units (2026-11-21): landing, then checks, then test steps, then
//! debt payments; urgent first; the longest wait first. Local slots keep their own rule.

use mason_scheduler::{explain, Kind, Ledger, Needs, Rank, WaitCause};

fn rank(steps: u32, priority: u8, id: u64) -> Rank {
    Rank::new(false, steps, priority, id, id, false)
}

fn gha(units: u32) -> Needs {
    Needs::of(&[("gha", units)])
}

/// A pool of one unit that is taken, with four waiters that arrived in the order given.
fn granted_in_order(ranks: Vec<(&str, Rank)>) -> Vec<String> {
    let mut l = Ledger::new();
    l.set_capacity("gha", 1, Kind::Pool);
    let holder = l.adopt(gha(1), "other", 0);
    let ids: Vec<(u64, String)> = ranks
        .into_iter()
        .enumerate()
        .map(|(i, (name, r))| (l.submit(r, gha(1), name, 10 + i as u64), name.to_string()))
        .collect();
    let mut out = vec![];
    let mut held = holder;
    for step in 0..ids.len() as u64 {
        l.release(held);
        let p = l.schedule(1000 + step, None, None);
        assert_eq!(p.grants.len(), 1, "one unit frees, one is granted: {p:?}");
        held = p.grants[0];
        out.push(ids.iter().find(|(i, _)| *i == held).unwrap().1.clone());
    }
    out
}

#[test]
fn units_free_one_at_a_time_go_landing_check_test_step_debt() {
    let step = rank(2, 2, 1);
    let got = granted_in_order(vec![
        ("debt", rank(0, 0, 2).debt()),
        ("test", step),
        ("check", rank(3, 2, 3).check()),
        ("landing", rank(1, 3, 4).landing()),
    ]);
    assert_eq!(got, ["landing", "check", "test", "debt"]);
}

#[test]
fn two_checks_go_by_priority_then_by_the_longer_wait() {
    let got = granted_in_order(vec![
        ("low", rank(3, 3, 1).check()),
        ("urgent", rank(3, 0, 2).check()),
    ]);
    assert_eq!(got, ["urgent", "low"]);
    let got = granted_in_order(vec![
        ("first", rank(3, 2, 5).check()),
        ("second", rank(3, 2, 4).check()),
    ]);
    assert_eq!(got, ["first", "second"], "the one that waited longer");
}

#[test]
fn an_old_debt_waits_behind_a_newer_test_step_for_local_slots_too() {
    let mut l = Ledger::new();
    l.set_capacity("test-suite", 1, Kind::Machine);
    let holder = l.adopt(Needs::of(&[("test-suite", 1)]), "other", 0);
    let ts = Needs::of(&[("test-suite", 1)]);
    let debt = l.submit(rank(0, 0, 1).debt(), ts.clone(), "old debt", 1);
    let step = l.submit(rank(2, 3, 9), ts, "new test step", 500);
    l.release(holder);
    let p = l.schedule(1000, None, None);
    assert_eq!(p.grants, vec![step]);
    assert!(p.waits.contains_key(&debt));
}

#[test]
fn debts_together_hold_at_most_debt_units_and_a_second_waits() {
    let mut l = Ledger::new();
    l.set_capacity("gha", 16, Kind::Pool);
    l.set_capacity("gha-debt", 4, Kind::Pool);
    let both = Needs::of(&[("gha", 4), ("gha-debt", 4)]);
    let a = l.submit(rank(0, 0, 1).debt(), both.clone(), "debt a", 1);
    let b = l.submit(rank(0, 0, 2).debt(), both, "debt b", 2);
    let p = l.schedule(10, None, None);
    assert_eq!(p.grants, vec![a], "gha has room, the debt cap does not");
    assert_eq!(
        p.waits[&b].cause,
        WaitCause::Full {
            resource: "gha-debt".into()
        }
    );
    // A test step still gets the free units meanwhile.
    let step = l.submit(rank(2, 2, 3), gha(8), "test step", 11);
    assert_eq!(l.schedule(11, None, None).grants, vec![step]);
    l.release(a);
    assert_eq!(l.schedule(12, None, None).grants, vec![b]);
}

#[test]
fn a_wait_names_the_kinds_ahead() {
    let mut l = Ledger::new();
    l.set_capacity("gha", 1, Kind::Pool);
    l.adopt(gha(1), "other", 0);
    l.submit(rank(1, 2, 1).landing(), gha(1), "a", 1);
    l.submit(rank(1, 2, 2).landing(), gha(1), "b", 2);
    l.submit(rank(1, 2, 3).check(), gha(1), "c", 3);
    let last = l.submit(rank(1, 2, 4), gha(1), "d", 4);
    let p = l.schedule(10, None, None);
    let text = explain(&p.waits[&last], 10);
    assert!(
        text.ends_with(": 2 landing runs and 1 check ahead"),
        "{text}"
    );
}
