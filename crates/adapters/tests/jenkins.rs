// SPDX-License-Identifier: GPL-3.0-or-later

//! The Jenkins adapter against recorded responses shaped as Jenkins' JSON API
//! documents them. No request leaves the process and no token is real.

use mason_adapter_api::http::Recorded;
use mason_adapter_api::kit::build_kit;
use mason_adapter_api::service::*;
use mason_adapters::github::TokenRef;
use mason_adapters::jenkins::Jenkins;
use std::sync::Arc;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/jenkins/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

struct J {
    j: Jenkins,
    http: Arc<Recorded>,
    _d: tempfile::TempDir,
}

fn jenkins(script: &str, crumb: bool) -> J {
    let d = tempfile::tempdir().unwrap();
    let tok = d.path().join("token");
    std::fs::write(&tok, "jenkins_recorded_test_token\n").unwrap();
    let http = Arc::new(Recorded::from_json(&fixture(script)).unwrap());
    let mut j = Jenkins::new(
        http.clone(),
        "https://ci.example.com/",
        "team/app",
        TokenRef::File(tok),
    );
    j.user = Some("mason".into());
    j.crumb = crumb;
    J { j, http, _d: d }
}

#[test]
fn jenkins_passes_the_build_kit_on_recorded_responses() {
    let g = jenkins("build_kit.json", true);
    let p = build_kit(&g.j, "refs/heads/mason/ci/x", "abc123");
    assert!(p.is_empty(), "{p:?}");
    assert!(g.http.unused().is_empty(), "{:?}", g.http.unused());
    let reqs = g.http.requests();
    let trigger = reqs.iter().find(|r| r.method == "POST").unwrap();
    assert!(
        trigger.url.ends_with(
            "/job/team/job/app/buildWithParameters?REF=refs%2Fheads%2Fmason%2Fci%2Fx&SHA=abc123"
        ),
        "{}",
        trigger.url
    );
    assert!(trigger
        .headers
        .iter()
        .any(|(k, v)| k == "Jenkins-Crumb" && v == "crumb-value"));
    // The credential travels as a header, basic auth with the user, never in the URL.
    let auth = &trigger
        .headers
        .iter()
        .find(|(k, _)| k == "Authorization")
        .unwrap()
        .1;
    assert!(auth.starts_with("Basic "));
    assert!(reqs.iter().all(|r| !r.url.contains("jenkins_recorded")));
}

#[test]
fn a_build_is_pending_then_green_or_red() {
    let g = jenkins("states.json", false);
    assert_eq!(
        g.j.status("queue:41").unwrap(),
        CiState::Pending,
        "waiting in the queue"
    );
    assert_eq!(
        g.j.status("queue:42").unwrap(),
        CiState::Pending,
        "building"
    );
    assert_eq!(g.j.status("7").unwrap(), CiState::Green);
    match g.j.status("8").unwrap() {
        CiState::Red { summary, .. } => assert!(summary.contains("UNSTABLE"), "{summary}"),
        other => panic!("{other:?}"),
    }
    assert!(g.j.status("nonsense").unwrap_err().permanent());
}

#[test]
fn errors_are_classified_and_an_outage_is_unavailable() {
    let g = jenkins("errors.json", false);
    assert_eq!(g.j.trigger("r", "s").unwrap_err().kind, ErrorKind::Auth);
    assert_eq!(g.j.status("9").unwrap_err().kind, ErrorKind::NotFound);
    assert_eq!(g.j.status("10").unwrap_err().kind, ErrorKind::Unavailable);
}
