// SPDX-License-Identifier: GPL-3.0-or-later

mod common;
use common::*;
use mason_core::*;
use mason_engine::{reduce, ReduceError, World};
use EventKind as E;
use State as S;

fn land(sim: &mut Sim, t: TaskId) {
    sim.ok(E::Admitted {
        task: t,
        entry: None,
    });
    for _ in 0..5 {
        sim.ok(E::StepSucceeded {
            task: t,
            next: None,
        });
    }
}

#[test]
fn happy_path_lands_merged_with_one_attempt_per_step() {
    let mut sim = Sim::new();
    let t = sim.file(false, false);
    assert_eq!(sim.activity(t), Activity::Waiting(WaitReason::Slot));
    sim.ok(E::Admitted {
        task: t,
        entry: None,
    });
    assert_eq!(sim.state(t), S::Exploring);
    assert_eq!(sim.activity(t), Activity::Running);
    for _ in 0..5 {
        sim.ok(E::StepSucceeded {
            task: t,
            next: None,
        });
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
    let fx = sim.ok(E::Admitted {
        task: t,
        entry: None,
    });
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
        sim.try_ev(E::Admitted {
            task: t,
            entry: None
        }),
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
        sim.try_ev(E::Admitted {
            task: b,
            entry: None
        }),
        Err(ReduceError::NotClaimable(Activity::Held(_)))
    ));
    sim.ok(E::Admitted {
        task: a,
        entry: None,
    });
    for _ in 0..4 {
        sim.ok(E::StepSucceeded {
            task: a,
            next: None,
        });
    }
    assert!(
        matches!(sim.activity(b), Activity::Held(_)),
        "not released before a lands"
    );
    let fx = sim.ok(E::StepSucceeded {
        task: a,
        next: None,
    });
    assert_eq!(sim.activity(b), Activity::Waiting(WaitReason::Slot));
    assert!(fx.contains(&Effect::WakeScheduler { task: b }));
    sim.ok(E::Admitted {
        task: b,
        entry: None,
    });
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
        Err(ReduceError::BlockerCycle(vec![a, c, b, a]))
    );
    assert_eq!(
        sim.try_ev(E::BlockerAdded {
            task: a,
            blocker: a
        }),
        Err(ReduceError::BlockerCycle(vec![a, a]))
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
        sim.try_ev(E::Admitted {
            task: t,
            entry: None
        }),
        Err(ReduceError::NotClaimable(_))
    ));
    let fx = sim.try_at(999, E::Tick).unwrap();
    assert!(fx.is_empty());
    let fx = sim.try_at(1_000, E::Tick).unwrap();
    assert_eq!(fx, vec![Effect::WakeScheduler { task: t }]);
    assert_eq!(sim.activity(t), Activity::Waiting(WaitReason::Slot));
    sim.ok(E::Admitted {
        task: t,
        entry: None,
    });
}

#[test]
fn exploration_request_charges_no_failure_but_step_failure_does() {
    let mut sim = Sim::new();
    let t = sim.task_in(S::Developing);
    sim.ok(E::ExplorationRequested {
        task: t,
        reason: "plan wrong".into(),
        to_step: String::new(),
    });
    assert_eq!((sim.state(t), sim.w.tasks[&t].failures), (S::Exploring, 0));
    sim.ok(E::StepSucceeded {
        task: t,
        next: None,
    });
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
        to_step: String::new(),
    });
    assert_eq!(sim.state(t), S::Developing);
    for _ in 0..4 {
        sim.ok(E::StepSucceeded {
            task: t,
            next: None,
        });
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
        let fx = sim.ok(E::Retried {
            task: t,
            to,
            step: String::new(),
        });
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
            to: S::Landed,
            step: String::new(),
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
        step: String::new(),
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
            answer: "x".into(),
            step: String::new(),
        }),
        Err(ReduceError::StaleAnswer)
    );
    sim.ok(E::Answered {
        task: t,
        decision,
        answer: "yes".into(),
        step: String::new(),
    });
    assert_eq!(sim.state(t), S::Developing);
    assert_eq!(sim.w.decisions[&decision].answer.as_deref(), Some("yes"));
    // A second answer to the same, now closed, decision is refused.
    assert!(sim
        .try_ev(E::Answered {
            task: t,
            decision,
            answer: "again".into(),
            step: String::new(),
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
        step: String::new(),
    });
    assert!(sim
        .try_ev(E::Answered {
            task: t,
            decision: d,
            answer: "late".into(),
            step: String::new(),
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
        sim.try_ev(E::Admitted {
            task: q,
            entry: None
        }),
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
    sim.ok(E::StepSucceeded {
        task: t,
        next: None,
    });
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
        body: String::new(),
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
        sim.ok(E::Admitted {
            task: b,
            entry: None,
        });
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
                body: String::new(),
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
            5 | 6 => E::Admitted { task, entry: None },
            7..=9 => E::StepSucceeded { task, next: None },
            10 => E::SentBack {
                task,
                reason: "r".into(),
                to_step: String::new(),
            },
            11 => E::ExplorationRequested {
                task,
                reason: "r".into(),
                to_step: String::new(),
            },
            12 => E::QuestionAsked {
                task,
                question: "q".into(),
            },
            13 => E::Answered {
                task,
                decision: DecisionId(rnd(6)),
                answer: "a".into(),
                step: String::new(),
            },
            14 => E::StepFailed {
                task,
                reason: "r".into(),
            },
            15 => E::Retried {
                task,
                to: State::ALL[rnd(11) as usize],
                step: String::new(),
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

// ---- Stage 7: named steps, pins, routes ----

fn entry(def: &str, step: &str, state: State, route: Route) -> Option<Entry> {
    Some(Entry {
        def: def.into(),
        step: step.into(),
        state,
        route,
    })
}

fn next(step: &str, state: State) -> Option<Next> {
    Some(Next {
        step: step.into(),
        state,
    })
}

#[test]
fn a_task_is_pinned_to_the_definition_it_started_under_and_records_its_route() {
    let mut sim = Sim::new();
    let t = sim.file(false, false);
    sim.ok(E::Admitted {
        task: t,
        entry: entry("hash-v1", "explore", State::Exploring, Route::Explore),
    });
    let task = &sim.w.tasks[&t];
    assert_eq!(
        (task.def.as_str(), task.step.as_str(), task.route),
        ("hash-v1", "explore", Some(Route::Explore))
    );
    assert_eq!(sim.w.step_runs.values().next().unwrap().name, "explore");
}

#[test]
fn a_skipped_exploring_route_cannot_start_in_exploring() {
    let mut sim = Sim::new();
    let t = sim.file(true, false);
    for route in [Route::Flag, Route::Clarity, Route::Classifier] {
        assert!(
            matches!(
                sim.try_ev(E::Admitted {
                    task: t,
                    entry: entry("h", "explore", State::Exploring, route)
                }),
                Err(ReduceError::Illegal { .. })
            ),
            "{route:?}"
        );
    }
    // Entries must be steps, not parking states.
    assert!(sim
        .try_ev(E::Admitted {
            task: t,
            entry: entry("h", "x", State::Landed, Route::Explore)
        })
        .is_err());
    sim.ok(E::Admitted {
        task: t,
        entry: entry("h", "develop", State::Developing, Route::Flag),
    });
    assert_eq!(sim.w.tasks[&t].route, Some(Route::Flag));
}

#[test]
fn next_may_stay_in_a_category_or_move_forward_but_never_back() {
    let mut sim = Sim::new();
    let t = sim.file(false, false);
    sim.ok(E::Admitted {
        task: t,
        entry: entry("h", "develop", State::Developing, Route::Flag),
    });
    // Lateral: a second developing step.
    sim.ok(E::StepSucceeded {
        task: t,
        next: next("develop-2", State::Developing),
    });
    assert_eq!(sim.w.tasks[&t].step, "develop-2");
    // Forward, skipping a category (no review step in this graph).
    sim.ok(E::StepSucceeded {
        task: t,
        next: next("test", State::Testing),
    });
    sim.ok(E::StepSucceeded {
        task: t,
        next: next("land", State::Landing),
    });
    // Back is not what `next` is for.
    assert!(matches!(
        sim.try_ev(E::StepSucceeded {
            task: t,
            next: next("develop", State::Developing)
        }),
        Err(ReduceError::Illegal { .. })
    ));
    // Landed only from Landing, and never to a parking state.
    sim.ok(E::StepSucceeded {
        task: t,
        next: next("", State::Landed),
    });
    assert_eq!(sim.w.tasks[&t].landed, Some(LandedOutcome::Merged));

    let t2 = sim.file(false, false);
    sim.ok(E::Admitted {
        task: t2,
        entry: entry("h", "develop", State::Developing, Route::Flag),
    });
    assert!(
        sim.try_ev(E::StepSucceeded {
            task: t2,
            next: next("", State::Landed)
        })
        .is_err(),
        "not from developing"
    );
    assert!(sim
        .try_ev(E::StepSucceeded {
            task: t2,
            next: next("x", State::NeedsAttention)
        })
        .is_err());
    assert!(sim
        .try_ev(E::StepSucceeded {
            task: t2,
            next: next("x", State::Failed)
        })
        .is_err());
}

#[test]
fn attempts_count_per_step_name_so_a_second_step_in_a_category_starts_at_one() {
    let mut sim = Sim::new();
    let t = sim.file(false, false);
    sim.ok(E::Admitted {
        task: t,
        entry: entry("h", "develop", State::Developing, Route::Flag),
    });
    sim.ok(E::StepSucceeded {
        task: t,
        next: next("test-fast", State::Testing),
    });
    sim.ok(E::StepSucceeded {
        task: t,
        next: next("test-device", State::Testing),
    });
    let runs: Vec<(&str, u32)> = sim
        .w
        .step_runs
        .values()
        .map(|r| (r.name.as_str(), r.attempt))
        .collect();
    assert_eq!(
        runs,
        vec![("develop", 1), ("test-fast", 1), ("test-device", 1)]
    );
    // Red on the second test step goes back to a named developing step, which now has attempt 2.
    sim.ok(E::SentBack {
        task: t,
        reason: "red".into(),
        to_step: "develop".into(),
    });
    let last = sim.w.step_runs.values().last().unwrap();
    assert_eq!((last.name.as_str(), last.attempt), ("develop", 2));
    assert_eq!(sim.w.tasks[&t].step_attempts["develop"], 2);
}

#[test]
fn exploration_requested_counts_how_often_a_skip_came_back_and_charges_nothing() {
    let mut sim = Sim::new();
    let t = sim.file(true, false);
    sim.ok(E::Admitted {
        task: t,
        entry: entry("h", "develop", State::Developing, Route::Flag),
    });
    sim.ok(E::ExplorationRequested {
        task: t,
        reason: "plan wrong".into(),
        to_step: "explore".into(),
    });
    let task = &sim.w.tasks[&t];
    assert_eq!(
        (
            task.state,
            task.explore_returns,
            task.failures,
            task.step.as_str()
        ),
        (State::Exploring, 1, 0, "explore")
    );
    assert_eq!(
        task.route,
        Some(Route::Flag),
        "the route it started by is kept"
    );
}

#[test]
fn explore_anyway_before_start_and_after_a_skip() {
    let mut sim = Sim::new();
    // Before the task starts: just a flag for admission to honour.
    let t = sim.file(true, false);
    sim.ok(E::ExploreAnyway {
        task: t,
        step: "explore".into(),
    });
    assert!(sim.w.tasks[&t].force_explore);
    assert_eq!(sim.state(t), State::Queued);

    // After a skip: stops the running step and goes to exploring, recorded as such.
    let u = sim.file(true, false);
    sim.ok(E::Admitted {
        task: u,
        entry: entry("h", "develop", State::Developing, Route::Clarity),
    });
    let fx = sim.ok(E::ExploreAnyway {
        task: u,
        step: "explore".into(),
    });
    assert_eq!(sim.state(u), State::Exploring);
    assert_eq!(sim.w.tasks[&u].route, Some(Route::ExploreAnyway));
    assert_eq!(sim.w.tasks[&u].failures, 0);
    assert!(fx.contains(&Effect::StopStepHost { task: u }));
    assert_eq!(
        sim.w.tasks[&u].explore_returns, 0,
        "not a request from the agent"
    );

    // A task that explored normally has nothing to be sent back to.
    let v = sim.file(false, false);
    sim.ok(E::Admitted {
        task: v,
        entry: entry("h", "explore", State::Exploring, Route::Explore),
    });
    assert!(sim
        .try_ev(E::ExploreAnyway {
            task: v,
            step: "explore".into()
        })
        .is_err());
}

#[test]
fn outside_text_claiming_explored_is_ignored_until_the_owner_approves() {
    let mut sim = Sim::new();
    let mk = |sim: &mut Sim, trust: TrustSource, proposed: bool| -> TaskId {
        let fx = sim.ok(E::TaskFiled {
            project: P,
            title: "t".into(),
            body: "explored: true".into(),
            priority: Priority::Normal,
            trust,
            explored: true, // the claim
            proposed,
            blockers: vec![],
            not_before: None,
        });
        match fx[0] {
            Effect::TaskCreated { task } => task,
            _ => unreachable!(),
        }
    };
    let owner = mk(&mut sim, TrustSource::Owner, false);
    let agent = mk(&mut sim, TrustSource::OwnerAgent, false);
    let outside = mk(&mut sim, TrustSource::Outside, true);
    assert!(sim.w.tasks[&owner].explored_counts());
    assert!(sim.w.tasks[&agent].explored_counts());
    assert!(
        !sim.w.tasks[&outside].explored_counts(),
        "an outsider's claim is ignored"
    );
    // Approving it is the owner's decision, and then the claim stands.
    sim.ok(E::Approved { task: outside });
    assert!(sim.w.tasks[&outside].approved && sim.w.tasks[&outside].explored_counts());
    // Legacy admission honours exactly that.
    let unapproved = mk(&mut sim, TrustSource::Outside, false); // arrived queued, never approved
    sim.ok(E::Admitted {
        task: unapproved,
        entry: None,
    });
    assert_eq!(
        (sim.state(unapproved), sim.w.tasks[&unapproved].route),
        (State::Exploring, Some(Route::Explore))
    );
    sim.ok(E::Admitted {
        task: owner,
        entry: None,
    });
    assert_eq!(
        (sim.state(owner), sim.w.tasks[&owner].route),
        (State::Developing, Some(Route::Flag))
    );
}

#[test]
fn removing_a_blocker_releases_a_queued_task_and_wakes_the_scheduler() {
    let mut sim = Sim::new();
    let a = sim.file(false, false);
    let b = sim.file_with(false, false, vec![a], None);
    assert!(matches!(sim.activity(b), Activity::Held(_)));
    let fx = sim.ok(E::BlockerRemoved {
        task: b,
        blocker: a,
    });
    assert_eq!(sim.activity(b), Activity::Waiting(WaitReason::Slot));
    assert!(fx.contains(&Effect::WakeScheduler { task: b }));
}

#[test]
fn a_started_task_is_not_pulled_back_by_a_removed_blocker() {
    let mut sim = Sim::new();
    let a = sim.file(false, false);
    let b = sim.file(false, false);
    sim.ok(E::Admitted {
        task: b,
        entry: None,
    });
    assert!(matches!(
        sim.try_ev(E::BlockerAdded {
            task: b,
            blocker: a
        }),
        Err(ReduceError::Illegal { .. })
    ));
    assert!(matches!(
        sim.try_ev(E::BlockerRemoved {
            task: b,
            blocker: a
        }),
        Err(ReduceError::Illegal { .. })
    ));
}

#[test]
fn retry_on_a_cancelled_task_is_refused() {
    let mut sim = Sim::new();
    let t = sim.task_in(State::Canceled);
    assert!(matches!(
        sim.try_ev(E::Retried {
            task: t,
            to: State::Developing,
            step: String::new()
        }),
        Err(ReduceError::Illegal { .. })
    ));
}

#[test]
fn releasing_one_of_two_blockers_keeps_the_task_held_and_all_makes_it_claimable() {
    let mut sim = Sim::new();
    let a = sim.file(false, false);
    let b = sim.file(false, false);
    let c = sim.file_with(false, false, vec![a, b], None);
    let fx = sim.ok(E::BlockersChanged {
        task: c,
        released: vec![a],
        added: vec![],
    });
    assert!(matches!(sim.activity(c), Activity::Held(_)));
    assert!(!fx.contains(&Effect::WakeScheduler { task: c }));
    let fx = sim.ok(E::BlockersChanged {
        task: c,
        released: vec![b],
        added: vec![],
    });
    assert_eq!(sim.activity(c), Activity::Waiting(WaitReason::Slot));
    assert!(fx.contains(&Effect::WakeScheduler { task: c }));
}

#[test]
fn releasing_a_task_that_is_not_a_blocker_is_refused() {
    let mut sim = Sim::new();
    let a = sim.file(false, false);
    let c = sim.file(false, false);
    assert_eq!(
        sim.try_ev(E::BlockersChanged {
            task: c,
            released: vec![a],
            added: vec![]
        }),
        Err(ReduceError::NotABlocker(a))
    );
}

#[test]
fn an_added_blocker_that_closes_a_loop_is_refused_naming_the_path() {
    let mut sim = Sim::new();
    let a = sim.file(false, false);
    let b = sim.file_with(false, false, vec![a], None);
    let before = sim.w.clone();
    assert_eq!(
        sim.try_ev(E::BlockersChanged {
            task: a,
            released: vec![],
            added: vec![b]
        }),
        Err(ReduceError::BlockerCycle(vec![a, b, a]))
    );
    assert_eq!(sim.w, before);
}

#[test]
fn blockers_changed_past_queued_is_refused() {
    let mut sim = Sim::new();
    let a = sim.file(false, false);
    let b = sim.file(false, false);
    sim.ok(E::Admitted {
        task: b,
        entry: None,
    });
    assert!(matches!(
        sim.try_ev(E::BlockersChanged {
            task: b,
            released: vec![],
            added: vec![a]
        }),
        Err(ReduceError::Illegal { .. })
    ));
}

#[test]
fn revived_puts_a_cancelled_task_back_in_the_queue_only_if_its_branch_is_kept() {
    let mut sim = Sim::new();
    let t = sim.task_in(State::Canceled);
    let fx = sim.ok(E::Revived { task: t });
    assert_eq!(sim.state(t), State::Queued);
    assert!(fx.contains(&Effect::CatchUpWithBase { task: t }));
    let gone = sim.file(false, false);
    sim.ok(E::Canceled {
        task: gone,
        delete_branch: true,
    });
    assert!(matches!(
        sim.try_ev(E::Revived { task: gone }),
        Err(ReduceError::Illegal { .. })
    ));
}

#[test]
fn replaying_blockers_changed_and_revived_gives_the_same_world() {
    let run = || {
        let mut sim = Sim::new();
        let a = sim.file(false, false);
        let b = sim.file(false, false);
        let c = sim.file_with(false, false, vec![a], None);
        sim.ok(E::Canceled {
            task: a,
            delete_branch: false,
        });
        sim.ok(E::BlockersChanged {
            task: c,
            released: vec![a],
            added: vec![b],
        });
        sim.ok(E::Revived { task: a });
        sim.w
    };
    let w = run();
    assert_eq!(w, run());
    assert_eq!(w.tasks.values().filter(|t| !t.blockers.is_empty()).count(), 1);
}
