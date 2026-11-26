// SPDX-License-Identifier: GPL-3.0-or-later

//! Adapter traits, the step contract, and the shared test-mode network guard.
//!
//! An agent adapter wraps a coding-agent CLI behind one interface (Part 6).
//! A step ends only by passing validation: the agent calls a contract tool and
//! the contract replies "accepted" or says precisely what is wrong.

pub mod apns;
pub mod card;
pub mod conformance;
pub mod contract;
pub mod device;
pub mod device_kit;
pub mod github_budget;
pub mod http;
pub mod kit;
pub mod netguard;
pub mod notify_channels;
pub mod notify_kit;
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

/// Time added to a session's deadline: queue time the agent could not work in (a
/// `run_check` or `mason run` waiting), and time granted because it kept making
/// progress. Shared with whoever grants it.
#[derive(Clone, Default)]
pub struct DeadlineExtra(
    Arc<std::sync::atomic::AtomicU64>,
    Option<Arc<std::path::PathBuf>>,
    Arc<std::sync::atomic::AtomicU64>,
    // The part of the queue credit that was spent waiting on GitHub (#799); in `waited` too.
    Arc<std::sync::atomic::AtomicU64>,
);

impl DeadlineExtra {
    /// Kept in `path` as well (s417): a restarted daemon that re-attaches to the same
    /// session reads back what was added before, instead of counting the whole wait as
    /// the agent's working time. A file that cannot be read starts at zero.
    pub fn kept_in(path: std::path::PathBuf) -> DeadlineExtra {
        let ms = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| t.trim().parse::<u64>().ok())
            .unwrap_or(0);
        DeadlineExtra(
            Arc::new(std::sync::atomic::AtomicU64::new(ms)),
            Some(Arc::new(path)),
            Arc::new(std::sync::atomic::AtomicU64::new(0)),
            Arc::new(std::sync::atomic::AtomicU64::new(0)),
        )
    }
    /// Queue time credited.
    pub fn add(&self, d: Duration) {
        let ms = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        let total = self.0.fetch_add(ms, Ordering::SeqCst).saturating_add(ms);
        if let Some(p) = &self.1 {
            let _ = std::fs::write(p.as_path(), total.to_string());
        }
    }
    /// Time credited for waiting on GitHub (a pool unit, a runner, the run): counted in
    /// [`waited`](Self::waited) as well, and apart in [`remote`](Self::remote).
    pub fn add_remote(&self, d: Duration) {
        self.add(d);
        let ms = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        self.3.fetch_add(ms, Ordering::SeqCst);
    }
    /// The part of the credit that was waiting on GitHub.
    pub fn remote(&self) -> Duration {
        Duration::from_millis(self.3.load(Ordering::SeqCst))
    }
    /// Put back the GitHub part of the credit after a restart.
    pub fn restore_remote(&self, d: Duration) {
        let ms = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        self.3.store(ms, Ordering::SeqCst);
    }
    /// Time granted for progress (commits or output).
    pub fn add_progress(&self, d: Duration) {
        let ms = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        self.2.fetch_add(ms, Ordering::SeqCst);
    }
    /// Put back what an earlier daemon had credited and granted for this step, so a
    /// restart does not lose it (s401).
    pub fn restore(&self, waited: Duration, progressed: Duration) {
        let ms = |d: Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        self.0.store(ms(waited), Ordering::SeqCst);
        self.2.store(ms(progressed), Ordering::SeqCst);
    }
    /// Queue time credited so far.
    pub fn waited(&self) -> Duration {
        Duration::from_millis(self.0.load(Ordering::SeqCst))
    }
    /// Time granted for progress so far.
    pub fn progressed(&self) -> Duration {
        Duration::from_millis(self.2.load(Ordering::SeqCst))
    }
    /// Everything added to the deadline.
    pub fn get(&self) -> Duration {
        self.waited() + self.progressed()
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
    /// Queue time of the agent's checks, added to `deadline` as it accrues.
    pub deadline_extra: DeadlineExtra,
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
    /// Told the model the CLI reports it really used, when the stream says.
    pub on_model: Option<ModelSink>,
    /// Told the invocation's cumulative token figures each time they change, with
    /// the host's id. The figures are cumulative per stream, so a journal read
    /// again from 0 rebuilds the same figure and the receiver replaces, never adds.
    pub on_tokens: Option<TokensSink>,
    /// Told of the agent's tool calls and todo lists as they arrive, before the
    /// turn ends (the runner calls it; it never holds a daemon type).
    pub on_agent_event: Option<AgentEventSink>,
    /// Called once per session invocation, on the first event that proves the
    /// provider accepted it: output tokens, text or a tool call.
    pub on_live: Option<LiveSink>,
    /// Silence limits for the session (see [`IdlePolicy`]).
    pub idle: Option<IdlePolicy>,
}

/// What to do about an agent that goes quiet. Silence is no new output from the
/// session and no contract call in flight. `0` turns a limit off.
#[derive(Clone)]
pub struct IdlePolicy {
    pub alert_after: Duration,
    pub stop_after: Duration,
    /// Told how long the session has been silent once past `alert_after`
    /// (`Some(ms)`), and `None` when output resumes or the session ends.
    pub report: IdleSink,
}

pub type IdleSink = std::sync::Arc<dyn Fn(Option<u64>) + Send + Sync>;

/// Receives the agent events that say what it is doing (`ToolCall`, `Plan`).
pub type AgentEventSink = std::sync::Arc<dyn Fn(&AgentEvent) + Send + Sync>;

/// Receives `(host id, cumulative figures)` while a session runs.
pub type TokensSink = std::sync::Arc<dyn Fn(&str, &TokenReport) + Send + Sync>;

/// Told once that the provider accepted the session.
pub type LiveSink = std::sync::Arc<dyn Fn() + Send + Sync>;

/// Receives the model name a session reports.
pub type ModelSink = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

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
            deadline_extra: DeadlineExtra::default(),
            contract: None,
            accepted: SubmissionFlag::default(),
            run_key: None,
            tools: None,
            progress: None,
            on_model: None,
            on_tokens: None,
            on_agent_event: None,
            on_live: None,
            idle: None,
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
    /// The agent CLI cannot authenticate (not logged in, a bad key). A provider
    /// hold like a limit, never a failure of the task: the owner logs in again.
    NotLoggedIn {
        detail: String,
    },
    Crash {
        detail: String,
    },
    /// The step host is gone, or never came up. Nothing was judged: the agent CLI
    /// did not crash, its process was lost with the host. Not a failure of the step.
    HostLost {
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
    /// The agent's own todo list, whole, each time it changes.
    Plan {
        items: Vec<PlanItem>,
    },
    /// A skill the agent called by name (Claude's `Skill` tool), before its `ToolCall`.
    Skill {
        name: String,
    },
    /// Final figures for this invocation, as reported by the stream.
    Tokens(TokenReport),
    /// The model the CLI says it is really using (Claude's `system/init`).
    Model(String),
    /// The provider's limits as the stream last told them.
    Usage(UsageSnapshot),
    ApiRetry {
        attempt: u32,
        status: Option<u32>,
        delay_ms: Option<u64>,
    },
    /// The context window's fill: the input of the latest message (uncached, cache read
    /// and cache write) against the model's window. Sent when the fill moves by a
    /// percentage point or more, or the window becomes known. Only from adapters whose
    /// stream says so.
    Context {
        used: u64,
        window: u64,
    },
    /// The CLI compacted the conversation. `subtype` is the `system` line's own, so a
    /// rename in a later CLI shows. The sizes are `None` when the stream did not say.
    Compacted {
        subtype: String,
        before: Option<u64>,
        after: Option<u64>,
    },
    /// Always the last event.
    Finished(Outcome),
}

/// One item of an agent's todo list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanItem {
    pub text: String,
    pub status: PlanStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanStatus {
    Pending,
    Doing,
    Done,
}

impl PlanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanStatus::Pending => "pending",
            PlanStatus::Doing => "doing",
            PlanStatus::Done => "done",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentCaps {
    pub resume: bool,
    pub mcp: bool,
    pub effort_levels: Vec<String>,
    pub usage_reports: bool,
    /// The adapter emits [`AgentEvent::Plan`] when the CLI exposes a todo list.
    pub plan_reports: bool,
}

#[derive(Debug)]
pub enum AdapterError {
    /// Refused by the test-mode guard.
    Net(netguard::NetError),
    Spawn(std::io::Error),
    /// The step host that was to run the CLI did not come up (it timed out, or
    /// exited at once). The CLI itself never ran.
    HostStart(std::io::Error),
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

#[cfg(test)]
mod deadline_extra_tests {
    use super::*;

    #[test]
    fn credit_kept_in_a_file_is_read_back_by_a_new_handle() {
        let dir = std::env::temp_dir().join(format!("mason-credit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("credit.txt");
        let a = DeadlineExtra::kept_in(path.clone());
        a.add(Duration::from_secs(90));
        a.add(Duration::from_secs(30));
        // What a restarted daemon re-attaching to the session builds.
        let b = DeadlineExtra::kept_in(path);
        assert_eq!(b.get(), Duration::from_secs(120));
        let none = DeadlineExtra::kept_in(dir.join("missing.txt"));
        assert_eq!(none.get(), Duration::ZERO);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
