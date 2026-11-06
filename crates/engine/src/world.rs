// SPDX-License-Identifier: GPL-3.0-or-later

use mason_core::*;
use std::collections::{BTreeMap, BTreeSet};

/// Everything the reducer knows. Deterministic collections only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct World {
    pub now: Timestamp,
    pub projects: BTreeMap<ProjectId, Project>,
    pub tasks: BTreeMap<TaskId, Task>,
    pub step_runs: BTreeMap<StepRunId, StepRun>,
    pub decisions: BTreeMap<DecisionId, Decision>,
    pub waits: BTreeMap<TaskId, Wait>,
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

    /// Release landed blockers and recompute every activity. Returns the
    /// queued tasks that just became claimable.
    pub(crate) fn refresh(&mut self, before: &BTreeMap<TaskId, Activity>) -> Vec<TaskId> {
        let landed: BTreeSet<TaskId> = self
            .tasks
            .values()
            .filter(|t| t.landed.is_some())
            .map(|t| t.id)
            .collect();
        let ids: Vec<TaskId> = self.tasks.keys().copied().collect();
        let mut woken = vec![];
        for id in ids {
            let mut t = self.tasks.remove(&id).expect("id from keys");
            t.blockers.retain(|b| !landed.contains(b));
            let act = self.derive_activity(&t);
            if t.state == State::Queued
                && act == Activity::Waiting(WaitReason::Slot)
                && before.get(&id) != Some(&act)
            {
                woken.push(id);
            }
            t.activity = act;
            self.tasks.insert(id, t);
        }
        woken
    }

    pub fn open_run(&self, task: TaskId) -> Option<StepRunId> {
        self.step_runs
            .values()
            .find(|r| r.task == task && r.outcome.is_none())
            .map(|r| r.id)
    }

    pub(crate) fn leave(&mut self, task: TaskId, outcome: StepOutcome) -> bool {
        match self.open_run(task) {
            Some(id) => {
                self.step_runs.get_mut(&id).expect("open run").outcome = Some(outcome);
                true
            }
            None => false,
        }
    }

    /// Move into a step state, opening a new step run.
    pub(crate) fn enter(&mut self, task: TaskId, step: State, fx: &mut Vec<Effect>) {
        let now = self.now;
        self.waits.remove(&task);
        let run = StepRunId(self.next_run);
        self.next_run += 1;
        let t = self.tasks.get_mut(&task).expect("task exists");
        let attempt = t.attempts.entry(step).or_insert(0);
        *attempt += 1;
        let attempt = *attempt;
        t.state = step;
        self.step_runs.insert(
            run,
            StepRun {
                id: run,
                task,
                step,
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
