// SPDX-License-Identifier: GPL-3.0-or-later

//! The step runner against the scripted fake agent: every way a step can end.

use mason_adapter_api::conformance::valid_plan;
use mason_adapter_api::contract::{CheckHandler, CheckRun, ContractHandle, NoChecks};
use mason_adapter_api::{RateLimitInfo, SessionSpec, StepKind};
use mason_adapters::fake::{FakeAction as A, FakeAdapter};
use mason_adapters::runner::*;
use serde_json::json;
use std::time::Duration;

fn call(tool: &str, args: serde_json::Value) -> A {
    A::Call {
        tool: tool.into(),
        args,
    }
}

fn spec(step: StepKind) -> SessionSpec {
    SessionSpec::new(step, "THE ORIGINAL PROMPT", std::env::temp_dir())
}

fn change() -> serde_json::Value {
    json!({"summary": "did it", "head": "0123abc"})
}

fn run(a: &FakeAdapter, step: StepKind) -> StepReport {
    let c = ContractHandle::new(step, Box::new(NoChecks));
    StepRunner::new(a, RunnerConfig::default()).run(spec(step), &c)
}

#[test]
fn accepted_in_one_turn() {
    let a = FakeAdapter::new(vec![vec![call("submit_plan", valid_plan())]]);
    let r = run(&a, StepKind::Explore);
    assert!(
        matches!(&r.end, StepEnd::Accepted { tool, .. } if tool == "submit_plan"),
        "{:?}",
        r.end
    );
    assert_eq!((r.nudges, r.rejections, a.invocations()), (0, 0, 1));
    assert_eq!(r.tool_calls, vec!["mcp__mason__submit_plan"]);
}

#[test]
fn rejected_then_fixed_in_the_same_session() {
    let a = FakeAdapter::new(vec![vec![
        call("submit_plan", json!({"summary": "x", "files": ["../etc"]})),
        call("submit_plan", valid_plan()),
    ]]);
    let r = run(&a, StepKind::Explore);
    assert!(matches!(r.end, StepEnd::Accepted { .. }));
    assert_eq!(r.rejections, 1, "one rejection, fixed without a nudge");
    assert_eq!((r.nudges, a.invocations()), (0, 1));
}

#[test]
fn no_result_is_nudged_with_a_note_then_fails() {
    let a = FakeAdapter::new(vec![vec![A::Say("I am done!".into())]; 3]);
    let r = run(&a, StepKind::Develop);
    assert_eq!(r.end, StepEnd::Failed(FailReason::NoResult { nudges: 2 }));
    assert_eq!(a.invocations(), 3, "first turn plus two nudges");
    let notes = a.notes();
    assert_eq!(notes.len(), 2);
    for n in &notes {
        assert!(
            n.contains("git status") && n.contains("submit_change"),
            "{n}"
        );
        assert!(n.contains("no human is watching"));
        assert!(
            !n.contains("THE ORIGINAL PROMPT"),
            "a resume must not replay the original prompt"
        );
    }
}

#[test]
fn a_nudge_can_rescue_the_step() {
    let a = FakeAdapter::new(vec![
        vec![A::Say("thinking".into())],
        vec![call("submit_change", change())],
    ]);
    let r = run(&a, StepKind::Develop);
    assert!(matches!(r.end, StepEnd::Accepted { .. }));
    assert_eq!(r.nudges, 1);
    assert_eq!(r.tokens.invocations, 2);
}

#[test]
fn timeout_fails_the_step_and_is_not_nudged() {
    let a = FakeAdapter::new(vec![vec![A::Sleep(Duration::from_secs(30))]]);
    let c = ContractHandle::new(StepKind::Develop, Box::new(NoChecks));
    let mut s = spec(StepKind::Develop);
    s.deadline = Some(Duration::from_millis(150));
    let r = StepRunner::new(&a, RunnerConfig::default()).run(s, &c);
    assert_eq!(r.end, StepEnd::Failed(FailReason::Timeout));
    assert_eq!(a.invocations(), 1);
}

#[test]
fn usage_limit_is_a_hold_not_a_failure_and_keeps_the_figures() {
    let a = FakeAdapter::new(vec![vec![
        A::RateLimit(RateLimitInfo {
            status: "rejected".into(),
            window: Some("five_hour".into()),
            resets_at_epoch: Some(1_800_000_000),
            utilization: None,
        }),
        A::UsageLimit(Some("3pm".into())),
    ]]);
    let r = run(&a, StepKind::Develop);
    assert_eq!(
        r.end,
        StepEnd::Hold(HoldReason::UsageLimit {
            resets_at: Some("3pm".into())
        })
    );
    assert_eq!(r.nudges, 0, "a hold is never nudged");
    assert_eq!(r.rate_limits.len(), 1);
    assert_eq!(r.rate_limits[0].utilization, None, "nothing invented");
}

#[test]
fn provider_outage_is_a_hold_and_retries_are_counted() {
    let a = FakeAdapter::new(vec![vec![
        A::ApiRetry { attempt: 1 },
        A::ApiRetry { attempt: 2 },
        A::ProviderDown("529 overloaded".into()),
    ]]);
    let r = run(&a, StepKind::Develop);
    assert_eq!(
        r.end,
        StepEnd::Hold(HoldReason::ProviderDown {
            detail: "529 overloaded".into()
        })
    );
    assert_eq!(r.api_retries, 2);
}

#[test]
fn a_held_step_resumes_with_a_note_and_can_then_finish() {
    let a = FakeAdapter::new(vec![
        vec![A::UsageLimit(None)],
        vec![call("submit_change", change())],
    ]);
    let c = ContractHandle::new(StepKind::Develop, Box::new(NoChecks));
    let runner = StepRunner::new(&a, RunnerConfig::default());
    let held = runner.run(spec(StepKind::Develop), &c);
    assert!(matches!(held.end, StepEnd::Hold(_)));
    let done = runner.resume(held, hold_note(), spec(StepKind::Develop), &c);
    assert!(matches!(done.end, StepEnd::Accepted { .. }));
    assert!(a.notes()[0].contains("nothing was lost"));
}

#[test]
fn a_crash_fails_the_step() {
    let a = FakeAdapter::new(vec![vec![A::Crash("segfault".into())]]);
    let r = run(&a, StepKind::Develop);
    assert_eq!(
        r.end,
        StepEnd::Failed(FailReason::Crash {
            detail: "segfault".into()
        })
    );
}

#[test]
fn ask_ends_the_turn_and_the_answer_resumes_it() {
    let a = FakeAdapter::new(vec![
        vec![call(
            "ask",
            json!({"ask": "Keep the old token format?", "options": [{"label": "yes", "recommended": true}, {"label": "no"}]}),
        )],
        vec![call("submit_change", change())],
    ]);
    let c = ContractHandle::new(StepKind::Develop, Box::new(NoChecks));
    let runner = StepRunner::new(&a, RunnerConfig::default());
    let asked = runner.run(spec(StepKind::Develop), &c);
    match &asked.end {
        StepEnd::Asked { question } => assert_eq!(question["ask"], "Keep the old token format?"),
        e => panic!("{e:?}"),
    }
    assert_eq!(
        asked.nudges, 0,
        "ending the turn after ask is not a missing result"
    );
    let done = runner.resume(
        asked,
        answer_note("yes, keep it"),
        spec(StepKind::Develop),
        &c,
    );
    assert!(matches!(done.end, StepEnd::Accepted { .. }));
    assert!(a.notes()[0].contains("yes, keep it") && !a.notes()[0].contains("ORIGINAL"));
    assert_eq!(done.tokens.invocations, 2);
}

struct Slow;
impl CheckHandler for Slow {
    fn run(&self, name: &str) -> CheckRun {
        if name == "device" {
            CheckRun::Long {
                id: "chk-device-1".into(),
            }
        } else {
            CheckRun::Done {
                passed: true,
                summary: "12 tests".into(),
            }
        }
    }
}

#[test]
fn a_long_check_suspends_the_session_and_resumes_with_the_result() {
    let a = FakeAdapter::new(vec![
        vec![
            call("run_check", json!({"name": "unit"})),
            call("run_check", json!({"name": "device"})),
        ],
        vec![call("submit_change", change())],
    ]);
    let c = ContractHandle::new(StepKind::Develop, Box::new(Slow));
    let runner = StepRunner::new(&a, RunnerConfig::default());
    let s = runner.run(spec(StepKind::Develop), &c);
    assert_eq!(
        s.end,
        StepEnd::Suspended {
            check_id: "chk-device-1".into()
        }
    );
    assert_eq!(s.nudges, 0);
    let done = runner.resume(
        s,
        check_result_note("device", true, "all green"),
        spec(StepKind::Develop),
        &c,
    );
    assert!(matches!(done.end, StepEnd::Accepted { .. }));
    assert!(a.notes()[0].contains("device") && a.notes()[0].contains("PASSED"));
}

#[test]
fn request_exploration_is_its_own_end_and_charges_nothing() {
    let a = FakeAdapter::new(vec![vec![call(
        "request_exploration",
        json!({"reason": "the file named in the plan does not exist"}),
    )]]);
    let r = run(&a, StepKind::Develop);
    assert_eq!(
        r.end,
        StepEnd::ExplorationRequested {
            reason: "the file named in the plan does not exist".into()
        }
    );
}

#[test]
fn tools_from_another_step_are_rejected() {
    let a = FakeAdapter::new(vec![vec![call("submit_change", change())]; 3]);
    let r = run(&a, StepKind::Review);
    assert_eq!(r.end, StepEnd::Failed(FailReason::NoResult { nudges: 2 }));
    assert!(r.rejections >= 1);
}

#[test]
fn token_totals_are_only_what_the_stream_reported() {
    // Reports nothing at all: nothing is shown.
    let a = FakeAdapter::new(vec![vec![call("submit_plan", valid_plan())]]).with_tokens(None);
    let r = run(&a, StepKind::Explore);
    assert_eq!(r.tokens.input, None);
    assert_eq!(r.tokens.cost_usd, None);
    assert_eq!(
        (r.tokens.invocations, r.tokens.invocations_reporting),
        (1, 0)
    );

    // Sums across invocations, per step.
    let a = FakeAdapter::new(vec![
        vec![A::Say("x".into())],
        vec![call("submit_plan", valid_plan())],
    ]);
    let r = run(&a, StepKind::Explore);
    assert_eq!(r.tokens.input, Some(20));
    assert_eq!(r.tokens.cache_read, Some(200));
    assert_eq!(r.tokens.cost_usd, Some(0.002));
    assert_eq!(r.tokens.invocations_reporting, 2);
}

#[test]
fn progress_and_proposals_are_collected_and_do_not_end_the_step() {
    let a = FakeAdapter::new(vec![vec![
        call(
            "report_progress",
            json!({"message": "reading the parser", "done": 1, "total": 3}),
        ),
        call(
            "file_proposal",
            json!({"title": "flaky test", "body": "b", "evidence": "e"}),
        ),
        call("submit_change", change()),
    ]]);
    let r = run(&a, StepKind::Develop);
    assert!(matches!(r.end, StepEnd::Accepted { .. }));
    assert_eq!(r.progress, vec!["reading the parser"]);
    assert_eq!(r.proposals.len(), 1);
}
