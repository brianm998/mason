// SPDX-License-Identifier: GPL-3.0-or-later
#![allow(dead_code)]

use mason_core::*;
use mason_engine::{reduce, ReduceError, World};

pub const P: ProjectId = ProjectId(1);

/// Drives the reducer with a simulated clock and checks invariants each step.
#[derive(Clone)]
pub struct Sim {
    pub w: World,
    pub t: u64,
    pub fx: Vec<Effect>,
}

impl Sim {
    pub fn new() -> Self {
        let mut s = Sim {
            w: World::new(),
            t: 0,
            fx: vec![],
        };
        s.ok(EventKind::ProjectRegistered {
            project: P,
            name: "p".into(),
        });
        s
    }

    pub fn try_at(&mut self, at: u64, kind: EventKind) -> Result<Vec<Effect>, ReduceError> {
        self.t = self.t.max(at);
        let (w, fx) = reduce(
            &self.w,
            &Event {
                at: Timestamp(at),
                kind,
            },
        )?;
        w.check_invariants()
            .expect("invariants hold after every event");
        self.w = w;
        self.fx = fx.clone();
        Ok(fx)
    }

    pub fn try_ev(&mut self, kind: EventKind) -> Result<Vec<Effect>, ReduceError> {
        self.t += 1;
        let t = self.t;
        self.try_at(t, kind)
    }

    pub fn ok(&mut self, kind: EventKind) -> Vec<Effect> {
        self.try_ev(kind).expect("event should be legal")
    }

    pub fn file(&mut self, explored: bool, proposed: bool) -> TaskId {
        self.file_with(explored, proposed, vec![], None)
    }

    pub fn file_with(
        &mut self,
        explored: bool,
        proposed: bool,
        blockers: Vec<TaskId>,
        not_before: Option<Timestamp>,
    ) -> TaskId {
        let fx = self.ok(EventKind::TaskFiled {
            project: P,
            title: "t".into(),
            body: String::new(),
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            explored,
            proposed,
            blockers,
            not_before,
        });
        match fx[0] {
            Effect::TaskCreated { task } => task,
            ref e => panic!("expected TaskCreated, got {e:?}"),
        }
    }

    pub fn state(&self, t: TaskId) -> State {
        self.w.tasks[&t].state
    }

    pub fn activity(&self, t: TaskId) -> Activity {
        self.w.tasks[&t].activity.clone()
    }

    /// Reach `target` by legal events from a fresh, unflagged task.
    pub fn task_in(&mut self, target: State) -> TaskId {
        use EventKind as E;
        let proposed = target == State::Proposed;
        let t = self.file(false, proposed);
        let succeed = |s: &mut Sim, n: usize| {
            for _ in 0..n {
                s.ok(E::StepSucceeded {
                    task: t,
                    next: None,
                });
            }
        };
        if matches!(target, State::Proposed | State::Queued) {
            return t;
        }
        if target == State::Canceled {
            self.ok(E::Canceled {
                task: t,
                delete_branch: false,
            });
            return t;
        }
        self.ok(E::Admitted {
            task: t,
            entry: None,
        });
        match target {
            State::Exploring => {}
            State::Developing => succeed(self, 1),
            State::Testing => succeed(self, 2),
            State::Review => succeed(self, 3),
            State::Landing => succeed(self, 4),
            State::Landed => succeed(self, 5),
            State::NeedsAttention => {
                succeed(self, 1);
                self.ok(E::QuestionAsked {
                    task: t,
                    question: "q?".into(),
                });
            }
            State::Failed => {
                succeed(self, 1);
                self.ok(E::StepFailed {
                    task: t,
                    reason: "boom".into(),
                    class: None,
                });
            }
            _ => unreachable!(),
        }
        assert_eq!(self.state(t), target);
        t
    }
}
