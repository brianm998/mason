// SPDX-License-Identifier: GPL-3.0-or-later

//! The conformance kit for [`AgentAdapter`](crate::AgentAdapter). Any
//! implementation, built in or external, must pass it. The adapter's owner
//! supplies a [`Harness`] that can arrange each [`Scenario`]; the kit checks
//! what the event stream must look like in every one.

use crate::contract::{ContractHandle, NoChecks};
use crate::{AgentAdapter, AgentEvent, Outcome, SessionSpec, StepKind};
use std::sync::mpsc::channel;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    /// The agent submits a valid plan and ends its turn.
    Accepted,
    /// The first submission is rejected; the agent fixes it in the same session.
    RejectedThenFixed,
    /// The turn ends with no submission at all.
    NoResult,
    /// The agent runs past the deadline.
    Timeout,
    /// The provider says the usage limit is reached.
    UsageLimit,
    /// The provider is down or overloaded.
    ProviderDown,
    /// The agent process dies without finishing.
    Crash,
    /// The agent runs until cancelled.
    Hang,
}

impl Scenario {
    pub const ALL: [Scenario; 8] = [
        Scenario::Accepted,
        Scenario::RejectedThenFixed,
        Scenario::NoResult,
        Scenario::Timeout,
        Scenario::UsageLimit,
        Scenario::ProviderDown,
        Scenario::Crash,
        Scenario::Hang,
    ];
}

pub trait Harness {
    /// An adapter arranged so that a session ends in `scenario`.
    fn adapter(&self, scenario: Scenario) -> Box<dyn AgentAdapter>;
    /// A spec for it. The kit adds the contract, the flag and any deadline.
    fn spec(&self, scenario: Scenario) -> SessionSpec;
    /// Called with the contract before the session starts. An out-of-process
    /// agent needs it served on a socket; the default does nothing.
    fn prepare(&self, _contract: &ContractHandle) {}
}

/// A valid `submit_plan` argument set, for harnesses to script with.
pub fn valid_plan() -> serde_json::Value {
    serde_json::json!({
        "summary": "Add the thing", "files": ["src/lib.rs"], "approach": "Write it",
        "acceptance_criteria": ["it works"]
    })
}

fn collect(
    adapter: &dyn AgentAdapter,
    h: &dyn Harness,
    mut spec: SessionSpec,
    scenario: Scenario,
    cancel_after: Option<Duration>,
) -> Result<(Vec<AgentEvent>, Option<String>, ContractHandle), String> {
    let contract = ContractHandle::new(StepKind::Explore, Box::new(NoChecks));
    spec.step = StepKind::Explore;
    spec.accepted = contract.flag();
    h.prepare(&contract);
    spec.contract = Some(contract.clone());
    if scenario == Scenario::Timeout {
        spec.deadline = Some(Duration::from_millis(400));
    }
    let mut handle = adapter
        .start(spec)
        .map_err(|e| format!("start failed: {e}"))?;
    let (tx, rx) = channel();
    let canceller = handle.canceller();
    std::thread::spawn(move || {
        let mut events = vec![];
        while let Some(e) = handle.next_event() {
            events.push(e);
        }
        let extra = handle.next_event(); // must stay None
        let _ = tx.send((events, extra.is_none(), handle.session_id()));
    });
    if let Some(d) = cancel_after {
        std::thread::sleep(d);
        canceller();
    }
    let (events, quiet, id) = rx
        .recv_timeout(Duration::from_secs(30))
        .map_err(|_| "the event stream did not finish within 30s".to_string())?;
    if !quiet {
        return Err("events continued after Finished".into());
    }
    Ok((events, id, contract))
}

fn check(adapter: &dyn AgentAdapter, h: &dyn Harness, s: Scenario) -> Result<(), String> {
    let cancel = (s == Scenario::Hang).then_some(Duration::from_millis(300));
    let (events, id, contract) = collect(adapter, h, h.spec(s), s, cancel)?;
    let finished: Vec<&Outcome> = events
        .iter()
        .filter_map(|e| {
            if let AgentEvent::Finished(o) = e {
                Some(o)
            } else {
                None
            }
        })
        .collect();
    if finished.len() != 1 {
        return Err(format!(
            "expected exactly one Finished, got {}: {events:?}",
            finished.len()
        ));
    }
    if !matches!(events.last(), Some(AgentEvent::Finished(_))) {
        return Err("Finished must be the last event".into());
    }
    let outcome = finished[0];
    let accepted = contract.flag().is_set();
    let ok = match s {
        Scenario::Accepted => matches!(outcome, Outcome::Accepted) && accepted,
        Scenario::RejectedThenFixed => {
            matches!(outcome, Outcome::Accepted) && accepted && contract.state().rejections >= 1
        }
        Scenario::NoResult => matches!(outcome, Outcome::NoResult) && !accepted,
        Scenario::Timeout => matches!(outcome, Outcome::Timeout),
        Scenario::UsageLimit => matches!(outcome, Outcome::UsageLimit { .. }),
        Scenario::ProviderDown => matches!(outcome, Outcome::ProviderDown { .. }),
        Scenario::Crash => matches!(outcome, Outcome::Crash { .. }),
        Scenario::Hang => matches!(outcome, Outcome::NoResult | Outcome::Crash { .. }),
    };
    if !ok {
        return Err(format!(
            "wrong outcome {outcome:?} (contract accepted: {accepted})"
        ));
    }
    if matches!(outcome, Outcome::Accepted) {
        if id.is_none() {
            return Err("an accepted session must expose its session id".into());
        }
        if adapter.capabilities().usage_reports
            && !events.iter().any(|e| matches!(e, AgentEvent::Tokens(_)))
        {
            return Err("adapter claims usage_reports but emitted no Tokens event".into());
        }
    }
    Ok(())
}

/// Run every scenario. `Err` lists each failure, naming the scenario.
pub fn run_all(h: &dyn Harness) -> Result<(), Vec<String>> {
    let mut failures = vec![];
    for s in Scenario::ALL {
        let adapter = h.adapter(s);
        if let Err(e) = check(adapter.as_ref(), h, s) {
            failures.push(format!("{} / {s:?}: {e}", adapter.name()));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures)
    }
}
