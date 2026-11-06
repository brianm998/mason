// SPDX-License-Identifier: GPL-3.0-or-later

use crate::World;
use mason_core::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReduceError {
    UnknownTask(TaskId),
    UnknownProject(ProjectId),
    DuplicateProject(ProjectId),
    /// The event is not allowed from the task's current state.
    Illegal {
        from: State,
        event: &'static str,
    },
    /// Admission refused: the task is held, paused or waiting on its start time.
    NotClaimable(Activity),
    /// The answer names a decision that is not the task's open one.
    StaleAnswer,
    BadRetryTarget(State),
    BlockerCycle,
    NoSuchWait,
}

fn name(k: &EventKind) -> &'static str {
    use EventKind::*;
    match k {
        ProjectRegistered { .. } => "ProjectRegistered",
        TaskFiled { .. } => "TaskFiled",
        Approved { .. } => "Approved",
        Rejected { .. } => "Rejected",
        Admitted { .. } => "Admitted",
        StepSucceeded { .. } => "StepSucceeded",
        SentBack { .. } => "SentBack",
        ExplorationRequested { .. } => "ExplorationRequested",
        QuestionAsked { .. } => "QuestionAsked",
        Answered { .. } => "Answered",
        StepFailed { .. } => "StepFailed",
        Retried { .. } => "Retried",
        Canceled { .. } => "Canceled",
        ClosedNoChange { .. } => "ClosedNoChange",
        Paused { .. } => "Paused",
        Resumed { .. } => "Resumed",
        WaitStarted { .. } => "WaitStarted",
        WaitEnded { .. } => "WaitEnded",
        BlockerAdded { .. } => "BlockerAdded",
        Tick => "Tick",
    }
}

/// The pure transition function. On error the input world is untouched.
pub fn reduce(world: &World, event: &Event) -> Result<(World, Vec<Effect>), ReduceError> {
    let mut w = world.clone();
    let mut fx = Vec::new();
    if event.at > w.now {
        w.now = event.at;
    }
    let before: BTreeMap<TaskId, Activity> = w
        .tasks
        .values()
        .map(|t| (t.id, t.activity.clone()))
        .collect();
    apply(&mut w, &event.kind, &mut fx)?;
    for task in w.refresh(&before) {
        fx.push(Effect::WakeScheduler { task });
    }
    Ok((w, fx))
}

fn get(w: &World, id: TaskId) -> Result<&Task, ReduceError> {
    w.tasks.get(&id).ok_or(ReduceError::UnknownTask(id))
}

fn illegal(from: State, k: &EventKind) -> ReduceError {
    ReduceError::Illegal {
        from,
        event: name(k),
    }
}

fn would_cycle(w: &World, task: TaskId, blocker: TaskId) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![blocker];
    while let Some(n) = stack.pop() {
        if n == task {
            return true;
        }
        if seen.insert(n) {
            if let Some(t) = w.tasks.get(&n) {
                stack.extend(t.blockers.iter().copied());
            }
        }
    }
    false
}

/// Stop whatever is running and release what it held.
fn wind_down(w: &mut World, task: TaskId, outcome: StepOutcome, fx: &mut Vec<Effect>) {
    if w.leave(task, outcome) {
        fx.push(Effect::StopStepHost { task });
    }
    fx.push(Effect::ReleaseLeases { task });
    w.settle(task);
}

fn apply(w: &mut World, k: &EventKind, fx: &mut Vec<Effect>) -> Result<(), ReduceError> {
    use EventKind as E;
    use State as S;
    match k {
        E::ProjectRegistered { project, name } => {
            if w.projects.contains_key(project) {
                return Err(ReduceError::DuplicateProject(*project));
            }
            w.projects.insert(
                *project,
                Project {
                    id: *project,
                    name: name.clone(),
                },
            );
        }
        E::TaskFiled {
            project,
            title,
            body,
            priority,
            trust,
            explored,
            proposed,
            blockers,
            not_before,
        } => {
            if !w.projects.contains_key(project) {
                return Err(ReduceError::UnknownProject(*project));
            }
            for b in blockers {
                get(w, *b)?;
            }
            let id = TaskId(w.next_task);
            w.next_task += 1;
            let task = Task {
                id,
                project: *project,
                title: title.clone(),
                body: body.clone(),
                priority: *priority,
                trust: *trust,
                enqueued_at: w.now,
                state: if *proposed { S::Proposed } else { S::Queued },
                activity: Activity::Idle, // recomputed by refresh
                landed: None,
                explored: *explored,
                blockers: blockers.iter().copied().collect(),
                not_before: *not_before,
                paused: false,
                failures: 0,
                attempts: BTreeMap::new(),
                decision: None,
                cancel_keeps_branch: true,
            };
            w.tasks.insert(id, task);
            fx.push(Effect::TaskCreated { task: id });
        }
        E::Approved { task } => {
            let from = get(w, *task)?.state;
            if from != S::Proposed {
                return Err(illegal(from, k));
            }
            w.tasks.get_mut(task).expect("exists").state = S::Queued;
        }
        E::Rejected { task } => {
            let from = get(w, *task)?.state;
            if from != S::Proposed {
                return Err(illegal(from, k));
            }
            w.tasks.get_mut(task).expect("exists").state = S::Canceled;
        }
        E::Admitted { task } => {
            let t = get(w, *task)?;
            if t.state != S::Queued {
                return Err(illegal(t.state, k));
            }
            if t.activity != Activity::Waiting(WaitReason::Slot) {
                return Err(ReduceError::NotClaimable(t.activity.clone()));
            }
            let to = if t.explored {
                S::Developing
            } else {
                S::Exploring
            };
            w.enter(*task, to, fx);
        }
        E::StepSucceeded { task } => {
            let from = get(w, *task)?.state;
            let next = match from {
                S::Exploring => S::Developing,
                S::Developing => S::Testing,
                S::Testing => S::Review,
                S::Review => S::Landing,
                S::Landing => S::Landed,
                _ => return Err(illegal(from, k)),
            };
            w.leave(*task, StepOutcome::Succeeded);
            fx.push(Effect::ReleaseLeases { task: *task });
            if next == S::Landed {
                let t = w.tasks.get_mut(task).expect("exists");
                t.state = S::Landed;
                t.landed = Some(LandedOutcome::Merged);
                w.settle(*task);
            } else {
                w.enter(*task, next, fx);
            }
        }
        E::SentBack { task, .. } => {
            let from = get(w, *task)?.state;
            if !matches!(from, S::Testing | S::Review | S::Landing) {
                return Err(illegal(from, k));
            }
            w.leave(*task, StepOutcome::SentBack);
            fx.push(Effect::ReleaseLeases { task: *task });
            w.enter(*task, S::Developing, fx);
        }
        E::ExplorationRequested { task, .. } => {
            let from = get(w, *task)?.state;
            if from != S::Developing {
                return Err(illegal(from, k));
            }
            w.leave(*task, StepOutcome::ExplorationRequested);
            fx.push(Effect::ReleaseLeases { task: *task });
            w.enter(*task, S::Exploring, fx); // no failure charged
        }
        E::QuestionAsked { task, question } => {
            let from = get(w, *task)?.state;
            if !matches!(from, S::Exploring | S::Developing | S::Testing | S::Landing) {
                return Err(illegal(from, k));
            }
            w.leave(*task, StepOutcome::Asked);
            fx.push(Effect::ReleaseLeases { task: *task });
            w.waits.remove(task);
            let id = DecisionId(w.next_decision);
            w.next_decision += 1;
            w.decisions.insert(
                id,
                Decision {
                    id,
                    task: *task,
                    question: question.clone(),
                    answer: None,
                },
            );
            let t = w.tasks.get_mut(task).expect("exists");
            t.state = S::NeedsAttention;
            t.decision = Some(id);
            fx.push(Effect::AskOwner {
                task: *task,
                decision: id,
            });
        }
        E::Answered {
            task,
            decision,
            answer,
        } => {
            let t = get(w, *task)?;
            if t.state != S::NeedsAttention {
                return Err(illegal(t.state, k));
            }
            if t.decision != Some(*decision) {
                return Err(ReduceError::StaleAnswer);
            }
            w.decisions
                .get_mut(decision)
                .ok_or(ReduceError::StaleAnswer)?
                .answer = Some(answer.clone());
            w.tasks.get_mut(task).expect("exists").decision = None;
            w.enter(*task, S::Developing, fx);
        }
        E::StepFailed { task, .. } => {
            let from = get(w, *task)?.state;
            if !from.is_step() {
                return Err(illegal(from, k));
            }
            wind_down(w, *task, StepOutcome::Failed, fx);
            let t = w.tasks.get_mut(task).expect("exists");
            t.state = S::Failed;
            t.failures += 1;
        }
        E::Retried { task, to } => {
            let from = get(w, *task)?.state;
            if !(from.is_step() || matches!(from, S::NeedsAttention | S::Failed)) {
                return Err(illegal(from, k));
            }
            if !to.is_step() {
                return Err(ReduceError::BadRetryTarget(*to));
            }
            if w.leave(*task, StepOutcome::Superseded) {
                fx.push(Effect::StopStepHost { task: *task });
            }
            fx.push(Effect::ReleaseLeases { task: *task });
            w.waits.remove(task);
            w.tasks.get_mut(task).expect("exists").decision = None;
            fx.push(Effect::CatchUpWithBase { task: *task });
            w.enter(*task, *to, fx);
        }
        E::Canceled {
            task,
            delete_branch,
        } => {
            let from = get(w, *task)?.state;
            if !(matches!(from, S::Proposed | S::Queued | S::NeedsAttention) || from.is_step()) {
                return Err(illegal(from, k));
            }
            wind_down(w, *task, StepOutcome::Canceled, fx);
            if *delete_branch {
                fx.push(Effect::DeleteBranch { task: *task });
            }
            let t = w.tasks.get_mut(task).expect("exists");
            t.state = S::Canceled;
            t.decision = None;
            t.cancel_keeps_branch = !*delete_branch;
        }
        E::ClosedNoChange { task, .. } => {
            let from = get(w, *task)?.state;
            if !(matches!(from, S::Queued | S::NeedsAttention) || from.is_step()) {
                return Err(illegal(from, k));
            }
            wind_down(w, *task, StepOutcome::Succeeded, fx);
            let t = w.tasks.get_mut(task).expect("exists");
            t.state = S::Landed;
            t.landed = Some(LandedOutcome::ClosedNoChange);
            t.decision = None;
        }
        E::Paused { task } => {
            let t = get(w, *task)?;
            if t.paused || !(matches!(t.state, S::Queued | S::NeedsAttention) || t.state.is_step())
            {
                return Err(illegal(t.state, k));
            }
            w.tasks.get_mut(task).expect("exists").paused = true;
        }
        E::Resumed { task } => {
            let t = get(w, *task)?;
            if !t.paused {
                return Err(illegal(t.state, k));
            }
            w.tasks.get_mut(task).expect("exists").paused = false;
        }
        E::WaitStarted { task, on } => {
            let from = get(w, *task)?.state;
            if !from.is_step() {
                return Err(illegal(from, k));
            }
            let since = w.now;
            w.waits.insert(
                *task,
                Wait {
                    task: *task,
                    on: on.clone(),
                    since,
                },
            );
        }
        E::WaitEnded { task } => {
            get(w, *task)?;
            if w.waits.remove(task).is_none() {
                return Err(ReduceError::NoSuchWait);
            }
        }
        E::BlockerAdded { task, blocker } => {
            let from = get(w, *task)?.state;
            get(w, *blocker)?;
            if !matches!(from, S::Proposed | S::Queued) {
                return Err(illegal(from, k));
            }
            if would_cycle(w, *task, *blocker) {
                return Err(ReduceError::BlockerCycle);
            }
            w.tasks
                .get_mut(task)
                .expect("exists")
                .blockers
                .insert(*blocker);
        }
        E::Tick => {}
    }
    Ok(())
}
