// SPDX-License-Identifier: GPL-3.0-or-later
//! `state_entered_at`: when a task entered its present state.
mod common;
use common::*;
use mason_core::*;
use mason_engine::reduce;
use EventKind as E;

fn entered(s: &Sim, t: TaskId) -> u64 {
    s.w.tasks[&t].state_entered_at.0
}

fn succeed(s: &mut Sim, t: TaskId) {
    s.ok(E::StepSucceeded {
        task: t,
        next: None,
    });
}

#[test]
fn filing_sets_it_to_the_filing_time() {
    let mut s = Sim::new();
    let t = s.file(false, false);
    assert_eq!(entered(&s, t), s.w.tasks[&t].enqueued_at.0);
    let p = s.file(false, true);
    assert_eq!(entered(&s, p), s.w.tasks[&p].enqueued_at.0);
}

#[test]
fn each_kind_of_transition_sets_it_to_the_event_time() {
    let mut s = Sim::new();
    let t = s.file(false, true);
    s.try_at(100, E::Approved { task: t }).unwrap();
    assert_eq!(entered(&s, t), 100, "approved: proposed -> queued");
    s.try_at(
        200,
        E::Admitted {
            task: t,
            entry: None,
        },
    )
    .unwrap();
    assert_eq!(entered(&s, t), 200, "admitted");
    s.try_at(
        300,
        E::StepSucceeded {
            task: t,
            next: None,
        },
    )
    .unwrap();
    assert_eq!(s.state(t), State::Developing);
    assert_eq!(entered(&s, t), 300, "step success into the next category");
    s.try_at(
        400,
        E::StepSucceeded {
            task: t,
            next: None,
        },
    )
    .unwrap();
    assert_eq!(s.state(t), State::Testing);
    s.try_at(
        500,
        E::SentBack {
            task: t,
            reason: "red".into(),
            to_step: String::new(),
        },
    )
    .unwrap();
    assert_eq!(s.state(t), State::Developing);
    assert_eq!(entered(&s, t), 500, "sent back to developing");
    s.try_at(
        600,
        E::StepFailed {
            task: t,
            reason: "x".into(),
            class: None,
        },
    )
    .unwrap();
    assert_eq!(s.state(t), State::Failed);
    assert_eq!(entered(&s, t), 600, "failed");
    s.try_at(
        700,
        E::Retried {
            task: t,
            to: State::Developing,
            step: String::new(),
        },
    )
    .unwrap();
    assert_eq!(entered(&s, t), 700, "retried out of failed");
    s.try_at(
        800,
        E::Canceled {
            task: t,
            delete_branch: false,
        },
    )
    .unwrap();
    assert_eq!(entered(&s, t), 800, "canceled");
}

#[test]
fn landing_and_questions_count() {
    let mut s = Sim::new();
    let t = s.task_in(State::Landing);
    s.try_at(
        1000,
        E::StepSucceeded {
            task: t,
            next: None,
        },
    )
    .unwrap();
    assert_eq!(s.state(t), State::Landed);
    assert_eq!(entered(&s, t), 1000);
    let q = s.task_in(State::Developing);
    s.try_at(
        1100,
        E::QuestionAsked {
            task: q,
            question: "q?".into(),
        },
    )
    .unwrap();
    assert_eq!(s.state(q), State::NeedsAttention);
    assert_eq!(entered(&s, q), 1100);
}

#[test]
fn a_same_state_attempt_leaves_it_alone() {
    let mut s = Sim::new();
    let t = s.task_in(State::Developing);
    let before = entered(&s, t);
    // A second step in the same category is a new attempt, not a new state.
    s.try_at(
        before + 5000,
        E::StepSucceeded {
            task: t,
            next: Some(Next {
                step: "develop2".into(),
                state: State::Developing,
            }),
        },
    )
    .unwrap();
    assert_eq!(s.state(t), State::Developing);
    assert_eq!(entered(&s, t), before);
    // So is a tick.
    s.try_at(before + 9000, E::Tick).unwrap();
    assert_eq!(entered(&s, t), before);
}

#[test]
fn replaying_the_events_gives_the_same_value() {
    let mut s = Sim::new();
    let a = s.file(false, false);
    s.try_at(
        50,
        E::Admitted {
            task: a,
            entry: None,
        },
    )
    .unwrap();
    succeed(&mut s, a);
    let want = entered(&s, a);
    // Replay: reduce the same events over a fresh world.
    let mut w = mason_engine::World::new();
    let evs = vec![
        Event {
            at: Timestamp(1),
            kind: E::ProjectRegistered {
                project: P,
                name: "p".into(),
            },
        },
        Event {
            at: Timestamp(2),
            kind: E::TaskFiled {
                project: P,
                title: "t".into(),
                body: String::new(),
                priority: Priority::Normal,
                trust: TrustSource::Owner,
                explored: false,
                proposed: false,
                blockers: vec![],
                not_before: None,
            },
        },
        Event {
            at: Timestamp(50),
            kind: E::Admitted {
                task: a,
                entry: None,
            },
        },
        Event {
            at: Timestamp(s.t),
            kind: E::StepSucceeded {
                task: a,
                next: None,
            },
        },
    ];
    for e in &evs {
        w = reduce(&w, e).unwrap().0;
    }
    assert_eq!(w.tasks[&a].state_entered_at.0, want);
}
