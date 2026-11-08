// SPDX-License-Identifier: GPL-3.0-or-later

//! Adapter traits, the step contract, and the shared test-mode network guard.
//!
//! An agent adapter wraps a coding-agent CLI behind one interface (Part 6).
//! A step ends only by passing validation: the agent calls a contract tool and
//! the contract replies "accepted" or says precisely what is wrong.

pub mod card;
pub mod conformance;
pub mod contract;
pub mod http;
pub mod kit;
pub mod netguard;
pub mod service;
pub mod usage;
pub mod webhook;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub use contract::{ContractHandle, StepKind};
pub use usage::{
    BillingKind, Blocked, BlockedWhy, Overage, OverageState, UsageModel, UsageSnapshot, Window,
    WindowSpec, WindowState,
};

pub type SessionId = String;

/// Set by the contract when a terminal submission is accepted. The adapter
/// reads it when the agent's turn ends, to tell `Accepted` from `NoResult`.
#[derive(Clone, Debug, Default)]
pub struct SubmissionFlag(Arc<AtomicBool>);

impl SubmissionFlag {
    pub fn set(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_set(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Clone)]
pub struct SessionSpec {
    pub step: StepKind,
    pub prompt: String,
    pub workdir: PathBuf,
    /// Extra environment, applied after the adapter's sanitizing.
    pub env: Vec<(String, String)>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Applied to every tool call the agent makes (explicit, never the default).
    pub tool_timeout: Duration,
    /// The whole invocation is cut off after this long: `Outcome::Timeout`.
    pub deadline: Option<Duration>,
    pub contract: Option<ContractHandle>,
    pub accepted: SubmissionFlag,
    /// Names this step run so a restarted controller can find its session
    /// again (see [`AgentAdapter::reattach`]). Required for hosted launches.
    pub run_key: Option<String>,
    /// Narrow the step's contract tools to these (a definition's `tools`).
    pub tools: Option<Vec<String>>,
    /// Told of every chunk of a step host's output as it arrives, with the host's
    /// id and the chunk's absolute offset in its journal (a journal read again is
    /// offered again from 0, so the receiver counts by stream and offset).
    pub progress: Option<ProgressSink>,
}

/// Receives `(host id, offset, bytes)` of a step's output.
pub type ProgressSink = std::sync::Arc<dyn Fn(&str, u64, &[u8]) + Send + Sync>;

impl SessionSpec {
    pub fn new(step: StepKind, prompt: &str, workdir: impl Into<PathBuf>) -> Self {
        SessionSpec {
            step,
            prompt: prompt.into(),
            workdir: workdir.into(),
            env: vec![],
            model: None,
            effort: None,
            tool_timeout: Duration::from_secs(600),
            deadline: None,
            contract: None,
            accepted: SubmissionFlag::default(),
            run_key: None,
            tools: None,
            progress: None,
        }
    }
}

/// What the stream honestly reported. Absent fields were not reported; they
/// are never estimated.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TokenReport {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_create: Option<u64>,
    pub cost_usd: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// The turn ended after the contract accepted a terminal submission.
    Accepted,
    /// The turn ended without one.
    NoResult,
    /// The invocation exceeded its deadline and was cut off.
    Timeout,
    /// A provider-wide hold, never a failure of the task.
    UsageLimit {
        resets_at: Option<String>,
    },
    ProviderDown {
        detail: String,
    },
    Crash {
        detail: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentEvent {
    Text(String),
    ToolCall {
        name: String,
        summary: String,
    },
    /// Final figures for this invocation, as reported by the stream.
    Tokens(TokenReport),
    /// The provider's limits as the stream last told them.
    Usage(UsageSnapshot),
    ApiRetry {
        attempt: u32,
        status: Option<u32>,
        delay_ms: Option<u64>,
    },
    /// Always the last event.
    Finished(Outcome),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentCaps {
    pub resume: bool,
    pub mcp: bool,
    pub effort_levels: Vec<String>,
    pub usage_reports: bool,
}

#[derive(Debug)]
pub enum AdapterError {
    /// Refused by the test-mode guard.
    Net(netguard::NetError),
    Spawn(std::io::Error),
    Other(String),
}

impl std::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for AdapterError {}

impl From<netguard::NetError> for AdapterError {
    fn from(e: netguard::NetError) -> Self {
        AdapterError::Net(e)
    }
}

pub trait AgentAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> AgentCaps;
    /// The measures this adapter can report for `billing`. Default: none.
    fn usage_model(&self, _billing: BillingKind) -> UsageModel {
        UsageModel::default()
    }
    /// Read the provider's usage without starting a session, by a route the
    /// CLI documents. Default: none (no such route was verified; decision
    /// 0045). The daemon refreshes idle accounts only where this returns
    /// `Some`.
    fn probe_usage(&self, _billing: BillingKind) -> Result<Option<UsageSnapshot>, AdapterError> {
        Ok(None)
    }
    fn start(&self, spec: SessionSpec) -> Result<Box<dyn SessionHandle>, AdapterError>;
    /// Continue a session with a note saying what happened. The note is sent
    /// instead of replaying the original prompt.
    fn resume(
        &self,
        id: &str,
        note: String,
        spec: SessionSpec,
    ) -> Result<Box<dyn SessionHandle>, AdapterError>;

    /// After the controller restarted: find the live (or just-finished)
    /// session for `run_key` and keep following it. `None` if there is none.
    fn reattach(
        &self,
        _run_key: &str,
        _spec: SessionSpec,
    ) -> Result<Option<Box<dyn SessionHandle>>, AdapterError> {
        Ok(None)
    }
}

/// Stops a session from any thread, even while another is blocked in
/// [`SessionHandle::next_event`].
pub type Canceller = Arc<dyn Fn() + Send + Sync>;

pub trait SessionHandle: Send {
    /// Known once the stream has said so.
    fn session_id(&self) -> Option<SessionId>;
    /// Blocks for the next event. `None` once `Finished` has been delivered.
    fn next_event(&mut self) -> Option<AgentEvent>;
    /// A handle that stops the session from another thread. After it is
    /// called, the next event is `Finished`.
    fn canceller(&self) -> Canceller;
    fn cancel(&mut self) {
        (self.canceller())()
    }
}
