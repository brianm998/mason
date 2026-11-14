// SPDX-License-Identifier: GPL-3.0-or-later

//! The ordering rule, the passing rule, and "no deadlock, no lost lease",
//! checked over random workloads by simulating the whole run.

use mason_scheduler::{explain, CpuGate, Kind, Ledger, Needs, Rank, StallGate, WaitCause};
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
                    let reserved: u32 = earlier.iter().map(|w| w.amounts.get(r).copied().unwrap_or(0)).sum();
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
    /// and wants the whole machine waits, no other pipeline request that uses
    /// the machine is granted (the machine drains for it).
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
                    if w.id != *g
                        && aged_exclusive
                        && !p.grants.contains(&w.id)
                        && !inter_of[g]
                        && machine_needs(&needs_of[g])
                        // An aged request that has waited longer goes first.
                        && !(now.saturating_sub(since_of[g]) >= MAX_WAIT
                            && since_of[g] <= since_of[&w.id])
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
    // The other project's route is shorter, so its requests outrank the exclusive one.
    let mine = |id: u64| Rank::new(false, 2, 2, id, id, false);
    let h1 = l.submit(mine(1), Needs::of(&[("test-suite", 1)]), "m#1", 0);
    let h2 = l.submit(mine(2), Needs::of(&[("test-suite", 1)]), "m#2", 0);
    assert_eq!(l.schedule(0, None, None).grants, vec![h1, h2]);
    let star = l.submit(
        Rank::new(false, 3, 2, 3, 3, false),
        Needs {
            exclusive: true,
            amounts: Needs::of(&[("compile", 1)]).amounts,
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
    assert!(explain(w, 1_000).contains("waited 1 s: first in line once the machine is free"));
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
        vec![land, old, near],
        "a landing first, then the aged, oldest first"
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
/// test-suite slot taken, held back every compile-only request with `compile` idle. Now they set
/// aside one compile slot each, and the rest go to the requests behind them.
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
    // Five compile slots, two set aside for #2 and #3: three compiles start, two wait.
    assert_eq!(p.grants, compiles[..3].to_vec());
    assert!(p.waits.contains_key(&a) && p.waits.contains_key(&b));
    assert!(matches!(
        p.waits[&compiles[3]].cause,
        WaitCause::Behind { ref resource } if resource == "compile"
    ));
    // The suite ends: #2 gets the test-suite slot and a compile slot it was promised.
    l.release(suite);
    let p = l.schedule(3, None, None);
    assert_eq!(p.grants, vec![a]);
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
