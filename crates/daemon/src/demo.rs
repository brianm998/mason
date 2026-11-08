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
use mason_core::progress::{Figure, Progress, SubProgress, TodoItem};
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
        // Six tasks in a running step, one for each way progress is shown.
        vec![
            filed(1, "Search: real count", false),
            filed(1, "Export: typical total", false),
            filed(1, "Webhooks: count only", false),
            filed(1, "Queue: gone quiet", false),
            filed(1, "Billing: sub-counts", false),
            filed(1, "Migrate: todo list", false),
        ],
        vec![
            admit(5, "develop", State::Developing),
            admit(6, "develop", State::Developing),
            admit(7, "develop", State::Developing),
            admit(8, "develop", State::Developing),
            admit(9, "develop", State::Developing),
            admit(10, "develop", State::Developing),
        ],
    ]
}

/// The first task of the progress cases, and the checkpoint that admits them.
const PROGRESS_TASK: u64 = 5;
const PROGRESS_CHECKPOINT: usize = 9;

fn counted(done: u64) -> Figure {
    Figure::counted(done)
}

/// One record per way progress is shown, for the six tasks admitted at the last checkpoint.
/// Times are relative to now, so the clients' "updated N s ago" is a few seconds and the
/// quiet one says 20 minutes; nothing moves them (the demo has no ticker).
fn progress_cases(now: u64) -> Vec<Progress> {
    let base = |i: u64, started: u64, last: u64| {
        let mut p = Progress::new(9000 + i, "develop", now - started);
        p.output.last_output_ms = now - last;
        p.updated_at_ms = now - last;
        p.stale_after_ms = 300_000;
        p
    };
    // 0. A real count: reported done and total, with a message.
    let mut real = base(0, 400_000, 4_000);
    real.unit = "tests".into();
    real.done = Figure::reported(212);
    real.total = Figure::reported(340);
    real.message = "running the api suite".into();
    real.output.lines = 1_240;
    real.output.bytes = 61_000;
    // 1. Only a typical total from twelve past runs: the count and the range, no percentage.
    let mut hist = base(1, 300_000, 2_000);
    hist.unit = "lines".into();
    hist.done = counted(1_240);
    hist.total = Figure::history(2_400);
    hist.total_low = 1_800;
    hist.history_samples = 12;
    hist.output.lines = 1_240;
    hist.output.bytes = 50_000;
    // 2. Nothing to measure against: the count alone.
    let mut none = base(2, 200_000, 6_000);
    none.unit = "tool calls".into();
    none.done = Figure::reported(37);
    none.output.lines = 90;
    none.output.bytes = 4_000;
    // 3. Quiet for twenty minutes: amber, "no output for".
    let mut quiet = base(3, 1_800_000, 1_200_000);
    quiet.unit = "lines".into();
    quiet.done = counted(412);
    quiet.output.lines = 412;
    quiet.output.bytes = 20_000;
    // 4. Sub-counts: suites, with a count of plan steps as a whole.
    let mut subs = base(4, 500_000, 3_000);
    subs.unit = "plan steps".into();
    subs.done = Figure::reported(3);
    subs.total = Figure::reported(7);
    subs.message = "checking the ledger".into();
    subs.output.lines = 300;
    subs.output.bytes = 15_000;
    subs.subs = vec![
        SubProgress {
            name: "unit".into(),
            unit: "tests".into(),
            done: Figure::reported(120),
            total: Figure::reported(120),
        },
        SubProgress {
            name: "integration".into(),
            unit: "tests".into(),
            done: Figure::reported(44),
            total: Figure::reported(90),
        },
    ];
    // 5. An agent's todo list.
    let mut todo = base(5, 600_000, 5_000);
    todo.unit = "todo items".into();
    todo.done = Figure::reported(1);
    todo.total = Figure::reported(3);
    todo.message = "writing the migration".into();
    todo.output.lines = 55;
    todo.output.bytes = 3_000;
    todo.todo = vec![
        TodoItem {
            text: "Read the schema".into(),
            status: "done".into(),
        },
        TodoItem {
            text: "Write the migration".into(),
            status: "doing".into(),
        },
        TodoItem {
            text: "Add a rollback test".into(),
            status: "pending".into(),
        },
    ];
    vec![real, hist, none, quiet, subs, todo]
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
        if i == PROGRESS_CHECKPOINT {
            // Before the events, so the views they produce already carry the records.
            for (n, p) in progress_cases(crate::daemon::now_ms())
                .into_iter()
                .enumerate()
            {
                self.progress_seed(TaskId(PROGRESS_TASK + n as u64), ProjectId(1), p);
            }
        }
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

// ---- LLM usage (s64): accounts in several states, so the usage page and the
// top bar can be seen and checked without a real provider. Fed straight into
// the store; no agent runs. Times are relative to the daemon's start.

/// The demo's provider accounts.
pub fn accounts() -> Vec<crate::providers::Account> {
    use crate::config::Provider;
    use crate::providers::{Account, Billing, Overage};
    let acct = |name: &str, adapter, billing, sessions, budget| Account {
        name: name.into(),
        adapter,
        billing,
        sessions: Some(sessions),
        warn_at: crate::providers::DEFAULT_WARN_AT,
        monthly_budget_usd: budget,
        overage: Overage::Hold,
    };
    vec![
        acct(
            crate::sched::DEFAULT_ACCOUNT,
            Provider::Claude,
            Billing::Subscription,
            3,
            None,
        ),
        acct(
            "claude-quiet",
            Provider::Claude,
            Billing::Subscription,
            2,
            None,
        ),
        acct(
            "claude-full",
            Provider::Claude,
            Billing::Subscription,
            2,
            None,
        ),
        acct("api-main", Provider::Claude, Billing::Api, 4, Some(200.0)),
        acct("codex", Provider::Codex, Billing::Subscription, 2, None),
    ]
}

fn window(
    id: &str,
    label: &str,
    len: u64,
    util: Option<f64>,
    resets: u64,
    state: mason_adapter_api::usage::WindowState,
) -> mason_adapter_api::Window {
    mason_adapter_api::Window {
        id: id.into(),
        label: label.into(),
        length_secs: Some(len),
        utilization: util,
        resets_at: Some(resets),
        state,
    }
}

/// Put the demo's usage into the store.
pub fn seed_usage(d: &Daemon) {
    use mason_adapter_api::usage::{
        Blocked, BlockedWhy, Overage, OverageState, UsageSnapshot, WindowState,
    };
    let now = crate::usage::now_secs();
    let ago = now - 12 * 60;
    let snap = |windows, overage, blocked| UsageSnapshot {
        windows,
        overage: Overage {
            state: overage,
            resets_at: None,
        },
        blocked,
        spend_usd: None,
        observed_at: Some(ago),
        session_id: None,
    };
    // A subscription with a percentage on both windows.
    let main = snap(
        vec![
            window(
                "five_hour",
                "5-hour",
                18000,
                Some(0.82),
                now + 4200,
                WindowState::Warning,
            ),
            window(
                "seven_day",
                "Weekly",
                7 * 86400,
                Some(0.41),
                now + 3 * 86400 + 4 * 3600,
                WindowState::Ok,
            ),
        ],
        OverageState::Disabled("out_of_credits".into()),
        None,
    );
    // Windows reported with no percentage: state and reset only.
    let quiet = snap(
        vec![
            window(
                "five_hour",
                "5-hour",
                18000,
                None,
                now + 2400,
                WindowState::Ok,
            ),
            window(
                "seven_day",
                "Weekly",
                7 * 86400,
                None,
                now + 5 * 86400,
                WindowState::Ok,
            ),
        ],
        OverageState::NotOffered,
        None,
    );
    // Full and refusing new work, with overage in use on the other window.
    let full = snap(
        vec![
            window(
                "five_hour",
                "5-hour",
                18000,
                Some(1.0),
                now + 7800,
                WindowState::Blocked,
            ),
            window(
                "seven_day",
                "Weekly",
                7 * 86400,
                Some(0.93),
                now + 86400,
                WindowState::Warning,
            ),
        ],
        OverageState::UsedUp,
        Some(Blocked {
            why: BlockedWhy::Window("five_hour".into()),
            until: Some(now + 7800),
        }),
    );
    for (name, s) in [
        (crate::sched::DEFAULT_ACCOUNT, &main),
        ("claude-quiet", &quiet),
        ("claude-full", &full),
    ] {
        crate::usage::record_snapshot(d, name, s);
        // A short history so the trend shows.
        for (back, u) in [(6 * 3600u64, 0.3f64), (3 * 3600, 0.5)] {
            let w = &s.windows[0];
            let _ = d.store.record_reading(mason_store::UsageReading {
                account: name.into(),
                window: w.id.clone(),
                at: now - back,
                utilization: w.utilization.map(|_| u),
                resets_at: w.resets_at,
                state: "ok".into(),
            });
        }
    }
    let run = |attempt: &str,
               task,
               project: &str,
               step: &str,
               account: &str,
               model: &str,
               inp,
               out,
               cost| {
        let _ = d.store.record_usage_run(mason_store::UsageRun {
            attempt: attempt.into(),
            task,
            project: project.into(),
            step: step.into(),
            account: account.into(),
            model: Some(model.into()),
            input: Some(inp),
            output: Some(out),
            cache_read: Some(inp * 4),
            cache_create: None,
            cost_usd: cost,
            at: now - 25 * 60,
        });
    };
    let m = crate::sched::DEFAULT_ACCOUNT;
    run("d1", 1, "web", "develop", m, "opus", 120_000, 18_000, None);
    run("d2", 2, "web", "review", m, "sonnet", 60_000, 4_000, None);
    run("d3", 3, "api", "develop", m, "opus", 90_000, 11_000, None);
    run(
        "a1",
        4,
        "web",
        "develop",
        "api-main",
        "sonnet",
        200_000,
        30_000,
        Some(3.4),
    );
    run(
        "a2",
        5,
        "api",
        "develop",
        "api-main",
        "haiku",
        50_000,
        5_000,
        Some(0.2),
    );
}
