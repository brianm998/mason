// SPDX-License-Identifier: GPL-3.0-or-later

//! Test mode refuses any non-loopback address. This file is its own test
//! binary because test mode is process-wide.

use mason_adapter_api::netguard::{self, NetError};
use mason_adapter_api::{AdapterError, AgentAdapter, SessionSpec, StepKind};
use mason_adapters::claude::ClaudeAdapter;
use std::net::TcpListener;

fn adapter(dir: &std::path::Path) -> ClaudeAdapter {
    let mut a = ClaudeAdapter::new(
        env!("CARGO_BIN_EXE_fake-claude"),
        env!("CARGO_BIN_EXE_mason-mcp"),
        dir.join("state"),
    );
    a.parent_env = Some(vec![]);
    a
}

fn spec(dir: &std::path::Path, base_url: Option<&str>) -> SessionSpec {
    let mut s = SessionSpec::new(StepKind::Explore, "p", dir);
    let script = dir.join("s.json");
    std::fs::write(&script, r#"{"turns":[[]]}"#).unwrap();
    s.env
        .push(("FAKE_CLAUDE_SCRIPT".into(), script.display().to_string()));
    if let Some(u) = base_url {
        s.env.push(("ANTHROPIC_BASE_URL".into(), u.into()));
    }
    s
}

#[test]
fn a_real_address_call_fails_in_test_mode() {
    netguard::enable_test_mode();
    assert!(netguard::test_mode());

    // Direct checks and the guarded connect.
    for real in [
        "api.anthropic.com:443",
        "https://api.github.com",
        "8.8.8.8:53",
        "example.com",
    ] {
        assert_eq!(
            netguard::check(real),
            Err(NetError::NonLoopback(real.into())),
            "{real}"
        );
    }
    let e = netguard::connect("example.com:80").unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);

    // Loopback works.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    assert!(netguard::check(&addr).is_ok());
    assert!(netguard::connect(&addr).is_ok());

    // The Claude adapter will not start against the real API, nor against the
    // default endpoint it would silently use if none were set.
    let d = tempfile::tempdir().unwrap();
    let a = adapter(d.path());
    match a.start(spec(d.path(), Some("https://api.anthropic.com"))) {
        Err(AdapterError::Net(NetError::NonLoopback(u))) => assert!(u.contains("anthropic.com")),
        other => panic!("expected a refusal, got {:?}", other.map(|_| "started")),
    }
    match a.start(spec(d.path(), None)) {
        Err(AdapterError::Net(NetError::EndpointRequired(v))) => {
            assert_eq!(v, "ANTHROPIC_BASE_URL")
        }
        other => panic!("expected a refusal, got {:?}", other.map(|_| "started")),
    }
    // A loopback endpoint is accepted.
    assert!(a.start(spec(d.path(), Some("http://127.0.0.1:9"))).is_ok());
}
