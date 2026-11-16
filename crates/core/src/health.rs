// SPDX-License-Identifier: GPL-3.0-or-later

//! The pipeline's health, judged from facts (s432, decision 0425).
//!
//! The daemon gathers the facts (task ages, slot waits, load, failures...); this module turns
//! them into findings, each with a stable key, a severity and the evidence behind it. A pure
//! function: no clock, no I/O. Time arrives in the facts. A healthy pipeline gives no findings.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warn,
    Severe,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
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
}

const MIN: u64 = 60_000;
const H: u64 = 60 * MIN;

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            stuck_factor: 3,
            stuck_min_ms: 30 * MIN,
            wait_ms: 2 * H,
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
pub fn judge(x: &Facts, l: &Limits) -> Vec<Finding> {
    let mut out = vec![];
    // stuck
    for t in &x.tasks {
        if let Some((what, ms)) = &t.waiting {
            if *ms > l.wait_ms {
                out.push(f(
                    format!("stuck:wait:{}", t.id),
                    "stuck",
                    Severity::Warn,
                    format!("#{} has waited {} min for {what}", t.id, mins(*ms)),
                    vec![format!("#{} is {}; the wait is `{what}`", t.id, t.state)],
                ));
                continue;
            }
        }
        if t.is_step {
            // Without history for the step, only the floor applies.
            let hi = t.typical_hi_ms.unwrap_or(0);
            let limit = hi.saturating_mul(l.stuck_factor).max(l.stuck_min_ms);
            if t.in_state_ms > limit && t.silent_ms >= l.stuck_min_ms {
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
        if *ms > l.host_silent_ms {
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
            }],
            load_pct: Some(40),
            ..Default::default()
        };
        assert!(judge(&x, &l()).is_empty());
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
