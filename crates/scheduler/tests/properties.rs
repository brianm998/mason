// SPDX-License-Identifier: GPL-3.0-or-later

//! The ordering rule, the passing rule, and "no deadlock, no lost lease",
//! checked over random workloads by simulating the whole run.

use mason_scheduler::{explain, Kind, Ledger, Needs, Rank, WaitCause};
use proptest::prelude::*;
use std::collections::BTreeMap;

const RESOURCES: [(&str, Kind); 5] = [
    ("compile", Kind::Machine),
    ("suite", Kind::Machine),
    ("phone", Kind::Pool),
    ("llm:claude", Kind::Llm),
    ("llm:other", Kind::Llm),
];

#[derive(Clone, Debug)]
struct Job {
    arrive: u64,
    run: u64,
    exclusive: bool,
    uses: Vec<(usize, u32)>,
    interactive: bool,
    steps: u32,
    priority: u8,
}

fn job() -> impl Strategy<Value = Job> {
    (
        0u64..20,
        1u64..10,
        prop::bool::weighted(0.1),
        prop::collection::vec((0usize..5, 1u32..3), 0..4),
        prop::bool::weighted(0.15),
        1u32..6,
        0u8..4,
    )
        .prop_map(
            |(arrive, run, exclusive, uses, interactive, steps, priority)| Job {
                arrive,
                run,
                exclusive,
                uses,
                interactive,
                steps,
                priority,
            },
        )
}

fn needs(j: &Job, caps: &[u32; 5]) -> Needs {
    // Never ask for more than a resource's limit, so every job can run.
    let mut amounts: BTreeMap<String, u32> = BTreeMap::new();
    for (i, n) in &j.uses {
        let e = amounts.entry(RESOURCES[*i].0.to_string()).or_insert(0);
        *e = (*e + n).min(caps[*i]);
    }
    amounts.retain(|_, n| *n > 0);
    Needs {
        exclusive: j.exclusive,
        amounts,
    }
}

fn uses_machine(n: &Needs) -> bool {
    n.exclusive
        || RESOURCES
            .iter()
            .any(|(r, k)| *k == Kind::Machine && n.amounts.contains_key(*r))
}

/// Would these two need something in common, under the passing rule?
fn conflict(a: &Needs, b: &Needs) -> bool {
    if a.amounts.keys().any(|r| b.amounts.contains_key(r)) {
        return true;
    }
    (a.exclusive && uses_machine(b)) || (b.exclusive && uses_machine(a))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// Whatever the workload: capacity is never exceeded, nothing passes a
    /// waiter it conflicts with, and every job runs and is released.
    #[test]
    fn the_rules_hold_for_any_workload(
        jobs in prop::collection::vec(job(), 1..25),
        caps in prop::array::uniform5(1u32..4),
    ) {
        let mut l = Ledger::new();
        for (i, (name, kind)) in RESOURCES.iter().enumerate() {
            l.set_capacity(name, caps[i], *kind);
        }
        let mut pending: Vec<(usize, &Job)> = jobs.iter().enumerate().collect();
        pending.sort_by_key(|(_, j)| j.arrive);
        let mut pending = pending.into_iter().peekable();
        let mut ends: BTreeMap<u64, (u64, usize)> = BTreeMap::new(); // lease id -> (end, job)
        let mut ranks: BTreeMap<u64, Rank> = BTreeMap::new();
        let mut needs_of: BTreeMap<u64, Needs> = BTreeMap::new();
        let mut now = 0u64;
        let mut finished = 0usize;
        let mut guard = 0;

        while finished < jobs.len() {
            guard += 1;
            prop_assert!(guard < 10_000, "no progress: a deadlock");
            // Arrivals.
            while let Some((i, j)) = pending.next_if(|(_, j)| j.arrive <= now) {
                let n = needs(j, &caps);
                let rank = Rank::new(j.interactive, j.steps, j.priority, j.arrive, i as u64, false);
                let id = l.submit(rank, n.clone(), &format!("#{i}"), now);
                ranks.insert(id, rank);
                needs_of.insert(id, n);
                ends.insert(id, (u64::MAX, i));
            }
            // Releases.
            let done: Vec<u64> = ends
                .iter()
                .filter(|(id, (end, _))| *end <= now && l.holds(**id))
                .map(|(id, _)| *id)
                .collect();
            for id in done {
                prop_assert!(l.release(id), "a lease was lost");
                ends.remove(&id);
                finished += 1;
            }
            // Grant.
            let before = l.snapshot();
            let p = l.schedule(now, None, None);
            for g in &p.grants {
                let (_, ji) = ends[g];
                ends.insert(*g, (now + jobs[ji].run, ji));
                // The passing rule: no earlier-ranked waiter that is still
                // waiting shares anything with this grant.
                for w in &before.waiting {
                    if w.id != *g && !p.grants.contains(&w.id) && ranks[&w.id] < ranks[g] {
                        prop_assert!(
                            !conflict(&needs_of[g], &needs_of[&w.id]),
                            "{} passed {} which it conflicts with", g, w.id
                        );
                    }
                }
            }
            // Capacity and exclusivity.
            let s = l.snapshot();
            for (name, used) in &s.used {
                prop_assert!(*used <= s.capacity[name].units, "{name} over its limit");
            }
            let machine_holders = s.holders.iter().filter(|h| uses_machine(&h.needs)).count();
            if s.holders.iter().any(|h| h.needs.exclusive) {
                prop_assert_eq!(machine_holders, 1, "exclusive was not alone");
            }
            // Every wait explains itself.
            for (id, w) in &p.waits {
                prop_assert!(!explain(w, now).is_empty());
                prop_assert!(w.position >= 1, "{id}");
            }
            // Advance to the next thing that can change anything.
            let next_release = ends.values().filter(|(e, _)| *e != u64::MAX).map(|(e, _)| *e).min();
            let next_arrival = pending.peek().map(|(_, j)| j.arrive);
            match (next_release, next_arrival) {
                (Some(a), Some(b)) => now = now.max(a.min(b)).max(now),
                (Some(a), None) => now = a.max(now),
                (None, Some(b)) => now = b.max(now),
                (None, None) => {}
            }
            // Releases at the same instant need another pass to be seen.
            if next_release.is_none() && next_arrival.is_none() && finished < jobs.len() {
                prop_assert!(false, "waiters but nothing running: a deadlock");
            }
        }
        let s = l.snapshot();
        prop_assert!(s.holders.is_empty() && s.waiting.is_empty());
    }

    /// With one resource and everything waiting at once, the grant order is the
    /// rank order and nothing else.
    #[test]
    fn grants_follow_the_rank_tuple(
        specs in prop::collection::vec((any::<bool>(), 1u32..8, 0u8..4, 0u64..30), 1..20)
    ) {
        let mut l = Ledger::new();
        l.set_capacity("compile", 1, Kind::Machine);
        let mut expect: Vec<(Rank, u64)> = vec![];
        for (i, (inter, steps, pri, enq)) in specs.iter().enumerate() {
            let rank = Rank::new(*inter, *steps, *pri, *enq, i as u64, false);
            let id = l.submit(rank, Needs::of(&[("compile", 1)]), &format!("#{i}"), 0);
            expect.push((rank, id));
        }
        expect.sort();
        let mut got = vec![];
        for _ in 0..specs.len() {
            let p = l.schedule(0, None, None);
            prop_assert_eq!(p.grants.len(), 1);
            got.push(p.grants[0]);
            l.release(p.grants[0]);
        }
        let want: Vec<u64> = expect.into_iter().map(|(_, id)| id).collect();
        prop_assert_eq!(got, want);
    }

    /// Lowering a limit, however often and whenever, never removes a holder.
    #[test]
    fn live_limit_changes_never_kill_a_holder(
        limits in prop::collection::vec(0u32..5, 1..12), n in 1usize..8
    ) {
        let mut l = Ledger::new();
        l.set_capacity("llm:claude", 4, Kind::Llm);
        let ids: Vec<u64> = (0..n).map(|i| {
            l.submit(Rank::new(false, 1, 2, i as u64, i as u64, false),
                     Needs::of(&[("llm:claude", 1)]), &format!("#{i}"), 0)
        }).collect();
        l.schedule(0, None, None);
        let held: Vec<u64> = ids.iter().copied().filter(|i| l.holds(*i)).collect();
        for lim in limits {
            l.set_capacity("llm:claude", lim, Kind::Llm);
            l.schedule(1, None, None);
            for h in &held {
                prop_assert!(l.holds(*h), "a limit change removed a holder");
            }
        }
    }
}

#[test]
fn a_compile_passes_a_waiting_test_suite_but_nothing_passes_an_exclusive_job() {
    let mut l = Ledger::new();
    l.set_capacity("compile", 4, Kind::Machine);
    l.set_capacity("suite", 1, Kind::Machine);
    let rank = |s, id| Rank::new(false, s, 2, id, id, false);
    let holder = l.submit(rank(1, 1), Needs::of(&[("suite", 1)]), "#1", 0);
    assert_eq!(l.schedule(0, None, None).grants, vec![holder]);
    // A second suite waits; a compile behind it passes: they share nothing.
    let suite2 = l.submit(rank(2, 2), Needs::of(&[("suite", 1)]), "#2", 0);
    let compile = l.submit(rank(3, 3), Needs::of(&[("compile", 1)]), "#3", 0);
    let p = l.schedule(1, None, None);
    assert_eq!(p.grants, vec![compile]);
    assert!(p.waits.contains_key(&suite2));
    // An exclusive job now waits for everything machine-class to drain, and
    // nothing machine-class may pass it.
    let excl = l.submit(rank(4, 4), Needs::exclusive(), "#4", 2);
    let late = l.submit(rank(5, 5), Needs::of(&[("compile", 1)]), "#5", 3);
    let p = l.schedule(4, None, None);
    assert!(p.grants.is_empty(), "granted {:?}", p.grants);
    assert!(matches!(
        p.waits[&late].cause,
        WaitCause::Exclusive | WaitCause::Behind { .. }
    ));
    for id in [holder, compile] {
        l.release(id);
    }
    // #2 (the second suite) is ranked ahead of the exclusive job, so it goes first.
    let p = l.schedule(5, None, None);
    assert_eq!(p.grants, vec![suite2]);
    l.release(suite2);
    assert_eq!(l.schedule(6, None, None).grants, vec![excl]);
    l.release(excl);
    assert_eq!(l.schedule(7, None, None).grants, vec![late]);
}

#[test]
fn the_memory_floor_applies_only_beside_other_work() {
    let mut l = Ledger::new();
    l.set_capacity("compile", 4, Kind::Machine);
    let rank = |id| Rank::new(false, 1, 2, id, id, false);
    let a = l.submit(rank(1), Needs::of(&[("compile", 1)]), "#1", 0);
    let b = l.submit(rank(2), Needs::of(&[("compile", 1)]), "#2", 0);
    // Alone on the machine, low memory does not stop the first one.
    let p = l.schedule(0, Some(100), Some(2000));
    assert_eq!(p.grants, vec![a]);
    let w = &p.waits[&b];
    assert_eq!(
        w.cause,
        WaitCause::Memory {
            free_mb: 100,
            floor_mb: 2000
        }
    );
    assert!(explain(w, 0).contains("100 MB free, 2000 MB needed"));
    // Enough memory: it starts.
    assert_eq!(l.schedule(1, Some(5000), Some(2000)).grants, vec![b]);
}

#[test]
fn a_request_bigger_than_its_limit_does_not_hold_up_the_others() {
    let mut l = Ledger::new();
    l.set_capacity("compile", 2, Kind::Machine);
    let big = l.submit(
        Rank::new(false, 1, 2, 1, 1, false),
        Needs::of(&[("compile", 3)]),
        "#1",
        0,
    );
    let ok = l.submit(
        Rank::new(false, 2, 2, 2, 2, false),
        Needs::of(&[("compile", 1)]),
        "#2",
        0,
    );
    let p = l.schedule(0, None, None);
    assert_eq!(p.grants, vec![ok]);
    assert!(explain(&p.waits[&big], 0).contains("needs 3 of compile slot but the limit is 2"));
    // Raise the limit and the big one runs once the other releases.
    l.release(ok);
    l.set_capacity("compile", 3, Kind::Machine);
    assert_eq!(l.schedule(1, None, None).grants, vec![big]);
}
