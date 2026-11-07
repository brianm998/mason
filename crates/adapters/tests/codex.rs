// SPDX-License-Identifier: GPL-3.0-or-later

//! The Codex adapter through the same conformance kit as Claude's, driving a
//! stand-in CLI that prints `codex exec --json` events: directly, under a step
//! host, and picked up again after the controller is gone. Nothing here is the
//! real Codex (see docs/stages/s20.md).

use mason_adapter_api::conformance::{run_all, valid_plan, Harness, Scenario};
use mason_adapter_api::contract::{ContractHandle, NoChecks};
use mason_adapter_api::{netguard, AgentAdapter, AgentEvent, Outcome, SessionSpec, StepKind};
use mason_adapters::codex::CodexAdapter;
use mason_adapters::hosted::HostLaunch;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

mod common;

fn fixture(name: &str) -> String {
    format!("{}/fixtures/codex/{name}", env!("CARGO_MANIFEST_DIR"))
}

struct CodexHarness {
    dir: tempfile::TempDir,
    n: AtomicUsize,
    hosted: bool,
}

impl CodexHarness {
    fn new(hosted: bool) -> Self {
        netguard::enable_test_mode();
        CodexHarness {
            dir: tempfile::tempdir_in("/tmp").unwrap(),
            n: AtomicUsize::new(0),
            hosted,
        }
    }
}

impl Harness for CodexHarness {
    fn adapter(&self, _: Scenario) -> Box<dyn AgentAdapter> {
        let mut a = CodexAdapter::new(
            env!("CARGO_BIN_EXE_fake-codex"),
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
            Scenario::UsageLimit => json!([{"replay": fixture("usage_limit.jsonl")}]),
            Scenario::ProviderDown => json!([{"replay": fixture("provider_down.jsonl")}]),
            Scenario::Crash => json!([{"stderr": "segfault"}, {"exit": 139}]),
        };
        let n = self.n.load(Ordering::SeqCst);
        let script = self.dir.path().join(format!("script{n}.json"));
        std::fs::write(&script, json!({"turns": [turn]}).to_string()).unwrap();
        let mut spec = SessionSpec::new(StepKind::Explore, "explore this", self.dir.path());
        spec.env = vec![
            ("FAKE_CODEX_SCRIPT".into(), script.display().to_string()),
            ("OPENAI_BASE_URL".into(), "http://127.0.0.1:9".into()),
        ];
        spec.run_key = Some(format!("k{n}"));
        spec
    }
}

#[test]
fn the_codex_adapter_passes_the_conformance_kit_against_the_standin_cli() {
    run_all(&CodexHarness::new(false)).unwrap_or_else(|f| panic!("{}", f.join("\n")));
}

#[test]
fn the_codex_adapter_passes_the_conformance_kit_with_the_cli_under_a_step_host() {
    let h = CodexHarness::new(true);
    run_all(&h).unwrap_or_else(|f| panic!("{}", f.join("\n")));
    let hosts = std::fs::read_dir(h.dir.path().join("h"))
        .expect("hosts were really used")
        .count();
    assert!(hosts >= 8, "one host per scenario, got {hosts} entries");
}

#[test]
fn a_provider_endpoint_off_loopback_is_refused() {
    let h = CodexHarness::new(false);
    let mut spec = h.spec(Scenario::Accepted);
    spec.env = vec![(
        "OPENAI_BASE_URL".into(),
        "https://api.example.com/v1".into(),
    )];
    assert!(h.adapter(Scenario::Accepted).start(spec).is_err());
}

#[test]
fn a_restarted_controller_picks_the_codex_session_up_again() {
    let h = CodexHarness::new(true);
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
        ("FAKE_CODEX_SCRIPT".into(), script.display().to_string()),
        ("OPENAI_BASE_URL".into(), "http://127.0.0.1:9".into()),
    ];
    spec.run_key = Some("restart".into());
    let sock = h.dir.path().join("c.sock");
    let adapter = h.adapter(Scenario::Accepted);

    let c1 = ContractHandle::new(StepKind::Explore, Box::new(NoChecks));
    c1.serve(&sock).unwrap();
    let mut s1 = spec.clone();
    s1.contract = Some(c1.clone());
    s1.accepted = c1.flag();
    drop(adapter.start(s1).unwrap());
    c1.shutdown();
    drop(c1);

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
    assert!(c2.flag().is_set());
    assert!(adapter.reattach("never-started", spec).unwrap().is_none());
}
