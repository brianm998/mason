// SPDX-License-Identifier: GPL-3.0-or-later

use crate::{DecisionId, ProjectId, TaskId, Timestamp};
use std::collections::{BTreeMap, BTreeSet};

/// The eleven fixed state categories (Part 4). `Landed` carries its outcome
/// in [`Task::landed`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum State {
    Proposed,
    Queued,
    Exploring,
    Developing,
    Testing,
    Review,
    Landing,
    NeedsAttention,
    Failed,
    Canceled,
    Landed,
}

impl State {
    pub const ALL: [State; 11] = [
        State::Proposed,
        State::Queued,
        State::Exploring,
        State::Developing,
        State::Testing,
        State::Review,
        State::Landing,
        State::NeedsAttention,
        State::Failed,
        State::Canceled,
        State::Landed,
    ];

    /// States in which a step is running (or may run) for the task.
    pub fn is_step(self) -> bool {
        matches!(
            self,
            State::Exploring | State::Developing | State::Testing | State::Review | State::Landing
        )
    }

    /// No further transitions except none at all.
    pub fn is_terminal(self) -> bool {
        matches!(self, State::Landed | State::Canceled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LandedOutcome {
    Merged,
    ClosedNoChange,
}

/// What a task is doing right now, separate from where it is in the graph.
/// Exactly one is true at any time; the reducer recomputes it after every
/// event, in the same transaction as the state change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Activity {
    /// Nothing to do: proposed, landed, failed or canceled.
    Idle,
    Running,
    Waiting(WaitReason),
    /// Blocked by unlanded tasks.
    Held(BTreeSet<TaskId>),
    Paused,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitReason {
    /// Queued, waiting to be claimed.
    Slot,
    NotBefore(Timestamp),
    /// Needs an answer from the owner.
    Owner,
    /// A named resource, such as "compile slot".
    On(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Urgent,
    High,
    Normal,
    Low,
}

/// Who filed a task. Outside text is untrusted (Part 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustSource {
    Owner,
    OwnerAgent,
    Outside,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub id: TaskId,
    pub project: ProjectId,
    pub title: String,
    pub priority: Priority,
    pub trust: TrustSource,
    pub enqueued_at: Timestamp,
    pub state: State,
    pub activity: Activity,
    pub landed: Option<LandedOutcome>,
    /// Filer declared exploration already done.
    pub explored: bool,
    /// Unlanded blockers. A queued task with any is held.
    pub blockers: BTreeSet<TaskId>,
    pub not_before: Option<Timestamp>,
    pub paused: bool,
    /// Failures charged to the task. A request to explore again charges none.
    pub failures: u32,
    /// Attempts started per step state.
    pub attempts: BTreeMap<State, u32>,
    pub decision: Option<DecisionId>,
    pub cancel_keeps_branch: bool,
}
