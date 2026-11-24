// SPDX-License-Identifier: GPL-3.0-or-later

//! The ordering rule, the passing rule, and "no deadlock, no lost lease",
//! checked over random workloads by simulating the whole run.

use mason_scheduler::{
    explain, CpuGate, Kind, Ledger, Needs, PolicyGate, Rank, StallGate, ThermalGate, WaitCause,
};
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
        flex: false,
    }
}

fn uses_machine(n: &Needs) -> bool {
    n.exclusive
        || RESOURCES
            .iter()
            .any(|(r, k)| *k == Kind::Machine && n.amounts.contains_key(*r))
}

/// Would these two conflict through the exclusive rules, under the passing rule?
fn machine_conflict(a: &Needs, b: &Needs) -> bool {
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
            let mut granted: Vec<u64> = p.grants.clone();
            granted.sort_by_key(|g| ranks[g]);
            let mut used = before.used.clone();
            for g in &granted {
                let (_, ji) = ends[g];
                ends.insert(*g, (now + jobs[ji].run, ji));
                for (r, a) in &needs_of[g].amounts {
                    *used.entry(r.clone()).or_insert(0) += a;
                }
                // The passing rule: a grant may pass an earlier-ranked waiter that is still
                // waiting only by using units that waiter does not need. Whatever the earlier
                // waiters need is still free after this grant. Nothing machine-class passes a
                // waiting exclusive job, and an exclusive job passes no machine-class waiter.
                let earlier: Vec<&Needs> = before
                    .waiting
                    .iter()
                    .filter(|w| !p.grants.contains(&w.id) && ranks[&w.id] < ranks[g])
                    .map(|w| &needs_of[&w.id])
                    .collect();
                for w in &earlier {
                    prop_assert!(
                        !machine_conflict(&needs_of[g], w),
                        "{} passed an exclusive conflict", g
                    );
                }
                for r in needs_of[g].amounts.keys() {
                    // s471: a waiter sets aside only what it is short of. Here that is the
                    // resources its holders alone leave too little of; the planner tests
                    // cover the first-in-line and behind-a-reservation cases.
                    let reserved: u32 = earlier
                        .iter()
                        .filter(|w| {
                            w.amounts.iter().any(|(k, a)| {
                                before.used.get(k).copied().unwrap_or(0) + a
                                    > before.capacity[k].units
                            }) && w.amounts.get(r).is_some_and(|a| {
                                before.used.get(r).copied().unwrap_or(0) + a
                                    > before.capacity[r].units
                            })
                        })
                        .map(|w| w.amounts.get(r).copied().unwrap_or(0))
                        .sum();
                    if reserved > 0 {
                        prop_assert!(
                            used[r] + reserved <= before.capacity[r].units,
                            "{} took {} units an earlier waiter needs", g, r
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

#[test]
fn the_first_compile_starts_however_busy_and_a_further_one_waits_for_the_cpu() {
    let mut l = Ledger::new();
    l.set_capacity("compile", 4, Kind::Machine);
    let gate = |busy_pct| {
        Some(CpuGate {
            busy_pct,
            max_pct: 50,
        })
    };
    let submit = |l: &mut Ledger, n: u64| {
        l.submit(
            Rank::new(false, 1, 2, n, n, false),
            Needs::of(&[("compile", 1)]),
            "#",
            0,
        )
    };
    let a = submit(&mut l, 1);
    let b = submit(&mut l, 2);
    // CPU use is the whole machine's: none of ours running, so it is someone
    // else's load, and the first compile starts anyway. The second waits.
    let p = l.schedule_gated(0, None, None, gate(71));
    assert_eq!(p.grants, vec![a]);
    assert_eq!(explain(&p.waits[&b], 0), "compile waiting: CPU 71% > 50%");
    // At the limit still waits; below it starts on the first pass.
    assert!(l.schedule_gated(1, None, None, gate(50)).grants.is_empty());
    assert_eq!(l.schedule_gated(2, None, None, gate(49)).grants, vec![b]);
    // A compile that has started is never revoked when load rises.
    let p = l.schedule_gated(3, None, None, gate(99));
    assert!(l.holds(a) && l.holds(b) && p.grants.is_empty());
}

#[test]
fn paced_compiles_start_one_per_gap_and_other_work_is_not_paced() {
    let mut l = Ledger::new();
    l.set_capacity("compile", 8, Kind::Machine);
    l.set_capacity("llm:claude", 2, Kind::Llm);
    l.pace_compiles(30_000);
    let idle = Some(CpuGate {
        busy_pct: 10,
        max_pct: 50,
    });
    let submit = |l: &mut Ledger, name, label| {
        l.submit(
            Rank::new(false, 1, 2, 1, 1, false),
            Needs::of(&[(name, 1)]),
            label,
            0,
        )
    };
    let a = submit(&mut l, "compile", "#1");
    let b = submit(&mut l, "compile", "#2");
    let c = submit(&mut l, "compile", "#3");
    let other = submit(&mut l, "llm:claude", "#4");

    // An idle machine still starts only one compile; the agent is not held up.
    let p = l.schedule_gated(1_000, None, None, idle);
    assert_eq!(p.grants, vec![a, other]);
    assert_eq!(
        explain(&p.waits[&b], 1_000),
        "compile waiting: letting the last compile show in CPU use (30 s)"
    );
    assert!(l.schedule_gated(30_999, None, None, idle).grants.is_empty());
    assert_eq!(l.schedule_gated(31_000, None, None, idle).grants, vec![b]);
    // A busy reading after the gap still holds the next one back.
    let hot = Some(CpuGate {
        busy_pct: 90,
        max_pct: 50,
    });
    assert!(l.schedule_gated(61_000, None, None, hot).grants.is_empty());
    assert_eq!(l.schedule_gated(61_001, None, None, idle).grants, vec![c]);
}

#[test]
fn the_cpu_gate_leaves_other_resources_alone() {
    let mut l = Ledger::new();
    l.set_capacity("llm:claude", 2, Kind::Llm);
    l.set_capacity("compile", 1, Kind::Machine);
    let hot = Some(CpuGate {
        busy_pct: 100,
        max_pct: 50,
    });
    let a = l.submit(
        Rank::new(false, 1, 2, 1, 1, false),
        Needs::of(&[("llm:claude", 1)]),
        "#1",
        0,
    );
    assert_eq!(l.schedule_gated(0, None, None, hot).grants, vec![a]);
}

#[test]
fn a_disk_stall_holds_only_a_further_compile() {
    let mut l = Ledger::new();
    l.set_capacity("llm:claude", 2, Kind::Llm);
    l.set_capacity("compile", 4, Kind::Machine);
    l.set_stall(Some(StallGate {
        slow_ms: 30_000,
        max_ms: 2_000,
    }));
    let compile = |n: u64| Rank::new(false, 1, 2, n, n, false);
    let first = l.submit(compile(1), Needs::of(&[("compile", 1)]), "#1", 0);
    assert_eq!(
        l.schedule(0, None, None).grants,
        vec![first],
        "the first compile starts: nothing of ours can be causing the stall"
    );
    let second = l.submit(compile(2), Needs::of(&[("compile", 1)]), "#2", 1);
    let agent = l.submit(compile(3), Needs::of(&[("llm:claude", 1)]), "#3", 1);
    let p = l.schedule(1, None, None);
    assert_eq!(p.grants, vec![agent], "agents are not held by a stall");
    assert!(matches!(
        p.waits[&second].cause,
        WaitCause::DiskStall { .. }
    ));
    l.set_stall(Some(StallGate {
        slow_ms: 3,
        max_ms: 2_000,
    }));
    assert_eq!(l.schedule(2, None, None).grants, vec![second]);
}

#[test]
fn a_busy_syspolicyd_holds_only_a_further_compile() {
    let mut l = Ledger::new();
    l.set_capacity("llm:claude", 2, Kind::Llm);
    l.set_capacity("compile", 4, Kind::Machine);
    l.set_policy(Some(PolicyGate {
        busy_pct: 640,
        max_pct: 300,
    }));
    let compile = |n: u64| Rank::new(false, 1, 2, n, n, false);
    let first = l.submit(compile(1), Needs::of(&[("compile", 1)]), "#1", 0);
    assert_eq!(
        l.schedule(0, None, None).grants,
        vec![first],
        "the first compile starts: nothing of ours is running, so the queue is never held for good"
    );
    let second = l.submit(compile(2), Needs::of(&[("compile", 1)]), "#2", 1);
    let agent = l.submit(compile(3), Needs::of(&[("llm:claude", 1)]), "#3", 1);
    let p = l.schedule(1, None, None);
    assert_eq!(p.grants, vec![agent], "agents are not held by syspolicyd");
    let w = &p.waits[&second];
    assert!(matches!(w.cause, WaitCause::Policy { .. }), "{:?}", w.cause);
    let text = explain(w, 1);
    assert!(
        text.contains("syspolicyd is at 640% CPU") && text.contains("limit 300%"),
        "{text}"
    );
    // A limit of 0 turns the gate off.
    l.set_policy(Some(PolicyGate {
        busy_pct: 640,
        max_pct: 0,
    }));
    assert_eq!(l.schedule(2, None, None).grants, vec![second]);
    let third = l.submit(compile(4), Needs::of(&[("compile", 1)]), "#4", 3);
    l.set_policy(Some(PolicyGate {
        busy_pct: 120,
        max_pct: 300,
    }));
    assert_eq!(
        l.schedule(3, None, None).grants,
        vec![third],
        "below the limit"
    );
}

#[test]
fn llm_sessions_beyond_the_limit_wait_and_none_are_refused() {
    let mut l = Ledger::new();
    l.set_capacity("llm:claude", 10, Kind::Llm);
    let ids: Vec<u64> = (1..=25)
        .map(|i| {
            l.submit(
                Rank::new(false, 1, 2, i, i, false),
                Needs::of(&[("llm:claude", 1)]),
                &format!("#{i}"),
                0,
            )
        })
        .collect();
    let p = l.schedule(0, None, None);
    assert_eq!(p.grants.len(), 10);
    assert_eq!(p.waits.len(), 15);
    assert!(p
        .waits
        .values()
        .all(|w| matches!(w.cause, WaitCause::Full { .. })));
    l.release(ids[0]);
    assert_eq!(l.schedule(1, None, None).grants, vec![ids[10]]);
}

#[test]
fn a_landing_is_not_held_by_the_cpu_or_the_pacing_but_is_by_the_count_and_a_stall() {
    let mut l = Ledger::new();
    l.set_capacity("compile", 2, Kind::Machine);
    l.pace_compiles(30_000);
    let hot = Some(CpuGate {
        busy_pct: 90,
        max_pct: 50,
    });
    let first = l.submit(
        Rank::new(false, 3, 2, 1, 1, false),
        Needs::of(&[("compile", 1)]),
        "#1",
        0,
    );
    assert_eq!(l.schedule_gated(0, None, None, hot).grants, vec![first]);
    let other = l.submit(
        Rank::new(false, 2, 2, 2, 2, false),
        Needs::of(&[("compile", 1)]),
        "#2",
        1,
    );
    let land = l.submit(
        Rank::new(false, 1, 2, 3, 3, false).landing(),
        Needs::of(&[("compile", 1)]),
        "batch 1",
        1,
    );
    // Hot and inside the gap: the landing starts, the other compile waits.
    let p = l.schedule_gated(1_000, None, None, hot);
    assert_eq!(p.grants, vec![land]);
    assert!(matches!(p.waits[&other].cause, WaitCause::Full { .. }));
    // A landing still needs a free slot.
    l.release(land);
    l.set_capacity("compile", 1, Kind::Machine);
    let land2 = l.submit(
        Rank::new(false, 1, 2, 4, 4, false).landing(),
        Needs::of(&[("compile", 1)]),
        "batch 2",
        2,
    );
    let p = l.schedule_gated(2_000, None, None, hot);
    assert!(p.grants.is_empty());
    assert!(matches!(p.waits[&land2].cause, WaitCause::Full { .. }));
    // And a stalled build disk holds it beside a running compile.
    l.set_capacity("compile", 4, Kind::Machine);
    l.set_stall(Some(StallGate {
        slow_ms: 30_000,
        max_ms: 2_000,
    }));
    let p = l.schedule_gated(3_000, None, None, None);
    assert!(matches!(p.waits[&land2].cause, WaitCause::DiskStall { .. }));
}

// ---- aging (`order.max_wait_s`) ------------------------------------------------------------

const MAX_WAIT: u64 = 5;

fn machine_needs(n: &Needs) -> bool {
    uses_machine(n)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// With aging on and holders that keep releasing, every request is
    /// granted (no starvation), and while a pipeline request that has aged
    /// and wants the whole machine waits, no pipeline request ranked behind it
    /// that uses the machine is granted (the machine drains for it). Work
    /// further along still goes first (s349).
    #[test]
    fn with_aging_every_request_is_eventually_granted(
        jobs in prop::collection::vec(job(), 1..30),
        caps in prop::array::uniform5(1u32..4),
    ) {
        let mut l = Ledger::new();
        l.set_max_wait(Some(MAX_WAIT));
        for (i, (name, kind)) in RESOURCES.iter().enumerate() {
            l.set_capacity(name, caps[i], *kind);
        }
        let mut pending: Vec<(usize, &Job)> = jobs.iter().enumerate().collect();
        pending.sort_by_key(|(_, j)| j.arrive);
        let mut pending = pending.into_iter().peekable();
        let mut ends: BTreeMap<u64, (u64, usize)> = BTreeMap::new();
        let mut needs_of: BTreeMap<u64, Needs> = BTreeMap::new();
        let mut inter_of: BTreeMap<u64, bool> = BTreeMap::new();
        let mut since_of: BTreeMap<u64, u64> = BTreeMap::new();
        let mut rank_of: BTreeMap<u64, Rank> = BTreeMap::new();
        let mut now = 0u64;
        let mut finished = 0usize;
        let mut guard = 0;
        while finished < jobs.len() {
            guard += 1;
            prop_assert!(guard < 10_000, "starvation or deadlock");
            while let Some((i, j)) = pending.next_if(|(_, j)| j.arrive <= now) {
                let n = needs(j, &caps);
                let rank = Rank::new(j.interactive, j.steps, j.priority, j.arrive, i as u64, false);
                let id = l.submit(rank, n.clone(), &format!("#{i}"), now);
                rank_of.insert(id, rank);
                needs_of.insert(id, n);
                inter_of.insert(id, j.interactive);
                since_of.insert(id, now);
                ends.insert(id, (u64::MAX, i));
            }
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
            let before = l.snapshot();
            let p = l.schedule(now, None, None);
            for g in &p.grants {
                let (_, ji) = ends[g];
                ends.insert(*g, (now + jobs[ji].run, ji));
                for w in &before.waiting {
                    let aged_exclusive = needs_of[&w.id].exclusive
                        && !inter_of[&w.id]
                        && now.saturating_sub(since_of[&w.id]) >= MAX_WAIT;
                    // A rank as the planner sees it now: aged once it has waited MAX_WAIT.
                    let eff = |id: u64| {
                        let r = rank_of[&id];
                        if now.saturating_sub(since_of[&id]) >= MAX_WAIT { r.aged(since_of[&id]) } else { r }
                    };
                    if w.id != *g
                        && aged_exclusive
                        && !p.grants.contains(&w.id)
                        && !inter_of[g]
                        && machine_needs(&needs_of[g])
                        // Only a request ranked ahead of it may pass: further along, or aged longer.
                        && eff(*g) > eff(w.id)
                    {
                        prop_assert!(
                            false,
                            "{g} was granted the machine ahead of aged exclusive {}", w.id
                        );
                    }
                }
            }
            let next_release = ends.values().filter(|(e, _)| *e != u64::MAX).map(|(e, _)| *e).min();
            let next_arrival = pending.peek().map(|(_, j)| j.arrive);
            // Time also moves to the moment a waiting request ages.
            let next_age = l
                .snapshot()
                .waiting
                .iter()
                .map(|r| since_of[&r.id] + MAX_WAIT)
                .filter(|t| *t > now)
                .min();
            let next = [next_release, next_arrival, next_age].into_iter().flatten().min();
            match next {
                Some(t) => now = t.max(now),
                None => prop_assert!(finished >= jobs.len(), "waiters but nothing running: a deadlock"),
            }
        }
        let s = l.snapshot();
        prop_assert!(s.holders.is_empty() && s.waiting.is_empty());
    }
}

/// Holders of `test-suite` and `compile` that keep arriving ahead of an
/// exclusive request ranked last.
#[test]
fn an_exclusive_request_at_the_back_is_first_once_it_has_waited_max_wait() {
    let mut l = Ledger::new();
    l.set_max_wait(Some(1_000));
    l.set_capacity("compile", 2, Kind::Machine);
    l.set_capacity("test-suite", 2, Kind::Machine);
    // As far along, but the other project's work is of higher priority, so its requests
    // outrank the exclusive one until that one has waited max_wait.
    let mine = |id: u64| Rank::new(false, 2, 1, id, id, false);
    let h1 = l.submit(mine(1), Needs::of(&[("test-suite", 1)]), "m#1", 0);
    let h2 = l.submit(mine(2), Needs::of(&[("test-suite", 1)]), "m#2", 0);
    assert_eq!(l.schedule(0, None, None).grants, vec![h1, h2]);
    let star = l.submit(
        Rank::new(false, 2, 2, 3, 3, false),
        Needs {
            exclusive: true,
            amounts: Needs::of(&[("compile", 1)]).amounts,
            flex: false,
        },
        "star#1",
        0,
    );
    // Before max_wait: a stream of earlier-ranked requests keeps being granted past it.
    let c1 = l.submit(mine(4), Needs::of(&[("compile", 1)]), "m#4", 10);
    let p = l.schedule(10, None, None);
    assert_eq!(
        p.grants,
        vec![c1],
        "a compile passes the exclusive request, as today"
    );
    assert!(matches!(
        p.waits[&star].cause,
        WaitCause::Full { .. } | WaitCause::Exclusive
    ));
    assert_eq!(p.waits[&star].aged_ms, None);
    l.release(c1);
    // After max_wait: it is first, and nothing new that uses the machine is granted.
    let late = l.submit(mine(5), Needs::of(&[("compile", 1)]), "m#5", 1_000);
    let p = l.schedule(1_000, None, None);
    assert!(
        p.grants.is_empty(),
        "no new machine-class request passes it: {p:?}"
    );
    let w = &p.waits[&star];
    assert_eq!(w.position, 1);
    assert_eq!(w.aged_ms, Some(1_000));
    assert!(explain(w, 1_000).contains("waited 1 s: ahead of work as far along"));
    assert!(p.waits[&late].position > 1);
    // The holders release; it is granted, then the others follow.
    l.release(h1);
    assert!(l.schedule(1_001, None, None).grants.is_empty());
    l.release(h2);
    assert_eq!(l.schedule(1_002, None, None).grants, vec![star]);
    l.release(star);
    assert_eq!(l.schedule(1_003, None, None).grants, vec![late]);
}

#[test]
fn aging_is_off_without_a_max_wait_and_a_landing_keeps_its_place() {
    let mut l = Ledger::new();
    l.set_capacity("test-suite", 1, Kind::Machine);
    let hold = l.submit(
        Rank::new(false, 1, 2, 0, 0, false),
        Needs::of(&[("test-suite", 1)]),
        "h",
        0,
    );
    assert_eq!(l.schedule(0, None, None).grants, vec![hold]);
    let old = l.submit(
        Rank::new(false, 5, 2, 1, 1, false),
        Needs::of(&[("test-suite", 1)]),
        "old",
        0,
    );
    let near = l.submit(
        Rank::new(false, 1, 2, 2, 2, false),
        Needs::of(&[("test-suite", 1)]),
        "near",
        500,
    );
    assert_eq!(l.snapshot_at(10_000).waiting[0].id, near);
    l.set_max_wait(Some(1_000));
    let land = l.submit(
        Rank::new(false, 9, 2, 3, 3, false).landing(),
        Needs::of(&[("test-suite", 1)]),
        "land",
        9_999,
    );
    let order: Vec<u64> = l.snapshot_at(10_000).waiting.iter().map(|r| r.id).collect();
    assert_eq!(
        order,
        vec![land, near, old],
        "a landing first, then work further along: both have aged, and aging never passes progress (s349)"
    );
}

#[test]
fn a_task_in_test_of_a_longer_route_is_not_behind_a_task_in_develop_of_a_shorter_one() {
    let mut l = Ledger::new();
    l.set_capacity("test-suite", 1, Kind::Machine);
    let hold = l.submit(
        Rank::new(false, 1, 2, 0, 0, false),
        Needs::of(&[("test-suite", 1)]),
        "h",
        0,
    );
    l.schedule(0, None, None);
    // Project A: explore, develop, test, review, land (5 on the route, 3 left in test).
    let a_test = l.submit(
        Rank::new(false, 3, 2, 5, 1, false).with_span(5),
        Needs::of(&[("test-suite", 1)]),
        "A#test",
        1,
    );
    // Project B: explore, develop, test, land (4 on the route, 3 left in develop). B's task
    // is older, so with raw steps (3 and 3) it would go first.
    let b_develop = l.submit(
        Rank::new(false, 3, 2, 1, 2, false).with_span(4),
        Needs::of(&[("test-suite", 1)]),
        "B#develop",
        0,
    );
    // A has 3/5 of its route left, B 3/4: A has come further.
    l.release(hold);
    assert_eq!(l.schedule(2, None, None).grants, vec![a_test]);
    l.release(a_test);
    assert_eq!(l.schedule(3, None, None).grants, vec![b_develop]);
}

/// 2026-11-13: two waiters that need one `compile` and one `test-suite` each, with every
/// test-suite slot taken, held back every compile-only request with `compile` idle. Then they set
/// aside one compile slot each; since s471 only the first in line does.
#[test]
fn waiters_set_aside_what_they_need_and_the_rest_is_used() {
    let mut l = Ledger::new();
    l.set_capacity("compile", 5, Kind::Machine);
    l.set_capacity("test-suite", 1, Kind::Machine);
    let rank = |s, id| Rank::new(false, s, 2, id, id, false);
    let suite = l.submit(rank(1, 1), Needs::of(&[("test-suite", 1)]), "#1", 0);
    assert_eq!(l.schedule(0, None, None).grants, vec![suite]);
    let both = || Needs::of(&[("compile", 1), ("test-suite", 1)]);
    let a = l.submit(rank(2, 2), both(), "#2", 1);
    let b = l.submit(rank(2, 3), both(), "#3", 1);
    let compiles: Vec<u64> = (4..9)
        .map(|i| {
            l.submit(
                rank(3, i),
                Needs::of(&[("compile", 1)]),
                &format!("#{i}"),
                1,
            )
        })
        .collect();
    let p = l.schedule(2, None, None);
    // Five compile slots; only #2, first in line for the test-suite slot, sets one aside
    // (s471, decision 0464): four compiles start, one waits.
    assert_eq!(p.grants, compiles[..4].to_vec());
    assert!(p.waits.contains_key(&a) && p.waits.contains_key(&b));
    assert!(matches!(
        p.waits[&compiles[4]].cause,
        WaitCause::Behind { ref resource } if resource == "compile"
    ));
    // The suite ends: #2 gets the test-suite slot and a compile slot it was promised.
    l.release(suite);
    let p = l.schedule(3, None, None);
    assert_eq!(p.grants, vec![a]);
}

/// s471: the overnight shape. Compile 6, test-suite 4 of 4 held, mixed waiters ranked ahead of
/// compile-only ones. Mixed waiters wait for `test-suite` and set aside no more `compile` than
/// the first in line needs.
fn overnight(mixed: u64, compiles: u64) -> (Ledger, Vec<u64>, Vec<u64>) {
    let mut l = Ledger::new();
    l.set_capacity("compile", 6, Kind::Machine);
    l.set_capacity("test-suite", 4, Kind::Machine);
    let rank = |s, id| Rank::new(false, s, 2, id, id, false);
    for i in 1..=4 {
        l.submit(rank(1, i), Needs::of(&[("test-suite", 1)]), "held", 0);
    }
    assert_eq!(l.schedule(0, None, None).grants.len(), 4);
    let m: Vec<u64> = (0..mixed)
        .map(|i| {
            l.submit(
                rank(2, 10 + i),
                Needs::of(&[("compile", 1), ("test-suite", 1)]),
                "mixed",
                1,
            )
        })
        .collect();
    let c: Vec<u64> = (0..compiles)
        .map(|i| l.submit(rank(3, 100 + i), Needs::of(&[("compile", 1)]), "build", 1))
        .collect();
    (l, m, c)
}

#[test]
fn waiters_short_of_test_suite_do_not_reserve_every_compile_slot() {
    let (mut l, m, c) = overnight(8, 3);
    let p = l.schedule(2, None, None);
    assert_eq!(p.grants, c);
    for id in &m {
        assert!(matches!(
            p.waits[id].cause,
            WaitCause::Full { ref resource } if resource == "test-suite"
        ));
    }
}

#[test]
fn the_overnight_ledger_grants_every_compile_only_waiter_that_fits() {
    let (mut l, m, c) = overnight(25, 10);
    let p = l.schedule(2, None, None);
    // Six compile slots, one set aside for the first mixed waiter.
    assert_eq!(p.grants, c[..5].to_vec());
    assert!(matches!(
        p.waits[&c[5]].cause,
        WaitCause::Behind { ref resource } if resource == "compile"
    ));
    for id in &m {
        assert!(matches!(
            p.waits[id].cause,
            WaitCause::Full { ref resource } if resource == "test-suite"
        ));
    }
}

#[test]
fn the_first_mixed_waiter_is_granted_when_test_suite_frees_despite_new_compiles() {
    let (mut l, m, c) = overnight(3, 5);
    let p = l.schedule(2, None, None);
    assert_eq!(p.grants, c);
    // Compile-only requests keep arriving and fill what is left.
    for i in 0..4 {
        l.submit(
            Rank::new(false, 3, 2, 200 + i, 200 + i, false),
            Needs::of(&[("compile", 1)]),
            "later",
            3,
        );
    }
    let p = l.schedule(3, None, None);
    assert!(p.grants.is_empty());
    // One test-suite unit frees: the first mixed waiter gets it and its compile slot.
    l.release(1);
    let p = l.schedule(4, None, None);
    assert_eq!(p.grants, vec![m[0]]);
}

#[test]
fn an_exclusive_waiter_still_blocks_machine_class_work() {
    let mut l = Ledger::new();
    l.set_capacity("compile", 6, Kind::Machine);
    l.set_capacity("test-suite", 4, Kind::Machine);
    let h = l.submit(
        Rank::new(false, 1, 2, 1, 1, false),
        Needs::of(&[("test-suite", 1)]),
        "held",
        0,
    );
    assert_eq!(l.schedule(0, None, None).grants, vec![h]);
    let mut ex = Needs::of(&[("test-suite", 1)]);
    ex.exclusive = true;
    let e = l.submit(Rank::new(false, 2, 2, 2, 2, false), ex, "exclusive", 1);
    let c = l.submit(
        Rank::new(false, 3, 2, 3, 3, false),
        Needs::of(&[("compile", 1)]),
        "build",
        1,
    );
    let p = l.schedule(2, None, None);
    assert!(p.grants.is_empty());
    assert!(p.waits.contains_key(&e));
    assert!(matches!(p.waits[&c].cause, WaitCause::Exclusive));
}

fn paced_ledger() -> (Ledger, CpuGate) {
    let mut l = Ledger::new();
    l.set_capacity("compile", 5, Kind::Machine);
    l.pace_compiles(30_000);
    let idle = CpuGate {
        busy_pct: 10,
        max_pct: 50,
    };
    (l, idle)
}

fn submit_compile(l: &mut Ledger, n: u64, now: u64) -> u64 {
    l.submit(
        Rank::new(false, 1, 2, n, n, false),
        Needs::of(&[("compile", 1)]),
        &format!("#{n}"),
        now,
    )
}

#[test]
fn the_pacing_pause_does_not_apply_with_no_compile_running() {
    let (mut l, idle) = paced_ledger();
    let a = submit_compile(&mut l, 1, 0);
    assert_eq!(l.schedule_gated(0, None, None, Some(idle)).grants, vec![a]);
    // It finished inside the gap: nothing runs, so the next one starts at once.
    assert!(l.release(a));
    let b = submit_compile(&mut l, 2, 5_000);
    assert_eq!(
        l.schedule_gated(5_000, None, None, Some(idle)).grants,
        vec![b]
    );
}

#[test]
fn the_pacing_pause_still_applies_beside_a_running_compile() {
    let (mut l, idle) = paced_ledger();
    let a = submit_compile(&mut l, 1, 0);
    assert_eq!(l.schedule_gated(0, None, None, Some(idle)).grants, vec![a]);
    let b = submit_compile(&mut l, 2, 5_000);
    let p = l.schedule_gated(5_000, None, None, Some(idle));
    assert!(p.grants.is_empty());
    assert!(matches!(p.waits[&b].cause, WaitCause::Settling { .. }));
}

#[test]
fn with_none_running_the_first_in_a_pass_starts_and_the_second_is_paced() {
    let (mut l, idle) = paced_ledger();
    let a = submit_compile(&mut l, 1, 0);
    let b = submit_compile(&mut l, 2, 0);
    let p = l.schedule_gated(1_000, None, None, Some(idle));
    assert_eq!(p.grants, vec![a]);
    assert!(matches!(p.waits[&b].cause, WaitCause::Settling { .. }));
}

#[test]
fn a_request_behind_a_paused_one_names_it_and_the_reason_not_a_holder() {
    let (mut l, _) = paced_ledger();
    // One compile slot, so the second compile is behind the first (which reserves it), not
    // merely held by the same gate.
    l.set_capacity("compile", 1, Kind::Machine);
    l.set_capacity("test-suite", 2, Kind::Machine);
    // Other work is running, so low memory pauses the first compile in line, with no
    // compile held.
    let t = l.submit(
        Rank::new(false, 1, 2, 1, 1, false),
        Needs::of(&[("test-suite", 1)]),
        "#1",
        0,
    );
    assert_eq!(l.schedule(0, None, None).grants, vec![t]);
    submit_compile(&mut l, 2, 0);
    let c = submit_compile(&mut l, 3, 0);
    let p = l.schedule_gated(0, Some(100), Some(2000), None);
    assert!(p.grants.is_empty());
    assert_eq!(
        explain(&p.waits[&c], 0),
        "Waiting for compile slot: 2nd in line, behind #2 (memory is low)"
    );
}

// ---- backfill while the machine drains (s385) --------------------------------------------

/// A holder expected to be done at 1_000, and an exclusive request waiting for the machine.
fn draining(holder_estimate: Option<u64>) -> (Ledger, u64) {
    let mut l = Ledger::new();
    l.set_capacity("compile", 3, Kind::Machine);
    l.set_capacity("test-suite", 2, Kind::Machine);
    let h = l.submit(
        Rank::new(false, 1, 2, 0, 0, false),
        Needs::of(&[("test-suite", 1)]),
        "holder",
        0,
    );
    l.set_expected(h, holder_estimate);
    assert_eq!(l.schedule(0, None, None).grants, vec![h]);
    let x = l.submit(
        Rank::new(false, 2, 2, 1, 1, false),
        Needs {
            exclusive: true,
            amounts: BTreeMap::new(),
            flex: false,
        },
        "exclusive",
        50,
    );
    (l, x)
}

#[test]
fn short_work_runs_while_the_machine_drains_and_long_or_unknown_work_waits() {
    let (mut l, x) = draining(Some(1_000));
    let later = |l: &mut Ledger, id: u64, e: Option<u64>| {
        let r = l.submit(
            Rank::new(false, 3, 2, id, id, false),
            Needs::of(&[("compile", 1)]),
            &format!("c{id}"),
            100,
        );
        l.set_expected(r, e);
        r
    };
    let short = later(&mut l, 2, Some(500));
    let long = later(&mut l, 3, Some(2_000));
    let unknown = later(&mut l, 4, None);
    let p = l.schedule(100, None, None);
    assert_eq!(
        p.grants,
        vec![short],
        "done by 600, before the drain ends at 1_000"
    );
    assert!(p.waits.contains_key(&x));
    assert_eq!(
        p.waits[&long].cause,
        WaitCause::Exclusive,
        "would run past the drain"
    );
    assert_eq!(p.waits[&unknown].cause, WaitCause::Exclusive, "no estimate");
    // Once the holders are done, the exclusive request goes first, as before.
    let held: Vec<u64> = l.snapshot().holders.iter().map(|h| h.id).collect();
    for id in held {
        l.release(id);
    }
    assert_eq!(l.schedule(1_000, None, None).grants, vec![x]);
}

#[test]
fn nothing_passes_a_waiting_exclusive_request_when_a_holder_has_no_estimate() {
    let (mut l, x) = draining(None);
    let c = l.submit(
        Rank::new(false, 3, 2, 2, 2, false),
        Needs::of(&[("compile", 1)]),
        "c",
        100,
    );
    l.set_expected(c, Some(10));
    let p = l.schedule(100, None, None);
    assert!(p.grants.is_empty(), "{p:?}");
    assert_eq!(p.waits[&c].cause, WaitCause::Exclusive);
    assert!(p.waits.contains_key(&x));
}

// ---- a waiting exclusive request must not idle free slots or freeze the queue (s552) ------

const MIN: u64 = 60_000;

fn work(l: &mut Ledger, id: u64, uses: &[(&str, u32)], expected: Option<u64>, now: u64) -> u64 {
    let r = l.submit(
        Rank::new(false, 3, 2, id, id, false),
        Needs::of(uses),
        &format!("w{id}"),
        now,
    );
    l.set_expected(r, expected);
    r
}

#[test]
fn holders_that_overran_do_not_stop_backfill_for_good() {
    let (mut l, x) = draining(Some(1_000));
    // The holder is expected done at 1_000; at 2_000 it still runs. The drain end moves to
    // one more expected run from now, 3_000, instead of staying in the past.
    let fits = work(&mut l, 2, &[("compile", 1)], Some(900), 2_000);
    let too_long = work(&mut l, 3, &[("compile", 1)], Some(1_500), 2_000);
    let p = l.schedule(2_000, None, None);
    assert_eq!(p.grants, vec![fits], "{p:?}");
    assert_eq!(p.waits[&too_long].cause, WaitCause::Exclusive);
    assert!(p.waits.contains_key(&x));
}

#[test]
fn an_exclusive_request_gives_up_its_freeze_after_its_bound_and_still_goes_first() {
    // A holder with no estimate: nothing passes before the bound (s385) ...
    let (mut l, x) = draining(None);
    l.set_exclusive_max_wait(Some(600));
    let c = work(&mut l, 2, &[("compile", 1)], Some(10), 100);
    let unknown = work(&mut l, 3, &[("compile", 1)], None, 100);
    let p = l.schedule(100, None, None);
    assert!(p.grants.is_empty(), "{p:?}");
    assert_eq!(p.waits[&x].thawed_ms, None);
    // ... and after it, work with an estimate runs again, and the request says so.
    let p = l.schedule(650, None, None);
    assert_eq!(p.grants, vec![c], "{p:?}");
    assert_eq!(p.waits[&unknown].cause, WaitCause::Exclusive, "no estimate");
    let w = &p.waits[&x];
    assert_eq!(w.thawed_ms, Some(600));
    assert!(
        explain(w, 650).contains("work runs again until the machine drains"),
        "{}",
        explain(w, 650)
    );
    // Once the holders are done, it goes first, ahead of work that arrived later.
    let held: Vec<u64> = l.snapshot().holders.iter().map(|h| h.id).collect();
    for id in held {
        l.release(id);
    }
    let late = work(&mut l, 9, &[("compile", 1)], Some(10), 700);
    let p = l.schedule(700, None, None);
    assert_eq!(p.grants, vec![x], "{p:?}");
    assert!(p.waits.contains_key(&late));
}

/// The 2026-11-17 09:22 case: 3 of 6 `test-suite` slots held (1 h, 48 min and 44 min, all past
/// their expected end), `compile` and `test-small` idle, an agent's exclusive `xcodebuild`
/// request first in line for 13 min, and ordinary waiters behind it.
fn replay_0922(max_wait: Option<u64>) -> (Ledger, u64, Vec<u64>, u64) {
    let now = 100 * MIN;
    let mut l = Ledger::new();
    l.set_capacity("compile", 6, Kind::Machine);
    l.set_capacity("test-suite", 6, Kind::Machine);
    l.set_capacity("test-small", 2, Kind::Machine);
    l.set_exclusive_max_wait(max_wait);
    for (i, ago) in [60u64, 48, 44].into_iter().enumerate() {
        let h = work(&mut l, i as u64, &[("test-suite", 1)], Some(40 * MIN), 0);
        assert_eq!(l.schedule(now - ago * MIN, None, None).grants, vec![h]);
    }
    let x = l.submit(
        Rank::new(false, 0, 0, 0, 10, false),
        Needs::exclusive(),
        "xcodebuild archive",
        now - 13 * MIN,
    );
    let waiters: Vec<u64> = (20..23)
        .map(|i| {
            work(
                &mut l,
                i,
                &[("test-suite", 1)],
                Some(30 * MIN),
                now - 12 * MIN,
            )
        })
        .collect();
    (l, x, waiters, now)
}

#[test]
fn the_0922_case_starts_the_waiters_at_once() {
    for bound in [None, Some(10 * MIN)] {
        let (mut l, x, waiters, now) = replay_0922(bound);
        let p = l.schedule(now, None, None);
        assert_eq!(p.grants, waiters, "bound {bound:?}: {p:?}");
        assert!(p.waits.contains_key(&x));
        // Everything running ends; the exclusive request is next, ahead of anything new.
        let held: Vec<u64> = l.snapshot().holders.iter().map(|h| h.id).collect();
        for id in held {
            l.release(id);
        }
        let late = work(&mut l, 50, &[("compile", 1)], Some(MIN), now);
        let p = l.schedule(now + MIN, None, None);
        assert_eq!(p.grants, vec![x]);
        assert!(p.waits.contains_key(&late));
    }
}

#[test]
fn a_hot_cpu_holds_further_machine_work_but_not_the_first_or_an_agent() {
    // 2026-11-18: the owner's iMac Pro sat at a 27% CPU speed limit under two test suites and
    // two compiles, ten minutes after a T2 watchdog reset.
    let mut l = Ledger::new();
    l.set_capacity("llm:claude", 2, Kind::Llm);
    l.set_capacity("compile", 4, Kind::Machine);
    l.set_capacity("test-suite", 3, Kind::Machine);
    l.set_thermal(Some(ThermalGate {
        speed_pct: 27,
        min_pct: 80,
    }));
    let rank = |n: u64| Rank::new(false, 1, 2, n, n, false);
    let first = l.submit(rank(1), Needs::of(&[("test-suite", 1)]), "#1", 0);
    assert_eq!(
        l.schedule(0, None, None).grants,
        vec![first],
        "the first machine job starts: nothing of ours runs, so a hot machine never holds the queue for good"
    );
    let suite = l.submit(rank(2), Needs::of(&[("test-suite", 1)]), "#2", 1);
    let compile = l.submit(rank(3), Needs::of(&[("compile", 1)]), "#3", 1);
    let agent = l.submit(rank(4), Needs::of(&[("llm:claude", 1)]), "#4", 1);
    let p = l.schedule(1, None, None);
    assert_eq!(p.grants, vec![agent], "agents are not held by heat");
    for id in [suite, compile] {
        let w = &p.waits[&id];
        assert!(
            matches!(w.cause, WaitCause::Thermal { .. }),
            "{:?}",
            w.cause
        );
        let text = explain(w, 1);
        assert!(text.contains("27%") && text.contains("80%"), "{text}");
    }
    // At or above the minimum, work starts again.
    l.set_thermal(Some(ThermalGate {
        speed_pct: 90,
        min_pct: 80,
    }));
    let p = l.schedule(2, None, None);
    assert!(
        p.grants.contains(&suite) && p.grants.contains(&compile),
        "{:?}",
        p.grants
    );
    // A minimum of 0 turns the gate off.
    let more = l.submit(rank(5), Needs::of(&[("compile", 1)]), "#5", 3);
    l.set_thermal(Some(ThermalGate {
        speed_pct: 10,
        min_pct: 0,
    }));
    assert_eq!(l.schedule(3, None, None).grants, vec![more]);
}

// ---- a converted all-compile request has the exclusive request's bound and backfill (s681) --

/// 6 compile units, one held (a long check, estimate `holder_estimate`), a converted request for
/// all 6 first in line, then `n` one-unit compile waiters.
fn converted_case(
    holder_estimate: Option<u64>,
    waiter_estimate: Option<u64>,
    n: u64,
) -> (Ledger, u64, Vec<u64>) {
    let mut l = Ledger::new();
    l.set_capacity("compile", 6, Kind::Machine);
    l.set_exclusive_max_wait(Some(10 * MIN));
    let h = l.submit(
        Rank::new(false, 1, 2, 0, 0, false),
        Needs::of(&[("compile", 1)]),
        "exit-script check",
        0,
    );
    l.set_expected(h, holder_estimate);
    assert_eq!(l.schedule(0, None, None).grants, vec![h]);
    let x = l.submit(
        Rank::new(false, 2, 2, 1, 1, false),
        Needs::of(&[("compile", 6)]),
        "starpipe heavy",
        50,
    );
    l.set_drains_compile(x, true);
    let ws = (0..n)
        .map(|i| work(&mut l, 10 + i, &[("compile", 1)], waiter_estimate, 100))
        .collect();
    (l, x, ws)
}

#[test]
fn converted_request_lets_short_compile_work_pass_and_holds_long_work() {
    let (mut l, x, ws) = converted_case(Some(60 * MIN), Some(MIN), 2);
    let long = work(&mut l, 50, &[("compile", 1)], Some(120 * MIN), 100);
    let unknown = work(&mut l, 51, &[("compile", 1)], None, 100);
    let p = l.schedule(100, None, None);
    assert_eq!(p.grants, ws, "{p:?}");
    assert!(p.waits.contains_key(&x));
    assert!(matches!(p.waits[&long].cause, WaitCause::Behind { .. }));
    assert!(matches!(p.waits[&unknown].cause, WaitCause::Behind { .. }));
    assert_eq!(p.waits[&long].blocker.as_ref().map(|b| b.id), Some(x));
}

#[test]
fn converted_request_gives_up_its_freeze_after_the_bound_then_goes_first() {
    // The 2026-11-19 16:55 case: a 2 h holder, the request for all 6 units, 11 waiters.
    let (mut l, x, ws) = converted_case(Some(127 * MIN), None, 11);
    let p = l.schedule(100, None, None);
    assert!(p.grants.is_empty(), "frozen before the bound: {p:?}");
    // Waiters carrying an estimate are granted once the bound has passed.
    for w in &ws {
        l.set_expected(*w, Some(5 * MIN));
    }
    let p = l.schedule(11 * MIN, None, None);
    assert_eq!(p.grants.len(), 5, "5 free units, all used: {p:?}");
    let w = &p.waits[&x];
    assert_eq!(w.thawed_ms, Some(11 * MIN - 50));
    assert!(w.thawed_compile);
    assert!(
        explain(w, 11 * MIN).contains("until compile drains"),
        "{}",
        explain(w, 11 * MIN)
    );
    // Once compile is free it starts first, ahead of the rest of the waiters.
    let held: Vec<u64> = l.snapshot().holders.iter().map(|h| h.id).collect();
    for id in held {
        l.release(id);
    }
    let p = l.schedule(12 * MIN, None, None);
    assert_eq!(p.grants, vec![x], "{p:?}");
}

#[test]
fn a_person_s_exclusive_request_keeps_its_rules_beside_converted_ones() {
    // An exclusive request is not touched by the new rule: no thaw flag for compile.
    let (mut l, x) = draining(Some(1_000));
    l.set_exclusive_max_wait(Some(600));
    let p = l.schedule(700, None, None);
    let w = &p.waits[&x];
    assert_eq!(w.thawed_ms, Some(650));
    assert!(!w.thawed_compile);
}
