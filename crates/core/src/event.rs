// SPDX-License-Identifier: GPL-3.0-or-later

use crate::*;

/// Where a task starts, and why. Chosen by the daemon from the task's
/// definition; the reducer records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Hash of the definition the task is pinned to from now on.
    pub def: String,
    pub step: String,
    pub state: State,
    pub route: Route,
}

/// The step that follows. `state` is its category; `Landed` ends the task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Next {
    pub step: String,
    pub state: State,
}

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
    /// A queued task was claimed. `entry` says where it starts, under which
    /// definition, and why exploring was or was not skipped. `None` is the
    /// older fixed behaviour: `exploring`, or `developing` when flagged explored.
    Admitted {
        task: TaskId,
        entry: Option<Entry>,
    },
    /// The current step finished successfully. `next` names the step that
    /// follows (its category is `state`; `Landed` ends the task). `None` is the
    /// older fixed progression through the categories.
    StepSucceeded {
        task: TaskId,
        next: Option<Next>,
    },
    /// Red tests, review changes, or a landing conflict or blame. Returns to
    /// the developing step named `to_step` (empty: the first one).
    SentBack {
        task: TaskId,
        reason: String,
        to_step: String,
    },
    /// The developing agent found the task doesn't hold up. No failure charged.
    /// Returns to the exploring step named `to_step` (empty: the first one).
    ExplorationRequested {
        task: TaskId,
        reason: String,
        to_step: String,
    },
    /// "Explore anyway": the owner wants exploring after all, either before the
    /// task starts or after it skipped it. Not a failure.
    ExploreAnyway {
        task: TaskId,
        step: String,
    },
    QuestionAsked {
        task: TaskId,
        question: String,
    },
    Answered {
        task: TaskId,
        decision: DecisionId,
        answer: String,
        /// The developing step to resume in (empty: the first one).
        step: String,
    },
    StepFailed {
        task: TaskId,
        reason: String,
        /// Whether the failure is about the code (`None`: not classified).
        class: Option<FailClass>,
    },
    /// Re-enter a step. Brings the branch up to date with the base first.
    Retried {
        task: TaskId,
        to: State,
        /// The step to re-enter (empty: the first one of that category).
        step: String,
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
        /// What was waited for, for totals (see `wait_class`). Absent in old logs.
        class: Option<String>,
    },
    WaitEnded {
        task: TaskId,
    },
    BlockerAdded {
        task: TaskId,
        blocker: TaskId,
    },
    /// A blocker was withdrawn (the tracker's dependency was removed). Only a
    /// proposed or queued task takes it; a started one is not pulled back.
    BlockerRemoved {
        task: TaskId,
        blocker: TaskId,
    },
    /// The owner released some blockers of a proposed or queued task and/or
    /// added others in one step. Released ids must be current blockers.
    BlockersChanged {
        task: TaskId,
        released: Vec<TaskId>,
        added: Vec<TaskId>,
    },
    /// The owner put a cancelled task (whose branch was kept) back in the queue.
    Revived {
        task: TaskId,
    },
    /// The tracker, which owns a task's title, body and priority, said they
    /// changed (Part 11: authority is split). Applies in any live state.
    TrackerEdited {
        task: TaskId,
        title: String,
        body: String,
        priority: Priority,
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
