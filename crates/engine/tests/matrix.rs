// SPDX-License-Identifier: GPL-3.0-or-later
//! Every (state, event) pair: legal exactly where Part 4's diagram says so,
//! landing in the right state, and leaving the world untouched when illegal.

mod common;
use common::*;
use mason_core::*;
use mason_engine::reduce;
use State as S;

fn events(t: TaskId, other: TaskId, decision: DecisionId) -> Vec<(&'static str, EventKind)> {
    use EventKind as E;
    vec![
        ("Approved", E::Approved { task: t }),
        ("Rejected", E::Rejected { task: t }),
        (
            "Admitted",
            E::Admitted {
                task: t,
                entry: None,
            },
        ),
        (
            "StepSucceeded",
            E::StepSucceeded {
                task: t,
                next: None,
            },
        ),
        (
            "SentBack",
            E::SentBack {
                task: t,
                reason: "r".into(),
                to_step: String::new(),
            },
        ),
        (
            "ExplorationRequested",
            E::ExplorationRequested {
                task: t,
                reason: "r".into(),
                to_step: String::new(),
            },
        ),
        (
            "QuestionAsked",
            E::QuestionAsked {
                task: t,
                question: "q".into(),
            },
        ),
        (
            "Answered",
            E::Answered {
                task: t,
                decision,
                answer: "a".into(),
                step: String::new(),
            },
        ),
        (
            "StepFailed",
            E::StepFailed {
                task: t,
                reason: "r".into(),
            },
        ),
        (
            "Retried",
            E::Retried {
                task: t,
                to: S::Testing,
                step: String::new(),
            },
        ),
        (
            "Canceled",
            E::Canceled {
                task: t,
                delete_branch: false,
            },
        ),
        (
            "ClosedNoChange",
            E::ClosedNoChange {
                task: t,
                reason: "dup".into(),
            },
        ),
        ("Paused", E::Paused { task: t }),
        (
            "ExploreAnyway",
            E::ExploreAnyway {
                task: t,
                step: "explore".into(),
            },
        ),
        ("Resumed", E::Resumed { task: t }),
        (
            "WaitStarted",
            E::WaitStarted {
                task: t,
                on: "slot".into(),
                class: None,
            },
        ),
        ("WaitEnded", E::WaitEnded { task: t }),
        (
            "BlockerAdded",
            E::BlockerAdded {
                task: t,
                blocker: other,
            },
        ),
        (
            "BlockerRemoved",
            E::BlockerRemoved {
                task: t,
                blocker: other,
            },
        ),
        (
            "BlockersChanged",
            E::BlockersChanged {
                task: t,
                released: vec![],
                added: vec![other],
            },
        ),
        ("Revived", E::Revived { task: t }),
        (
            "TrackerEdited",
            E::TrackerEdited {
                task: t,
                title: "new title".into(),
                body: "new body".into(),
                priority: Priority::High,
            },
        ),
        ("Tick", E::Tick),
    ]
}

/// The diagram, written out independently of the reducer.
/// `Some(state)` means legal and ends there.
fn expected(from: State, ev: &str) -> Option<State> {
    let step = from.is_step();
    let live = step || matches!(from, S::Queued | S::NeedsAttention);
    match ev {
        "Approved" if from == S::Proposed => Some(S::Queued),
        "Rejected" if from == S::Proposed => Some(S::Canceled),
        "Admitted" if from == S::Queued => Some(S::Exploring),
        "StepSucceeded" => match from {
            S::Exploring => Some(S::Developing),
            S::Developing => Some(S::Testing),
            S::Testing => Some(S::Review),
            S::Review => Some(S::Landing),
            S::Landing => Some(S::Landed),
            _ => None,
        },
        "SentBack" if matches!(from, S::Testing | S::Review | S::Landing) => Some(S::Developing),
        "ExplorationRequested" if from == S::Developing => Some(S::Exploring),
        "QuestionAsked"
            if matches!(from, S::Exploring | S::Developing | S::Testing | S::Landing) =>
        {
            Some(S::NeedsAttention)
        }
        "Answered" if from == S::NeedsAttention => Some(S::Developing),
        "StepFailed" if step => Some(S::Failed),
        "Retried" if step || matches!(from, S::NeedsAttention | S::Failed) => Some(S::Testing),
        "Canceled" if live || from == S::Proposed => Some(S::Canceled),
        "ClosedNoChange" if live => Some(S::Landed),
        "Paused" if live => Some(from),
        "ExploreAnyway" if from == S::Queued => Some(from), // before it starts; after a skip is tested separately
        "WaitStarted" if step => Some(from),
        "BlockerAdded" if matches!(from, S::Proposed | S::Queued) => Some(from),
        "BlockerRemoved" if matches!(from, S::Proposed | S::Queued) => Some(from),
        "BlockersChanged" if matches!(from, S::Proposed | S::Queued) => Some(from),
        "Revived" if from == S::Canceled => Some(S::Queued),
        "TrackerEdited" if live || from == S::Proposed => Some(from),
        "Tick" => Some(from),
        _ => None, // includes Resumed (never paused) and WaitEnded (no wait)
    }
}

#[test]
fn every_state_and_event_pair() {
    let mut checked = 0;
    for from in State::ALL {
        for (name, _) in events(TaskId(0), TaskId(0), DecisionId(0)) {
            let mut sim = Sim::new();
            let t = sim.task_in(from);
            let other = sim.file(false, false);
            let decision = sim.w.tasks[&t].decision.unwrap_or(DecisionId(999));
            let ev = events(t, other, decision)
                .into_iter()
                .find(|(n, _)| *n == name)
                .unwrap()
                .1;
            let before = sim.w.clone();
            let got = reduce(
                &before,
                &Event {
                    at: Timestamp(sim.t + 1),
                    kind: ev,
                },
            );
            match (expected(from, name), got) {
                (Some(to), Ok((w, _))) => {
                    w.check_invariants().unwrap();
                    assert_eq!(w.tasks[&t].state, to, "{from:?} --{name}--> wrong state");
                }
                (None, Err(_)) => {}
                (Some(_), Err(e)) => panic!("{from:?} --{name}-- should be legal, got {e:?}"),
                (None, Ok(_)) => panic!("{from:?} --{name}-- should be illegal"),
            }
            assert_eq!(sim.w, before, "reduce must not mutate its input");
            checked += 1;
        }
    }
    assert_eq!(checked, 11 * 23);
}
