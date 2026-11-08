// SPDX-License-Identifier: GPL-3.0-or-later

//! The scripted fake agent: an in-process [`AgentAdapter`] that plays a script,
//! one "turn" per invocation (start, then each resume). It is what the runner
//! tests drive, and it passes the same conformance kit as the real adapter.

use mason_adapter_api::{
    AdapterError, AgentAdapter, AgentCaps, AgentEvent, BillingKind, Canceller, Outcome,
    SessionHandle, SessionSpec, TokenReport, UsageModel, UsageSnapshot,
};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub enum FakeAction {
    Say(String),
    /// Call a contract tool directly, as the agent would over MCP.
    Call {
        tool: String,
        args: Value,
    },
    /// Emit a usage snapshot.
    Usage(UsageSnapshot),
    ApiRetry {
        attempt: u32,
    },
    Sleep(Duration),
    /// End the invocation with a provider usage limit.
    UsageLimit(Option<String>),
    ProviderDown(String),
    Crash(String),
}

pub type Turn = Vec<FakeAction>;

pub struct FakeAdapter {
    turns: Vec<Turn>,
    next: AtomicUsize,
    tokens: Option<TokenReport>,
    notes: Arc<Mutex<Vec<String>>>,
    model: UsageModel,
}

impl FakeAdapter {
    pub fn new(turns: Vec<Turn>) -> FakeAdapter {
        FakeAdapter {
            turns,
            next: AtomicUsize::new(0),
            tokens: Some(TokenReport {
                input: Some(10),
                output: Some(5),
                cache_read: Some(100),
                cache_create: Some(20),
                cost_usd: Some(0.001),
            }),
            notes: Arc::default(),
            model: UsageModel::default(),
        }
    }

    /// What each invocation reports (or `None`: the stream reports nothing).
    pub fn with_tokens(mut self, t: Option<TokenReport>) -> Self {
        self.tokens = t;
        self
    }

    /// What this fake says it can report (the same for either billing kind).
    pub fn with_usage_model(mut self, m: UsageModel) -> Self {
        self.model = m;
        self
    }

    /// The resume notes received, oldest first.
    pub fn notes(&self) -> Vec<String> {
        self.notes.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn invocations(&self) -> usize {
        self.next.load(Ordering::SeqCst)
    }

    fn run_turn(
        &self,
        id: String,
        spec: SessionSpec,
    ) -> Result<Box<dyn SessionHandle>, AdapterError> {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        let script = self.turns.get(n).cloned().unwrap_or_default();
        let (tx, rx) = channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let tokens = self.tokens.clone();
        let c2 = cancelled.clone();
        thread::spawn(move || {
            let start = Instant::now();
            let over = |c: &AtomicBool| -> Option<Outcome> {
                if spec.deadline.is_some_and(|d| start.elapsed() >= d) {
                    return Some(Outcome::Timeout);
                }
                c.load(Ordering::SeqCst).then_some(Outcome::NoResult)
            };
            let mut end: Option<Outcome> = None;
            for action in script {
                if let Some(o) = over(&c2) {
                    end = Some(o);
                    break;
                }
                match action {
                    FakeAction::Say(t) => {
                        let _ = tx.send(AgentEvent::Text(t));
                    }
                    FakeAction::Call { tool, args } => {
                        let _ = tx.send(AgentEvent::ToolCall {
                            name: format!("mcp__mason__{tool}"),
                            summary: args.to_string(),
                        });
                        if let Some(c) = &spec.contract {
                            c.call(&tool, &args);
                        }
                    }
                    FakeAction::Usage(u) => {
                        let _ = tx.send(AgentEvent::Usage(u));
                    }
                    FakeAction::ApiRetry { attempt } => {
                        let _ = tx.send(AgentEvent::ApiRetry {
                            attempt,
                            status: Some(529),
                            delay_ms: Some(1),
                        });
                    }
                    FakeAction::Sleep(d) => {
                        let until = Instant::now() + d;
                        while Instant::now() < until {
                            if let Some(o) = over(&c2) {
                                end = Some(o);
                                break;
                            }
                            thread::sleep(Duration::from_millis(5));
                        }
                        if end.is_some() {
                            break;
                        }
                    }
                    FakeAction::UsageLimit(r) => {
                        end = Some(Outcome::UsageLimit { resets_at: r });
                        break;
                    }
                    FakeAction::ProviderDown(d) => {
                        end = Some(Outcome::ProviderDown { detail: d });
                        break;
                    }
                    FakeAction::Crash(d) => {
                        end = Some(Outcome::Crash { detail: d });
                        break;
                    }
                }
            }
            if let Some(t) = tokens {
                let _ = tx.send(AgentEvent::Tokens(t));
            }
            let outcome = end.unwrap_or_else(|| {
                if spec.accepted.is_set() {
                    Outcome::Accepted
                } else {
                    Outcome::NoResult
                }
            });
            let _ = tx.send(AgentEvent::Finished(outcome));
        });
        Ok(Box::new(FakeSession {
            rx,
            id,
            cancelled,
            finished: false,
        }))
    }
}

impl AgentAdapter for FakeAdapter {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn capabilities(&self) -> AgentCaps {
        AgentCaps {
            resume: true,
            mcp: true,
            effort_levels: vec![],
            usage_reports: self.tokens.is_some(),
        }
    }

    fn usage_model(&self, _billing: BillingKind) -> UsageModel {
        self.model.clone()
    }

    fn start(&self, spec: SessionSpec) -> Result<Box<dyn SessionHandle>, AdapterError> {
        self.run_turn("fake-session-1".into(), spec)
    }

    fn resume(
        &self,
        id: &str,
        note: String,
        spec: SessionSpec,
    ) -> Result<Box<dyn SessionHandle>, AdapterError> {
        self.notes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(note);
        self.run_turn(id.into(), spec)
    }
}

struct FakeSession {
    rx: Receiver<AgentEvent>,
    id: String,
    cancelled: Arc<AtomicBool>,
    finished: bool,
}

impl SessionHandle for FakeSession {
    fn session_id(&self) -> Option<String> {
        Some(self.id.clone())
    }

    fn next_event(&mut self) -> Option<AgentEvent> {
        if self.finished {
            return None;
        }
        let e = self
            .rx
            .recv()
            .unwrap_or(AgentEvent::Finished(Outcome::Crash {
                detail: "fake lost".into(),
            }));
        if matches!(e, AgentEvent::Finished(_)) {
            self.finished = true;
        }
        Some(e)
    }

    fn canceller(&self) -> Canceller {
        let c = self.cancelled.clone();
        Arc::new(move || c.store(true, Ordering::SeqCst))
    }
}
