// SPDX-License-Identifier: GPL-3.0-or-later

//! `masond --demo`: a daemon that plays a scripted scenario instead of doing
//! work. It runs no agents and touches no repository; it appends the same
//! events the real pipeline does, so the same views, the same protocol and
//! the same commands apply. Both monitors must pass the same conformance run
//! against it (Part 17): at every checkpoint of the script the views say
//! exactly the same thing, so a client can be checked against it.
//!
//! By default the script advances on a timer, for a person to watch. With
//! `--demo-manual` it advances only when told (`POST /demo/next`), so a test
//! can stop at each checkpoint and compare.

use crate::daemon::Daemon;
use mason_adapter_api::card::{Card, CardOption, Origin};
use mason_core::*;
use std::sync::Arc;
use std::time::Duration;

/// A checkpoint is the state after one step of the script.
pub fn checkpoints() -> usize {
    script().len()
}

fn card(ask: &str, why: &str, options: &[(&str, bool)], evidence: &[&str]) -> String {
    Card {
        ask: ask.into(),
        why: why.into(),
        options: options
            .iter()
            .map(|(l, r)| CardOption {
                label: (*l).into(),
                recommended: *r,
            })
            .collect(),
        evidence: evidence.iter().map(|e| (*e).into()).collect(),
        links: vec![],
        key: String::new(),
        origin: Origin::Agent,
        problems: vec![],
    }
    .to_text()
}

fn filed(project: u64, title: &str, proposed: bool) -> EventKind {
    EventKind::TaskFiled {
        project: ProjectId(project),
        title: title.into(),
        body: format!("Demo task: {title}."),
        priority: Priority::Normal,
        trust: TrustSource::Owner,
        explored: false,
        proposed,
        blockers: vec![],
        not_before: None,
    }
}

fn admit(task: u64, step: &str, state: State) -> EventKind {
    EventKind::Admitted {
        task: TaskId(task),
        entry: Some(Entry {
            def: mason_definition::builtin::builtin().hash.clone(),
            step: step.into(),
            state,
            route: Route::Explore,
        }),
    }
}

fn next(task: u64, step: &str, state: State) -> EventKind {
    EventKind::StepSucceeded {
        task: TaskId(task),
        next: Some(Next {
            step: step.into(),
            state,
        }),
    }
}

/// The script. Each inner list is one step; the state after it is a
/// checkpoint. Task ids and project ids are the ones the reducer allocates.
fn script() -> Vec<Vec<EventKind>> {
    let wait = "Waiting for compile slot: 2nd in line, behind #88 (2 steps from landing), held by #91 for 4 min";
    vec![
        vec![
            EventKind::ProjectRegistered {
                project: ProjectId(1),
                name: "web-api".into(),
            },
            EventKind::ProjectRegistered {
                project: ProjectId(2),
                name: "ios-app".into(),
            },
        ],
        vec![
            filed(1, "Rate limit headers", false),
            filed(1, "Paging cursor", false),
            filed(1, "Old token format", false),
            filed(2, "Share sheet", false),
            filed(2, "Flaky login test", true),
        ],
        vec![
            admit(0, "explore", State::Exploring),
            admit(1, "explore", State::Exploring),
            admit(3, "explore", State::Exploring),
        ],
        vec![
            next(0, "develop", State::Developing),
            admit(2, "explore", State::Exploring),
            EventKind::QuestionAsked {
                task: TaskId(2),
                question: card(
                    "Keep the old token format for one release?",
                    "The ticket and the release notes do not say whether clients still send it.",
                    &[
                        ("Yes, keep it for one release", true),
                        ("No, drop it now", false),
                    ],
                    &["ticket: 'tokens are changing'", "no mention in RELEASES.md"],
                ),
            },
        ],
        vec![
            next(1, "develop", State::Developing),
            next(1, "test", State::Testing),
            EventKind::WaitStarted {
                task: TaskId(1),
                on: wait.into(),
            },
        ],
        vec![EventKind::StepFailed {
            task: TaskId(3),
            reason: "the agent ended its turn 3 times without submitting".into(),
        }],
        vec![
            next(0, "test", State::Testing),
            next(0, "land", State::Landing),
            EventKind::StepSucceeded {
                task: TaskId(0),
                next: Some(Next {
                    step: String::new(),
                    state: State::Landed,
                }),
            },
        ],
    ]
}

impl Daemon {
    /// Play the next step of the script. Returns the checkpoint reached, or
    /// `None` when the script is over.
    pub fn demo_next(self: &Arc<Self>) -> Option<usize> {
        let steps = script();
        let i = {
            let mut pos = self.demo_pos.lock().unwrap_or_else(|e| e.into_inner());
            if *pos >= steps.len() {
                return None;
            }
            *pos += 1;
            *pos
        };
        for kind in steps[i - 1].clone() {
            if let Err(e) = self.emit(kind) {
                tracing::warn!(error = %e, "a demo event was refused");
            }
        }
        Some(i)
    }

    pub fn demo_position(&self) -> usize {
        *self.demo_pos.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Advance on a timer, for a person to watch.
    pub fn spawn_demo_player(self: &Arc<Self>, every: Duration) {
        let weak = Arc::downgrade(self);
        let _ = std::thread::Builder::new()
            .name("demo-player".into())
            .spawn(move || loop {
                std::thread::sleep(every);
                let Some(d) = weak.upgrade() else { break };
                if d.demo_next().is_none() {
                    break;
                }
            });
    }
}
