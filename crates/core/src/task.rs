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

/// What a task waits on while it sits in an approval step. Such a wait can last
/// days, so it does not hold one of the daemon's running slots.
pub const APPROVAL_WAIT: &str = "approval by the owner";

/// Why a task started where it did (Part 4, "Skipping exploration").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// Exploring happens: the usual route.
    Explore,
    /// Skipped: the filer, trusted, declared the task already explored.
    Flag,
    /// Skipped: the deterministic clarity check passed.
    Clarity,
    /// Skipped: the project's optional classifier said the task is clear.
    Classifier,
    /// The owner asked for exploring after all.
    ExploreAnyway,
}

impl Route {
    pub fn name(self) -> &'static str {
        match self {
            Route::Explore => "explore",
            Route::Flag => "flag",
            Route::Clarity => "clarity",
            Route::Classifier => "classifier",
            Route::ExploreAnyway => "explore_anyway",
        }
    }

    pub fn from_name(s: &str) -> Option<Route> {
        [
            Route::Explore,
            Route::Flag,
            Route::Clarity,
            Route::Classifier,
            Route::ExploreAnyway,
        ]
        .into_iter()
        .find(|r| r.name() == s)
    }

    /// Did this route skip exploring?
    pub fn skipped_exploring(self) -> bool {
        matches!(self, Route::Flag | Route::Clarity | Route::Classifier)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub id: TaskId,
    pub project: ProjectId,
    pub title: String,
    pub body: String,
    pub priority: Priority,
    pub trust: TrustSource,
    pub enqueued_at: Timestamp,
    /// When the task entered its present state (set at filing and on every
    /// state change; a retry within the same state does not move it).
    pub state_entered_at: Timestamp,
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
    /// Why the task last failed, until it is retried.
    pub failure: Option<String>,
    /// What kind of failure it was, when it was recorded (`None`: recorded before
    /// failures were classified).
    pub failure_class: Option<FailClass>,
    /// Hash of the definition the task is pinned to (set when it starts).
    pub def: String,
    /// The step it is in (or last was in).
    pub step: String,
    /// Attempts per step name.
    pub step_attempts: BTreeMap<String, u32>,
    /// Why it started where it did (set when it starts).
    pub route: Option<Route>,
    /// How many times the developing agent sent it back to exploring.
    pub explore_returns: u32,
    /// The owner approved it (matters for tasks from outside).
    pub approved: bool,
    /// "Explore anyway" was asked before the task started.
    pub force_explore: bool,
}

impl Task {
    /// Does the `explored` flag count? Only for tasks from the owner or the
    /// owner's agents. On anything from outside it is ignored until the owner
    /// approves the task (Part 4).
    pub fn explored_counts(&self) -> bool {
        self.explored && (self.trust != TrustSource::Outside || self.approved)
    }
}

/// What a failure says about the change, decided when the failure is recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FailClass {
    /// About the code: red rounds used up, a weakening, an agent failure, a base conflict.
    Code,
    /// Not about the code: a landing push, fetch or auth failure, a CI service error,
    /// a lost host. Nothing was judged wrong, so a retry resumes at the failed step.
    Outside,
}

impl FailClass {
    pub fn name(self) -> &'static str {
        match self {
            FailClass::Code => "code",
            FailClass::Outside => "outside",
        }
    }

    pub fn parse(s: &str) -> Option<FailClass> {
        match s {
            "code" => Some(FailClass::Code),
            "outside" => Some(FailClass::Outside),
            _ => None,
        }
    }

    /// Classify the failure of a step of `state` with this `reason`. A failure that is not about
    /// the code is `Outside`: landing's "could not land" verdict (push, fetch, auth, CI service,
    /// services unusable), a lost command, a lost or never-started step host, a fetch that timed
    /// out, an agent CLI that exited with no result. Red rounds, weakenings, conflicts and agent
    /// submissions are about the code, as is anything this does not name.
    pub fn classify(state: State, reason: &str) -> FailClass {
        let r = reason;
        let outside = (state == State::Landing && r.starts_with("landing failed: "))
            || r.contains("the command was lost")
            || r.contains("the step host was lost")
            || r.contains("step host did not start")
            || r.starts_with("fetching the remote failed")
            || (r.contains("git fetch") && r.contains("timed out"))
            || (r.starts_with("the agent failed: the CLI exited with status")
                && r.contains("and no result"))
            || (r.contains("mason/ci/") && (r.contains("push") || r.contains("refused")));
        if outside {
            FailClass::Outside
        } else {
            FailClass::Code
        }
    }
}

#[cfg(test)]
mod fail_class_tests {
    use super::*;

    #[test]
    fn classes() {
        let push = "landing failed: could not push mason/ci/x: Permission denied";
        assert_eq!(
            FailClass::classify(State::Landing, push),
            FailClass::Outside
        );
        assert_eq!(
            FailClass::classify(State::Testing, "still red after 3 rounds"),
            FailClass::Code
        );
        assert_eq!(
            FailClass::classify(State::Testing, "the command was lost (its host died)"),
            FailClass::Outside
        );
        // The reasons the event log records for failures outside the code (s247).
        for outside in [
            "fetching the remote failed (git fetch --prune origin timed out after 120s) and there is no last-known origin/develop to work from",
            "the step host was lost 3 times",
            "the agent failed: the step host was lost before the agent finished",
            "could not start the step: Spawn(TimedOut) step host did not start within 30s",
            "the agent failed: the CLI exited with status 1 and no result",
            "could not push mason/ci/t12 to origin: refused",
        ] {
            assert_eq!(
                FailClass::classify(State::Developing, outside),
                FailClass::Outside,
                "{outside}"
            );
        }
        // And what stays about the code, whatever step it came from.
        for code in [
            "still red after 3 rounds",
            "suite `x` is red",
            "the branch conflicts with origin/develop in: a.rs; still conflicting with the base after 3 merges",
            "the change weakens assertions and does not say so",
            "the agent failed: the agent submitted nothing",
            "the agent failed: the CLI exited with status 1 and said: rate limited",
        ] {
            assert_eq!(
                FailClass::classify(State::Testing, code),
                FailClass::Code,
                "{code}"
            );
        }
        assert_eq!(FailClass::parse("outside"), Some(FailClass::Outside));
        assert_eq!(FailClass::parse("x"), None);
    }
}
