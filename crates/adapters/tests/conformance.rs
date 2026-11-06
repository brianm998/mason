// SPDX-License-Identifier: GPL-3.0-or-later

//! Both adapters through the same conformance kit: the scripted fake, and the
//! Claude adapter driving a stand-in CLI that speaks the real stream format.

use mason_adapter_api::conformance::{run_all, valid_plan, Harness, Scenario};
use mason_adapter_api::contract::ContractHandle;
use mason_adapter_api::{netguard, AgentAdapter, SessionSpec, StepKind};
use mason_adapters::claude::ClaudeAdapter;
use mason_adapters::fake::{FakeAction, FakeAdapter};
use mason_adapters::hosted::HostLaunch;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

mod common;

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
    hosted: bool,
}

impl ClaudeHarness {
    fn new() -> Self {
        Self::build(false)
    }

    /// The CLI runs under a step host, as it does in the daemon.
    fn hosted() -> Self {
        Self::build(true)
    }

    fn build(hosted: bool) -> Self {
        // Test mode: the adapter refuses any provider endpoint that is not loopback.
        netguard::enable_test_mode();
        // Short paths: a host's socket lives under the registry root.
        ClaudeHarness {
            dir: tempfile::tempdir_in("/tmp").unwrap(),
            n: AtomicUsize::new(0),
            hosted,
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
        if self.hosted {
            a = a.with_host(HostLaunch {
                host_bin: common::step_host_bin(),
                root: self.dir.path().join("h"),
                heartbeat: Duration::from_millis(100),
                linger: Duration::from_secs(30),
            });
        }
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
        spec.run_key = Some(format!("k{n}"));
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

#[test]
fn the_claude_adapter_passes_the_conformance_kit_with_the_cli_under_a_step_host() {
    let h = ClaudeHarness::hosted();
    run_all(&h).unwrap_or_else(|f| panic!("{}", f.join("\n")));
    let hosts = std::fs::read_dir(h.dir.path().join("h"))
        .expect("hosts were really used")
        .count();
    assert!(hosts >= 8, "one host per scenario, got {hosts} entries");
}

#[test]
fn a_restarted_controller_picks_the_session_up_again() {
    use mason_adapter_api::contract::NoChecks;
    use mason_adapter_api::{AgentEvent, Outcome};
    let h = ClaudeHarness::hosted();
    let script = h.dir.path().join("slow.json");
    std::fs::write(
        &script,
        json!({"turns": [[
            {"say": "working"}, {"sleep_ms": 1200},
            {"call": {"tool": "submit_plan", "args": valid_plan()}}]]})
        .to_string(),
    )
    .unwrap();
    let mut spec = SessionSpec::new(StepKind::Explore, "explore this", h.dir.path());
    spec.env = vec![
        ("FAKE_CLAUDE_SCRIPT".into(), script.display().to_string()),
        ("ANTHROPIC_BASE_URL".into(), "http://127.0.0.1:9".into()),
    ];
    spec.run_key = Some("restart".into());
    let sock = h.dir.path().join("c.sock");
    let adapter = h.adapter(Scenario::Accepted);

    // First controller: starts the session, then "dies" (its handle is dropped).
    let c1 = ContractHandle::new(StepKind::Explore, Box::new(NoChecks));
    c1.serve(&sock).unwrap();
    let mut s1 = spec.clone();
    s1.contract = Some(c1.clone());
    s1.accepted = c1.flag();
    let first = adapter.start(s1).unwrap();
    drop(first);
    c1.shutdown();
    drop(c1);

    // The agent is still running and its contract call is retrying. A new
    // controller serves the same socket and reattaches by run key.
    std::thread::sleep(Duration::from_millis(300));
    let c2 = ContractHandle::new(StepKind::Explore, Box::new(NoChecks));
    c2.serve(&sock).unwrap();
    let mut s2 = spec.clone();
    s2.contract = Some(c2.clone());
    s2.accepted = c2.flag();
    let mut again = adapter
        .reattach("restart", s2)
        .unwrap()
        .expect("a session to reattach to");
    let mut saw_text = false;
    let outcome = loop {
        match again.next_event().unwrap() {
            AgentEvent::Text(t) if t == "working" => saw_text = true,
            AgentEvent::Finished(o) => break o,
            _ => {}
        }
    };
    assert!(saw_text, "the journal is replayed from the start");
    assert_eq!(outcome, Outcome::Accepted);
    assert!(
        c2.flag().is_set(),
        "the new controller received the submission"
    );
    assert!(adapter.reattach("never-started", spec).unwrap().is_none());
}
