// SPDX-License-Identifier: GPL-3.0-or-later

mod common;
use common::*;
use mason_core::*;
use mason_engine::{reduce, ReduceError, World};
use EventKind as E;
use State as S;

fn land(sim: &mut Sim, t: TaskId) {
    sim.ok(E::Admitted { task: t });
    for _ in 0..5 {
        sim.ok(E::StepSucceeded { task: t });
    }
}

#[test]
fn happy_path_lands_merged_with_one_attempt_per_step() {
    let mut sim = Sim::new();
    let t = sim.file(false, false);
    assert_eq!(sim.activity(t), Activity::Waiting(WaitReason::Slot));
    sim.ok(E::Admitted { task: t });
    assert_eq!(sim.state(t), S::Exploring);
    assert_eq!(sim.activity(t), Activity::Running);
    for _ in 0..5 {
        sim.ok(E::StepSucceeded { task: t });
    }
    let task = &sim.w.tasks[&t];
    assert_eq!(
        (task.state, task.landed),
        (S::Landed, Some(LandedOutcome::Merged))
    );
    assert_eq!(task.activity, Activity::Idle);
    assert!(task.attempts.values().all(|&n| n == 1));
}

#[test]
fn explored_flag_goes_straight_to_developing() {
    let mut sim = Sim::new();
    let t = sim.file(true, false);
    let fx = sim.ok(E::Admitted { task: t });
    assert_eq!(sim.state(t), S::Developing);
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::StartStep {
            step: S::Developing,
            ..
        }
    )));
    assert!(!sim.w.tasks[&t].attempts.contains_key(&S::Exploring));
}

#[test]
fn proposed_waits_for_approval() {
    let mut sim = Sim::new();
    let t = sim.file(false, true);
    assert_eq!(sim.activity(t), Activity::Idle);
    assert!(matches!(
        sim.try_ev(E::Admitted { task: t }),
        Err(ReduceError::Illegal { .. })
    ));
    sim.ok(E::Approved { task: t });
    assert_eq!(sim.activity(t), Activity::Waiting(WaitReason::Slot));
}

#[test]
fn blocker_releases_itself_when_blocker_lands() {
    let mut sim = Sim::new();
    let a = sim.file(false, false);
    let b = sim.file_with(false, false, vec![a], None);
    assert_eq!(sim.activity(b), Activity::Held([a].into()));
    assert!(matches!(
        sim.try_ev(E::Admitted { task: b }),
        Err(ReduceError::NotClaimable(Activity::Held(_)))
    ));
    sim.ok(E::Admitted { task: a });
    for _ in 0..4 {
        sim.ok(E::StepSucceeded { task: a });
    }
    assert!(
        matches!(sim.activity(b), Activity::Held(_)),
        "not released before a lands"
    );
    let fx = sim.ok(E::StepSucceeded { task: a });
    assert_eq!(sim.activity(b), Activity::Waiting(WaitReason::Slot));
    assert!(fx.contains(&Effect::WakeScheduler { task: b }));
    sim.ok(E::Admitted { task: b });
}

#[test]
fn closing_with_no_change_also_releases_blockers() {
    let mut sim = Sim::new();
    let a = sim.file(false, false);
    let b = sim.file_with(false, false, vec![a], None);
    sim.ok(E::ClosedNoChange {
        task: a,
        reason: "dup".into(),
    });
    assert_eq!(sim.w.tasks[&a].landed, Some(LandedOutcome::ClosedNoChange));
    assert_eq!(sim.activity(b), Activity::Waiting(WaitReason::Slot));
}

#[test]
fn canceled_blocker_does_not_release() {
    let mut sim = Sim::new();
    let a = sim.file(false, false);
    let b = sim.file_with(false, false, vec![a], None);
    sim.ok(E::Canceled {
        task: a,
        delete_branch: false,
    });
    assert!(matches!(sim.activity(b), Activity::Held(_)));
}

#[test]
fn blocker_cycles_and_unknowns_are_refused() {
    let mut sim = Sim::new();
    let a = sim.file(false, false);
    let b = sim.file_with(false, false, vec![a], None);
    let c = sim.file_with(false, false, vec![b], None);
    assert_eq!(
        sim.try_ev(E::BlockerAdded {
            task: a,
            blocker: c
        }),
        Err(ReduceError::BlockerCycle)
    );
    assert_eq!(
        sim.try_ev(E::BlockerAdded {
            task: a,
            blocker: a
        }),
        Err(ReduceError::BlockerCycle)
    );
    assert_eq!(
        sim.try_ev(E::BlockerAdded {
            task: a,
            blocker: TaskId(77)
        }),
        Err(ReduceError::UnknownTask(TaskId(77)))
    );
}

#[test]
fn not_before_time_elapses_on_tick() {
    let mut sim = Sim::new();
    let t = sim.file_with(false, false, vec![], Some(Timestamp(1_000)));
    assert_eq!(
        sim.activity(t),
        Activity::Waiting(WaitReason::NotBefore(Timestamp(1_000)))
    );
    assert!(matches!(
        sim.try_ev(E::Admitted { task: t }),
        Err(ReduceError::NotClaimable(_))
    ));
    let fx = sim.try_at(999, E::Tick).unwrap();
    assert!(fx.is_empty());
    let fx = sim.try_at(1_000, E::Tick).unwrap();
    assert_eq!(fx, vec![Effect::WakeScheduler { task: t }]);
    assert_eq!(sim.activity(t), Activity::Waiting(WaitReason::Slot));
    sim.ok(E::Admitted { task: t });
}

#[test]
fn exploration_request_charges_no_failure_but_step_failure_does() {
    let mut sim = Sim::new();
    let t = sim.task_in(S::Developing);
    sim.ok(E::ExplorationRequested {
        task: t,
        reason: "plan wrong".into(),
    });
    assert_eq!((sim.state(t), sim.w.tasks[&t].failures), (S::Exploring, 0));
    sim.ok(E::StepSucceeded { task: t });
    assert_eq!(sim.w.tasks[&t].attempts[&S::Developing], 2);
    sim.ok(E::StepFailed {
        task: t,
        reason: "x".into(),
    });
    assert_eq!((sim.state(t), sim.w.tasks[&t].failures), (S::Failed, 1));
}

#[test]
fn sent_back_work_returns_to_developing_and_can_land_later() {
    let mut sim = Sim::new();
    let t = sim.task_in(S::Testing);
    sim.ok(E::SentBack {
        task: t,
        reason: "red".into(),
    });
    assert_eq!(sim.state(t), S::Developing);
    for _ in 0..4 {
        sim.ok(E::StepSucceeded { task: t });
    }
    assert_eq!(sim.state(t), S::Landed);
    assert_eq!(sim.w.tasks[&t].attempts[&S::Testing], 2);
}

#[test]
fn retry_goes_to_any_step_and_catches_up_with_base_first() {
    for to in [
        S::Exploring,
        S::Developing,
        S::Testing,
        S::Review,
        S::Landing,
    ] {
        let mut sim = Sim::new();
        let t = sim.task_in(S::Failed);
        let fx = sim.ok(E::Retried { task: t, to });
        assert_eq!(sim.state(t), to);
        let catch = fx
            .iter()
            .position(|e| *e == Effect::CatchUpWithBase { task: t })
            .unwrap();
        let start = fx
            .iter()
            .position(|e| matches!(e, Effect::StartStep { .. }))
            .unwrap();
        assert!(catch < start, "catch up before the step starts");
    }
    let mut sim = Sim::new();
    let t = sim.task_in(S::Failed);
    assert_eq!(
        sim.try_ev(E::Retried {
            task: t,
            to: S::Landed
        }),
        Err(ReduceError::BadRetryTarget(S::Landed))
    );
}

#[test]
fn retry_of_running_step_stops_its_host() {
    let mut sim = Sim::new();
    let t = sim.task_in(S::Testing);
    let fx = sim.ok(E::Retried {
        task: t,
        to: S::Developing,
    });
    assert!(fx.contains(&Effect::StopStepHost { task: t }));
}

#[test]
fn cancel_from_every_non_terminal_state_stops_and_releases() {
    for from in State::ALL {
        if matches!(from, S::Landed | S::Canceled | S::Failed) {
            continue;
        }
        let mut sim = Sim::new();
        let t = sim.task_in(from);
        let fx = sim.ok(E::Canceled {
            task: t,
            delete_branch: false,
        });
        assert_eq!(sim.state(t), S::Canceled, "from {from:?}");
        assert_eq!(sim.activity(t), Activity::Idle);
        assert!(fx.contains(&Effect::ReleaseLeases { task: t }));
        assert_eq!(
            fx.contains(&Effect::StopStepHost { task: t }),
            from.is_step(),
            "{from:?}"
        );
        assert!(
            !fx.contains(&Effect::DeleteBranch { task: t }),
            "branch kept by default"
        );
        assert!(sim.w.tasks[&t].cancel_keeps_branch);
    }
}

#[test]
fn cancel_can_delete_the_branch_when_asked() {
    let mut sim = Sim::new();
    let t = sim.task_in(S::Developing);
    let fx = sim.ok(E::Canceled {
        task: t,
        delete_branch: true,
    });
    assert!(fx.contains(&Effect::DeleteBranch { task: t }));
    assert!(!sim.w.tasks[&t].cancel_keeps_branch);
}

#[test]
fn questions_are_tied_to_their_decision() {
    let mut sim = Sim::new();
    let t = sim.task_in(S::Developing);
    let fx = sim.ok(E::QuestionAsked {
        task: t,
        question: "which?".into(),
    });
    let Effect::AskOwner { decision, .. } = fx[fx.len() - 1] else {
        panic!("{fx:?}")
    };
    assert_eq!(sim.activity(t), Activity::Waiting(WaitReason::Owner));
    assert_eq!(
        sim.try_ev(E::Answered {
            task: t,
            decision: DecisionId(42),
            answer: "x".into()
        }),
        Err(ReduceError::StaleAnswer)
    );
    sim.ok(E::Answered {
        task: t,
        decision,
        answer: "yes".into(),
    });
    assert_eq!(sim.state(t), S::Developing);
    assert_eq!(sim.w.decisions[&decision].answer.as_deref(), Some("yes"));
    // A second answer to the same, now closed, decision is refused.
    assert!(sim
        .try_ev(E::Answered {
            task: t,
            decision,
            answer: "again".into()
        })
        .is_err());
}

#[test]
fn retry_while_waiting_for_owner_makes_the_old_answer_stale() {
    let mut sim = Sim::new();
    let t = sim.task_in(S::Developing);
    sim.ok(E::QuestionAsked {
        task: t,
        question: "q".into(),
    });
    let d = sim.w.tasks[&t].decision.unwrap();
    sim.ok(E::Retried {
        task: t,
        to: S::Developing,
    });
    assert!(sim
        .try_ev(E::Answered {
            task: t,
            decision: d,
            answer: "late".into()
        })
        .is_err());
}

#[test]
fn pause_and_resume_change_only_activity() {
    let mut sim = Sim::new();
    let t = sim.task_in(S::Developing);
    sim.ok(E::Paused { task: t });
    assert_eq!(
        (sim.state(t), sim.activity(t)),
        (S::Developing, Activity::Paused)
    );
    assert!(sim.try_ev(E::Paused { task: t }).is_err());
    sim.ok(E::Resumed { task: t });
    assert_eq!(sim.activity(t), Activity::Running);

    let q = sim.file(false, false);
    sim.ok(E::Paused { task: q });
    assert!(matches!(
        sim.try_ev(E::Admitted { task: q }),
        Err(ReduceError::NotClaimable(Activity::Paused))
    ));
    let fx = sim.ok(E::Resumed { task: q });
    assert_eq!(fx, vec![Effect::WakeScheduler { task: q }]);
}

#[test]
fn waits_are_activity_not_state_and_clear_when_the_step_moves_on() {
    let mut sim = Sim::new();
    let t = sim.task_in(S::Testing);
    sim.ok(E::WaitStarted {
        task: t,
        on: "compile slot".into(),
    });
    assert_eq!(
        sim.activity(t),
        Activity::Waiting(WaitReason::On("compile slot".into()))
    );
    assert_eq!(sim.state(t), S::Testing);
    sim.ok(E::WaitEnded { task: t });
    assert_eq!(sim.activity(t), Activity::Running);
    sim.ok(E::WaitStarted {
        task: t,
        on: "phone".into(),
    });
    sim.ok(E::StepSucceeded { task: t });
    assert!(sim.w.waits.is_empty());
    assert_eq!(sim.activity(t), Activity::Running);
}

#[test]
fn unknown_project_and_duplicate_project_are_refused() {
    let mut sim = Sim::new();
    assert_eq!(
        sim.try_ev(E::ProjectRegistered {
            project: P,
            name: "again".into()
        }),
        Err(ReduceError::DuplicateProject(P))
    );
    let r = sim.try_ev(E::TaskFiled {
        project: ProjectId(9),
        title: "t".into(),
        priority: Priority::Low,
        trust: TrustSource::Outside,
        explored: false,
        proposed: false,
        blockers: vec![],
        not_before: None,
    });
    assert_eq!(r, Err(ReduceError::UnknownProject(ProjectId(9))));
    assert_eq!(
        sim.try_ev(E::Approved { task: TaskId(5) }),
        Err(ReduceError::UnknownTask(TaskId(5)))
    );
}

#[test]
fn time_never_goes_backwards_and_enqueue_time_comes_from_events() {
    let mut sim = Sim::new();
    sim.try_at(500, E::Tick).unwrap();
    let t = sim.file(false, false);
    assert!(sim.w.tasks[&t].enqueued_at >= Timestamp(500));
    let now = sim.w.now;
    sim.try_at(1, E::Tick).unwrap();
    assert_eq!(sim.w.now, now, "a stale timestamp doesn't rewind the clock");
}

#[test]
fn replaying_the_same_events_gives_the_same_world() {
    let run = || {
        let mut sim = Sim::new();
        let a = sim.file(false, false);
        let b = sim.file_with(true, false, vec![a], Some(Timestamp(50)));
        land(&mut sim, a);
        sim.try_at(60, E::Tick).unwrap();
        sim.ok(E::Admitted { task: b });
        sim.ok(E::QuestionAsked {
            task: b,
            question: "q".into(),
        });
        sim.w
    };
    assert_eq!(run(), run());
}

/// Random event soup: invariants hold after every accepted event, and a
/// refused event leaves the world exactly as it was.
#[test]
fn random_events_never_break_invariants() {
    let mut seed = 0x2545F4914F6CDD1Du64;
    let mut rnd = move |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    let mut w = World::new();
    let mut at = 0u64;
    let mut accepted = 0;
    let reg = Event {
        at: Timestamp(0),
        kind: E::ProjectRegistered {
            project: P,
            name: "p".into(),
        },
    };
    w = reduce(&w, &reg).unwrap().0;
    for _ in 0..20_000 {
        at += rnd(3);
        let task = TaskId(rnd(24));
        let kind = match if w.tasks.len() < 24 {
            rnd(20)
        } else {
            3 + rnd(17)
        } {
            0..=2 => E::TaskFiled {
                project: P,
                title: "t".into(),
                priority: Priority::Normal,
                trust: TrustSource::Owner,
                explored: rnd(2) == 0,
                proposed: rnd(4) == 0,
                blockers: if rnd(3) == 0 {
                    vec![TaskId(rnd(24))]
                } else {
                    vec![]
                },
                not_before: if rnd(4) == 0 {
                    Some(Timestamp(at + rnd(50)))
                } else {
                    None
                },
            },
            3 => E::Approved { task },
            4 => E::Rejected { task },
            5 | 6 => E::Admitted { task },
            7..=9 => E::StepSucceeded { task },
            10 => E::SentBack {
                task,
                reason: "r".into(),
            },
            11 => E::ExplorationRequested {
                task,
                reason: "r".into(),
            },
            12 => E::QuestionAsked {
                task,
                question: "q".into(),
            },
            13 => E::Answered {
                task,
                decision: DecisionId(rnd(6)),
                answer: "a".into(),
            },
            14 => E::StepFailed {
                task,
                reason: "r".into(),
            },
            15 => E::Retried {
                task,
                to: State::ALL[rnd(11) as usize],
            },
            16 => {
                if rnd(2) == 0 {
                    E::Canceled {
                        task,
                        delete_branch: false,
                    }
                } else {
                    E::ClosedNoChange {
                        task,
                        reason: "d".into(),
                    }
                }
            }
            17 => {
                if rnd(2) == 0 {
                    E::Paused { task }
                } else {
                    E::Resumed { task }
                }
            }
            18 => {
                if rnd(2) == 0 {
                    E::WaitStarted {
                        task,
                        on: "x".into(),
                    }
                } else {
                    E::WaitEnded { task }
                }
            }
            _ => {
                if rnd(2) == 0 {
                    E::BlockerAdded {
                        task,
                        blocker: TaskId(rnd(24)),
                    }
                } else {
                    E::Tick
                }
            }
        };
        let before = w.clone();
        match reduce(
            &before,
            &Event {
                at: Timestamp(at),
                kind,
            },
        ) {
            Ok((next, _)) => {
                next.check_invariants().unwrap();
                w = next;
                accepted += 1;
            }
            Err(_) => assert_eq!(w, before),
        }
    }
    assert!(
        accepted > 500,
        "the soup should exercise many transitions, got {accepted}"
    );
}
