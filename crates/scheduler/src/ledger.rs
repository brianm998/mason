// SPDX-License-Identifier: GPL-3.0-or-later

use crate::explain::{explain, WaitCause};
use crate::plan::{
    plan, Capacity, CompilePace, CpuGate, DiskGate, Holder, Kind, Needs, Plan, Request, World,
    COMPILE,
};
use crate::rank::Rank;
use std::collections::BTreeMap;

/// Who holds what and who waits, with the capacities that bound them. The
/// daemon keeps one of these behind a lock; every change goes through here so
/// a lease is never lost or granted twice.
#[derive(Clone, Debug, Default)]
pub struct Ledger {
    capacity: BTreeMap<String, Capacity>,
    holders: Vec<Holder>,
    waiting: Vec<Request>,
    next_id: u64,
    compile_gap_ms: Option<u64>,
    last_compile_start: Option<u64>,
    disk: Option<DiskGate>,
    /// When each resource last had a grant. Only observed, never consulted by `plan`.
    last_grant: BTreeMap<String, u64>,
    last_any_grant: Option<u64>,
}

/// A wait the owner should hear about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stall {
    pub resource: String,
    /// The request can never fit (too big for its limit, or an undeclared resource): no
    /// amount of waiting helps. Otherwise the resource has granted nothing for a long time.
    pub never: bool,
    /// How many requests wait on the resource for this cause.
    pub waiting: usize,
    /// How long the resource has granted nothing to the longest-waiting one.
    pub idle_ms: u64,
    /// The first in line, as the owner reads it.
    pub text: String,
    /// What the first in line is: its label, for `never` stalls.
    pub label: String,
    pub cause: WaitCause,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub capacity: BTreeMap<String, Capacity>,
    /// Units held per resource (can exceed capacity after a live decrease).
    pub used: BTreeMap<String, u32>,
    pub holders: Vec<Holder>,
    /// In rank order.
    pub waiting: Vec<Request>,
}

impl Ledger {
    pub fn new() -> Ledger {
        Ledger::default()
    }

    /// Set a limit. Lowering it never takes anything away from a holder: it
    /// only stops new grants until the count falls.
    pub fn set_capacity(&mut self, name: &str, units: u32, kind: Kind) {
        self.capacity
            .insert(name.to_string(), Capacity { units, kind });
    }

    /// Start compiles one at a time, at least this far apart.
    pub fn pace_compiles(&mut self, gap_ms: u64) {
        self.compile_gap_ms = Some(gap_ms);
    }

    fn compile_pace(&self) -> Option<CompilePace> {
        self.compile_gap_ms.map(|gap_ms| CompilePace {
            gap_ms,
            last_start: self.last_compile_start,
        })
    }

    /// The caller's latest disk reading against its floor, used by every plan after this.
    pub fn set_disk(&mut self, disk: Option<DiskGate>) {
        self.disk = disk;
    }

    pub fn capacity(&self, name: &str) -> Option<Capacity> {
        self.capacity.get(name).copied()
    }

    pub fn declared(&self, name: &str) -> bool {
        self.capacity.contains_key(name)
    }

    /// Queue a request. Returns its id.
    pub fn submit(&mut self, rank: Rank, needs: Needs, label: &str, now: u64) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.waiting.push(Request {
            id,
            rank,
            needs,
            label: label.to_string(),
            since: now,
        });
        id
    }

    /// A holder that already runs (adopted after a restart, or one the caller
    /// took out of band). It counts against capacity like any other.
    pub fn adopt(&mut self, needs: Needs, label: &str, now: u64) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.note_grant(&needs, now);
        self.holders.push(Holder {
            id,
            needs,
            label: label.to_string(),
            since: now,
        });
        id
    }

    /// Change a waiting request's rank (its task moved, or a priority changed).
    pub fn rerank(&mut self, id: u64, rank: Rank) {
        if let Some(r) = self.waiting.iter_mut().find(|r| r.id == id) {
            r.rank = rank;
        }
    }

    /// Grant whatever can be granted now. Granted requests become holders.
    pub fn schedule(
        &mut self,
        now: u64,
        free_memory_mb: Option<u64>,
        memory_floor_mb: Option<u64>,
    ) -> Plan {
        self.schedule_gated(now, free_memory_mb, memory_floor_mb, None)
    }

    /// As `schedule`, with the CPU gate on `compile`.
    pub fn schedule_gated(
        &mut self,
        now: u64,
        free_memory_mb: Option<u64>,
        memory_floor_mb: Option<u64>,
        cpu: Option<CpuGate>,
    ) -> Plan {
        let p = plan(
            &World {
                now,
                capacity: &self.capacity,
                holders: &self.holders,
                free_memory_mb,
                memory_floor_mb,
                cpu,
                compile_pace: self.compile_pace(),
                disk: self.disk,
            },
            &self.waiting,
        );
        for id in &p.grants {
            if let Some(i) = self.waiting.iter().position(|r| r.id == *id) {
                let r = self.waiting.remove(i);
                if r.needs.amounts.contains_key(COMPILE) {
                    self.last_compile_start = Some(now);
                }
                self.note_grant(&r.needs, now);
                self.holders.push(Holder {
                    id: r.id,
                    needs: r.needs,
                    label: r.label,
                    since: now,
                });
            }
        }
        p
    }

    fn note_grant(&mut self, needs: &Needs, now: u64) {
        for r in needs.amounts.keys() {
            self.last_grant.insert(r.clone(), now);
        }
        self.last_any_grant = Some(now);
    }

    /// Waits that will not move by themselves, as of `now`, without granting anything.
    /// A request that can never fit is always reported. Any other is reported when nothing
    /// has been granted on its resource for `long_after_ms` (0: never), measured from the
    /// later of the last grant and the request's arrival. A long queue that keeps moving
    /// is not a stall. Disk waits have their own alert and are left out.
    pub fn stalls(
        &self,
        now: u64,
        free_memory_mb: Option<u64>,
        memory_floor_mb: Option<u64>,
        cpu: Option<CpuGate>,
        long_after_ms: u64,
    ) -> Vec<Stall> {
        let plan = self.waits_gated(now, free_memory_mb, memory_floor_mb, cpu);
        let mut out: Vec<Stall> = vec![];
        let mut by_rank: Vec<&Request> = self.waiting.iter().collect();
        by_rank.sort_by_key(|r| (r.rank, r.id));
        for r in by_rank {
            let Some(w) = plan.waits.get(&r.id) else {
                continue;
            };
            let (resource, never, last) = match &w.cause {
                WaitCause::TooBig { resource, .. } | WaitCause::Unknown { resource } => {
                    (resource.clone(), true, None)
                }
                WaitCause::Full { resource } | WaitCause::Behind { resource } => {
                    (resource.clone(), false, self.last_grant.get(resource))
                }
                WaitCause::Cpu { .. } | WaitCause::Settling { .. } => {
                    (COMPILE.to_string(), false, self.last_grant.get(COMPILE))
                }
                WaitCause::Exclusive => ("exclusive".into(), false, self.last_any_grant.as_ref()),
                WaitCause::Memory { .. } => ("memory".into(), false, self.last_any_grant.as_ref()),
                WaitCause::Disk { .. } => continue,
            };
            let idle_ms = now.saturating_sub(last.copied().unwrap_or(0).max(r.since));
            if !never && (long_after_ms == 0 || idle_ms < long_after_ms) {
                continue;
            }
            if let Some(s) = out
                .iter_mut()
                .find(|s| s.resource == resource && s.never == never)
            {
                s.waiting += 1;
                s.idle_ms = s.idle_ms.max(idle_ms);
                continue;
            }
            out.push(Stall {
                resource,
                never,
                waiting: 1,
                idle_ms,
                text: explain(w, now),
                label: r.label.clone(),
                cause: w.cause.clone(),
            });
        }
        out
    }

    /// Drop a holder or a waiting request. `false` if there was no such id.
    pub fn release(&mut self, id: u64) -> bool {
        let before = self.holders.len() + self.waiting.len();
        self.holders.retain(|h| h.id != id);
        self.waiting.retain(|r| r.id != id);
        before != self.holders.len() + self.waiting.len()
    }

    pub fn holds(&self, id: u64) -> bool {
        self.holders.iter().any(|h| h.id == id)
    }

    pub fn is_waiting(&self, id: u64) -> bool {
        self.waiting.iter().any(|r| r.id == id)
    }

    pub fn holder(&self, id: u64) -> Option<&Holder> {
        self.holders.iter().find(|h| h.id == id)
    }

    /// The waits as of now, without granting anything.
    pub fn waits(
        &self,
        now: u64,
        free_memory_mb: Option<u64>,
        memory_floor_mb: Option<u64>,
    ) -> Plan {
        self.waits_gated(now, free_memory_mb, memory_floor_mb, None)
    }

    pub fn waits_gated(
        &self,
        now: u64,
        free_memory_mb: Option<u64>,
        memory_floor_mb: Option<u64>,
        cpu: Option<CpuGate>,
    ) -> Plan {
        plan(
            &World {
                now,
                capacity: &self.capacity,
                holders: &self.holders,
                free_memory_mb,
                memory_floor_mb,
                cpu,
                compile_pace: self.compile_pace(),
                disk: self.disk,
            },
            &self.waiting,
        )
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut used: BTreeMap<String, u32> = BTreeMap::new();
        for h in &self.holders {
            for (r, a) in &h.needs.amounts {
                *used.entry(r.clone()).or_insert(0) += a;
            }
        }
        let mut waiting = self.waiting.clone();
        waiting.sort_by_key(|r| (r.rank, r.id));
        Snapshot {
            capacity: self.capacity.clone(),
            used,
            holders: self.holders.clone(),
            waiting,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rank(steps: u32, enq: u64, id: u64) -> Rank {
        Rank::new(false, steps, 2, enq, id, false)
    }

    #[test]
    fn grants_in_rank_order_and_explains_the_rest() {
        let mut l = Ledger::new();
        l.set_capacity("compile", 1, Kind::Machine);
        let far = l.submit(rank(4, 1, 1), Needs::of(&[("compile", 1)]), "#1", 0);
        let near = l.submit(rank(1, 2, 2), Needs::of(&[("compile", 1)]), "#2", 0);
        let p = l.schedule(0, None, None);
        // Fewer steps remaining wins even though #1 queued first.
        assert_eq!(p.grants, vec![near]);
        assert!(l.holds(near) && l.is_waiting(far));
        let w = &p.waits[&far];
        assert_eq!(
            explain(w, 120_000),
            "Waiting for compile slot: 1st in line, held by #2 for 2 min"
        );
        assert!(l.release(near));
        assert_eq!(l.schedule(1, None, None).grants, vec![far]);
    }

    #[test]
    fn lowering_a_limit_takes_nothing_away() {
        let mut l = Ledger::new();
        l.set_capacity("llm:claude", 2, Kind::Llm);
        let a = l.submit(rank(1, 1, 1), Needs::of(&[("llm:claude", 1)]), "#1", 0);
        let b = l.submit(rank(1, 2, 2), Needs::of(&[("llm:claude", 1)]), "#2", 0);
        let c = l.submit(rank(1, 3, 3), Needs::of(&[("llm:claude", 1)]), "#3", 0);
        assert_eq!(l.schedule(0, None, None).grants, vec![a, b]);
        l.set_capacity("llm:claude", 1, Kind::Llm);
        assert!(l.holds(a) && l.holds(b), "nothing was killed");
        assert!(l.schedule(1, None, None).grants.is_empty());
        l.release(a);
        assert!(
            l.schedule(2, None, None).grants.is_empty(),
            "still one held"
        );
        l.release(b);
        assert_eq!(l.schedule(3, None, None).grants, vec![c]);
    }

    #[test]
    fn releasing_an_unknown_id_says_so() {
        let mut l = Ledger::new();
        assert!(!l.release(7));
    }

    #[test]
    fn a_stream_of_task_linked_requests_never_holds_back_a_waiting_test_step() {
        let mut l = Ledger::new();
        l.set_capacity("compile", 1, Kind::Machine);
        let held = l.submit(rank(1, 1, 1), Needs::of(&[("compile", 1)]), "#0", 0);
        l.schedule(0, None, None);
        // The test step (2 steps from landing) waits from the start.
        let test = l.submit(rank(2, 1, 2), Needs::of(&[("compile", 1)]), "#1 test", 1);
        // An agent still developing (3 steps) keeps queueing runs, each later
        // than the last, one after another as each is served.
        let mut stream = vec![];
        let mut now = 2;
        for n in 0..5 {
            let r = Rank::new(false, 3, 1, now, 100 + n, false);
            stream.push(l.submit(r, Needs::of(&[("compile", 1)]), "task 9 `cargo test`", now));
            now += 1;
        }
        l.release(held);
        l.schedule(now, None, None);
        assert!(l.holds(test), "the test step is next, not one of the runs");
        assert!(stream.iter().all(|id| l.is_waiting(*id)));
        // Even a request that arrives right now does not pass it, and the
        // earlier runs are not starved by the later ones either.
        l.release(test);
        l.schedule(now + 1, None, None);
        assert!(l.holds(stream[0]));
    }

    fn disk_ledger() -> Ledger {
        let mut l = Ledger::new();
        l.set_capacity("compile", 2, Kind::Machine);
        l.set_capacity("llm:claude", 2, Kind::Llm);
        l.set_capacity("simulator", 1, Kind::Pool);
        l
    }

    fn gate(free_gb: u64) -> Option<DiskGate> {
        Some(DiskGate {
            free_mb: free_gb * 1024,
            floor_mb: 20 * 1024,
        })
    }

    #[test]
    fn below_the_disk_floor_nothing_is_granted_and_the_wait_says_why() {
        let mut l = disk_ledger();
        l.set_disk(gate(12));
        let c = l.submit(rank(1, 1, 1), Needs::of(&[("compile", 1)]), "#1", 0);
        let a = l.submit(rank(1, 2, 2), Needs::of(&[("llm:claude", 1)]), "#2", 1);
        let p = l.schedule(2, None, None);
        assert!(
            p.grants.is_empty(),
            "nothing runs below the floor, idle or not"
        );
        for id in [c, a] {
            assert_eq!(
                explain(&p.waits[&id], 2),
                "Waiting: disk has 12 GB free, floor 20 GB"
            );
        }
    }

    #[test]
    fn at_the_disk_floor_work_is_granted() {
        let mut l = disk_ledger();
        l.set_disk(gate(20));
        let c = l.submit(rank(1, 1, 1), Needs::of(&[("compile", 1)]), "#1", 0);
        let a = l.submit(rank(1, 2, 2), Needs::of(&[("llm:claude", 1)]), "#2", 1);
        l.schedule(2, None, None);
        assert!(l.holds(c) && l.holds(a));
    }

    #[test]
    fn the_disk_floor_does_not_hold_what_uses_neither_compile_nor_an_llm_session() {
        let mut l = disk_ledger();
        l.set_disk(gate(1));
        let s = l.submit(rank(1, 1, 1), Needs::of(&[("simulator", 1)]), "#1", 0);
        l.schedule(1, None, None);
        assert!(l.holds(s));
    }

    #[test]
    fn running_work_is_left_alone_when_the_disk_fills() {
        let mut l = disk_ledger();
        let c = l.submit(rank(1, 1, 1), Needs::of(&[("compile", 1)]), "#1", 0);
        l.schedule(1, None, None);
        l.set_disk(gate(3));
        let n = l.submit(rank(1, 2, 2), Needs::of(&[("compile", 1)]), "#2", 2);
        l.schedule(3, None, None);
        assert!(l.holds(c), "the holder keeps its slot");
        assert!(l.is_waiting(n));
        l.set_disk(gate(30));
        l.schedule(4, None, None);
        assert!(l.holds(n));
    }

    const MIN: u64 = 60_000;

    #[test]
    fn a_waiter_with_no_grant_for_longer_than_the_limit_is_reported() {
        let mut l = Ledger::new();
        l.set_capacity("compile", 1, Kind::Machine);
        let h = l.submit(rank(1, 1, 1), Needs::of(&[("compile", 1)]), "#1", 0);
        l.schedule(0, None, None);
        l.submit(rank(1, 2, 2), Needs::of(&[("compile", 1)]), "#2", 0);
        l.submit(rank(1, 3, 3), Needs::of(&[("compile", 1)]), "#3", 0);
        assert!(l.holds(h));
        assert!(l.stalls(59 * MIN, None, None, None, 60 * MIN).is_empty());
        let s = l.stalls(61 * MIN, None, None, None, 60 * MIN);
        assert_eq!(s.len(), 1, "one per resource, not per task");
        assert_eq!(s[0].resource, "compile");
        assert_eq!(s[0].waiting, 2);
        assert!(!s[0].never);
        assert!(s[0].text.contains("held by #1"), "{}", s[0].text);
        assert!(
            l.stalls(61 * MIN, None, None, None, 0).is_empty(),
            "0 is off"
        );
    }

    #[test]
    fn a_queue_that_is_moving_is_not_a_stall() {
        let mut l = Ledger::new();
        l.set_capacity("compile", 1, Kind::Machine);
        let a = l.submit(rank(1, 1, 1), Needs::of(&[("compile", 1)]), "#1", 0);
        l.submit(rank(1, 2, 2), Needs::of(&[("compile", 1)]), "#2", 0);
        l.schedule(0, None, None);
        l.release(a);
        // #2 has waited 90 minutes, but was granted just now.
        l.schedule(90 * MIN, None, None);
        l.submit(rank(1, 3, 3), Needs::of(&[("compile", 1)]), "#3", 90 * MIN);
        assert!(l.stalls(100 * MIN, None, None, None, 60 * MIN).is_empty());
        assert_eq!(l.stalls(151 * MIN, None, None, None, 60 * MIN).len(), 1);
    }

    #[test]
    fn a_new_arrival_is_not_long_just_because_the_last_grant_was_old() {
        let mut l = Ledger::new();
        l.set_capacity("compile", 1, Kind::Machine);
        l.submit(rank(1, 1, 1), Needs::of(&[("compile", 1)]), "#1", 0);
        l.schedule(0, None, None);
        l.submit(rank(1, 2, 2), Needs::of(&[("compile", 1)]), "#2", 200 * MIN);
        assert!(l.stalls(201 * MIN, None, None, None, 60 * MIN).is_empty());
    }

    #[test]
    fn a_request_that_can_never_fit_is_reported_at_once() {
        let mut l = Ledger::new();
        l.set_capacity("widget", 0, Kind::Machine);
        l.submit(rank(1, 1, 1), Needs::of(&[("widget", 1)]), "#1", 5);
        l.submit(rank(1, 2, 2), Needs::of(&[("gadget", 1)]), "#2", 5);
        let s = l.stalls(6, None, None, None, 60 * MIN);
        assert_eq!(s.len(), 2);
        assert!(s.iter().all(|s| s.never));
        assert!(matches!(s[0].cause, WaitCause::TooBig { .. }));
        assert!(matches!(s[1].cause, WaitCause::Unknown { .. }));
        // Not governed by the long-wait setting.
        assert_eq!(l.stalls(6, None, None, None, 0).len(), 2);
    }

    #[test]
    fn the_cpu_gate_is_a_long_wait_on_compile_and_the_disk_is_left_out() {
        let mut l = Ledger::new();
        l.set_capacity("compile", 4, Kind::Machine);
        l.submit(rank(1, 1, 1), Needs::of(&[("compile", 1)]), "#1", 0);
        let hot = Some(CpuGate {
            busy_pct: 100,
            max_pct: 50,
        });
        let s = l.stalls(61 * MIN, None, None, hot, 60 * MIN);
        assert_eq!(s.len(), 1);
        assert!(s[0].text.contains("CPU 100% > 50%"), "{}", s[0].text);
        l.set_disk(gate(1));
        assert!(l.stalls(61 * MIN, None, None, None, 60 * MIN).is_empty());
    }
}
