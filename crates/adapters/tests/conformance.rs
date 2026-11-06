// SPDX-License-Identifier: GPL-3.0-or-later

//! Both adapters through the same conformance kit: the scripted fake, and the
//! Claude adapter driving a stand-in CLI that speaks the real stream format.

use mason_adapter_api::conformance::{run_all, valid_plan, Harness, Scenario};
use mason_adapter_api::contract::ContractHandle;
use mason_adapter_api::{netguard, AgentAdapter, SessionSpec, StepKind};
use mason_adapters::claude::ClaudeAdapter;
use mason_adapters::fake::{FakeAction, FakeAdapter};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

fn call(tool: &str, args: serde_json::Value) -> FakeAction {
    FakeAction::Call {
        tool: tool.into(),
        args,
    }
}

struct FakeHarness;

impl Harness for FakeHarness {
    fn adapter(&self, s: Scenario) -> Box<dyn AgentAdapter> {
        let turn = match s {
            Scenario::Accepted => vec![call("submit_plan", valid_plan())],
            Scenario::RejectedThenFixed => {
                vec![
                    call("submit_plan", json!({"summary": ""})),
                    call("submit_plan", valid_plan()),
                ]
            }
            Scenario::NoResult => vec![FakeAction::Say("hmm".into())],
            Scenario::Timeout | Scenario::Hang => vec![FakeAction::Sleep(Duration::from_secs(60))],
            Scenario::UsageLimit => vec![FakeAction::UsageLimit(Some("3pm".into()))],
            Scenario::ProviderDown => vec![
                FakeAction::ApiRetry { attempt: 1 },
                FakeAction::ProviderDown("529".into()),
            ],
            Scenario::Crash => vec![FakeAction::Crash("boom".into())],
        };
        Box::new(FakeAdapter::new(vec![turn]))
    }

    fn spec(&self, _: Scenario) -> SessionSpec {
        SessionSpec::new(StepKind::Explore, "explore", std::env::temp_dir())
    }
}

#[test]
fn the_fake_agent_passes_the_conformance_kit() {
    run_all(&FakeHarness).unwrap_or_else(|f| panic!("{}", f.join("\n")));
}

fn fixture(name: &str) -> String {
    format!("{}/fixtures/claude/{name}", env!("CARGO_MANIFEST_DIR"))
}

struct ClaudeHarness {
    dir: tempfile::TempDir,
    n: AtomicUsize,
}

impl ClaudeHarness {
    fn new() -> Self {
        // Test mode: the adapter refuses any provider endpoint that is not loopback.
        netguard::enable_test_mode();
        ClaudeHarness {
            dir: tempfile::tempdir().unwrap(),
            n: AtomicUsize::new(0),
        }
    }
}

impl Harness for ClaudeHarness {
    fn adapter(&self, _: Scenario) -> Box<dyn AgentAdapter> {
        let mut a = ClaudeAdapter::new(
            env!("CARGO_BIN_EXE_fake-claude"),
            env!("CARGO_BIN_EXE_mason-mcp"),
            self.dir.path().join("state"),
        );
        a.parent_env = Some(vec![(
            "PATH".into(),
            std::env::var("PATH").unwrap_or_default(),
        )]);
        Box::new(a)
    }

    fn prepare(&self, c: &ContractHandle) {
        let n = self.n.fetch_add(1, Ordering::SeqCst);
        c.serve(&self.dir.path().join(format!("c{n}.sock")))
            .unwrap();
    }

    fn spec(&self, s: Scenario) -> SessionSpec {
        let turn = match s {
            Scenario::Accepted => json!([{"call": {"tool": "submit_plan", "args": valid_plan()}}]),
            Scenario::RejectedThenFixed => json!([
                {"call": {"tool": "submit_plan", "args": {"summary": ""}}},
                {"call": {"tool": "submit_plan", "args": valid_plan()}}]),
            Scenario::NoResult => json!([{"say": "I am not sure what to do"}]),
            Scenario::Timeout | Scenario::Hang => json!([{"sleep_ms": 60000}]),
            Scenario::UsageLimit => json!([{"replay": fixture("synthetic_usage_limit.jsonl")}]),
            Scenario::ProviderDown => json!([{"replay": fixture("synthetic_provider_down.jsonl")}]),
            Scenario::Crash => json!([{"stderr": "segfault"}, {"exit": 139}]),
        };
        let n = self.n.load(Ordering::SeqCst);
        let script = self.dir.path().join(format!("script{n}.json"));
        std::fs::write(&script, json!({"turns": [turn]}).to_string()).unwrap();
        let mut spec = SessionSpec::new(StepKind::Explore, "explore this", self.dir.path());
        spec.env = vec![
            ("FAKE_CLAUDE_SCRIPT".into(), script.display().to_string()),
            ("ANTHROPIC_BASE_URL".into(), "http://127.0.0.1:9".into()),
        ];
        spec
    }
}

#[test]
fn the_claude_adapter_passes_the_conformance_kit_against_the_standin_cli() {
    run_all(&ClaudeHarness::new()).unwrap_or_else(|f| panic!("{}", f.join("\n")));
}

#[test]
fn a_captured_real_run_replayed_through_the_adapter_reports_real_figures() {
    use mason_adapter_api::contract::NoChecks;
    use mason_adapters::runner::{FailReason, RunnerConfig, StepEnd, StepRunner};
    let h = ClaudeHarness::new();
    let script = h.dir.path().join("real.json");
    std::fs::write(
        &script,
        json!({"turns": [[{"replay": fixture("real_plain.jsonl")}]]}).to_string(),
    )
    .unwrap();
    let mut spec = SessionSpec::new(StepKind::Explore, "say pong", h.dir.path());
    spec.env = vec![
        ("FAKE_CLAUDE_SCRIPT".into(), script.display().to_string()),
        ("ANTHROPIC_BASE_URL".into(), "http://127.0.0.1:9".into()),
    ];
    let contract = ContractHandle::new(StepKind::Explore, Box::new(NoChecks));
    contract.serve(&h.dir.path().join("real.sock")).unwrap();
    let a = h.adapter(Scenario::Accepted);
    let r = StepRunner::new(a.as_ref(), RunnerConfig { max_nudges: 1 }).run(spec, &contract);
    // The real run never calls a contract tool, so it is nudged once and then fails.
    assert_eq!(r.end, StepEnd::Failed(FailReason::NoResult { nudges: 1 }));
    assert_eq!(r.tokens.invocations, 2);
    assert!(
        r.tokens.cache_read.unwrap() > 20_000,
        "the real capture's cache figures: {:?}",
        r.tokens
    );
    let first = std::fs::read_to_string(fixture("real_plain.jsonl")).unwrap();
    let first: serde_json::Value = serde_json::from_str(first.lines().next().unwrap()).unwrap();
    let real_id = first["session_id"].as_str().unwrap().to_string();
    assert_eq!(
        r.session_id,
        Some(real_id),
        "the session id comes from the captured stream"
    );
}
