// SPDX-License-Identifier: GPL-3.0-or-later

//! The pipeline's health, judged from facts (s432, decision 0425).
//!
//! The daemon gathers the facts (task ages, slot waits, load, failures...); this module turns
//! them into findings, each with a stable key, a severity and the evidence behind it. A pure
//! function: no clock, no I/O. Time arrives in the facts. A healthy pipeline gives no findings.

use std::collections::BTreeMap;

use crate::{Event, EventKind, TaskId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Worth seeing, never worth a session.
    Note,
    Warn,
    Severe,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Severity::Note => "note",
            Severity::Warn => "warn",
            Severity::Severe => "severe",
        }
    }
}

/// One thing that looks wrong. `key` is the same on every check while the thing stays wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub key: String,
    /// `stuck`, `overloaded`, `failures` or `daemon`.
    pub category: &'static str,
    pub severity: Severity,
    pub summary: String,
    pub evidence: Vec<String>,
}

/// The thresholds (`[health]` in the daemon config). Times in milliseconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// A step is stuck past this many times its usual longest time...
    pub stuck_factor: u64,
    /// ...and at least this long with no progress.
    pub stuck_min_ms: u64,
    /// A task waiting on a slot or a hold for longer than this.
    pub wait_ms: u64,
    /// A wait this old while the holders progress is noted (never a session).
    pub aged_wait_ms: u64,
    /// A queue with work and free slots that started nothing for this long.
    pub queue_idle_ms: u64,
    /// A step host alive but silent for this long.
    pub host_silent_ms: u64,
    /// A slot wait that long is overload.
    pub slot_wait_ms: u64,
    pub load_pct: u32,
    pub stall_ms: u64,
    pub frozen_launch_ms: u64,
    pub mem_free_mb: u64,
    pub cpu_gate_ms: u64,
    /// A failed task without a diagnosis for this long.
    pub undiagnosed_ms: u64,
    /// A pending diagnosis older than this.
    pub pending_diagnosis_ms: u64,
    /// The same failure on this many tasks.
    pub same_failure: usize,
    /// Landing or the base red for this long.
    pub red_ms: u64,
    pub update_failures: u32,
    pub restarts: u32,
    pub late_loops: u32,
    /// Landing counts as unable to land when it failed for this many tasks with none landed.
    pub cannot_land: u32,
    /// A finding handled within this long is not raised again.
    pub cooldown_ms: u64,
    /// This many compiles in uninterruptible wait with next to no CPU...
    pub blocked_compiles: u32,
    /// ...for at least this long.
    pub blocked_compile_ms: u64,
    /// A task in one state longer than this many times its usual longest time (`stuck:long`)...
    pub long_factor: u64,
    /// ...and at least this long in develop, review and test...
    pub long_min_ms: u64,
    /// ...or this long in landing.
    pub long_landing_min_ms: u64,
    /// A develop run with more than this many full `run_check` rounds and no submission.
    pub checks_max: u32,
}

const MIN: u64 = 60_000;
const H: u64 = 60 * MIN;

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            stuck_factor: 3,
            stuck_min_ms: 30 * MIN,
            wait_ms: 2 * H,
            aged_wait_ms: 6 * H,
            queue_idle_ms: 15 * MIN,
            host_silent_ms: 30 * MIN,
            slot_wait_ms: 30 * MIN,
            load_pct: 95,
            stall_ms: 5_000,
            frozen_launch_ms: 2 * MIN,
            mem_free_mb: 512,
            cpu_gate_ms: 30 * MIN,
            undiagnosed_ms: 30 * MIN,
            pending_diagnosis_ms: 6 * H,
            same_failure: 3,
            red_ms: 3 * H,
            update_failures: 3,
            restarts: 3,
            late_loops: 3,
            cannot_land: 3,
            cooldown_ms: 6 * H,
            blocked_compiles: 4,
            blocked_compile_ms: 10 * MIN,
            long_factor: 6,
            long_min_ms: 6 * H,
            long_landing_min_ms: 2 * H,
            checks_max: 6,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskFact {
    pub id: u64,
    pub state: String,
    /// The state is one an agent or a suite works in.
    pub is_step: bool,
    pub in_state_ms: u64,
    /// The longest time this step usually takes, when there is history for it.
    pub typical_hi_ms: Option<u64>,
    /// Since the last progress event, or since the state was entered.
    pub silent_ms: u64,
    /// What it waits for (a slot, a hold) and for how long.
    pub waiting: Option<(String, u64)>,
    /// The holders of the slot it waits for; `None` when the wait has none (a gate, a pause).
    pub holders: Option<HolderFact>,
    /// Waits for a GitHub runner pool unit: no progress is expected, overloaded:slots and
    /// stuck:wait judge it.
    pub pool_wait: bool,
    /// Of `in_state_ms`, the time spent waiting for rationed capacity ([`capacity_wait_ms`]):
    /// `stuck:long` does not count it.
    pub capacity_wait_ms: u64,
}

/// How the text of a wait for a node starts when no unit of a node is in the broker's line (that
/// wait is recorded under the `environment` class).
pub const NODE_WAIT_PREFIX: &str = "Waiting for a node";

/// Whether a recorded wait was for capacity the pipeline rations: a GitHub runner unit, a node,
/// or a compile, test-suite or exclusive slot. Every other wait (the owner, a pause, the
/// environment) is not.
pub fn is_capacity_wait(on: &str, recorded: Option<&str>) -> bool {
    let class = crate::wait_class(on, recorded);
    matches!(
        class.as_str(),
        "gha" | "gha-macos" | "gha-debt" | "compile" | "test-suite" | "exclusive"
    ) || class.starts_with("node:")
        || (class == "environment" && on.starts_with(NODE_WAIT_PREFIX))
}

/// The time `task` spent in capacity waits within `[from, now)`, from the log's wait events
/// (the ledger `wait_by_class` totals), oldest first. A wait ends at its `WaitEnded`, at the next
/// wait of the task, or when a new run of the step starts (`run_starts`: a retry drops the wait
/// without an event of its own); one still open counts up to `now`. `from` is when the task
/// entered its state: no wait outlives a change of state.
pub fn capacity_wait_ms(
    events: &[Event],
    task: TaskId,
    run_starts: &[u64],
    from: u64,
    now: u64,
) -> u64 {
    let mut total = 0u64;
    // The start of the open wait, when it is a capacity wait.
    let mut open: Option<u64> = None;
    let close = |since: u64, end: u64| {
        let end = run_starts
            .iter()
            .copied()
            .filter(|s| *s > since)
            .fold(end.min(now), u64::min);
        end.saturating_sub(since.max(from))
    };
    for e in events {
        let (t, next) = match &e.kind {
            EventKind::WaitStarted { task, on, class } => (
                *task,
                is_capacity_wait(on, class.as_deref()).then_some(e.at.0),
            ),
            EventKind::WaitEnded { task } => (*task, None),
            _ => continue,
        };
        if t != task || e.at.0 < from {
            continue;
        }
        if let Some(since) = open.take() {
            total += close(since, e.at.0);
        }
        open = next;
    }
    if let Some(since) = open {
        total += close(since, now);
    }
    total
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HolderFact {
    /// Age of the newest holder grant.
    pub last_grant_ms: u64,
    /// The longest current holder run.
    pub run_ms: u64,
    /// Waiting tasks in the same class.
    pub queue_depth: u32,
    /// The waiter's place in that queue (1 is next in line).
    pub position: u32,
    /// A holder is a runner-pool check lease: its run length is one job, not the check.
    pub check: bool,
}

/// Holder facts from the grant times (`since`) of a slot's holders; `None` without holders.
pub fn holder_fact(
    held_since: &[u64],
    now: u64,
    queue_depth: u32,
    position: u32,
    check: bool,
) -> Option<HolderFact> {
    let newest = held_since.iter().max()?;
    let oldest = held_since.iter().min()?;
    Some(HolderFact {
        last_grant_ms: now.saturating_sub(*newest),
        run_ms: now.saturating_sub(*oldest),
        queue_depth,
        position,
        check,
    })
}

/// What a task's current develop run has done: for `stuck:long`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoundFact {
    pub task: u64,
    /// Full `run_check` rounds recorded in the current run.
    pub rounds: u32,
    /// The step run's attempt number.
    pub attempt: u32,
    /// The last few rounds, newest last: `green` or `red (n failing)`.
    pub last_checks: Vec<String>,
    /// One line per open remote run of the task (kind, runs_on, waiting, poll age).
    pub remote: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FailedFact {
    pub task: u64,
    /// A short signature of the failure; the same signature is the same failure.
    pub signature: String,
    pub failed_ms: u64,
    /// A diagnosis is expected for it (class `code`, the project asks for them) and none exists.
    pub undiagnosed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub tasks: Vec<TaskFact>,
    /// Check rounds of tasks in a step (for `stuck:long`).
    pub rounds: Vec<RoundFact>,
    pub queued: u32,
    pub queued_oldest_ms: u64,
    pub running: u32,
    pub free_slots: bool,
    pub paused: bool,
    /// Step hosts that are alive but silent: (task, silent ms).
    pub silent_hosts: Vec<(u64, u64)>,
    /// Slot waits: (task, slot, ms).
    pub slot_waits: Vec<(u64, String, u64)>,
    pub load_pct: Option<u32>,
    /// `load_pct` is the sampler's assumed-busy fallback, not a measurement.
    pub load_assumed: bool,
    /// Where `load_pct` was read from (`mach`, `loadavg`, `assumed-busy`, ...).
    pub load_source: Option<&'static str>,
    pub stall_ms: u64,
    /// Launches that have not finished: (task, ms).
    pub frozen_launches: Vec<(Option<u64>, u64)>,
    pub syspolicyd_down: bool,
    pub mem_free_mb: Option<u64>,
    /// The most time one task has spent under a CPU cause (gate or settle) in its current wait.
    pub cpu_gate_held_ms: u64,
    pub failed: Vec<FailedFact>,
    /// Pending diagnoses: (diagnosis id, task, age ms).
    pub pending_diagnoses: Vec<(u64, u64, u64)>,
    /// A red landing or base: (what, ms).
    pub red: Vec<(String, u64)>,
    pub update_failures: u32,
    pub restarts: u32,
    pub late_loops: u32,
    /// Tasks whose landing failed lately and tasks that landed lately (MASON's own project).
    pub landing_failed: u32,
    pub landed: u32,
    pub launch_blocked: bool,
    pub update_blocked: bool,
    /// Processes in uninterruptible wait (see [`parse_ps`]), for the stalled-compile finding.
    pub procs: Vec<ProcFact>,
}

/// One process from `ps -axo pid=,stat=,etime=,time=,command=`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcFact {
    pub pid: u32,
    /// The state letters (`U`, `S`, `R+`...).
    pub state: String,
    pub elapsed_s: u64,
    pub cpu_s: u64,
    pub command: String,
}

/// `[[dd-]hh:]mm:ss` (ps's `etime` and `time`) in seconds.
fn clock_s(t: &str) -> Option<u64> {
    let (days, rest) = match t.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, t),
    };
    let mut s = 0u64;
    for part in rest.split(':') {
        // `time` can carry a fraction: `0:01.50`.
        let n: f64 = part.parse().ok()?;
        s = s * 60 + n as u64;
    }
    Some(days * 86_400 + s)
}

/// Parse the output of `ps -axo pid=,stat=,etime=,time=,command=`; lines that do not fit are skipped.
pub fn parse_ps(text: &str) -> Vec<ProcFact> {
    text.lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let state = it.next()?.to_string();
            let elapsed_s = clock_s(it.next()?)?;
            let cpu_s = clock_s(it.next()?)?;
            let command = it.collect::<Vec<_>>().join(" ");
            Some(ProcFact {
                pid,
                state,
                elapsed_s,
                cpu_s,
                command,
            })
        })
        .collect()
}

/// The directory a rustc command searches (`-L dependency=<dir>`).
pub fn dependency_dir(command: &str) -> Option<&str> {
    let mut it = command.split_whitespace();
    while let Some(w) = it.next() {
        if w == "-L" {
            if let Some(d) = it.next().and_then(|a| a.strip_prefix("dependency=")) {
                return Some(d);
            }
        } else if let Some(d) = w.strip_prefix("-Ldependency=") {
            return Some(d);
        }
    }
    None
}

/// Compiles stuck in uninterruptible wait with next to no CPU, grouped by the directory their
/// `-L dependency=` names: (directory, count, longest elapsed seconds). A rustc counts when its
/// state has `U`, it has run at least `min_s`, and it used under 5% of that in CPU.
pub fn blocked_compiles(procs: &[ProcFact], min_s: u64) -> Vec<(String, u32, u64)> {
    let mut by: std::collections::BTreeMap<String, (u32, u64)> = Default::default();
    for p in procs {
        if !p.state.contains('U') || p.elapsed_s < min_s || p.cpu_s * 20 > p.elapsed_s {
            continue;
        }
        let first = p.command.split_whitespace().next();
        if first.is_none_or(|c| c.rsplit('/').next() != Some("rustc")) {
            continue;
        }
        let dir = dependency_dir(&p.command).unwrap_or("(no -L dependency=)");
        let e = by.entry(dir.to_string()).or_default();
        e.0 += 1;
        e.1 = e.1.max(p.elapsed_s);
    }
    by.into_iter().map(|(d, (n, t))| (d, n, t)).collect()
}

fn f(
    key: String,
    category: &'static str,
    severity: Severity,
    summary: String,
    evidence: Vec<String>,
) -> Finding {
    Finding {
        key,
        category,
        severity,
        summary,
        evidence,
    }
}

/// A wait reason that names the CPU gate.
pub fn is_cpu_cause(reason: &str) -> bool {
    reason.to_lowercase().contains("cpu")
}

fn mins(ms: u64) -> u64 {
    ms / MIN
}

fn list(ids: &[u64]) -> String {
    ids.iter()
        .map(|i| format!("#{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Facts to findings, sorted by key. Healthy facts give none.
/// Starts that were not a planned takeover. `events` are the `start`, `planned` and `cancelled`
/// health rows in order; a `start` right after `planned` is the successor of a takeover.
pub fn unplanned_starts(events: &[&str]) -> u32 {
    let mut planned = false;
    let mut n = 0;
    for e in events {
        match *e {
            "planned" => planned = true,
            "cancelled" => planned = false,
            "start" => {
                if planned {
                    planned = false;
                } else {
                    n += 1;
                }
            }
            _ => {}
        }
    }
    n
}

pub fn judge(x: &Facts, l: &Limits) -> Vec<Finding> {
    let mut out = vec![];
    // stuck
    for t in &x.tasks {
        if let Some((what, ms)) = &t.waiting {
            if *ms > l.wait_ms {
                // The work ahead of this task, and the whole queue (for the aged-wait note).
                // A check holder's run is one job of a full-CI check that runs for hours, so it
                // gives no bound: only a quiet queue or the aged-wait note applies.
                let bound = t
                    .holders
                    .as_ref()
                    .filter(|h| !h.check)
                    .map(|h| (h.position as u64).saturating_mul(h.run_ms).max(l.wait_ms));
                let queue_bound = t.holders.as_ref().filter(|h| !h.check).map(|h| {
                    (h.queue_depth as u64)
                        .saturating_mul(h.run_ms)
                        .max(l.wait_ms)
                });
                let quiet = t
                    .holders
                    .as_ref()
                    .is_none_or(|h| h.last_grant_ms >= l.wait_ms);
                if quiet || bound.is_some_and(|b| *ms > b) {
                    let mut ev = vec![format!("#{} is {}; the wait is `{what}`", t.id, t.state)];
                    if let Some(h) = &t.holders {
                        let bound_text = match bound {
                            Some(b) => format!("{} min", mins(b)),
                            None => "none: a full-CI check holds the pool".to_string(),
                        };
                        ev.push(format!(
                            "newest holder grant {} min ago, longest holder run {} min, position {}, bound {bound_text}",
                            mins(h.last_grant_ms),
                            mins(h.run_ms),
                            h.position,
                        ));
                    }
                    out.push(f(
                        format!("stuck:wait:{}", t.id),
                        "stuck",
                        Severity::Warn,
                        format!("#{} has waited {} min for {what}", t.id, mins(*ms)),
                        ev,
                    ));
                } else if *ms > l.aged_wait_ms {
                    out.push(f(
                        format!("stuck:aged-wait:{}", t.id),
                        "stuck",
                        Severity::Note,
                        format!(
                            "#{} has waited {} min for {what}; holders are progressing",
                            t.id,
                            mins(*ms)
                        ),
                        {
                            let mut ev =
                                vec![format!("#{} is {}; the wait is `{what}`", t.id, t.state)];
                            if let Some(b) = queue_bound {
                                ev.push(format!("whole-queue bound {} min", mins(b)));
                            }
                            ev
                        },
                    ));
                }
                continue;
            }
        }
        if t.is_step {
            let round = x.rounds.iter().find(|r| r.task == t.id);
            let rounds = round.map_or(0, |r| r.rounds);
            let hi = t.typical_hi_ms.unwrap_or(0);
            let floor = if t.state == "landing" {
                l.long_landing_min_ms
            } else {
                l.long_min_ms
            };
            let long = hi.saturating_mul(l.long_factor).max(floor);
            // Waits for rationed capacity are not the task's own time, and a wait for a runner
            // pool unit is judged by stuck:wait.
            let working = t.in_state_ms.saturating_sub(t.capacity_wait_ms);
            let too_long = working > long && !t.pool_wait;
            let too_many = rounds > l.checks_max;
            if too_long || too_many {
                let mut ev = vec![
                    format!(
                        "in {} {} min, of which {} min waiting for capacity (a GitHub runner, a node or a slot); working {} min, limit {} min",
                        t.state,
                        mins(t.in_state_ms),
                        mins(t.capacity_wait_ms),
                        mins(working),
                        mins(long)
                    ),
                    format!(
                        "{rounds} full check rounds in this run (limit {}), attempt {}",
                        l.checks_max,
                        round.map_or(0, |r| r.attempt)
                    ),
                ];
                if let Some(r) = round {
                    for c in &r.last_checks {
                        ev.push(format!("check: {c}"));
                    }
                    for c in &r.remote {
                        ev.push(format!("remote run: {c}"));
                    }
                }
                ev.push(format!("no progress for {} min", mins(t.silent_ms)));
                out.push(f(
                    format!("stuck:long:{}", t.id),
                    "stuck",
                    Severity::Warn,
                    format!(
                        "#{} has been {} for {} min over {rounds} check round(s), whatever its activity",
                        t.id,
                        t.state,
                        mins(t.in_state_ms)
                    ),
                    ev,
                ));
            }
            // Without history for the step, only the floor applies.
            let hi = t.typical_hi_ms.unwrap_or(0);
            let limit = hi.saturating_mul(l.stuck_factor).max(l.stuck_min_ms);
            if t.in_state_ms > limit && t.silent_ms >= l.stuck_min_ms && !t.pool_wait {
                out.push(f(
                    format!("stuck:task:{}", t.id),
                    "stuck",
                    Severity::Warn,
                    format!(
                        "#{} has been {} for {} min, usually at most {} min, with no progress for {} min",
                        t.id,
                        t.state,
                        mins(t.in_state_ms),
                        mins(hi),
                        mins(t.silent_ms)
                    ),
                    vec![format!(
                        "limit {} x {} min = {} min",
                        l.stuck_factor,
                        mins(hi),
                        mins(limit)
                    )],
                ));
            }
        }
    }
    if x.queued > 0
        && x.free_slots
        && x.running == 0
        && !x.paused
        && x.queued_oldest_ms > l.queue_idle_ms
    {
        out.push(f(
            "stuck:queue".into(),
            "stuck",
            Severity::Severe,
            format!(
                "{} queued, slots free, nothing started for {} min",
                x.queued,
                mins(x.queued_oldest_ms)
            ),
            vec![],
        ));
    }
    for (t, ms) in &x.silent_hosts {
        // A host waiting on a resource is reported by stuck:wait, not as silent.
        let waiting = x.tasks.iter().any(|k| k.id == *t && k.waiting.is_some());
        if *ms > l.host_silent_ms && !waiting {
            out.push(f(
                format!("stuck:host:{t}"),
                "stuck",
                Severity::Warn,
                format!(
                    "the step host of #{t} is alive but silent for {} min",
                    mins(*ms)
                ),
                vec![],
            ));
        }
    }
    // overloaded
    let slow: Vec<&(u64, String, u64)> = x
        .slot_waits
        .iter()
        .filter(|(_, _, ms)| *ms > l.slot_wait_ms)
        .collect();
    let cpu_listed = slow.iter().any(|(_, s, _)| is_cpu_cause(s));
    if !slow.is_empty() {
        out.push(f(
            "overloaded:slots".into(),
            "overloaded",
            Severity::Warn,
            format!("{} tasks wait long for a slot", slow.len()),
            slow.iter()
                .map(|(t, s, ms)| {
                    let held = if is_cpu_cause(s) {
                        "; held by the CPU gate"
                    } else {
                        ""
                    };
                    format!("#{t} waits {} min for {s}{held}", mins(*ms))
                })
                .collect(),
        ));
    }
    // An assumed-busy reading is a placeholder, not a measurement: no finding.
    if let Some(p) = x.load_pct.filter(|p| !x.load_assumed && *p >= l.load_pct) {
        let summary = format!("the machine is at {p}% load");
        let evidence = match x.load_source {
            Some(src) => vec![format!("CPU reading from {src}")],
            None => vec![],
        };
        out.push(f(
            "overloaded:load".into(),
            "overloaded",
            Severity::Warn,
            summary,
            evidence,
        ));
    }
    if x.stall_ms >= l.stall_ms {
        out.push(f(
            "overloaded:disk".into(),
            "overloaded",
            Severity::Warn,
            format!("file creates on a disk take {} ms", x.stall_ms),
            vec![],
        ));
    }
    let frozen: Vec<&(Option<u64>, u64)> = x
        .frozen_launches
        .iter()
        .filter(|(_, ms)| *ms > l.frozen_launch_ms)
        .collect();
    if !frozen.is_empty() {
        let mut ev: Vec<String> = frozen
            .iter()
            .map(|(t, ms)| {
                format!(
                    "{} launching for {} s",
                    t.map_or("a step host".to_string(), |t| format!("#{t}")),
                    ms / 1000
                )
            })
            .collect();
        if x.syspolicyd_down {
            ev.push("syspolicyd is not running".into());
        }
        out.push(f(
            "overloaded:launches".into(),
            "overloaded",
            Severity::Severe,
            format!("{} step hosts do not finish launching", frozen.len()),
            ev,
        ));
    }
    for (dir, n, secs) in blocked_compiles(&x.procs, l.blocked_compile_ms / 1000) {
        if n >= l.blocked_compiles {
            out.push(f(
                format!("overloaded:blocked-compiles:{dir}"),
                "overloaded",
                Severity::Severe,
                format!(
                    "{n} compiles have sat in uninterruptible wait for up to {} min with no CPU, listing `{dir}`",
                    secs / 60
                ),
                vec![
                    format!("their `-L dependency=` is `{dir}`; a build directory with hundreds of thousands of files stalls rustc's directory listing"),
                    "check the size of that directory's own entry (`ls -ld`); rename it aside to unblock them".into(),
                ],
            ));
        }
    }
    if let Some(m) = x.mem_free_mb.filter(|m| *m < l.mem_free_mb) {
        out.push(f(
            "overloaded:memory".into(),
            "overloaded",
            Severity::Warn,
            format!("{m} MB of memory free"),
            vec![],
        ));
    }
    if x.cpu_gate_held_ms >= l.cpu_gate_ms && !cpu_listed {
        out.push(f(
            "overloaded:cpu-gate".into(),
            "overloaded",
            Severity::Warn,
            format!(
                "the CPU gate has held work back for {} min",
                mins(x.cpu_gate_held_ms)
            ),
            vec![],
        ));
    }
    // failures not addressed
    let undiagnosed: Vec<u64> = x
        .failed
        .iter()
        .filter(|t| t.undiagnosed && t.failed_ms > l.undiagnosed_ms)
        .map(|t| t.task)
        .collect();
    if !undiagnosed.is_empty() {
        out.push(f(
            "failures:undiagnosed".into(),
            "failures",
            Severity::Warn,
            format!("failed without a diagnosis: {}", list(&undiagnosed)),
            vec![],
        ));
    }
    let old: Vec<&(u64, u64, u64)> = x
        .pending_diagnoses
        .iter()
        .filter(|(_, _, age)| *age > l.pending_diagnosis_ms)
        .collect();
    if !old.is_empty() {
        out.push(f(
            "failures:pending-diagnosis".into(),
            "failures",
            Severity::Warn,
            format!("{} diagnoses wait for the owner", old.len()),
            old.iter()
                .map(|(id, t, age)| format!("diagnosis #{id} of #{t}, {} min old", mins(*age)))
                .collect(),
        ));
    }
    let mut by_sig: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
    for t in &x.failed {
        if !t.signature.is_empty() {
            by_sig.entry(&t.signature).or_default().push(t.task);
        }
    }
    for (sig, ids) in by_sig {
        if ids.len() >= l.same_failure {
            out.push(f(
                format!("failures:same:{sig}"),
                "failures",
                Severity::Warn,
                format!("the same failure on {} tasks: {sig}", ids.len()),
                vec![format!("tasks {}", list(&ids))],
            ));
        }
    }
    for (what, ms) in &x.red {
        if *ms > l.red_ms {
            out.push(f(
                format!("failures:red:{what}"),
                "failures",
                Severity::Severe,
                format!("{what} has been red for {} min", mins(*ms)),
                vec![],
            ));
        }
    }
    if x.update_failures >= l.update_failures {
        out.push(f(
            "failures:update".into(),
            "failures",
            Severity::Severe,
            format!(
                "the self-update failed {} times in a row",
                x.update_failures
            ),
            vec![],
        ));
    }
    // the daemon itself
    if x.restarts >= l.restarts {
        out.push(f(
            "daemon:restarts".into(),
            "daemon",
            Severity::Severe,
            format!("the daemon restarted {} times lately", x.restarts),
            vec![],
        ));
    }
    if x.late_loops >= l.late_loops {
        out.push(f(
            "daemon:late-loop".into(),
            "daemon",
            Severity::Warn,
            format!("the event loop woke late {} times lately", x.late_loops),
            vec![],
        ));
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

/// The findings not handled within the cooldown. `handled` is (key, when it was handled).
pub fn fresh(
    findings: &[Finding],
    handled: &[(String, u64)],
    now: u64,
    cooldown_ms: u64,
) -> Vec<Finding> {
    findings
        .iter()
        .filter(|fd| fd.severity != Severity::Note)
        .filter(|fd| {
            !handled
                .iter()
                .any(|(k, at)| *k == fd.key && now.saturating_sub(*at) < cooldown_ms)
        })
        .cloned()
        .collect()
}

/// Whether the report is severe: the session then runs on a stronger tier.
pub fn severe(findings: &[Finding]) -> bool {
    findings.iter().any(|f| f.severity == Severity::Severe)
}

/// Why nothing can land for MASON's own project, or `None` when the pipeline could take in a
/// task and heal itself. The daemon asks this itself: the session's word is not enough.
pub fn cannot_land(x: &Facts, l: &Limits) -> Option<String> {
    if x.launch_blocked {
        return Some("step hosts cannot launch".into());
    }
    if x.update_blocked {
        return Some("the self-update cannot take over".into());
    }
    if x.landing_failed >= l.cannot_land && x.landed == 0 {
        return Some(format!(
            "landing failed for {} tasks and nothing landed",
            x.landing_failed
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(v: &[Finding]) -> Vec<String> {
        v.iter().map(|f| f.key.clone()).collect()
    }

    fn l() -> Limits {
        Limits::default()
    }

    #[test]
    fn load_finding_names_a_known_source() {
        let x = Facts {
            load_pct: Some(99),
            load_source: Some("loadavg"),
            ..Default::default()
        };
        let out = judge(&x, &l());
        let f = out.iter().find(|f| f.key == "overloaded:load").unwrap();
        assert_eq!(f.summary, "the machine is at 99% load");
        assert!(f.evidence.iter().any(|e| e.contains("loadavg")));
    }

    fn rustc(pid: u32, state: &str, elapsed_s: u64, cpu_s: u64, dir: &str) -> ProcFact {
        ProcFact {
            pid,
            state: state.into(),
            elapsed_s,
            cpu_s,
            command: format!("/x/bin/rustc --crate-name a -L dependency={dir} src/lib.rs"),
        }
    }

    #[test]
    fn many_compiles_in_uninterruptible_wait_name_their_directory() {
        let dir = "/c/mason-target.noindex/base-cargo-workspace/debug/deps";
        let mut procs: Vec<ProcFact> = (1..=5).map(|i| rustc(i, "U", 2760, 3, dir)).collect();
        // Busy, sleeping and too-young ones do not count.
        procs.push(rustc(10, "R", 2760, 2000, dir));
        procs.push(rustc(11, "U", 30, 0, dir));
        procs.push(rustc(12, "U", 2760, 3, "/c/other/debug/deps"));
        let x = Facts {
            procs,
            ..Default::default()
        };
        let out = judge(&x, &l());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].key, format!("overloaded:blocked-compiles:{dir}"));
        assert!(out[0].summary.starts_with("5 compiles"));
        assert!(out[0].summary.contains(dir));
        // Fewer than the limit: quiet.
        let x = Facts {
            procs: vec![rustc(1, "U", 2760, 3, dir)],
            ..Default::default()
        };
        assert!(judge(&x, &l()).is_empty());
    }

    #[test]
    fn ps_rows_parse() {
        let v = parse_ps("  42 U     46:00 0:01.50 /x/rustc -L dependency=/d/deps a.rs\n 7 Ss 1-02:03:04 10:00 launchd\njunk\n");
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].pid, v[0].elapsed_s, v[0].cpu_s), (42, 2760, 1));
        assert_eq!(dependency_dir(&v[0].command), Some("/d/deps"));
        assert_eq!(v[1].elapsed_s, 93_784);
    }

    #[test]
    fn a_busy_task_in_one_state_for_hours_is_stuck_long() {
        let task = |in_state_ms| TaskFact {
            id: 5,
            state: "developing".into(),
            is_step: true,
            in_state_ms,
            typical_hi_ms: Some(20 * MIN),
            silent_ms: MIN,
            waiting: None,
            holders: None,
            pool_wait: false,
            capacity_wait_ms: 0,
        };
        let x = |t: TaskFact, rounds: u32| Facts {
            tasks: vec![t],
            rounds: vec![RoundFact {
                task: 5,
                rounds,
                attempt: 2,
                last_checks: vec!["red (3 failing)".into()],
                remote: Vec::new(),
            }],
            ..Default::default()
        };
        assert!(judge(&x(task(5 * H), 2), &l()).is_empty());
        let out = judge(&x(task(7 * H), 2), &l());
        assert_eq!(keys(&out), vec!["stuck:long:5".to_string()]);
        assert_eq!(out[0].severity, Severity::Warn);
        assert!(out[0].evidence.iter().any(|e| e.contains("attempt 2")));
        // The 7th round without submitting, at any age.
        assert!(judge(&x(task(MIN), 6), &l()).is_empty());
        assert_eq!(
            keys(&judge(&x(task(MIN), 7), &l())),
            vec!["stuck:long:5".to_string()]
        );
        // Landing has a shorter floor.
        let mut t = task(3 * H);
        t.state = "landing".into();
        t.typical_hi_ms = None;
        assert_eq!(
            keys(&judge(&x(t, 0), &l())),
            vec!["stuck:long:5".to_string()]
        );
    }

    /// A develop run 900 minutes in its state, against a limit of 815.
    fn long_develop(capacity_min: u64, pool_wait: bool, rounds: u32) -> Vec<Finding> {
        let limits = Limits {
            long_min_ms: 815 * MIN,
            ..l()
        };
        let x = Facts {
            tasks: vec![TaskFact {
                id: 5,
                state: "developing".into(),
                is_step: true,
                in_state_ms: 900 * MIN,
                typical_hi_ms: None,
                silent_ms: MIN,
                waiting: pool_wait.then(|| ("a GitHub runner".into(), 30 * MIN)),
                holders: None,
                pool_wait,
                capacity_wait_ms: capacity_min * MIN,
            }],
            rounds: vec![RoundFact {
                task: 5,
                rounds,
                ..Default::default()
            }],
            ..Default::default()
        };
        judge(&x, &limits)
    }

    #[test]
    fn stuck_long_does_not_count_capacity_waits() {
        assert!(long_develop(850, false, 0).is_empty());
        let out = long_develop(50, false, 0);
        assert_eq!(keys(&out), ["stuck:long:5"]);
        assert!(
            out[0].evidence[0].starts_with("in developing 900 min, of which 50 min waiting for"),
            "{:?}",
            out[0].evidence
        );
        assert!(out[0].evidence[0].ends_with("working 850 min, limit 815 min"));
    }

    #[test]
    fn a_pool_wait_past_the_limit_is_stuck_wait_only() {
        // A short current wait: stuck:long stays quiet while the pool wait lasts.
        assert!(long_develop(0, true, 0).is_empty());
        let x = Facts {
            tasks: vec![TaskFact {
                id: 5,
                state: "developing".into(),
                is_step: true,
                in_state_ms: 900 * MIN,
                waiting: Some(("a GitHub runner".into(), 850 * MIN)),
                pool_wait: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(keys(&judge(&x, &l())), ["stuck:wait:5"]);
    }

    #[test]
    fn the_seventh_check_round_is_stuck_long_whatever_the_waits() {
        for (capacity, pool_wait) in [(0, false), (900, false), (900, true)] {
            assert_eq!(
                keys(&long_develop(capacity, pool_wait, 7)),
                ["stuck:long:5"]
            );
        }
    }

    #[test]
    fn capacity_waits_are_totalled_from_the_wait_events() {
        let task = TaskId(5);
        let start = |at, task, class: &str, on: &str| Event {
            at: crate::Timestamp(at),
            kind: EventKind::WaitStarted {
                task,
                on: on.into(),
                class: Some(class.into()),
            },
        };
        let end = |at| Event {
            at: crate::Timestamp(at),
            kind: EventKind::WaitEnded { task },
        };
        let events = [
            // Before the task entered its state.
            start(10, task, "gha", ""),
            end(90),
            start(100, task, "gha", ""),
            end(130),
            // Another task's wait.
            start(130, TaskId(6), "gha", ""),
            // The owner's, replaced by a compile wait, which a node wait replaces.
            start(140, task, "approval", ""),
            start(200, task, "compile", ""),
            start(210, task, "environment", "Waiting for a node: full"),
            end(215),
            start(220, task, "environment", "git is busy"),
            end(300),
            // Dropped by the run that starts at 330.
            start(310, task, "node:n1", ""),
            // Still open.
            start(400, task, "test-suite", ""),
        ];
        assert_eq!(
            capacity_wait_ms(&events, task, &[100, 330], 100, 407),
            30 + 10 + 5 + 20 + 7
        );
    }

    #[test]
    fn stuck_long_names_open_remote_runs() {
        let mut x = Facts {
            tasks: vec![TaskFact {
                id: 5,
                state: "developing".into(),
                is_step: true,
                in_state_ms: 7 * H,
                typical_hi_ms: Some(20 * MIN),
                silent_ms: 0,
                waiting: None,
                holders: None,
                pool_wait: false,
                capacity_wait_ms: 0,
            }],
            rounds: vec![RoundFact {
                task: 5,
                ..Default::default()
            }],
            ..Default::default()
        };
        let out = judge(&x, &l());
        assert!(!out[0].evidence.iter().any(|e| e.contains("remote")));
        x.rounds[0].remote =
            vec!["test step on node marmot, waiting false, polled 1 min ago".into()];
        let out = judge(&x, &l());
        assert_eq!(
            out[0]
                .evidence
                .iter()
                .filter(|e| e.contains("remote run"))
                .count(),
            1
        );
    }

    #[test]
    fn healthy_facts_give_no_findings() {
        assert!(judge(&Facts::default(), &l()).is_empty());
        let x = Facts {
            tasks: vec![TaskFact {
                id: 1,
                state: "developing".into(),
                is_step: true,
                in_state_ms: 10 * MIN,
                typical_hi_ms: Some(20 * MIN),
                silent_ms: MIN,
                waiting: None,
                holders: None,
                pool_wait: false,
                capacity_wait_ms: 0,
            }],
            load_pct: Some(40),
            ..Default::default()
        };
        assert!(judge(&x, &l()).is_empty());
    }

    #[test]
    fn a_silent_host_waiting_on_a_resource_is_not_stuck() {
        let mut t = TaskFact {
            id: 9,
            state: "developing".into(),
            is_step: true,
            in_state_ms: 2 * H,
            typical_hi_ms: None,
            silent_ms: 2 * H,
            waiting: Some(("exclusive".into(), 52 * MIN)),
            holders: None,
            pool_wait: false,
            capacity_wait_ms: 0,
        };
        let x = Facts {
            tasks: vec![t.clone()],
            silent_hosts: vec![(9, 2 * H)],
            ..Default::default()
        };
        assert!(!keys(&judge(&x, &l())).contains(&"stuck:host:9".to_string()));
        t.waiting = None;
        let x = Facts {
            tasks: vec![t],
            silent_hosts: vec![(9, 2 * H)],
            ..Default::default()
        };
        assert!(keys(&judge(&x, &l())).contains(&"stuck:host:9".to_string()));
    }

    #[test]
    fn a_task_far_past_its_usual_time_without_progress_is_stuck() {
        let mut t = TaskFact {
            id: 7,
            state: "developing".into(),
            is_step: true,
            in_state_ms: 5 * H,
            typical_hi_ms: Some(20 * MIN),
            silent_ms: 2 * H,
            waiting: None,
            holders: None,
            pool_wait: false,
            capacity_wait_ms: 0,
        };
        let x = Facts {
            tasks: vec![t.clone()],
            ..Default::default()
        };
        assert_eq!(keys(&judge(&x, &l())), ["stuck:task:7"]);
        // Progress lately: slow, not stuck.
        t.silent_ms = MIN;
        let x = Facts {
            tasks: vec![t.clone()],
            ..Default::default()
        };
        assert!(judge(&x, &l()).is_empty());
        // A long wait on a slot is its own finding.
        t.waiting = Some(("compile".into(), 3 * H));
        let x = Facts {
            tasks: vec![t],
            ..Default::default()
        };
        assert_eq!(keys(&judge(&x, &l())), ["stuck:wait:7"]);
    }

    #[test]
    fn a_step_queued_for_a_runner_pool_is_not_stuck_task() {
        let mut t = TaskFact {
            id: 11,
            state: "landing".into(),
            is_step: true,
            in_state_ms: 36 * MIN,
            typical_hi_ms: None,
            silent_ms: 36 * MIN,
            waiting: Some(("a GitHub runner: 19 of 16 in use".into(), 36 * MIN)),
            holders: None,
            pool_wait: true,
            capacity_wait_ms: 0,
        };
        let x = |t: &TaskFact| Facts {
            tasks: vec![t.clone()],
            slot_waits: vec![(11, "a GitHub runner: 19 of 16 in use".into(), 36 * MIN)],
            ..Default::default()
        };
        assert_eq!(keys(&judge(&x(&t), &l())), ["overloaded:slots"]);
        // Not a pool wait: judged as before.
        t.pool_wait = false;
        assert!(keys(&judge(&x(&t), &l())).contains(&"stuck:task:11".to_string()));
        // A pool wait past wait_ms is still stuck:wait.
        t.pool_wait = true;
        t.waiting = Some(("a GitHub runner".into(), 3 * H));
        assert!(keys(&judge(&x(&t), &l())).contains(&"stuck:wait:11".to_string()));
    }

    fn waiter(id: u64, wait: u64, h: Option<HolderFact>) -> TaskFact {
        TaskFact {
            id,
            state: "developing".into(),
            is_step: true,
            in_state_ms: wait,
            typical_hi_ms: None,
            silent_ms: MIN,
            waiting: Some(("compile".into(), wait)),
            holders: h,
            pool_wait: false,
            capacity_wait_ms: 0,
        }
    }

    fn crowd(wait: u64, grant: u64) -> Vec<Finding> {
        // Two holders: the newest granted `grant` ago, the oldest at least 20 min ago, so
        // `grant` alone says how long since the last grant.
        let h = holder_fact(
            &[100 * H - grant, 100 * H - grant.max(20 * MIN)],
            100 * H,
            23,
            23,
            false,
        );
        let x = Facts {
            tasks: (1..=23).map(|i| waiter(i, wait, h.clone())).collect(),
            ..Default::default()
        };
        judge(&x, &l())
    }

    #[test]
    fn waiters_behind_progressing_holders_are_not_stuck() {
        assert!(crowd(3 * H, 10 * MIN).is_empty());
        assert_eq!(crowd(3 * H, 3 * H).len(), 23);
        // Past queue_depth * run (23 * 20 min = 460 min).
        let f = crowd(9 * H, 10 * MIN);
        assert!(f.iter().all(|x| x.key.starts_with("stuck:wait:")));
        assert_eq!(f.len(), 23);
    }

    #[test]
    fn waiters_are_judged_by_the_work_ahead_of_them() {
        let at = |position: u32, wait: u64| {
            let h = holder_fact(
                &[100 * H - 10 * MIN, 100 * H - 90 * MIN],
                100 * H,
                9,
                position,
                false,
            );
            let x = Facts {
                tasks: vec![waiter(7, wait, h)],
                ..Default::default()
            };
            keys(&judge(&x, &l()))
        };
        // Position 2 x 90 min = 180 min.
        assert_eq!(at(2, 200 * MIN), ["stuck:wait:7"]);
        // Under the wait_ms floor.
        assert!(at(2, 100 * MIN).is_empty());
        // Position 9 x 90 min = 810 min.
        assert!(!at(9, 200 * MIN)
            .iter()
            .any(|k| k.starts_with("stuck:wait:")));
    }

    #[test]
    fn check_holders_give_no_run_length_bound() {
        let at = |grant: u64, wait: u64| {
            let h = holder_fact(
                &[100 * H - grant, 100 * H - 8 * MIN.max(grant)],
                100 * H,
                9,
                2,
                true,
            );
            let x = Facts {
                tasks: vec![waiter(7, wait, h)],
                ..Default::default()
            };
            judge(&x, &l())
        };
        // #1111: newest grant 1 min ago, longest run 8 min, position 2, waited 747 min.
        let f = at(MIN, 747 * MIN);
        assert_eq!(keys(&f), ["stuck:aged-wait:7"]);
        assert_eq!(f[0].severity, Severity::Note);
        // No grant for 3 h: the quiet branch still fires.
        assert_eq!(keys(&at(3 * H, 4 * H)), ["stuck:wait:7"]);
    }

    #[test]
    fn an_old_progressing_wait_is_a_note_that_starts_no_session() {
        let f = crowd(7 * H, 10 * MIN);
        assert_eq!(f.len(), 23);
        assert!(f
            .iter()
            .all(|x| x.severity == Severity::Note && x.key.starts_with("stuck:aged-wait:")));
        assert!(fresh(&f, &[], 0, H).is_empty());
        assert!(!severe(&f));
    }

    #[test]
    fn a_queue_with_free_slots_that_starts_nothing_is_severe() {
        let x = Facts {
            queued: 4,
            queued_oldest_ms: H,
            free_slots: true,
            ..Default::default()
        };
        let v = judge(&x, &l());
        assert_eq!(keys(&v), ["stuck:queue"]);
        assert!(severe(&v));
        let paused = Facts { paused: true, ..x };
        assert!(judge(&paused, &l()).is_empty());
    }

    #[test]
    fn overload_has_a_finding_for_each_symptom() {
        let x = Facts {
            slot_waits: vec![(3, "compile".into(), H)],
            load_pct: Some(99),
            stall_ms: 20_000,
            frozen_launches: vec![(Some(3), 10 * MIN)],
            syspolicyd_down: true,
            mem_free_mb: Some(100),
            cpu_gate_held_ms: H,
            silent_hosts: vec![(4, H)],
            ..Default::default()
        };
        let v = judge(&x, &l());
        assert_eq!(
            keys(&v),
            [
                "overloaded:cpu-gate",
                "overloaded:disk",
                "overloaded:launches",
                "overloaded:load",
                "overloaded:memory",
                "overloaded:slots",
                "stuck:host:4"
            ]
        );
        let launches = v.iter().find(|f| f.key == "overloaded:launches").unwrap();
        assert!(launches.evidence.iter().any(|e| e.contains("syspolicyd")));
    }

    #[test]
    fn one_wait_is_one_finding() {
        let x = Facts {
            slot_waits: vec![(439, "cpu gate".into(), 90 * MIN)],
            cpu_gate_held_ms: 90 * MIN,
            ..Default::default()
        };
        let v = judge(&x, &l());
        assert_eq!(keys(&v), ["overloaded:slots"]);
        assert!(v[0].evidence.iter().any(|e| e.contains("CPU gate")));
    }

    #[test]
    fn assumed_load_is_not_a_finding() {
        let x = Facts {
            load_pct: Some(100),
            load_assumed: true,
            load_source: Some("assumed-busy"),
            ..Default::default()
        };
        assert!(judge(&x, &l()).is_empty());
    }

    #[test]
    fn measured_load_still_shows() {
        let x = Facts {
            load_pct: Some(100),
            load_assumed: false,
            load_source: Some("mach"),
            ..Default::default()
        };
        assert_eq!(keys(&judge(&x, &l())), ["overloaded:load"]);
    }

    #[test]
    fn failures_not_addressed_are_found() {
        let failed = |task, sig: &str, undiagnosed| FailedFact {
            task,
            signature: sig.into(),
            failed_ms: H,
            undiagnosed,
        };
        let x = Facts {
            failed: vec![
                failed(1, "clippy", true),
                failed(2, "clippy", false),
                failed(3, "clippy", false),
                failed(4, "other", false),
            ],
            pending_diagnoses: vec![(9, 2, 10 * H)],
            red: vec![("landing:mason".into(), 5 * H), ("base:mason".into(), MIN)],
            update_failures: 3,
            ..Default::default()
        };
        assert_eq!(
            keys(&judge(&x, &l())),
            [
                "failures:pending-diagnosis",
                "failures:red:landing:mason",
                "failures:same:clippy",
                "failures:undiagnosed",
                "failures:update"
            ]
        );
    }

    #[test]
    fn the_daemon_itself_is_watched() {
        let x = Facts {
            restarts: 4,
            late_loops: 3,
            ..Default::default()
        };
        assert_eq!(
            keys(&judge(&x, &l())),
            ["daemon:late-loop", "daemon:restarts"]
        );
    }

    #[test]
    fn a_planned_start_is_not_a_restart() {
        assert_eq!(unplanned_starts(&["planned", "start"]), 0);
        assert_eq!(unplanned_starts(&["start", "start", "start"]), 3);
        assert_eq!(unplanned_starts(&["planned", "cancelled", "start"]), 1);
        assert_eq!(unplanned_starts(&["planned", "start", "start", "start"]), 2);
        let x = Facts {
            restarts: unplanned_starts(&["planned", "start", "start", "start", "start"]),
            ..Default::default()
        };
        assert_eq!(keys(&judge(&x, &l())), ["daemon:restarts"]);
    }

    #[test]
    fn keys_are_the_same_on_every_tick() {
        let mk = |ms| Facts {
            stall_ms: 9_000,
            slot_waits: vec![(5, "test".into(), ms)],
            ..Default::default()
        };
        let a = keys(&judge(&mk(H), &l()));
        let b = keys(&judge(&mk(2 * H), &l()));
        assert_eq!(a, b);
    }

    #[test]
    fn a_handled_finding_waits_out_its_cooldown_and_a_new_one_goes_through() {
        let x = Facts {
            stall_ms: 9_000,
            load_pct: Some(99),
            ..Default::default()
        };
        let all = judge(&x, &l());
        let handled = vec![("overloaded:disk".to_string(), 1_000)];
        let c = l().cooldown_ms;
        let fresh_now = fresh(&all, &handled, 2_000, c);
        assert_eq!(keys(&fresh_now), ["overloaded:load"]);
        // After the cooldown the same finding is raised again.
        assert_eq!(fresh(&all, &handled, 1_000 + c, c).len(), 2);
    }

    #[test]
    fn nothing_can_land_is_decided_from_facts() {
        assert_eq!(cannot_land(&Facts::default(), &l()), None);
        let x = Facts {
            landing_failed: 3,
            ..Default::default()
        };
        assert!(cannot_land(&x, &l()).unwrap().contains("landing failed"));
        let y = Facts {
            landing_failed: 3,
            landed: 1,
            ..Default::default()
        };
        assert_eq!(cannot_land(&y, &l()), None);
        let z = Facts {
            launch_blocked: true,
            ..Default::default()
        };
        assert!(cannot_land(&z, &l()).unwrap().contains("launch"));
    }
}
