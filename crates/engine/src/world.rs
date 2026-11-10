// SPDX-License-Identifier: GPL-3.0-or-later

use mason_core::*;
use std::collections::{BTreeMap, BTreeSet};

static CLONES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many times a `World` has been deep-copied in this process. For tests that
/// prove replay and snapshots do not copy.
pub fn world_clones() -> usize {
    CLONES.load(std::sync::atomic::Ordering::Relaxed)
}

impl Clone for World {
    fn clone(&self) -> Self {
        CLONES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        World {
            now: self.now,
            projects: self.projects.clone(),
            tasks: self.tasks.clone(),
            step_runs: self.step_runs.clone(),
            decisions: self.decisions.clone(),
            waits: self.waits.clone(),
            open_runs: self.open_runs.clone(),
            next_task: self.next_task,
            next_run: self.next_run,
            next_decision: self.next_decision,
        }
    }
}

/// Everything the reducer knows. Deterministic collections only.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct World {
    pub now: Timestamp,
    pub projects: BTreeMap<ProjectId, Project>,
    pub tasks: BTreeMap<TaskId, Task>,
    pub step_runs: BTreeMap<StepRunId, StepRun>,
    pub decisions: BTreeMap<DecisionId, Decision>,
    pub waits: BTreeMap<TaskId, Wait>,
    /// Task to its open step run, kept by `enter` and `leave`.
    pub(crate) open_runs: BTreeMap<TaskId, StepRunId>,
    pub(crate) next_task: u64,
    pub(crate) next_run: u64,
    pub(crate) next_decision: u64,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    /// The one place activity is decided. Called for every task after every
    /// event, so state and activity always change together.
    fn derive_activity(&self, t: &Task) -> Activity {
        match t.state {
            State::Proposed | State::Failed | State::Canceled | State::Landed => Activity::Idle,
            _ if t.paused => Activity::Paused,
            State::Queued if !t.blockers.is_empty() => Activity::Held(t.blockers.clone()),
            State::Queued => match t.not_before {
                Some(nb) if nb > self.now => Activity::Waiting(WaitReason::NotBefore(nb)),
                _ => Activity::Waiting(WaitReason::Slot),
            },
            State::NeedsAttention => Activity::Waiting(WaitReason::Owner),
            _ => match self.waits.get(&t.id) {
                Some(w) => Activity::Waiting(WaitReason::On(w.on.clone())),
                None => Activity::Running,
            },
        }
    }

    /// Release landed blockers and recompute every activity, changing only the
    /// tasks that differ (no task is copied). Returns the queued tasks that just
    /// became claimable, and every task it changed.
    pub(crate) fn refresh(&mut self, changed: &mut Vec<TaskId>) -> Vec<TaskId> {
        let mut updates: Vec<(TaskId, Option<BTreeSet<TaskId>>, Activity)> = vec![];
        for t in self.tasks.values() {
            let released = if t
                .blockers
                .iter()
                .any(|b| self.tasks.get(b).is_some_and(|x| x.landed.is_some()))
            {
                let mut kept = t.blockers.clone();
                kept.retain(|b| self.tasks.get(b).is_none_or(|x| x.landed.is_none()));
                Some(kept)
            } else {
                None
            };
            let act = match &released {
                Some(kept) => {
                    let mut c = t.clone();
                    c.blockers = kept.clone();
                    self.derive_activity(&c)
                }
                None => self.derive_activity(t),
            };
            if released.is_some() || act != t.activity {
                updates.push((t.id, released, act));
            }
        }
        let mut woken = vec![];
        for (id, released, act) in updates {
            let t = self.tasks.get_mut(&id).expect("id from tasks");
            if let Some(kept) = released {
                t.blockers = kept;
            }
            if t.state == State::Queued
                && act == Activity::Waiting(WaitReason::Slot)
                && t.activity != act
            {
                woken.push(id);
            }
            t.activity = act;
            changed.push(id);
        }
        woken
    }

    pub fn open_run(&self, task: TaskId) -> Option<StepRunId> {
        self.open_runs.get(&task).copied()
    }

    pub(crate) fn leave(&mut self, task: TaskId, outcome: StepOutcome) -> bool {
        match self.open_runs.remove(&task) {
            Some(id) => {
                self.step_runs.get_mut(&id).expect("open run").outcome = Some(outcome);
                true
            }
            None => false,
        }
    }

    /// Move into a step state, opening a new step run.
    pub(crate) fn enter(&mut self, task: TaskId, step: State, name: &str, fx: &mut Vec<Effect>) {
        let now = self.now;
        self.waits.remove(&task);
        let run = StepRunId(self.next_run);
        self.next_run += 1;
        let t = self.tasks.get_mut(&task).expect("task exists");
        let by_state = t.attempts.entry(step).or_insert(0);
        *by_state += 1;
        let mut attempt = *by_state;
        if !name.is_empty() {
            // With named steps an attempt counts that step's runs, so a second
            // step in the same category starts again at 1.
            let by_name = t.step_attempts.entry(name.to_string()).or_insert(0);
            *by_name += 1;
            attempt = *by_name;
        }
        t.state = step;
        t.step = name.to_string();
        self.open_runs.insert(task, run);
        self.step_runs.insert(
            run,
            StepRun {
                id: run,
                task,
                step,
                name: name.to_string(),
                attempt,
                started_at: now,
                outcome: None,
            },
        );
        fx.push(Effect::StartStep {
            task,
            step,
            run,
            attempt,
        });
    }

    /// Clean-up common to every move into a terminal or idle state.
    pub(crate) fn settle(&mut self, task: TaskId) {
        self.waits.remove(&task);
        if let Some(t) = self.tasks.get_mut(&task) {
            t.paused = false;
        }
    }

    /// Structural invariants that must hold after every event.
    pub fn check_invariants(&self) -> Result<(), String> {
        let mut scanned: BTreeMap<TaskId, StepRunId> = BTreeMap::new();
        for r in self.step_runs.values().filter(|r| r.outcome.is_none()) {
            scanned.entry(r.task).or_insert(r.id);
        }
        if scanned != self.open_runs {
            return Err(format!(
                "open-run index {:?} differs from the step runs {:?}",
                self.open_runs, scanned
            ));
        }
        for t in self.tasks.values() {
            let id = t.id;
            if t.activity != self.derive_activity(t) {
                return Err(format!(
                    "{id}: activity {:?} out of step with state",
                    t.activity
                ));
            }
            if t.landed.is_some() != (t.state == State::Landed) {
                return Err(format!("{id}: landed outcome mismatch"));
            }
            if matches!(t.activity, Activity::Held(_)) && t.state != State::Queued {
                return Err(format!("{id}: held outside queued"));
            }
            if t.decision.is_some() != (t.state == State::NeedsAttention) {
                return Err(format!("{id}: decision/needs-attention mismatch"));
            }
            let open = self
                .step_runs
                .values()
                .filter(|r| r.task == id && r.outcome.is_none());
            let open = open.count();
            if open != usize::from(t.state.is_step()) {
                return Err(format!("{id}: {open} open step runs in {:?}", t.state));
            }
            if t.state.is_terminal() && self.waits.contains_key(&id) {
                return Err(format!("{id}: wait on terminal task"));
            }
            for b in &t.blockers {
                if !self.tasks.contains_key(b) {
                    return Err(format!("{id}: unknown blocker {b}"));
                }
            }
        }
        Ok(())
    }
}
