// SPDX-License-Identifier: GPL-3.0-or-later

use crate::explain::{Ahead, HeldBy, Wait, WaitCause};
use crate::rank::Rank;
use std::collections::{BTreeMap, BTreeSet};

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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holder {
    pub id: u64,
    pub needs: Needs,
    pub label: String,
    pub since: u64,
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
#[derive(Default)]
struct Blocked {
    names: BTreeSet<String>,
    /// An exclusive request waits: no machine-class job may pass it.
    machine_all: bool,
    /// Some machine-class request waits: an exclusive one may not pass it.
    machine_any: bool,
}

impl Blocked {
    fn add(&mut self, capacity: &BTreeMap<String, Capacity>, n: &Needs) {
        self.names.extend(n.amounts.keys().cloned());
        if n.exclusive {
            self.machine_all = true;
        }
        if uses_machine(capacity, n) {
            self.machine_any = true;
        }
    }

    /// The first resource of `n` an earlier waiter also needs, if any.
    fn touches(&self, capacity: &BTreeMap<String, Capacity>, n: &Needs) -> Option<Touch> {
        if let Some(r) = n.amounts.keys().find(|r| self.names.contains(*r)) {
            return Some(Touch::Name(r.clone()));
        }
        if (self.machine_all && uses_machine(capacity, n)) || (n.exclusive && self.machine_any) {
            return Some(Touch::Machine);
        }
        None
    }
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
/// when its whole set fits and it uses nothing an earlier waiter needs; that is
/// the passing rule, "a compile can pass a waiting test suite, and nothing
/// passes a waiting exclusive job". Whoever is not granted gets a [`Wait`].
pub fn plan(world: &World, waiting: &[Request]) -> Plan {
    let capacity = world.capacity;
    let mut order: Vec<&Request> = waiting.iter().collect();
    order.sort_by_key(|r| (r.rank, r.id));
    let mut usage = Usage::new(capacity, world.holders);
    let mut blocked = Blocked::default();
    let mut out = Plan::default();
    let mut ahead: Vec<&Request> = vec![];
    // Holders as the plan goes: those granted earlier in it count as held.
    let mut holders: Vec<Holder> = world.holders.to_vec();
    let mut last_compile_start = world.compile_pace.and_then(|p| p.last_start);

    for r in order {
        let compiles = r.needs.amounts.contains_key(COMPILE);
        let touch = blocked.touches(capacity, &r.needs);
        let fitted = fit(capacity, &usage, &r.needs);
        let low_memory = match (world.free_memory_mb, world.memory_floor_mb) {
            (Some(free), Some(floor)) if usage.any && free < floor => Some((free, floor)),
            _ => None,
        };
        let compile_running = usage.used.get(COMPILE).is_some_and(|n| *n > 0);
        // Only a further compile waits for the CPU: the first always starts. CPU use
        // is the whole machine's, so other work (another pipeline's builds, an
        // editor indexing) would otherwise hold every compile back with none of
        // ours running, which is the queue waiting on an idle `compile`.
        let hot = match world.cpu {
            Some(g) if compiles && compile_running && g.busy_pct >= g.max_pct => Some(g),
            _ => None,
        };
        let stalled = match world.stall {
            Some(g) if compiles && compile_running && g.slow_ms >= g.max_ms => Some(g),
            _ => None,
        };
        let settling = world
            .compile_pace
            .filter(|_| compiles)
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
                });
                out.grants.push(r.id);
                if compiles {
                    last_compile_start = Some(world.now);
                }
            }
            Some(cause) => {
                let wait = build_wait(capacity, &holders, &ahead, cause.clone());
                out.waits.insert(r.id, wait);
                // A request that can never fit must not hold up the others.
                if !matches!(fitted, Fit::Never(_)) {
                    blocked.add(capacity, &r.needs);
                    ahead.push(r);
                }
            }
        }
    }
    out
}

fn build_wait(
    capacity: &BTreeMap<String, Capacity>,
    holders: &[Holder],
    ahead: &[&Request],
    cause: WaitCause,
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
        | WaitCause::Disk { .. } => true,
        _ => false,
    };
    let ahead_of_me: Vec<&&Request> = ahead.iter().filter(|o| same(o)).collect();
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
            label: h.label.clone(),
            since: h.since,
        })
        .collect();
    Wait {
        kind: match &cause {
            WaitCause::Full { resource }
            | WaitCause::Behind { resource }
            | WaitCause::TooBig { resource, .. }
            | WaitCause::Unknown { resource } => kind_of(capacity, resource),
            _ => None,
        },
        cause,
        position: ahead_of_me.len() + 1,
        behind: ahead_of_me
            .iter()
            .map(|o| Ahead {
                label: o.label.clone(),
                steps: (!o.rank.interactive()).then(|| o.rank.steps()),
            })
            .collect(),
        held_by,
    }
}
