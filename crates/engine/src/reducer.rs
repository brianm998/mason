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
    /// Adding the blocker would close a loop; the path starts and ends at the task.
    BlockerCycle(Vec<TaskId>),
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
        ExploreAnyway { .. } => "ExploreAnyway",
        Canceled { .. } => "Canceled",
        ClosedNoChange { .. } => "ClosedNoChange",
        Paused { .. } => "Paused",
        Resumed { .. } => "Resumed",
        WaitStarted { .. } => "WaitStarted",
        WaitEnded { .. } => "WaitEnded",
        BlockerAdded { .. } => "BlockerAdded",
        BlockerRemoved { .. } => "BlockerRemoved",
        TrackerEdited { .. } => "TrackerEdited",
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
    let state_before: BTreeMap<TaskId, State> = w.tasks.values().map(|t| (t.id, t.state)).collect();
    apply(&mut w, &event.kind, &mut fx)?;
    // The only writer of `state_entered_at`: any task whose state differs from
    // before (or is new) entered its present state now.
    let now = w.now;
    for t in w.tasks.values_mut() {
        if state_before.get(&t.id) != Some(&t.state) {
            t.state_entered_at = now;
        }
    }
    for task in w.refresh(&before) {
        fx.push(Effect::WakeScheduler { task });
    }
    Ok((w, fx))
}

/// How far along the graph a category is. A step may be followed by one in the
/// same category or a later one; going back is only by sending work back.
fn order(s: State) -> u8 {
    match s {
        State::Exploring => 0,
        State::Developing => 1,
        State::Testing => 2,
        State::Review => 3,
        State::Landing => 4,
        _ => 99,
    }
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

/// The path `task -> blocker -> ... -> task` that adding the edge would close, if any.
fn would_cycle(w: &World, task: TaskId, blocker: TaskId) -> Option<Vec<TaskId>> {
    let mut seen = BTreeSet::new();
    // Each stack entry is the path from `blocker` to the node under test.
    let mut stack = vec![vec![blocker]];
    while let Some(path) = stack.pop() {
        let n = *path.last().expect("never empty");
        if n == task {
            let mut full = vec![task];
            full.extend(path);
            return Some(full);
        }
        if seen.insert(n) {
            if let Some(t) = w.tasks.get(&n) {
                for b in &t.blockers {
                    let mut p = path.clone();
                    p.push(*b);
                    stack.push(p);
                }
            }
        }
    }
    None
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
                state_entered_at: w.now,
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
                failure: None,
                def: String::new(),
                step: String::new(),
                step_attempts: BTreeMap::new(),
                route: None,
                explore_returns: 0,
                approved: false,
                force_explore: false,
            };
            w.tasks.insert(id, task);
            fx.push(Effect::TaskCreated { task: id });
        }
        E::Approved { task } => {
            let from = get(w, *task)?.state;
            if from != S::Proposed {
                return Err(illegal(from, k));
            }
            let t = w.tasks.get_mut(task).expect("exists");
            t.state = S::Queued;
            t.approved = true;
        }
        E::Rejected { task } => {
            let from = get(w, *task)?.state;
            if from != S::Proposed {
                return Err(illegal(from, k));
            }
            w.tasks.get_mut(task).expect("exists").state = S::Canceled;
        }
        E::Admitted { task, entry } => {
            let t = get(w, *task)?;
            if t.state != S::Queued {
                return Err(illegal(t.state, k));
            }
            if t.activity != Activity::Waiting(WaitReason::Slot) {
                return Err(ReduceError::NotClaimable(t.activity.clone()));
            }
            let (to, name, route, def) = match entry {
                Some(e) => {
                    // The entry must be a step, and skipping exploring must not start in it.
                    if !e.state.is_step()
                        || (e.route.skipped_exploring() && e.state == S::Exploring)
                    {
                        return Err(illegal(t.state, k));
                    }
                    (e.state, e.step.clone(), e.route, e.def.clone())
                }
                None if t.explored_counts() => {
                    (S::Developing, String::new(), Route::Flag, String::new())
                }
                None => (S::Exploring, String::new(), Route::Explore, String::new()),
            };
            let tm = w.tasks.get_mut(task).expect("exists");
            tm.route = Some(route);
            tm.def = def;
            w.enter(*task, to, &name, fx);
        }
        E::StepSucceeded { task, next } => {
            let from = get(w, *task)?.state;
            let (target, name) = match next {
                Some(n) => {
                    let ok = if n.state == S::Landed {
                        from == S::Landing
                    } else {
                        from.is_step() && n.state.is_step() && order(n.state) >= order(from)
                    };
                    if !ok {
                        return Err(illegal(from, k));
                    }
                    (n.state, n.step.clone())
                }
                None => {
                    let t = match from {
                        S::Exploring => S::Developing,
                        S::Developing => S::Testing,
                        S::Testing => S::Review,
                        S::Review => S::Landing,
                        S::Landing => S::Landed,
                        _ => return Err(illegal(from, k)),
                    };
                    (t, String::new())
                }
            };
            w.leave(*task, StepOutcome::Succeeded);
            fx.push(Effect::ReleaseLeases { task: *task });
            if target == S::Landed {
                let t = w.tasks.get_mut(task).expect("exists");
                t.state = S::Landed;
                t.landed = Some(LandedOutcome::Merged);
                w.settle(*task);
            } else {
                w.enter(*task, target, &name, fx);
            }
        }
        E::SentBack { task, to_step, .. } => {
            let from = get(w, *task)?.state;
            if !matches!(from, S::Testing | S::Review | S::Landing) {
                return Err(illegal(from, k));
            }
            w.leave(*task, StepOutcome::SentBack);
            fx.push(Effect::ReleaseLeases { task: *task });
            w.enter(*task, S::Developing, to_step, fx);
        }
        E::ExplorationRequested { task, to_step, .. } => {
            let from = get(w, *task)?.state;
            if from != S::Developing {
                return Err(illegal(from, k));
            }
            w.leave(*task, StepOutcome::ExplorationRequested);
            fx.push(Effect::ReleaseLeases { task: *task });
            w.tasks.get_mut(task).expect("exists").explore_returns += 1;
            w.enter(*task, S::Exploring, to_step, fx); // no failure charged
        }
        E::ExploreAnyway { task, step } => {
            let t = get(w, *task)?;
            if t.state == S::Queued {
                w.tasks.get_mut(task).expect("exists").force_explore = true;
            } else if t.state.is_step() && t.route.is_some_and(Route::skipped_exploring) {
                if w.leave(*task, StepOutcome::Superseded) {
                    fx.push(Effect::StopStepHost { task: *task });
                }
                fx.push(Effect::ReleaseLeases { task: *task });
                w.waits.remove(task);
                w.tasks.get_mut(task).expect("exists").route = Some(Route::ExploreAnyway);
                w.enter(*task, S::Exploring, step, fx);
            } else {
                return Err(illegal(t.state, k));
            }
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
            step,
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
            w.enter(*task, S::Developing, step, fx);
        }
        E::StepFailed { task, reason } => {
            let from = get(w, *task)?.state;
            if !from.is_step() {
                return Err(illegal(from, k));
            }
            wind_down(w, *task, StepOutcome::Failed, fx);
            let t = w.tasks.get_mut(task).expect("exists");
            t.state = S::Failed;
            t.failures += 1;
            t.failure = Some(reason.clone());
        }
        E::Retried { task, to, step } => {
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
            w.tasks.get_mut(task).expect("exists").failure = None;
            w.tasks.get_mut(task).expect("exists").decision = None;
            fx.push(Effect::CatchUpWithBase { task: *task });
            w.enter(*task, *to, step, fx);
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
            if let Some(path) = would_cycle(w, *task, *blocker) {
                return Err(ReduceError::BlockerCycle(path));
            }
            w.tasks
                .get_mut(task)
                .expect("exists")
                .blockers
                .insert(*blocker);
        }
        E::BlockerRemoved { task, blocker } => {
            // A task already past queued is not pulled back: its blockers were
            // satisfied (or ignored) when it started.
            let from = get(w, *task)?.state;
            if !matches!(from, S::Proposed | S::Queued) {
                return Err(illegal(from, k));
            }
            w.tasks
                .get_mut(task)
                .expect("exists")
                .blockers
                .remove(blocker);
        }
        E::TrackerEdited {
            task,
            title,
            body,
            priority,
        } => {
            let from = get(w, *task)?.state;
            if !(from.is_step() || matches!(from, S::Proposed | S::Queued | S::NeedsAttention)) {
                return Err(illegal(from, k));
            }
            let t = w.tasks.get_mut(task).expect("exists");
            t.title = title.clone();
            t.body = body.clone();
            t.priority = *priority;
        }
        E::Tick => {}
    }
    Ok(())
}
