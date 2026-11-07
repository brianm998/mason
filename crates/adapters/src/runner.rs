// SPDX-License-Identifier: GPL-3.0-or-later

//! The step runner: drives one agent step to an end the pipeline can act on.
//! Code moves tasks; the model works inside the step (Part 1).
//!
//! * A turn that ends with no accepted submission is nudged, up to a limit,
//!   then the step fails.
//! * A usage limit or an outage is a *hold*, never a failure.
//! * `ask` and a long `run_check` end the turn on purpose; the step is then
//!   `Asked` or `Suspended`, holds no agent process, and is resumed later with
//!   a note saying what happened.
//! * Token figures are exactly what the stream reported.

use mason_adapter_api::contract::ContractHandle;
use mason_adapter_api::{
    AgentAdapter, AgentEvent, Outcome, RateLimitInfo, SessionSpec, StepKind, TokenReport,
};
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct RunnerConfig {
    /// How many times a turn that ended with no submission is continued.
    pub max_nudges: u32,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        RunnerConfig { max_nudges: 2 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum HoldReason {
    UsageLimit { resets_at: Option<String> },
    ProviderDown { detail: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum FailReason {
    NoResult { nudges: u32 },
    Timeout,
    Crash { detail: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum StepEnd {
    Accepted {
        tool: String,
        submission: Value,
    },
    /// The developing agent found the task does not hold up. No failure charged.
    ExplorationRequested {
        reason: String,
    },
    /// The agent asked the owner and ended its turn.
    Asked {
        question: Value,
    },
    /// A long check is running under the scheduler; the session is suspended.
    Suspended {
        check_id: String,
    },
    /// A provider-wide wait. Charged to no task.
    Hold(HoldReason),
    Failed(FailReason),
}

/// Would a stronger model help? Yes after an agent that ended its turns with no
/// result or crashed; no after a timeout (more model is not more time), a hold
/// (a wait), a question (the owner's turn) or anything that succeeded.
pub fn worth_a_stronger_model(end: &StepEnd) -> bool {
    matches!(
        end,
        StepEnd::Failed(FailReason::NoResult { .. } | FailReason::Crash { .. })
    )
}

#[cfg(test)]
mod ladder_tests {
    use super::*;

    #[test]
    fn only_no_result_and_a_crash_move_up_a_model() {
        assert!(worth_a_stronger_model(&StepEnd::Failed(
            FailReason::NoResult { nudges: 2 }
        )));
        assert!(worth_a_stronger_model(&StepEnd::Failed(
            FailReason::Crash { detail: "x".into() }
        )));
        for end in [
            StepEnd::Failed(FailReason::Timeout),
            StepEnd::Hold(HoldReason::UsageLimit { resets_at: None }),
            StepEnd::Hold(HoldReason::ProviderDown { detail: "d".into() }),
            StepEnd::Asked {
                question: serde_json::json!({}),
            },
            StepEnd::Accepted {
                tool: "t".into(),
                submission: serde_json::json!({}),
            },
            StepEnd::ExplorationRequested { reason: "r".into() },
        ] {
            assert!(!worth_a_stronger_model(&end), "{end:?}");
        }
    }
}

/// Sums of what the stream reported. A field is `None` unless some invocation
/// reported it, so a provider that reports nothing shows nothing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TokenTotals {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_create: Option<u64>,
    pub cost_usd: Option<f64>,
    pub invocations: u32,
    pub invocations_reporting: u32,
}

fn add(a: &mut Option<u64>, b: Option<u64>) {
    if let Some(b) = b {
        *a = Some(a.unwrap_or(0) + b);
    }
}

impl TokenTotals {
    pub fn add(&mut self, t: &TokenReport) {
        add(&mut self.input, t.input);
        add(&mut self.output, t.output);
        add(&mut self.cache_read, t.cache_read);
        add(&mut self.cache_create, t.cache_create);
        if let Some(c) = t.cost_usd {
            self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + c);
        }
        self.invocations_reporting += 1;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StepReport {
    pub end: StepEnd,
    pub session_id: Option<String>,
    pub tokens: TokenTotals,
    pub nudges: u32,
    pub rejections: u32,
    pub api_retries: u32,
    pub rate_limits: Vec<RateLimitInfo>,
    pub tool_calls: Vec<String>,
    pub progress: Vec<String>,
    pub proposals: Vec<Value>,
}

impl StepReport {
    fn empty() -> StepReport {
        StepReport {
            end: StepEnd::Failed(FailReason::NoResult { nudges: 0 }),
            session_id: None,
            tokens: TokenTotals::default(),
            nudges: 0,
            rejections: 0,
            api_retries: 0,
            rate_limits: vec![],
            tool_calls: vec![],
            progress: vec![],
            proposals: vec![],
        }
    }
}

pub struct StepRunner<'a> {
    adapter: &'a dyn AgentAdapter,
    cfg: RunnerConfig,
}

/// The note sent when a turn ended with no accepted submission. It says what
/// happened and to verify the tree, never to replay the original prompt.
pub fn nudge_note(step: StepKind, nudge: u32) -> String {
    format!(
        "Your turn ended without an accepted submission (reminder {nudge}). Check where the work \
         stands first (look at the files and run git status and git log), then finish it and call {}. \
         Do not stop to ask whether to continue: no human is watching.",
        step.terminal_tools().join(" or ")
    )
}

pub fn check_result_note(check: &str, passed: bool, summary: &str) -> String {
    format!(
        "The check you started ({check}) has finished: {}. {summary}\nVerify the current state of \
         the tree, then continue and finish the step.",
        if passed { "PASSED" } else { "FAILED" }
    )
}

pub fn answer_note(answer: &str) -> String {
    format!(
        "The owner answered your question: {answer}\nVerify the current state of the tree, then \
         continue and finish the step."
    )
}

pub fn hold_note() -> String {
    "The provider was unavailable for a while and your session was paused; nothing was lost. \
     Verify the current state of the tree, then continue and finish the step."
        .into()
}

impl<'a> StepRunner<'a> {
    pub fn new(adapter: &'a dyn AgentAdapter, cfg: RunnerConfig) -> Self {
        StepRunner { adapter, cfg }
    }

    /// Run a step from the start. The contract is wired into the spec here.
    pub fn run(&self, mut spec: SessionSpec, contract: &ContractHandle) -> StepReport {
        spec.accepted = contract.flag();
        spec.contract = Some(contract.clone());
        let report = StepReport::empty();
        match self.adapter.start(spec.clone()) {
            Ok(h) => self.drive(report, h, spec, contract),
            Err(e) => self.fail(
                report,
                FailReason::Crash {
                    detail: e.to_string(),
                },
            ),
        }
    }

    /// Take over a session that is already running, found again after the
    /// controller restarted. Nudge and failure counts start afresh.
    pub fn adopt(
        &self,
        handle: Box<dyn mason_adapter_api::SessionHandle>,
        mut spec: SessionSpec,
        contract: &ContractHandle,
    ) -> StepReport {
        spec.accepted = contract.flag();
        spec.contract = Some(contract.clone());
        self.drive(StepReport::empty(), handle, spec, contract)
    }

    /// Continue a step that ended `Asked`, `Suspended` or `Hold`, with a note
    /// saying what happened. Totals accumulate onto `prior`.
    pub fn resume(
        &self,
        prior: StepReport,
        note: String,
        mut spec: SessionSpec,
        contract: &ContractHandle,
    ) -> StepReport {
        spec.accepted = contract.flag();
        spec.contract = Some(contract.clone());
        contract.clear_pending();
        let Some(id) = prior.session_id.clone() else {
            return self.fail(
                prior,
                FailReason::Crash {
                    detail: "no session id to resume".into(),
                },
            );
        };
        match self.adapter.resume(&id, note, spec.clone()) {
            Ok(h) => self.drive(prior, h, spec, contract),
            Err(e) => self.fail(
                prior,
                FailReason::Crash {
                    detail: e.to_string(),
                },
            ),
        }
    }

    fn fail(&self, mut r: StepReport, why: FailReason) -> StepReport {
        r.end = StepEnd::Failed(why);
        r
    }

    fn drive(
        &self,
        mut r: StepReport,
        mut handle: Box<dyn mason_adapter_api::SessionHandle>,
        spec: SessionSpec,
        contract: &ContractHandle,
    ) -> StepReport {
        loop {
            r.tokens.invocations += 1;
            let outcome = loop {
                match handle.next_event() {
                    Some(AgentEvent::Tokens(t)) => r.tokens.add(&t),
                    Some(AgentEvent::RateLimit(l)) => r.rate_limits.push(l),
                    Some(AgentEvent::ApiRetry { .. }) => r.api_retries += 1,
                    Some(AgentEvent::ToolCall { name, .. }) => r.tool_calls.push(name),
                    Some(AgentEvent::Text(_)) => {}
                    Some(AgentEvent::Finished(o)) => break o,
                    None => {
                        break Outcome::Crash {
                            detail: "stream ended without Finished".into(),
                        }
                    }
                }
            };
            r.session_id = handle.session_id().or(r.session_id);
            let st = contract.state();
            r.rejections = st.rejections;
            r.progress = st.progress.clone();
            r.proposals = st.proposals.clone();
            match outcome {
                Outcome::Accepted => {
                    let (tool, submission) = st.accepted.unwrap_or_default_pair();
                    r.end = if tool == "request_exploration" {
                        StepEnd::ExplorationRequested {
                            reason: submission["reason"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                        }
                    } else {
                        StepEnd::Accepted { tool, submission }
                    };
                    return r;
                }
                Outcome::NoResult => {
                    // Ending the turn is the instruction after `ask` and after a long check.
                    if let Some(check_id) = st.suspended_check {
                        r.end = StepEnd::Suspended { check_id };
                        return r;
                    }
                    if let Some(question) = st.asked {
                        r.end = StepEnd::Asked { question };
                        return r;
                    }
                    if r.nudges >= self.cfg.max_nudges {
                        let nudges = r.nudges;
                        return self.fail(r, FailReason::NoResult { nudges });
                    }
                    let Some(id) = r.session_id.clone() else {
                        return self.fail(
                            r,
                            FailReason::Crash {
                                detail: "no session id to nudge".into(),
                            },
                        );
                    };
                    r.nudges += 1;
                    match self
                        .adapter
                        .resume(&id, nudge_note(spec.step, r.nudges), spec.clone())
                    {
                        Ok(h) => handle = h,
                        Err(e) => {
                            return self.fail(
                                r,
                                FailReason::Crash {
                                    detail: e.to_string(),
                                },
                            )
                        }
                    }
                }
                Outcome::Timeout => return self.fail(r, FailReason::Timeout),
                Outcome::UsageLimit { resets_at } => {
                    r.end = StepEnd::Hold(HoldReason::UsageLimit { resets_at });
                    return r;
                }
                Outcome::ProviderDown { detail } => {
                    r.end = StepEnd::Hold(HoldReason::ProviderDown { detail });
                    return r;
                }
                Outcome::Crash { detail } => return self.fail(r, FailReason::Crash { detail }),
            }
        }
    }
}

trait OrDefaultPair {
    fn unwrap_or_default_pair(self) -> (String, Value);
}

impl OrDefaultPair for Option<(String, Value)> {
    fn unwrap_or_default_pair(self) -> (String, Value) {
        self.unwrap_or_else(|| ("unknown".into(), Value::Null))
    }
}
