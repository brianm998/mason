// SPDX-License-Identifier: GPL-3.0-or-later
//! Plugin adapters through the same conformance kits the built-in ones pass, a parser
//! and an environment over the protocol, and `step.cancel` reaching a running step.

use mason_adapter_api::kit::{build_kit, host_kit, tracker_kit, HostScenario};
use mason_adapter_api::service::*;
use mason_plugins::adapters::{PluginCi, PluginHost, PluginTracker};
use mason_plugins::host::{self, Launch, Plugin, StepRequest};
use mason_plugins::kit::plugin_kit;
use mason_plugins::manifest::{parse_manifest, MANIFEST_FILE};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Generous: only a broken plugin ever waits this long. No test asserts on elapsed time.
const T: Duration = Duration::from_secs(30);

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn launch(name: &str) -> (Launch, mason_plugins::Manifest) {
    let dir = fixture(name);
    let m = parse_manifest(&std::fs::read_to_string(dir.join(MANIFEST_FILE)).unwrap()).unwrap();
    (
        Launch {
            dir,
            run: m.run.clone(),
            log: None,
        },
        m,
    )
}

#[test]
fn the_fixture_plugins_pass_the_plugin_kit() {
    for name in ["adapters", "parser", "canceler"] {
        let checks = plugin_kit(&fixture(name), T);
        let failed: Vec<_> = checks.iter().filter(|c| !c.ok).collect();
        assert!(failed.is_empty(), "{name}: {failed:?}");
    }
}

#[test]
fn a_plugin_tracker_passes_tracker_kit() {
    let (l, m) = launch("adapters");
    let t = PluginTracker::new(l, &m.name, m.protocol, "fixture-adapters", T);
    assert_eq!(tracker_kit(&t), Vec::<String>::new());
    assert_eq!(t.name(), "plugin:fixture-adapters");
    let Ok(Listing::Items { items, .. }) = t.list(None) else {
        panic!("a listing")
    };
    assert_eq!(items[0].title, "First issue");
    assert_eq!(items[0].priority, "high");
    t.close(&items[0], "done").unwrap();
    let Ok(Listing::Items { items, .. }) = t.list(None) else {
        panic!("a listing")
    };
    assert_eq!(items.len(), 1);
}

#[test]
fn a_plugin_host_passes_host_kit() {
    let (l, m) = launch("adapters");
    let h = PluginHost::new(l, &m.name, m.protocol, "fixture-adapters", T);
    let s = HostScenario {
        base: "main".into(),
        good_head: "feature".into(),
        missing_head: "missing-branch".into(),
        reviewer: None,
    };
    assert_eq!(host_kit(&h, &s), Vec::<String>::new());
    // An error code maps to its kind.
    assert_eq!(h.pr_status(77).unwrap_err().kind, ErrorKind::NotFound);
    assert!(h.merge_pr(77).is_err());
}

#[test]
fn a_plugin_ci_passes_build_kit() {
    let (l, m) = launch("adapters");
    let c = PluginCi::new(l, &m.name, m.protocol, "fixture-adapters", T);
    assert_eq!(build_kit(&c, "refs/mason/x", "abc123"), Vec::<String>::new());
    assert!(c.doctor(Path::new("/tmp")).is_empty());
}

#[test]
fn an_adapter_the_plugin_does_not_provide_is_refused() {
    let (l, m) = launch("adapters");
    let t = PluginTracker::new(l, &m.name, m.protocol, "something-else", T);
    let e = t.list(None).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Invalid, "{e}");
    assert!(e.detail.contains("does not say it provides"), "{e}");
}

#[test]
fn a_parser_and_an_environment_answer_over_the_protocol() {
    let (l, m) = launch("parser");
    let mut p = Plugin::spawn(&l).unwrap();
    let init = host::initialize_as(&mut p, T, m.protocol).unwrap();
    assert!(init.provides.contains(&"parser:fixture-parser".to_string()));
    let v = p
        .call(
            "parser.parse",
            &json!({"text": "PASS a\nFAIL b: broke\nnoise\n"}),
            T,
        )
        .unwrap();
    assert_eq!(
        v["tests"],
        json!([
            {"name": "a", "passed": true},
            {"name": "b", "passed": false, "detail": "broke"}
        ])
    );
    let ok = p.call("environment.health", &json!({"config": {}}), T).unwrap();
    assert_eq!(ok["ok"], true);
    let sick = p
        .call("environment.health", &json!({"config": {"broken": true}}), T)
        .unwrap();
    assert_eq!(sick["ok"], false);
    p.shutdown(T).unwrap();
}

#[test]
fn cancel_reaches_a_running_step() {
    let (l, m) = launch("canceler");
    let mut p = Plugin::spawn(&l).unwrap();
    host::initialize_as(&mut p, T, m.protocol).unwrap();
    let req = StepRequest {
        step: "wait".into(),
        step_type: "wait-for-cancel".into(),
        config: json!({}),
        task: 1,
        title: "t".into(),
        worktree: "/tmp".into(),
        env: json!({}),
    };
    // The step does not answer until it is told to cancel. The flag turns on at the third
    // look, so the test counts looks, not seconds.
    let mut looks = 0;
    let r = host::step_run_cancelable(&mut p, &req, T, &mut |_| {}, &mut || {
        looks += 1;
        looks >= 3
    })
    .unwrap();
    assert!(!r.passed);
    assert_eq!(r.summary, "canceled");
    p.shutdown(T).unwrap();
}

#[test]
fn run_step_ends_a_canceled_step_as_the_plugin_answers() {
    let (l, _) = launch("canceler");
    let r = host::run_step(
        &l,
        "canceler",
        &StepRequest {
            step: "s".into(),
            step_type: "wait-for-cancel".into(),
            config: json!({}),
            task: 1,
            title: "t".into(),
            worktree: "/tmp".into(),
            env: json!({}),
        },
        3,
        T,
        T,
        &mut |_| {},
        &mut || true,
    )
    .unwrap();
    assert!(!r.passed);
    assert_eq!(r.summary, "canceled");
}
