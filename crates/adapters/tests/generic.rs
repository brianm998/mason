// SPDX-License-Identifier: GPL-3.0-or-later

//! The generic build adapter, each half as a command and as a URL (recorded
//! responses, no network), through the build-system conformance kit.

use mason_adapter_api::http::Recorded;
use mason_adapter_api::kit::build_kit;
use mason_adapter_api::service::*;
use mason_adapters::generic::{Generic, Half};
use std::sync::Arc;

fn cmd(c: &str) -> Half {
    Half::Command(c.into())
}

#[test]
fn command_halves_pass_the_build_kit_and_get_the_environment() {
    let d = tempfile::tempdir().unwrap();
    let g = Generic::new(
        cmd("echo \"run-$MASON_SHA\"; echo \"$MASON_REF\" > ref.txt"),
        cmd("echo green"),
        d.path(),
    );
    let p = build_kit(&g, "refs/heads/mason/ci/x", "abc");
    assert!(p.is_empty(), "{p:?}");
    assert_eq!(g.trigger("r", "abc").unwrap(), "run-abc");
    assert_eq!(
        std::fs::read_to_string(d.path().join("ref.txt"))
            .unwrap()
            .trim(),
        "r"
    );
}

#[test]
fn command_status_reads_red_and_its_failed_jobs() {
    let g = Generic::new(
        cmd("true"),
        cmd(
            "if [ \"$MASON_RUN\" = bad ]; then printf 'red\\nlint, unit\\n'; else echo pending; fi",
        ),
        ".",
    );
    assert_eq!(
        g.trigger("r", "sha1").unwrap(),
        "sha1",
        "no output: the commit is the id"
    );
    assert_eq!(g.status("ok").unwrap(), CiState::Pending);
    assert_eq!(
        g.status("bad").unwrap(),
        CiState::Red {
            failed: vec!["lint".into(), "unit".into()],
            summary: "lint, unit failed".into(),
            jobs: vec![],
        }
    );
}

#[test]
fn a_failing_command_is_unavailable() {
    let g = Generic::new(cmd("exit 3"), cmd("true"), ".");
    assert_eq!(
        g.trigger("r", "s").unwrap_err().kind,
        ErrorKind::Unavailable
    );
}

#[test]
fn url_halves_pass_the_build_kit_on_recorded_responses() {
    let http = Arc::new(
        Recorded::from_json(
            r#"[
          {"method": "POST", "url": "https://ci.example.com/start?ref=refs%2Fheads%2Fx&sha=abc",
           "response": {"status": 200, "body": "build-9\n"}},
          {"method": "GET", "url": "https://ci.example.com/status/build-9",
           "repeat": true,
           "response": {"status": 200, "body": {"state": "red", "failed": ["unit"]}}}
        ]"#,
        )
        .unwrap(),
    );
    let g = Generic::new(
        Half::Url("https://ci.example.com/start?ref={ref}&sha={sha}".into()),
        Half::Url("https://ci.example.com/status/{run}".into()),
        ".",
    )
    .with_http(http.clone());
    let p = build_kit(&g, "refs/heads/x", "abc");
    assert!(p.is_empty(), "{p:?}");
    assert!(http.unused().is_empty(), "{:?}", http.unused());
    assert_eq!(
        g.status("build-9").unwrap(),
        CiState::Red {
            failed: vec!["unit".into()],
            summary: "unit failed".into(),
            jobs: vec![],
        }
    );
}

#[test]
fn halves_can_be_mixed_and_a_url_error_is_classified() {
    let http = Arc::new(
        Recorded::from_json(
            r#"[{"method": "GET", "url": "/status/", "response": {"status": 404, "body": ""}}]"#,
        )
        .unwrap(),
    );
    let g = Generic::new(
        cmd("echo r1"),
        Half::Url("https://ci.example.com/status/{run}".into()),
        ".",
    )
    .with_http(http);
    assert_eq!(g.trigger("r", "s").unwrap(), "r1");
    assert_eq!(g.status("r1").unwrap_err().kind, ErrorKind::NotFound);
}

#[test]
fn a_url_half_without_a_client_is_refused() {
    let g = Generic::new(Half::Url("https://x/{sha}".into()), cmd("true"), ".");
    assert!(g.trigger("r", "s").unwrap_err().permanent());
}
