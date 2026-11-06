// SPDX-License-Identifier: GPL-3.0-or-later

use crate::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepOutcome {
    Succeeded,
    /// Sent back to development (red tests, review changes, conflict).
    SentBack,
    Failed,
    Asked,
    ExplorationRequested,
    Superseded,
    Canceled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepRun {
    pub id: StepRunId,
    pub task: TaskId,
    pub step: State,
    pub attempt: u32,
    pub started_at: Timestamp,
    pub outcome: Option<StepOutcome>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wait {
    pub task: TaskId,
    pub on: String,
    pub since: Timestamp,
}

/// A question for the owner. Its id is the freshness token for answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub id: DecisionId,
    pub task: TaskId,
    pub question: String,
    pub answer: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerdictKind {
    Unproven,
    Unverified,
    Untested,
    Verified,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub id: VerdictId,
    pub task: TaskId,
    pub kind: VerdictKind,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    pub id: BatchId,
    pub project: ProjectId,
    pub tasks: Vec<TaskId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    pub id: ProposalId,
    pub project: ProjectId,
    pub title: String,
    pub evidence: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alert {
    pub id: AlertId,
    pub condition: String,
    pub raised_at: Timestamp,
    pub cleared_at: Option<Timestamp>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Override {
    pub id: OverrideId,
    pub action: String,
    pub at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Debt {
    pub id: DebtId,
    pub task: TaskId,
    pub what: String,
    pub paid: bool,
}
