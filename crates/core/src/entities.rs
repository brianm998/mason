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
    /// The step's category.
    pub step: State,
    /// The step's name in the definition (empty for runs from older logs).
    pub name: String,
    pub attempt: u32,
    pub started_at: Timestamp,
    pub outcome: Option<StepOutcome>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wait {
    pub task: TaskId,
    pub on: String,
    /// As recorded with the wait; `None` for a wait from an old log.
    pub class: Option<String>,
    pub since: Timestamp,
}

impl Wait {
    /// What this wait was for: the recorded class, or one read off the text.
    pub fn class_name(&self) -> String {
        wait_class(&self.on, self.class.as_deref())
    }
}

/// The class a wait is totalled under. A recorded class wins; an old event has none, so one
/// is read off the start of its text, with `other` when nothing matches.
pub fn wait_class(on: &str, recorded: Option<&str>) -> String {
    if let Some(c) = recorded.filter(|c| !c.is_empty()) {
        return c.to_string();
    }
    let has = |p: &str| on.starts_with(p);
    if let Some(r) = on.strip_prefix("Waiting for LLM session (") {
        if let Some(a) = r.split(')').next() {
            return format!("llm:{a}");
        }
    }
    if has("Waiting for the machine") {
        return "exclusive".into();
    }
    if has("Waiting for free memory") {
        return "memory".into();
    }
    if has("Waiting: disk has") {
        return "disk-floor".into();
    }
    if has("compile waiting: CPU") || has("compile waiting: letting the last compile") {
        return "cpu".into();
    }
    if has("compile waiting: the build disk") {
        return "disk-stall".into();
    }
    if has("Granted, but waiting for the kernel lock") {
        return "outside-pipeline".into();
    }
    if has("paused by ") {
        return "pause".into();
    }
    if on.contains("` is red on ") && on.contains(" too: ") {
        return "base-red".into();
    }
    if has("environment problem") || has("git is busy") {
        return "environment".into();
    }
    if has("waiting for your approval") || on == crate::APPROVAL_WAIT {
        return "approval".into();
    }
    if let Some(r) = on.strip_prefix("Waiting for ") {
        if let Some(n) = r.split(" slot").next().filter(|_| r.contains(" slot")) {
            return n.to_string();
        }
    }
    "other".into()
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

#[cfg(test)]
mod wait_class_tests {
    use super::wait_class;

    #[test]
    fn a_recorded_class_wins_and_old_text_is_read_by_its_start() {
        assert_eq!(wait_class("anything", Some("compile")), "compile");
        for (text, class) in [
            (
                "Waiting for compile slot: 2nd in line, held by #1 for 4 min",
                "compile",
            ),
            ("Waiting for test-suite slot: 1st in line", "test-suite"),
            (
                "Waiting for LLM session (claude): 10th in line",
                "llm:claude",
            ),
            ("Waiting for the machine: 1st in line", "exclusive"),
            ("compile waiting: CPU 71% > 50%", "cpu"),
            (
                "compile waiting: the build disk took 3.0 s to answer (limit 1.0 s)",
                "disk-stall",
            ),
            ("Waiting: disk has 3 GB free, floor 10 GB", "disk-floor"),
            ("paused by owner (all for everything)", "pause"),
            ("git is busy or not answering (fetch): x", "environment"),
            (
                "`compose-clients` is red on 812a7849ab too: error: x",
                "base-red",
            ),
            ("something new", "other"),
        ] {
            assert_eq!(wait_class(text, None), class, "{text}");
        }
    }
}
