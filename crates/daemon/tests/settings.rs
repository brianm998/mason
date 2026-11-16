// SPDX-License-Identifier: GPL-3.0-or-later

//! The Settings page's daemon side (s430): each setting reads and writes back through the same
//! parsers that load the files, comments survive, a bad value changes nothing, only the owner
//! writes, and a change is live without a restart.

mod support;
use mason_daemon::{attention, notify, Daemon};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::*;

type Received = Arc<Mutex<Vec<(String, String)>>>;

/// A webhook receiver on loopback: records each body and its signature header.
fn receiver() -> (String, Received) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/hook", l.local_addr().unwrap());
    let got: Received = Arc::default();
    let g = got.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let mut buf = vec![];
            let mut chunk = [0u8; 4096];
            loop {
                let n = s.read(&mut chunk).unwrap_or(0);
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let want: usize = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if body.len() >= want {
                        let sig = head
                            .lines()
                            .find_map(|l| l.strip_prefix("X-Mason-Signature: "))
                            .unwrap_or("")
                            .to_string();
                        g.lock().unwrap().push((sig, body.to_string()));
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let _ =
                s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    });
    (url, got)
}

fn call(
    d: &Arc<mason_daemon::Daemon>,
    method: &str,
    who: &str,
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
            let req = axum::http::Request::builder()
                .method(method.as_str())
                .uri(path)
                .header("content-type", "application/json")
                .header("x-mason-as", who)
                .body(axum::body::Body::from(body.to_string()))
                .unwrap();
            let resp = mason_daemon::http::api_router(d)
                .oneshot(req)
                .await
                .unwrap();
            let status = resp.status().as_u16();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            )
        })
}

const EXTRA: &str = r#"# keep this comment
[proposals]
agent_normal = "proposed" # trailing note
per_day = 20

[schedule.nightly]
kind = "task"
every = "daily"
title = "Nightly sweep"

[tracker]
kind = "github"
api = "http://127.0.0.1:9"
repo = "o/r"
token = "env:MASON_SETTINGS_TEST_TOKEN"
interval_s = 60
"#;

fn world() -> (Scratch, Arc<Daemon>, String) {
    let s = Scratch::new("true", json!({"steps": {}}));
    s.write_project("scratch", "true", EXTRA);
    let d = s.start_with(|c| {
        c.owner_names = vec!["brian".into()];
        c.reload_interval = Duration::from_secs(3600);
    });
    (s, d, "brian".into())
}

fn file(s: &Scratch) -> String {
    std::fs::read_to_string(s.config_dir.join("projects/scratch.toml")).unwrap()
}

const URL: &str = "/projects/scratch/settings";

#[test]
fn reading_shows_the_policy_schedules_and_tracker_and_never_a_secret() {
    let (_s, d, owner) = world();
    let (code, v) = call(&d, "GET", &owner, URL, json!(null));
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["proposals"]["agent_normal"], "proposed");
    assert_eq!(v["proposals"]["agent_blocks"], "queued");
    assert_eq!(v["proposals"]["per_day"], 20);
    assert_eq!(v["proposals"]["introspection"]["high_large"], "proposed");
    assert_eq!(v["schedules"][0]["name"], "nightly");
    assert_eq!(v["schedules"][0]["every"], "daily");
    assert_eq!(v["tracker"]["interval_s"], 60);
    assert_eq!(v["tracker"]["token"], "env:MASON_SETTINGS_TEST_TOKEN");
    let (code, _) = call(&d, "GET", &owner, "/projects/nope/settings", json!(null));
    assert_eq!(code, 404);
}

#[test]
fn a_literal_secret_in_the_file_is_reported_only_as_set() {
    let s = Scratch::new("true", json!({"steps": {}}));
    s.write_project("scratch", "true", "");
    let d = s.start();
    // The loader refuses a literal token, so the file is edited after the daemon started:
    // reading must still never echo it.
    let mut t = file(&s);
    t.push_str("[tracker]\nkind = \"github\"\nrepo = \"o/r\"\ntoken = \"ghp_literal_value\"\n");
    std::fs::write(s.config_dir.join("projects/scratch.toml"), t).unwrap();
    let (_, v) = call(&d, "GET", "brian", URL, json!(null));
    assert_eq!(v["tracker"]["token"], "set in file, not shown", "{v}");
    assert!(!v.to_string().contains("ghp_literal_value"));
}

#[test]
fn only_the_owner_writes() {
    let (s, d, _) = world();
    let before = file(&s);
    let body = json!({"proposals": {"per_day": 5}});
    for who in ["somebody-else", ""] {
        let (code, _) = call(&d, "POST", who, URL, body.clone());
        assert_eq!(code, 403, "{who:?}");
        let (code, _) = call(
            &d,
            "POST",
            who,
            "/notify/settings",
            json!({"quiet": {"sound": "22:00-07:00"}}),
        );
        assert_eq!(code, 403, "{who:?}");
    }
    assert_eq!(file(&s), before);
    assert!(!s.config_dir.join("notify.toml").exists());
}

#[test]
fn an_invalid_value_is_refused_and_the_file_is_byte_identical() {
    let (s, d, owner) = world();
    let before = file(&s);
    for body in [
        json!({"proposals": {"per_day": 0}}),
        json!({"proposals": {"agent_normal": "now"}}),
        json!({"proposals": {"introspection": {"low_small": "soon"}}}),
        json!({"proposals": {"nonsense": 1}}),
        json!({"schedules": {"nightly": {"every": "soon"}}}),
        json!({"schedules": {"nightly": {"kind": "ci"}}}),
        json!({"schedules": {"missing": {"every": "1h"}}}),
        json!({"tracker": {"import_as": "x"}}),
        json!({"tracker": {"token": "ghp_literal"}}),
        json!({"tracker": {"webhook_secret": "literal"}}),
        json!({"tracker": {"repo": "other/repo"}}),
        json!({"tracker": {"interval_s": 0}}),
        json!({"unknown": {}}),
    ] {
        let (code, v) = call(&d, "POST", &owner, URL, body.clone());
        assert_eq!(code, 400, "{body}: {v}");
        assert_eq!(file(&s), before, "{body}");
    }
    let before = "# mine\n[routing.paused]\nsound = \"0\"\n";
    std::fs::write(s.config_dir.join("notify.toml"), before).unwrap();
    for body in [
        json!({"routing": {"paused": {"fax": "0"}}}),
        json!({"routing": {"no_such_condition": {"sound": "0"}}}),
        // monitor-only conditions (`notify::MONITOR_ONLY`) have no routes to edit
        json!({"routing": {"paused": {"sound": "0"}}}),
        json!({"routing": {"debt": {"sound": "0"}}}),
        json!({"routing": {"provider_hold": {"sound": "0"}}}),
        json!({"routing": {"paused": {"sound": "whenever"}}}),
        json!({"quiet": {"sound": "late"}}),
        json!({"password": "hunter2"}),
        json!({"email": {"password": "hunter2"}}),
        json!({"speech": {"command": "rm"}}),
    ] {
        let (code, v) = call(&d, "POST", &owner, "/notify/settings", body.clone());
        assert_eq!(code, 400, "{body}: {v}");
        assert_eq!(
            std::fs::read_to_string(s.config_dir.join("notify.toml")).unwrap(),
            before,
            "{body}"
        );
    }
}

#[test]
fn a_valid_change_keeps_comments_and_is_live_for_what_is_admitted_next() {
    let (s, d, owner) = world();
    let body = json!({
        "proposals": {"agent_normal": "queued", "per_day": 7,
                      "introspection": {"high_small": "queued"}},
        "schedules": {"nightly": {"every": "6h", "title": "Sweep often"}},
        "tracker": {"interval_s": 15, "label": "ops", "import_as": "queued", "require_label": true},
    });
    let (code, v) = call(&d, "POST", &owner, URL, body);
    assert_eq!(code, 200, "{v}");
    let text = file(&s);
    assert!(text.contains("# keep this comment"), "{text}");
    assert!(text.contains("# trailing note"), "{text}");
    assert!(text.contains("agent_normal = \"queued\""), "{text}");
    assert!(text.contains("every = \"6h\""), "{text}");

    let pid = d.project_id_by_name("scratch").unwrap();
    let p = d.project(pid).unwrap();
    let cfg = p.proposals();
    assert_eq!(cfg.caps.per_day, 7);
    assert_eq!(
        cfg.policy.agent_normal,
        mason_core::proposal::Disposition::Queued
    );
    assert_eq!(p.schedules()[0].every_ms, 6 * 3_600_000);
    let t = p.services.tracker.as_ref().unwrap();
    assert_eq!(
        (t.interval_s, t.label.as_str(), t.require_label),
        (15, "ops", true)
    );
    assert_eq!(d.tracker_interval(pid), Some(15), "the running poller");

    let (_, v) = call(&d, "GET", &owner, URL, json!(null));
    assert_eq!(v["proposals"]["per_day"], 7);
    assert_eq!(v["proposals"]["introspection"]["high_small"], "queued");
    assert_eq!(v["schedules"][0]["title"], "Sweep often");
    assert_eq!(v["tracker"]["import_as"], "queued");
    // The same change again is a no-op.
    let (_, v) = call(
        &d,
        "POST",
        &owner,
        URL,
        json!({"proposals": {"per_day": 7}}),
    );
    assert_eq!(v["result"], "unchanged");
}

#[test]
fn a_notify_route_change_is_used_by_the_next_tick() {
    let (url, got) = receiver();
    let (s, d, owner) = world();
    std::fs::write(
        s.config_dir.join("notify.toml"),
        format!("# my webhook\n[webhook]\nurl = \"{url}\"\n"),
    )
    .unwrap();
    attention::pause(&d, "", "all", "brian", 3600).unwrap();
    attention::sync_alerts(&d);
    let t0 = mason_daemon::daemon::now_ms();
    notify::tick_at(&d, t0);
    assert_eq!(got.lock().unwrap().len(), 0, "no webhook route yet");

    let (code, v) = call(
        &d,
        "POST",
        &owner,
        "/notify/settings",
        json!({"routing": {"failed": {"webhook": "0"}}, "quiet": {"sound": "22:00-07:00"}}),
    );
    assert_eq!(code, 200, "{v}");
    let text = std::fs::read_to_string(s.config_dir.join("notify.toml")).unwrap();
    assert!(
        text.contains("# my webhook") && text.contains("webhook = \"0\""),
        "{text}"
    );
    // `paused` is monitor-only and not routable through the API, so the tick is shown using a
    // route by writing one for it in the file, which the tick re-reads.
    std::fs::write(
        s.config_dir.join("notify.toml"),
        format!("{text}\n[routing.paused]\nwebhook = \"0\"\n"),
    )
    .unwrap();
    notify::tick_at(&d, t0 + 1);
    assert_eq!(got.lock().unwrap().len(), 1, "the new route was used");

    let (_, v) = call(&d, "GET", &owner, "/notify/settings", json!(null));
    assert_eq!(v["quiet"]["sound"], "22:00-07:00");
    let (_, v) = call(
        &d,
        "POST",
        &owner,
        "/notify/settings",
        json!({"quiet": {"sound": null}}),
    );
    assert_eq!(v["result"], "applied");
}
