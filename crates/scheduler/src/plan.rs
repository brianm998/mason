// SPDX-License-Identifier: GPL-3.0-or-later

use crate::explain::{Ahead, HeldBy, Wait, WaitCause};
use crate::rank::Rank;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Sessions on a provider account. Named `llm:<account>`.
    Llm,
    /// `compile`, `test-suite`, ...: held by kernel file locks. `exclusive`
    /// (the whole machine) conflicts with every one of these.
    Machine,
    /// Simulators, phones, staging environments: leased, with heartbeats.
    Pool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capacity {
    pub units: u32,
    pub kind: Kind,
}

/// Everything a step needs. It is acquired as one set or not at all, which
/// rules out deadlock from holding one resource while waiting for another.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Needs {
    /// The whole machine: nothing else machine-class runs beside it.
    pub exclusive: bool,
    pub amounts: BTreeMap<String, u32>,
}

impl Needs {
    pub fn of(items: &[(&str, u32)]) -> Needs {
        Needs {
            exclusive: false,
            amounts: items
                .iter()
                .filter(|(_, n)| *n > 0)
                .map(|(r, n)| (r.to_string(), *n))
                .collect(),
        }
    }

    pub fn exclusive() -> Needs {
        Needs {
            exclusive: true,
            amounts: BTreeMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.exclusive && self.amounts.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub rank: Rank,
    pub needs: Needs,
    /// How the owner sees it: `#88`, or a command line.
    pub label: String,
    pub since: u64,
    /// How long it is expected to hold what it asks for, at most (s385): known for a suite's
    /// build or run from its recorded times. `None`: unknown.
    pub expected_ms: Option<u64>,
    /// A converted request (s681): an agent's `--exclusive` turned into all `compile` units. It
    /// freezes `compile` like an exclusive request freezes the machine, with the same bound
    /// (`exclusive_max_wait`) and backfill, scoped to `compile`.
    pub drains_compile: bool,
    /// The owner's self-update build (s868): the CPU, stall, syspolicyd, thermal and pacing
    /// holds do not apply to it, except the hard heat ceiling ([`World::heat_ceiling`]).
    pub hold_exempt: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holder {
    pub id: u64,
    pub needs: Needs,
    pub label: String,
    pub since: u64,
    /// When it is expected to be done, from its request's `expected_ms`. `None`: unknown.
    pub expected_ms: Option<u64>,
}

/// What the plan is made against.
pub struct World<'a> {
    /// Becomes the `since` of whatever is granted in this plan.
    pub now: u64,
    pub capacity: &'a BTreeMap<String, Capacity>,
    pub holders: &'a [Holder],
    /// Measured by the caller. With a floor, starting beside other work needs
    /// at least this much free. Load average is never consulted.
    pub free_memory_mb: Option<u64>,
    pub memory_floor_mb: Option<u64>,
    /// Measured by the caller: overall CPU use, and the level at or above which
    /// no further compile is started. `None` means no CPU gate.
    pub cpu: Option<CpuGate>,
    /// How far apart compiles start. `None` means they are not paced.
    pub compile_pace: Option<CompilePace>,
    /// Measured by the caller: free disk against its floor. `None` means no disk gate.
    pub disk: Option<DiskGate>,
    /// Measured by the caller: how slowly the build disk has been answering.
    /// `None` means no stall gate.
    pub stall: Option<StallGate>,
    /// Measured by the caller: how busy macOS's `syspolicyd` is checking new programs.
    /// `None` means no syspolicyd gate.
    pub policy: Option<PolicyGate>,
    /// Measured by the caller: how far the CPU's speed is limited for heat or power.
    /// `None` means no thermal gate.
    pub thermal: Option<ThermalGate>,
    /// A request that has waited this long (same unit as `now`) moves ahead of
    /// all other pipeline work that is not a landing, oldest first
    /// (`order.max_wait_s`). `None` means requests do not age.
    pub max_wait: Option<u64>,
    /// A waiting exclusive request that has waited this long (same unit as `now`) gives up its
    /// freeze (s552): ordinary work may start again as long as it is done before the machine
    /// has drained. `None`: the freeze has no bound.
    pub exclusive_max_wait: Option<u64>,
    /// The die is over the hard ceiling (`thermal_max_c + 2`): even a hold-exempt request waits.
    pub heat_ceiling: bool,
}

/// The build disk has stopped answering promptly. A stall shows up as *less* CPU
/// use (everything is blocked on the disk), so the CPU gate would let more
/// compiles in at the worst moment. Only a reason to wait while a compile is
/// running, so a stall that is not ours can never hold the queue for good.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StallGate {
    pub slow_ms: u64,
    pub max_ms: u64,
}

/// macOS's `syspolicyd` scans every new executable the first time it runs, signed or not, at
/// about 0.3 to 0.8 CPU s per 40 MB test binary. Each workspace build links about 180 of them,
/// and ten tasks building at once kept it at 7 cores until WindowServer's watchdog killed the
/// owner's session (2026-11-16). Like a stall, it is only a reason to wait while a compile is
/// running, so a busy syspolicyd that is not ours can never hold the queue for good.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PolicyGate {
    pub busy_pct: u32,
    pub max_pct: u32,
}

/// The CPU is held below full speed for heat or power (`CPU_Speed_Limit` under 100). On the
/// owner's iMac Pro the SMC's power-limit storms came minutes before T2 watchdog resets
/// (2026-11-18 07:29 and 12:23), with only a third of the machine busy. Like a stall, it is only a
/// reason to wait while machine work is running, so a hot machine with nothing of ours on it
/// can never hold the queue for good; work already running is never revoked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThermalGate {
    /// The CPU speed limit in percent (100: not limited).
    pub speed_pct: u32,
    /// Below this, no further machine work starts. 0 turns the gate off.
    pub min_pct: u32,
}

/// The disk floor. Below it no `compile` and no LLM session is granted, even when nothing
/// else is running. Work already running is never revoked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskGate {
    pub free_mb: u64,
    pub floor_mb: u64,
}

/// The CPU gate on the `compile` resource. Checked only when a further compile
/// would start beside one already running (the first always starts); one that
/// has started is never revoked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuGate {
    pub busy_pct: u32,
    pub max_pct: u32,
}

/// Compiles start one at a time, with a gap between them. A CPU reading says
/// nothing about a compile that has only just started, so without the gap every
/// waiter would see the same idle machine and all of them would start at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompilePace {
    pub gap_ms: u64,
    pub last_start: Option<u64>,
}

/// The resource the CPU gate applies to.
pub const COMPILE: &str = "compile";

/// The pool every GitHub Actions job a remote run uses is leased from (#700). GitHub's free plan
/// runs 20 jobs at once per account, so MASON keeps to 16 and leaves the rest to workflows it
/// does not start.
pub const GHA: &str = "gha";

/// The pool of the macOS jobs among them: GitHub allows 5 at once, MASON takes 4. A macOS run
/// holds one unit of each pool.
pub const GHA_MACOS: &str = "gha-macos";

/// The pool of the runner units debt payments hold at most (`[remote_runs] debt_units`): a debt
/// asks for it beside its `gha` units, so debts together never hold more than its size.
pub const GHA_DEBT: &str = "gha-debt";

/// Whether `n` asks for runner units and nothing else (a debt's cap pool counts as one).
pub fn is_runner_request(n: &Needs) -> bool {
    !n.exclusive
        && !n.amounts.is_empty()
        && n.amounts.keys().all(|k| is_gha_pool(k) || k == GHA_DEBT)
}

/// Whether `name` is one of the two GitHub runner pools.
pub fn is_gha_pool(name: &str) -> bool {
    name == GHA || name == GHA_MACOS
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// In the order they were granted.
    pub grants: Vec<u64>,
    pub waits: BTreeMap<u64, Wait>,
}

struct Usage {
    used: BTreeMap<String, u32>,
    exclusive: bool,
    machine_any: bool,
    any: bool,
}

impl Usage {
    fn new(capacity: &BTreeMap<String, Capacity>, holders: &[Holder]) -> Usage {
        let mut u = Usage {
            used: BTreeMap::new(),
            exclusive: false,
            machine_any: false,
            any: false,
        };
        for h in holders {
            u.add(capacity, &h.needs);
        }
        u
    }

    fn add(&mut self, capacity: &BTreeMap<String, Capacity>, n: &Needs) {
        self.any = true;
        if n.exclusive {
            self.exclusive = true;
            self.machine_any = true;
        }
        for (r, a) in &n.amounts {
            *self.used.entry(r.clone()).or_insert(0) += a;
            if kind_of(capacity, r) == Some(Kind::Machine) {
                self.machine_any = true;
            }
        }
    }
}

/// A request for the GitHub runner pools and nothing else: a remote run.
fn only_gha_pools(n: &Needs) -> bool {
    is_runner_request(n)
}

fn kind_of(capacity: &BTreeMap<String, Capacity>, r: &str) -> Option<Kind> {
    capacity.get(r).map(|c| c.kind)
}

fn uses_machine(capacity: &BTreeMap<String, Capacity>, n: &Needs) -> bool {
    n.exclusive
        || n.amounts
            .keys()
            .any(|r| kind_of(capacity, r) == Some(Kind::Machine))
}

/// What earlier, still-waiting requests need, for the passing rule.
///
/// Each waiter sets aside (reserves) the units it needs. A later request may use only what is
/// left after those, so it can never take what an earlier waiter is waiting for, and that waiter
/// is not starved. Until 2026-11-13 a waiter blocked every later request that named any of its
/// resources, however many units were free. Two waiters needing one `compile` and one
/// `test-suite` each, with every test-suite slot taken, held back 16 compile-only builds with
/// `compile` at 0 of 5 and the CPU 81% idle.
///
/// A waiter reserves only on the resources it is short of (s471): those where the holders and
/// earlier waiters leave too little for it, the `Full` or `Behind` one. Units of a resource that
/// would fit are not set aside, or N waiting for a full `test-suite` would set aside N `compile`
/// units with `compile` idle. To keep it from starving, the waiter first in line for its short
/// resources (no earlier waiter reserved any of it) also reserves its other units, so when the
/// short resource frees, the rest is there for it in the same pass. A waiter held for no resource
/// (the CPU, the disk, the machine) reserves everything it needs.
#[derive(Default)]
struct Blocked {
    reserved: BTreeMap<String, u32>,
    /// An exclusive request waits: no machine-class job may pass it.
    machine_all: bool,
    /// Some machine-class request waits: an exclusive one may not pass it.
    machine_any: bool,
    /// When the first waiting exclusive request can start at the earliest: the latest
    /// expected end of the machine-class holders when it began to wait (s385). Work sure to be
    /// done by then may pass it, since it delays nothing. `None`: unknown, nothing passes.
    /// A holder that has run past its expected end counts as ending one more expected run from
    /// now, so the end never falls into the past and stops all backfill (s552).
    drain_end: Option<u64>,
    /// A converted all-`compile` request waits (s681): compile work may pass it only when done
    /// in time, as for `machine_all`, and it reserves no `compile` units for the passing rule.
    compile_drain: bool,
    /// As `drain_end`, from the `compile` holders only.
    compile_drain_end: Option<u64>,
}

impl Blocked {
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        capacity: &BTreeMap<String, Capacity>,
        usage: &Usage,
        n: &Needs,
        holders: &[Holder],
        now: u64,
        thawed: Option<u64>,
        drains_compile: bool,
    ) {
        let short: Vec<&String> = n
            .amounts
            .iter()
            .filter(|(r, a)| {
                let reserved = self.reserved.get(*r).copied().unwrap_or(0);
                let used = usage.used.get(*r).copied().unwrap_or(0);
                let units = capacity.get(*r).map_or(0, |c| c.units);
                used + reserved + **a > units
            })
            .map(|(r, _)| r)
            .collect();
        let first = short
            .iter()
            .all(|r| self.reserved.get(*r).copied().unwrap_or(0) == 0);
        for (r, a) in &n.amounts {
            // A converted request holds compile by the drain rule, not by reserving it (s681).
            if drains_compile && r == COMPILE {
                continue;
            }
            if short.is_empty() || first || short.contains(&r) {
                *self.reserved.entry(r.clone()).or_insert(0) += a;
            }
        }
        if n.exclusive && !self.machine_all {
            self.machine_all = true;
            self.drain_end = drain_end(capacity, holders, now, thawed, false);
        }
        if drains_compile && !self.compile_drain {
            self.compile_drain = true;
            self.compile_drain_end = drain_end(capacity, holders, now, thawed, true);
        }
        if uses_machine(capacity, n) {
            self.machine_any = true;
        }
    }

    /// A waiting exclusive request holds `n` back: it uses the machine and cannot be done
    /// before the machine has drained.
    fn held_by_exclusive(
        &self,
        capacity: &BTreeMap<String, Capacity>,
        n: &Needs,
        now: u64,
        expected_ms: Option<u64>,
        drains_compile: bool,
    ) -> bool {
        let done_in_time = matches!(
            (self.drain_end, expected_ms),
            (Some(end), Some(e)) if !n.exclusive && now.saturating_add(e) <= end
        );
        (self.machine_all && !n.exclusive && uses_machine(capacity, n) && !done_in_time)
            || self.held_by_compile_drain(n, now, expected_ms, drains_compile)
    }

    /// A waiting converted request holds `n` back: it needs `compile` and cannot be done
    /// before the `compile` holders have drained (s681).
    fn held_by_compile_drain(
        &self,
        n: &Needs,
        now: u64,
        expected_ms: Option<u64>,
        drains_compile: bool,
    ) -> bool {
        let done_in_time = matches!(
            (self.compile_drain_end, expected_ms),
            (Some(end), Some(e)) if now.saturating_add(e) <= end
        );
        self.compile_drain
            && !drains_compile
            && !n.exclusive
            && n.amounts.contains_key(COMPILE)
            && !done_in_time
    }

    /// The first resource of `n` that does not fit once earlier waiters' reservations are set
    /// aside, if any (or the machine, for the exclusive rules).
    fn touches(
        &self,
        capacity: &BTreeMap<String, Capacity>,
        usage: &Usage,
        n: &Needs,
        now: u64,
        expected_ms: Option<u64>,
        drains_compile: bool,
    ) -> Option<Touch> {
        let short = n.amounts.iter().find(|(r, a)| {
            let reserved = self.reserved.get(*r).copied().unwrap_or(0);
            if reserved == 0 {
                return false;
            }
            let used = usage.used.get(*r).copied().unwrap_or(0);
            let units = capacity.get(*r).map_or(0, |c| c.units);
            used + reserved + **a > units
        });
        if let Some((r, _)) = short {
            return Some(Touch::Name(r.clone()));
        }
        // Backfill (s385): work that will be done before the machine has drained for the
        // waiting exclusive request delays nothing, so it may run in the meantime.
        let done_in_time = matches!(
            (self.drain_end, expected_ms),
            (Some(end), Some(e)) if !n.exclusive && now.saturating_add(e) <= end
        );
        if (self.machine_all && uses_machine(capacity, n) && !done_in_time)
            || (n.exclusive && self.machine_any)
        {
            return Some(Touch::Machine);
        }
        if self.held_by_compile_drain(n, now, expected_ms, drains_compile) {
            return Some(Touch::Name(COMPILE.to_string()));
        }
        None
    }
}

/// When the machine-class holders are expected to be done, the latest of them: `None` when
/// any of them has no estimate, since then the drain's end is unknown (s385).
///
/// A holder past its expected end is not done "in the past": it is taken to need one more
/// expected run from `now` (s552). Before that, `drain_end` slid behind `now` once the holders
/// overran, which is the usual case for test runs of 20 to 90 min, and nothing could ever pass
/// the waiting exclusive request again. With `unknown_ms` (the request has given up its
/// freeze), a holder with no estimate is taken to end that long from `now`.
fn drain_end(
    capacity: &BTreeMap<String, Capacity>,
    holders: &[Holder],
    now: u64,
    unknown_ms: Option<u64>,
    compile_only: bool,
) -> Option<u64> {
    let mut end = 0u64;
    let counts = |h: &Holder| {
        if compile_only {
            h.needs.amounts.contains_key(COMPILE)
        } else {
            uses_machine(capacity, &h.needs)
        }
    };
    for h in holders.iter().filter(|h| counts(h)) {
        let e = match h.expected_ms {
            Some(e) => {
                let due = h.since.saturating_add(e);
                if due < now {
                    now.saturating_add(e)
                } else {
                    due
                }
            }
            None => now.saturating_add(unknown_ms?),
        };
        end = end.max(e);
    }
    Some(end)
}

enum Touch {
    Name(String),
    Machine,
}

enum Fit {
    Yes,
    Never(WaitCause),
    Full(WaitCause),
}

fn fit(capacity: &BTreeMap<String, Capacity>, usage: &Usage, n: &Needs) -> Fit {
    for (r, a) in &n.amounts {
        match capacity.get(r) {
            None => {
                return Fit::Never(WaitCause::Unknown {
                    resource: r.clone(),
                })
            }
            Some(c) if *a > c.units => {
                return Fit::Never(WaitCause::TooBig {
                    resource: r.clone(),
                    need: *a,
                    limit: c.units,
                })
            }
            _ => {}
        }
    }
    for (r, a) in &n.amounts {
        let used = usage.used.get(r).copied().unwrap_or(0);
        if used + a > capacity[r].units {
            return Fit::Full(WaitCause::Full {
                resource: r.clone(),
            });
        }
    }
    if n.exclusive && usage.machine_any {
        return Fit::Full(WaitCause::Exclusive);
    }
    if !n.exclusive && usage.exclusive && uses_machine(capacity, n) {
        return Fit::Full(WaitCause::Exclusive);
    }
    Fit::Yes
}

/// Decide who is granted now. Requests are taken in rank order. One is granted
/// when its whole set fits in what is left after the units earlier waiters need
/// are set aside; that is the passing rule, "a compile can pass a waiting test
/// suite, and may use a free compile slot a waiter does not need, but nothing
/// passes a waiting exclusive job". Whoever is not granted gets a [`Wait`].
pub fn plan(world: &World, waiting: &[Request]) -> Plan {
    let capacity = world.capacity;
    // A request that has waited long enough is ranked as aged. Everything
    // else, capacity, gates and the passing rule, is as for a younger one.
    // A request for runner units only is ordered by kind, urgency and wait (2026-11-21).
    let rank_of = |r: &Request| {
        let aged = matches!(world.max_wait, Some(m) if world.now.saturating_sub(r.since) >= m);
        if is_runner_request(&r.needs) {
            r.rank.for_runners(r.since, aged)
        } else if aged {
            r.rank.aged(r.since)
        } else {
            r.rank
        }
    };
    let mut order: Vec<&Request> = waiting.iter().collect();
    order.sort_by_key(|r| (rank_of(r), r.id));
    let mut usage = Usage::new(capacity, world.holders);
    let mut blocked = Blocked::default();
    let mut out = Plan::default();
    let mut ahead: Vec<(&Request, WaitCause)> = vec![];
    // Holders as the plan goes: those granted earlier in it count as held.
    let mut holders: Vec<Holder> = world.holders.to_vec();
    let mut last_compile_start = world.compile_pace.and_then(|p| p.last_start);

    for r in order {
        let compiles = r.needs.amounts.contains_key(COMPILE);
        let touch = blocked.touches(
            capacity,
            &usage,
            &r.needs,
            world.now,
            r.expected_ms,
            r.drains_compile,
        );
        let fitted = fit(capacity, &usage, &r.needs);
        let low_memory = match (world.free_memory_mb, world.memory_floor_mb) {
            // A remote run uses none of this machine, so memory here does not hold it (#700).
            (Some(free), Some(floor)) if usage.any && free < floor && !only_gha_pools(&r.needs) => {
                Some((free, floor))
            }
            _ => None,
        };
        let compile_running = usage.used.get(COMPILE).is_some_and(|n| *n > 0);
        // Only a further compile waits for the CPU or the pacing: the first always
        // starts. CPU use is the whole machine's, so other work (another pipeline's builds, an
        // editor indexing) would otherwise hold every compile back with none of
        // ours running, which is the queue waiting on an idle `compile`.
        // A landing is not held by the CPU or the pacing either: its verification
        // unblocks every task in the batch. The disk, memory and the count still hold it.
        let gated = compiles && !r.rank.is_landing();
        let exempt = r.hold_exempt;
        let hot = match world.cpu {
            Some(g) if !exempt && gated && compile_running && g.busy_pct >= g.max_pct => Some(g),
            _ => None,
        };
        let stalled = match world.stall {
            Some(g) if !exempt && compiles && compile_running && g.slow_ms >= g.max_ms => Some(g),
            _ => None,
        };
        let checking = match world.policy {
            Some(g)
                if !exempt
                    && compiles
                    && compile_running
                    && g.max_pct > 0
                    && g.busy_pct >= g.max_pct =>
            {
                Some(g)
            }
            _ => None,
        };
        let throttled = match world.thermal {
            _ if exempt => world.heat_ceiling.then(|| {
                world.thermal.unwrap_or(ThermalGate {
                    speed_pct: 0,
                    min_pct: 1,
                })
            }),
            Some(g)
                if g.min_pct > 0
                    && g.speed_pct < g.min_pct
                    && usage.machine_any
                    && uses_machine(capacity, &r.needs) =>
            {
                Some(g)
            }
            _ => None,
        };
        // The pacing lets a started compile show in the CPU reading; with none running
        // there is nothing to wait for.
        let settling = world
            .compile_pace
            .filter(|_| gated && compile_running && !exempt)
            .and_then(|p| Some(last_compile_start? + p.gap_ms))
            .filter(|until| world.now < *until);
        let low_disk = match world.disk {
            Some(g)
                if g.free_mb < g.floor_mb
                    && r.needs
                        .amounts
                        .keys()
                        .any(|n| n == COMPILE || kind_of(capacity, n) == Some(Kind::Llm)) =>
            {
                Some(g)
            }
            _ => None,
        };
        let cause = match (&fitted, &touch, low_memory) {
            (Fit::Never(c), _, _) | (Fit::Full(c), _, _) => Some(c.clone()),
            // Every request the disk gate holds says so, not only the first in line: the
            // ones behind it are held by the disk too. Who is granted does not change.
            (Fit::Yes, _, _) if low_disk.is_some() => {
                let g = low_disk.expect("checked");
                Some(WaitCause::Disk {
                    free_mb: g.free_mb,
                    floor_mb: g.floor_mb,
                })
            }
            (Fit::Yes, Some(Touch::Name(n)), _) => Some(WaitCause::Behind {
                resource: n.clone(),
            }),
            (Fit::Yes, Some(Touch::Machine), _) => Some(WaitCause::Exclusive),
            (Fit::Yes, None, _) if hot.is_some() => {
                let g = hot.expect("checked");
                Some(WaitCause::Cpu {
                    busy_pct: g.busy_pct,
                    max_pct: g.max_pct,
                })
            }
            (Fit::Yes, None, _) if stalled.is_some() => {
                let g = stalled.expect("checked");
                Some(WaitCause::DiskStall {
                    slow_ms: g.slow_ms,
                    max_ms: g.max_ms,
                })
            }
            (Fit::Yes, None, _) if checking.is_some() => {
                let g = checking.expect("checked");
                Some(WaitCause::Policy {
                    busy_pct: g.busy_pct,
                    max_pct: g.max_pct,
                })
            }
            (Fit::Yes, None, _) if throttled.is_some() => {
                let g = throttled.expect("checked");
                Some(WaitCause::Thermal {
                    speed_pct: g.speed_pct,
                    min_pct: g.min_pct,
                })
            }
            (Fit::Yes, None, _) if settling.is_some() => Some(WaitCause::Settling {
                until: settling.expect("checked"),
            }),
            (Fit::Yes, None, Some((free_mb, floor_mb))) => {
                Some(WaitCause::Memory { free_mb, floor_mb })
            }
            (Fit::Yes, None, None) => None,
        };
        match cause {
            None => {
                usage.add(capacity, &r.needs);
                holders.push(Holder {
                    id: r.id,
                    needs: r.needs.clone(),
                    label: r.label.clone(),
                    since: world.now,
                    expected_ms: r.expected_ms,
                });
                out.grants.push(r.id);
                if compiles {
                    last_compile_start = Some(world.now);
                }
            }
            Some(cause) => {
                // Held by the passing rule, not by a full resource, a gate or its size: it has
                // a request it may not pass. A waiting exclusive request is the root of it
                // even for a compile that is also behind other compiles.
                let touched = matches!(fitted, Fit::Yes)
                    && touch.is_some()
                    && matches!(cause, WaitCause::Behind { .. } | WaitCause::Exclusive);
                let by_exclusive = touched
                    && blocked.held_by_exclusive(
                        capacity,
                        &r.needs,
                        world.now,
                        r.expected_ms,
                        r.drains_compile,
                    );
                let mut wait = build_wait(
                    capacity,
                    &holders,
                    &ahead,
                    cause.clone(),
                    touched,
                    by_exclusive,
                );
                if rank_of(r).is_aged() && !matches!(fitted, Fit::Never(_)) {
                    wait.aged_ms = Some(world.now.saturating_sub(r.since));
                }
                // An exclusive request past its bound gives up the freeze and says so.
                let thaw = match world.exclusive_max_wait {
                    Some(m)
                        if (r.needs.exclusive || r.drains_compile)
                            && !matches!(fitted, Fit::Never(_)) =>
                    {
                        let waited = world.now.saturating_sub(r.since);
                        (waited >= m).then_some(m)
                    }
                    _ => None,
                };
                let first_of_kind = if r.needs.exclusive {
                    !blocked.machine_all
                } else {
                    !blocked.compile_drain
                };
                if thaw.is_some() && first_of_kind {
                    wait.thawed_ms = Some(world.now.saturating_sub(r.since));
                    wait.thawed_compile = !r.needs.exclusive;
                }
                out.waits.insert(r.id, wait);
                // A request that can never fit must not hold up the others.
                if !matches!(fitted, Fit::Never(_)) {
                    blocked.add(
                        capacity,
                        &usage,
                        &r.needs,
                        &holders,
                        world.now,
                        thaw,
                        r.drains_compile,
                    );
                    ahead.push((r, cause));
                }
            }
        }
    }
    out
}

fn build_wait(
    capacity: &BTreeMap<String, Capacity>,
    holders: &[Holder],
    ahead: &[(&Request, WaitCause)],
    cause: WaitCause,
    touched: bool,
    by_exclusive: bool,
) -> Wait {
    // The requests ahead that need what this one is stuck on.
    let same = |other: &Request| match &cause {
        WaitCause::Full { resource } | WaitCause::Behind { resource } => {
            other.needs.amounts.contains_key(resource)
        }
        WaitCause::Exclusive => uses_machine(capacity, &other.needs),
        WaitCause::Memory { .. }
        | WaitCause::Cpu { .. }
        | WaitCause::Settling { .. }
        | WaitCause::DiskStall { .. }
        | WaitCause::Policy { .. }
        | WaitCause::Thermal { .. }
        | WaitCause::Disk { .. } => true,
        _ => false,
    };
    let ahead_of_me: Vec<&(&Request, WaitCause)> = ahead.iter().filter(|o| same(o.0)).collect();
    let held = |h: &Holder| match &cause {
        WaitCause::Full { resource } | WaitCause::Behind { resource } => {
            h.needs.amounts.contains_key(resource)
        }
        WaitCause::Exclusive => uses_machine(capacity, &h.needs),
        _ => false,
    };
    let held_by: Vec<HeldBy> = holders
        .iter()
        .filter(|h| held(h))
        .map(|h| HeldBy {
            id: h.id,
            label: h.label.clone(),
            since: h.since,
        })
        .collect();
    let to_ahead_ref = |(o, c): &(&Request, WaitCause)| Ahead {
        id: o.id,
        exclusive: o.needs.exclusive,
        label: o.label.clone(),
        steps: (!o.rank.interactive()).then(|| o.rank.steps()),
        cause: Some(c.clone()),
    };
    let to_ahead = |(o, c): &&(&Request, WaitCause)| Ahead {
        id: o.id,
        exclusive: o.needs.exclusive,
        label: o.label.clone(),
        steps: (!o.rank.interactive()).then(|| o.rank.steps()),
        cause: Some(c.clone()),
    };
    // Held back by the passing rule: the first waiter ahead that it may not pass. A request
    // that is not itself exclusive is held by a waiting exclusive one, not by any machine job.
    let blocker = if by_exclusive {
        // The waiting exclusive request, wherever it is in line.
        ahead
            .iter()
            .find(|o| o.0.needs.exclusive)
            .or_else(|| ahead.iter().find(|o| o.0.drains_compile))
            .map(&to_ahead_ref)
    } else if touched {
        ahead_of_me.first().copied().map(&to_ahead_ref)
    } else {
        None
    };
    let pool = match &cause {
        WaitCause::Full { resource } | WaitCause::Behind { resource }
            if is_gha_pool(resource) || resource == GHA_DEBT =>
        {
            capacity.get(resource).map(|c| {
                let used: u32 = holders
                    .iter()
                    .filter_map(|h| h.needs.amounts.get(resource))
                    .sum();
                (used, c.units)
            })
        }
        _ => None,
    };
    let mut runs_ahead = [0u32; 4];
    if matches!(&cause, WaitCause::Full { resource } | WaitCause::Behind { resource }
        if is_gha_pool(resource) || resource == GHA_DEBT)
    {
        for (o, _) in &ahead_of_me {
            if is_runner_request(&o.needs) {
                runs_ahead[o.rank.runner_kind() as usize] += 1;
            }
        }
    }
    Wait {
        pool,
        runs_ahead,
        blocker,
        kind: match &cause {
            WaitCause::Full { resource }
            | WaitCause::Behind { resource }
            | WaitCause::TooBig { resource, .. }
            | WaitCause::Unknown { resource } => kind_of(capacity, resource),
            _ => None,
        },
        cause,
        position: ahead_of_me.len() + 1,
        behind: ahead_of_me.iter().map(to_ahead).collect(),
        held_by,
        aged_ms: None,
        thawed_ms: None,
        thawed_compile: false,
    }
}
