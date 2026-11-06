// SPDX-License-Identifier: GPL-3.0-or-later

use crate::*;

/// Every input to the reducer. Time rides along in `at`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub at: Timestamp,
    pub kind: EventKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventKind {
    ProjectRegistered {
        project: ProjectId,
        name: String,
    },
    /// File a task. The reducer allocates its id (see [`Effect::TaskCreated`]).
    TaskFiled {
        project: ProjectId,
        title: String,
        /// The task text from the tracker. Untrusted when `trust` is `Outside`.
        body: String,
        priority: Priority,
        trust: TrustSource,
        explored: bool,
        /// `true` files it as `proposed`, waiting for the owner.
        proposed: bool,
        blockers: Vec<TaskId>,
        not_before: Option<Timestamp>,
    },
    Approved {
        task: TaskId,
    },
    Rejected {
        task: TaskId,
    },
    /// A queued task was claimed. Goes to `exploring`, or straight to
    /// `developing` when the task is flagged explored.
    Admitted {
        task: TaskId,
    },
    /// The current step finished successfully; moves to the next state.
    StepSucceeded {
        task: TaskId,
    },
    /// Red tests, review changes, or a landing conflict or blame.
    SentBack {
        task: TaskId,
        reason: String,
    },
    /// The developing agent found the task doesn't hold up. No failure charged.
    ExplorationRequested {
        task: TaskId,
        reason: String,
    },
    QuestionAsked {
        task: TaskId,
        question: String,
    },
    Answered {
        task: TaskId,
        decision: DecisionId,
        answer: String,
    },
    StepFailed {
        task: TaskId,
        reason: String,
    },
    /// Re-enter a step. Brings the branch up to date with the base first.
    Retried {
        task: TaskId,
        to: State,
    },
    Canceled {
        task: TaskId,
        delete_branch: bool,
    },
    /// Landed with no change: already fixed, or a duplicate.
    ClosedNoChange {
        task: TaskId,
        reason: String,
    },
    Paused {
        task: TaskId,
    },
    Resumed {
        task: TaskId,
    },
    WaitStarted {
        task: TaskId,
        on: String,
    },
    WaitEnded {
        task: TaskId,
    },
    BlockerAdded {
        task: TaskId,
        blocker: TaskId,
    },
    /// Time passing, so not-before times can elapse.
    Tick,
}

/// What the reducer asks the outside world to do. Effects run on workers and
/// report back as new events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    TaskCreated {
        task: TaskId,
    },
    StartStep {
        task: TaskId,
        step: State,
        run: StepRunId,
        attempt: u32,
    },
    StopStepHost {
        task: TaskId,
    },
    ReleaseLeases {
        task: TaskId,
    },
    CatchUpWithBase {
        task: TaskId,
    },
    DeleteBranch {
        task: TaskId,
    },
    AskOwner {
        task: TaskId,
        decision: DecisionId,
    },
    /// A queued task became claimable.
    WakeScheduler {
        task: TaskId,
    },
}
