// SPDX-License-Identifier: GPL-3.0-or-later

//! The Project page's daemon side (s1232): the settings catalogue lists every key with its
//! effective value and source, each writable key can be set and unset with comments intact, a
//! refused value leaves the file byte for byte, no response holds a secret, a viewer writes
//! nothing, a paired device (an owner's included) cannot write the keys that run a program or
//! weaken a guard, and a tier-mapping edit changes what the next launch of a step would use.

use crate::support::*;
use mason_daemon::{projsettings, Daemon};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

fn call(
    d: &Arc<Daemon>,
    method: &str,
    who: &str,
    paired: bool,
    path: &str,
    body: Value,
) -> (u16, Value) {
    let d = d.clone();
    let (method, who, path) = (method.to_string(), who.to_string(), path.to_string());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            use tower::ServiceExt;
            let mut req = axum::http::Request::builder()
                .method(method.as_str())
                .uri(path)
                .header("content-type", "application/json")
                .header("x-mason-as", who);
            if paired {
                req = req.header("x-mason-paired", "1");
            }
            let req = req.body(axum::body::Body::from(body.to_string())).unwrap();
            let resp = mason_daemon::http::api_router(d)
                .oneshot(req)
                .await
                .unwrap();
            let status = resp.status().as_u16();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            )
        })
}

const EXTRA: &str = r#"# keep this comment
tool_timeout_s = 900 # trailing note
[proposals]
agent_normal = "proposed"

[tracker]
kind = "github"
api = "http://127.0.0.1:9"
repo = "o/r"
token = "env:PROJSETTINGS_TEST_TOKEN"

[mystery]
colour = "green"
api_token = "hunter2-literal"
"#;

fn world() -> (Scratch, Arc<Daemon>) {
    let s = Scratch::new("true", json!({"steps": {}}));
    s.write_project("scratch", "true", EXTRA);
    // A secret-looking environment variable next to the fake agent's own.
    let f = s.config_dir.join("projects/scratch.toml");
    let t = std::fs::read_to_string(&f).unwrap();
    std::fs::write(&f, format!("{t}MY_API_TOKEN = \"supersecret-value\"\n")).unwrap();
    let d = s.start_with(|c| {
        c.owner_names = vec!["brian".into()];
        c.reload_interval = Duration::from_secs(3600);
    });
    (s, d)
}

fn file(s: &Scratch) -> String {
    std::fs::read_to_string(s.config_dir.join("projects/scratch.toml")).unwrap()
}

const URL: &str = "/projects/scratch/settings";

fn get(d: &Arc<Daemon>, who: &str, paired: bool) -> Value {
    let (code, v) = call(d, "GET", who, paired, URL, json!(null));
    assert_eq!(code, 200, "{v}");
    v["catalogue"].clone()
}

fn entry(cat: &Value, key: &str) -> Value {
    cat["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["entries"].as_array().unwrap().iter())
        .find(|e| e["key"] == key)
        .unwrap_or_else(|| panic!("no entry {key}"))
        .clone()
}

fn set(d: &Arc<Daemon>, who: &str, paired: bool, body: Value) -> (u16, Value) {
    call(d, "POST", who, paired, URL, body)
}

#[test]
fn the_catalogue_lists_every_key_with_its_effective_value_and_source() {
    let (_s, d) = world();
    let cat = get(&d, "brian", false);
    let ids: Vec<&str> = cat["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "project",
            "models",
            "limits",
            "testing",
            "runs",
            "services",
            "proposals"
        ]
    );
    for k in projsettings::KEYS.iter().filter(|k| !k.path.contains('*')) {
        entry(&cat, k.path);
    }
    let tool = entry(&cat, "tool_timeout_s");
    assert_eq!(
        (tool["effective"].clone(), tool["source"].clone()),
        (json!(900), json!("project file"))
    );
    let step = entry(&cat, "step_timeout_s");
    assert_eq!(step["effective"], 120);
    let idle = entry(&cat, "agent_idle_alert_s");
    assert_eq!(
        (idle["effective"].clone(), idle["source"].clone()),
        (json!(2700), json!("default"))
    );
    assert_eq!(entry(&cat, "provider")["effective"], "claude");
    assert_eq!(
        entry(&cat, "proposals.agent_normal")["effective"],
        "proposed"
    );
    assert_eq!(entry(&cat, "tracker.interval_s")["effective"], 60);
    assert_eq!(entry(&cat, "path")["writable"], false);
    assert_eq!(entry(&cat, "test")["sensitivity"], "runs_a_program");
    assert_eq!(entry(&cat, "tool_timeout_s")["effect"], "next_step_run");
    // A key the catalogue does not know is listed, read-only, and hidden if it looks secret.
    let un = cat["unrecognised"].as_array().unwrap();
    let colour = un.iter().find(|u| u["key"] == "mystery.colour").unwrap();
    assert_eq!(
        (colour["written"].clone(), colour["writable"].clone()),
        (json!("green"), json!(false))
    );
    let tok = un.iter().find(|u| u["key"] == "mystery.api_token").unwrap();
    assert_eq!(tok["written"], "set in file, not shown");
}

#[test]
fn the_models_view_shows_a_real_model_and_where_it_came_from_for_each_agent_step() {
    let (_s, d) = world();
    let m = get(&d, "brian", false)["models"].clone();
    let steps = m["steps"].as_array().unwrap();
    let develop = steps
        .iter()
        .find(|s| s["step"] == "develop")
        .expect("develop row");
    assert_eq!(develop["model"]["value"], "sonnet");
    assert!(
        develop["model"]["from"]
            .as_str()
            .unwrap()
            .contains("built-in"),
        "{develop}"
    );
    let explore = steps
        .iter()
        .find(|s| s["step"] == "explore")
        .expect("explore row");
    assert_eq!(explore["model"]["value"], "haiku");
    assert_eq!(m["grid"].as_array().unwrap().len(), 6);
    assert!(m["diagnose"]["source"].is_string());
    assert!(m["escalation"]["model"].is_string());
}

#[test]
fn a_tier_mapping_edit_changes_the_next_launch_and_unsetting_it_puts_it_back() {
    let (s, d) = world();
    let before = file(&s);
    let (code, v) = set(
        &d,
        "brian",
        false,
        json!({"set": {"provider.claude.light.medium.model": "sonnet"}}),
    );
    assert_eq!(code, 200, "{v}");
    assert!(file(&s).contains("[provider.claude.light.medium]"));
    assert!(file(&s).contains("# keep this comment"));
    let m = get(&d, "brian", false)["models"].clone();
    let explore = m["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["step"] == "explore")
        .unwrap()
        .clone();
    assert_eq!(explore["model"]["value"], "sonnet", "{explore}");
    assert!(
        explore["model"]["from"]
            .as_str()
            .unwrap()
            .contains("project file"),
        "{explore}"
    );
    let cell = m["grid"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["tier"] == "light" && c["level"] == "medium")
        .unwrap()
        .clone();
    assert_eq!(
        (cell["source"].clone(), cell["entry"]["model"].clone()),
        (json!("project file"), json!("sonnet"))
    );
    let (code, v) = set(
        &d,
        "brian",
        false,
        json!({"set": {"provider.claude.light.medium.model": null}}),
    );
    assert_eq!(code, 200, "{v}");
    assert_eq!(
        file(&s),
        before,
        "unsetting the only entry leaves the file as it was"
    );
    let m = get(&d, "brian", false)["models"].clone();
    let explore = m["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["step"] == "explore")
        .unwrap()
        .clone();
    assert_eq!(explore["model"]["value"], "haiku");
}

#[test]
fn a_provider_string_becomes_use_when_a_mapping_table_is_written() {
    let (s, d) = world();
    let (code, v) = set(&d, "brian", false, json!({"set": {"provider": "codex"}}));
    assert_eq!(code, 200, "{v}");
    assert!(file(&s).contains("provider = \"codex\""));
    let (code, v) = set(
        &d,
        "brian",
        false,
        json!({"set": {"provider.codex.deep.high.effort": "high"}}),
    );
    assert_eq!(code, 200, "{v}");
    let t = file(&s);
    assert!(
        t.contains("[provider]") && t.contains("use = \"codex\""),
        "{t}"
    );
    assert!(!t.contains("provider = \"codex\""), "{t}");
    mason_daemon::config::parse_project("scratch", &t).expect("the file still loads");
    assert_eq!(
        entry(&get(&d, "brian", false), "provider")["effective"],
        "codex"
    );
}

#[test]
fn every_writable_key_sets_and_unsets_with_comments_intact() {
    let (s, d) = world();
    let before = file(&s);
    let samples: Vec<(&str, Value)> = vec![
        ("test", json!("cargo test")),
        ("build", json!("cargo build")),
        ("model", json!("sonnet")),
        ("ladder", json!(["haiku", "sonnet"])),
        ("max_retries", json!(2)),
        ("escalate_model", json!("opus")),
        ("diagnose", json!(false)),
        ("sandbox", json!("none")),
        ("agent.user_skills", json!(false)),
        ("agent_env.FOO", json!("bar")),
        ("step.develop.budget", json!("2h")),
        ("step.test.max_rounds", json!(3)),
        ("step.test.command", json!("true")),
        ("step_timeout_s", json!(300)),
        ("agent_idle_stop_s", json!(0)),
        ("cont_after_s", json!(60)),
        ("auto_retry", json!(false)),
        ("hold_back_ignore", json!(["a"])),
        ("park_extra", json!(2)),
        ("small_run_s", json!(10)),
        ("test_reruns", json!(2)),
        ("rerun_filter", json!("cargo")),
        ("reuse_green", json!(true)),
        ("task_cache_keep_days", json!(7)),
        ("clarity_classifier", json!("true")),
        ("placement.mode", json!("favor-local")),
        ("proposals.per_day", json!(5)),
        ("proposals.introspection.low_small", json!("queued")),
        ("introspect.min_samples", json!(4)),
        ("introspect.no_result_rate", json!(0.5)),
        ("tracker.interval_s", json!(30)),
        ("tracker.label", json!("bots")),
        ("tracker.webhook_secret", json!("env:HOOK")),
        ("tool_timeout_s", json!(1200)),
    ];
    for (key, v) in samples {
        let (code, r) = set(
            &d,
            "brian",
            false,
            json!({"set": {key: v}, "confirm": true}),
        );
        assert_eq!(code, 200, "set {key}: {r}");
        let t = file(&s);
        assert!(
            t.contains("# keep this comment") && t.contains("# trailing note"),
            "{key}: {t}"
        );
        let e = entry(&get(&d, "brian", false), key);
        assert_eq!(e["written"], v, "{key}");
        let (code, r) = set(
            &d,
            "brian",
            false,
            json!({"set": {key: null}, "confirm": true}),
        );
        assert_eq!(code, 200, "unset {key}: {r}");
        let after = file(&s);
        assert!(
            after.contains("# keep this comment") && after.contains("# trailing note"),
            "{key}"
        );
        if key != "tool_timeout_s" {
            assert_eq!(
                entry(&get(&d, "brian", false), key)["written"],
                Value::Null,
                "{key}"
            );
        }
        // Put the file back for the next key (the unset of tool_timeout_s removed a key that
        // was in the fixture).
        std::fs::write(s.config_dir.join("projects/scratch.toml"), &before).unwrap();
        d.reload_registration("scratch").unwrap();
    }
}

#[test]
fn a_refused_value_leaves_the_file_byte_for_byte() {
    let (s, d) = world();
    let before = file(&s);
    let bad = [
        json!({"set": {"step_timeout_s": "soon"}}),
        json!({"set": {"max_retries": 9}}),
        json!({"set": {"sandbox": "docker"}, "confirm": true}),
        json!({"set": {"tracker.token": "ghp_literal_token"}}),
        json!({"set": {"tracker.nope": 1}}),
        json!({"set": {"agent_env.GITHUB_TOKEN": "ghp_literal"}, "confirm": true}),
        json!({"set": {"provider.claude.light.medium.effort": "turbo"}}),
        json!({"set": {"provider.claude.fast.medium.model": "x"}}),
        json!({"set": {"step.develop.budget": "whenever"}}),
        json!({"set": {"rerun_filter": "no-placeholder"}}),
        json!({"set": {"diagnose_effort": "turbo"}}),
        json!({"set": {"account": "nobody"}}),
    ];
    for b in bad {
        let (code, r) = set(&d, "brian", false, b.clone());
        assert!(code >= 400, "{b} was accepted: {r}");
        assert_eq!(file(&s), before, "{b} changed the file");
    }
    // A fixed key is refused with the restart message.
    for k in ["path", "base", "remote", "self.holds"] {
        let (code, r) = set(&d, "brian", false, json!({"set": {k: "x"}}));
        assert_eq!(code, 409, "{k}: {r}");
        assert!(r.to_string().contains("restart"), "{r}");
        assert_eq!(file(&s), before);
    }
}

#[test]
fn a_program_key_needs_confirmation_and_a_preview_shows_the_diff_without_writing() {
    let (s, d) = world();
    let before = file(&s);
    let (code, r) = set(&d, "brian", false, json!({"set": {"test": "make check"}}));
    assert_eq!(code, 409, "{r}");
    assert!(
        r.to_string().contains("make check"),
        "the refusal shows the diff: {r}"
    );
    assert_eq!(file(&s), before);
    let (code, r) = set(
        &d,
        "brian",
        false,
        json!({"set": {"tool_timeout_s": 700}, "preview": true}),
    );
    assert_eq!(code, 200, "{r}");
    assert_eq!(r["result"], "preview");
    assert!(
        r["diff"]
            .as_str()
            .unwrap()
            .contains("+ tool_timeout_s = 700"),
        "{r}"
    );
    assert_eq!(file(&s), before);
}

#[test]
fn a_viewer_sees_everything_and_changes_nothing() {
    let (s, d) = world();
    let before = file(&s);
    let cat = get(&d, "viewer:3", false);
    assert_eq!(entry(&cat, "tool_timeout_s")["writable"], false);
    assert_eq!(cat["caller"]["owner"], false);
    for body in [
        json!({"set": {"tool_timeout_s": 5}}),
        json!({"set": {"test": "x"}, "confirm": true}),
    ] {
        let (code, _) = set(&d, "viewer:3", false, body);
        assert_eq!(code, 403);
    }
    assert_eq!(file(&s), before);
}

#[test]
fn a_paired_owner_may_set_plain_keys_but_never_the_program_or_guard_keys() {
    let (s, d) = world();
    let cat = get(&d, "brian", true);
    assert_eq!(entry(&cat, "tool_timeout_s")["writable"], true);
    for k in [
        "test",
        "build",
        "claude_bin",
        "codex_bin",
        "sandbox",
        "agent.user_skills",
        "clarity_classifier",
        "card_rewriter",
        "triage.command",
        "ci.trigger",
        "ci.url",
        "ci.trigger_url",
        "tracker.api",
    ] {
        assert_eq!(entry(&cat, k)["writable"], false, "{k}");
    }
    let before = file(&s);
    for (k, v) in [
        ("test", json!("x")),
        ("sandbox", json!("none")),
        ("agent_env.A", json!("b")),
        ("step.test.command", json!("x")),
        ("schedule.nightly.command", json!("x")),
    ] {
        let (code, r) = set(&d, "brian", true, json!({"set": {k: v}, "confirm": true}));
        assert_eq!(code, 403, "{k}: {r}");
        assert_eq!(file(&s), before);
    }
    let (code, r) = set(&d, "brian", true, json!({"set": {"tool_timeout_s": 1000}}));
    assert_eq!(code, 200, "{r}");
}

#[test]
fn no_response_holds_a_secret() {
    let (s, d) = world();
    let (_, v) = call(&d, "GET", "brian", false, URL, json!(null));
    let text = v.to_string();
    for secret in ["supersecret-value", "hunter2-literal"] {
        assert!(!text.contains(secret), "{secret} leaked");
    }
    assert_eq!(
        entry(&v["catalogue"], "tracker.token")["written"],
        "env:PROJSETTINGS_TEST_TOKEN"
    );
    assert_eq!(
        entry(&v["catalogue"], "agent_env.MY_API_TOKEN")["written"],
        "set in file, not shown"
    );
    let (code, r) = set(
        &d,
        "brian",
        false,
        json!({"set": {"tracker.token": "ghp_literal_secret"}}),
    );
    assert!(code >= 400);
    assert!(
        !r.to_string().contains("ghp_literal_secret"),
        "an error echoes the secret: {r}"
    );
    assert!(!file(&s).contains("ghp_literal_secret"));
}
