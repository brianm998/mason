// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg(unix)]
// Unix-only: the fixtures use permission bits and git.

//! Multi-machine nodes, phase 1 (`docs/multi-machine-nodes-plan.md`): two daemons on loopback,
//! each with its own data and configuration directories. B serves as a node; A is a
//! coordinator that pairs with it by code, sees whether it accepts work, and sends it its
//! settings. Nothing about timing is asserted, and mDNS is off (multicast is unreliable where
//! tests run; the TXT parsing has its own unit test).
//!
//! This is its own target because it sets an environment variable.

mod support;
use axum::extract::State;
use axum::Json;
use mason_daemon::node_client::{ClientError, NodeClient};
use mason_daemon::{lan, node_serve, nodes, Daemon};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use support::*;

struct Pair {
    rt: tokio::runtime::Runtime,
    a: Arc<Daemon>,
    b: Arc<Daemon>,
    sa: Scratch,
    sb: Scratch,
    port: u16,
}

fn pair() -> Pair {
    std::env::set_var("MASON_NO_MDNS", "1");
    let sb = Scratch::new("true", json!({"steps": {}}));
    let b = sb.start_with(|c| {
        c.node.serve = true;
        c.node.port = 0;
        c.node.bind = vec![std::net::IpAddr::from([127, 0, 0, 1])];
    });
    let sa = Scratch::new("true", json!({"steps": {}}));
    let a = sa.start();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let port = rt.block_on(node_serve::start(b.clone())).unwrap();
    Pair {
        rt,
        a,
        b,
        sa,
        sb,
        port,
    }
}

fn run<T>(p: &Pair, f: impl std::future::Future<Output = T>) -> T {
    p.rt.block_on(f)
}

/// A pairing code and the node's fingerprint, as `mason node pair-code` prints them.
fn code(p: &Pair) -> (String, String) {
    let v = run(p, node_serve::issue_code(State(p.b.clone())))
        .unwrap()
        .0;
    (
        v["code"].as_str().unwrap().to_string(),
        v["fingerprint"].as_str().unwrap().to_string(),
    )
}

fn add(p: &Pair, code: &str, fp: &str, name: &str) -> Result<Value, (u16, String)> {
    run(
        p,
        nodes::add(
            State(p.a.clone()),
            Json(json!({"host": format!("127.0.0.1:{}", p.port), "fp": fp, "code": code, "name": name})),
        ),
    )
    .map(|j| j.0)
    .map_err(|(s, j)| (s.as_u16(), j.0["error"].as_str().unwrap_or_default().to_string()))
}

fn listed(p: &Pair, name: &str) -> Value {
    let v = run(p, nodes::list(State(p.a.clone()))).unwrap().0;
    v["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("{name} is not listed: {v}"))
}

fn poll(p: &Pair) {
    run(p, nodes::poll_once(&p.a));
}

fn set_switch(p: &Pair, on: bool) {
    let _ = run(
        p,
        node_serve::accept(
            State(p.b.clone()),
            axum::http::HeaderMap::new(),
            Json(json!({"on": on})),
        ),
    )
    .unwrap();
}

/// Every file under `dir` that holds `needle`.
fn files_with(dir: &Path, needle: &str, out: &mut Vec<String>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            files_with(&p, needle, out);
        } else if std::fs::read(&p)
            .is_ok_and(|b| b.windows(needle.len()).any(|w| w == needle.as_bytes()))
        {
            out.push(p.display().to_string());
        }
    }
}

#[test]
fn a_node_pairs_by_code_reports_its_switch_and_takes_the_coordinators_settings() {
    let p = pair();
    // Settings are the coordinator's, in its daemon.toml, read live.
    std::fs::write(
        p.sa.config_dir.join("daemon.toml"),
        "[nodes.default]\nmax_jobs = 3\n",
    )
    .unwrap();

    // An unpaired coordinator is turned away; a wrong code is refused and costs an attempt.
    let (c, fp) = code(&p);
    let wrong = if c == "00000000" {
        "11111111"
    } else {
        "00000000"
    };
    assert_eq!(add(&p, wrong, &fp, "b").unwrap_err().0, 400);

    // Pair with the right code, and it works once only.
    let paired = add(&p, &c, &fp, "b").unwrap();
    assert_eq!(paired["paired"], "b");
    assert_eq!(
        add(&p, &c, &fp, "b2").unwrap_err().0,
        400,
        "a code is used up by its first use"
    );
    assert_eq!(p.b.node.coordinators().len(), 1);

    // The switch is off on a fresh node: A sees B and why it takes nothing.
    let n = listed(&p, "b");
    assert_eq!(n["accepting"], false, "{n}");
    assert!(
        n["state"]
            .as_str()
            .unwrap()
            .contains("turned remote work off"),
        "{n}"
    );
    assert_eq!(n["usable"], false);

    // The owner turns it on at B; the next poll shows it.
    set_switch(&p, true);
    poll(&p);
    let n = listed(&p, "b");
    assert_eq!(n["accepting"], true, "{n}");
    assert_eq!(n["usable"], true);
    assert!(n["cores"].as_u64().unwrap() > 0);

    // A's settings reached B and show in B's own status.
    let local = run(&p, node_serve::local_status(State(p.b.clone())))
        .unwrap()
        .0;
    assert_eq!(local["settings"]["max_jobs"], 3, "{local}");
    assert_eq!(local["jobs"]["max"], 3);
    assert_eq!(local["serving"], true);

    // ... and a live change of them on A reaches B with the next poll, no restart.
    std::fs::write(
        p.sa.config_dir.join("daemon.toml"),
        "[nodes.default]\nmax_jobs = 3\n[nodes.b]\nmax_jobs = 5\nmax_other_cpu_pct = 70\n",
    )
    .unwrap();
    poll(&p);
    let local = run(&p, node_serve::local_status(State(p.b.clone())))
        .unwrap()
        .0;
    assert_eq!(local["settings"]["max_jobs"], 5);
    assert_eq!(local["settings"]["max_other_cpu_pct"], 70);

    // Off again: the next poll says so.
    set_switch(&p, false);
    poll(&p);
    assert_eq!(listed(&p, "b")["accepting"], false);

    // Nothing of this is on B's disk but the fingerprint list and the switch: no token and no
    // copy of A's settings, in its data or its configuration directory.
    for needle in ["max_jobs", "max_other_cpu_pct", "token", "Bearer", "mt_"] {
        let mut hits = vec![];
        files_with(&p.sb.data_dir.join("node"), needle, &mut hits);
        files_with(&p.sb.config_dir, needle, &mut hits);
        assert!(hits.is_empty(), "{needle} found in {hits:?}");
    }
    let node_files: Vec<String> = std::fs::read_dir(p.sb.data_dir.join("node"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        node_files
            .iter()
            .all(|f| f == "id" || f == "coordinators.json"),
        "{node_files:?}"
    );

    // B forgets A: A is told "not paired (403)".
    let me = lan::hostname();
    let _ = run(
        &p,
        node_serve::coordinator_remove(State(p.b.clone()), Json(json!({"name": me}))),
    )
    .unwrap();
    poll(&p);
    let n = listed(&p, "b");
    assert!(
        n["state"].as_str().unwrap().contains("not paired (403)"),
        "{n}"
    );
    assert_eq!(n["usable"], false);
}

#[test]
fn a_client_certificate_that_was_never_paired_gets_403_and_a_wrong_server_fingerprint_is_refused() {
    let p = pair();
    let (c, fp) = code(&p);
    let identity = |dir: &Path| lan::load_or_create_identity(dir).unwrap();

    // A's certificate, not paired yet: the handshake works, the request is refused.
    let client = NodeClient::new(identity(&p.sa.data_dir), "127.0.0.1", p.port, &fp).unwrap();
    let r = run(&p, client.request("POST", "/node/status", Some(&json!({})))).unwrap();
    assert_eq!(r.status, 403, "{}", r.error_text());

    // The pairing route is open to any certificate that has a code, and to nobody without one.
    let r = run(
        &p,
        client.request(
            "POST",
            "/node-pair",
            Some(&json!({"code": "12345678", "name": "x"})),
        ),
    )
    .unwrap();
    assert!(r.status == 400, "{}", r.status);

    // The wrong pinned fingerprint: the connection is refused before anything is sent.
    let wrong = NodeClient::new(
        identity(&p.sa.data_dir),
        "127.0.0.1",
        p.port,
        &"0".repeat(64),
    )
    .unwrap();
    let e = run(&p, wrong.request("POST", "/node/status", Some(&json!({})))).unwrap_err();
    assert!(matches!(e, ClientError::WrongFingerprint { .. }), "{e:?}");

    // A different certificate cannot ride on A's pairing.
    add(&p, &c, &fp, "b").unwrap();
    let other = tempfile::tempdir().unwrap();
    let stranger = NodeClient::new(identity(other.path()), "127.0.0.1", p.port, &fp).unwrap();
    let r = run(
        &p,
        stranger.request("POST", "/node/status", Some(&json!({}))),
    )
    .unwrap();
    assert_eq!(r.status, 403);
    let paired = NodeClient::new(identity(&p.sa.data_dir), "127.0.0.1", p.port, &fp).unwrap();
    let r = run(&p, paired.request("POST", "/node/status", Some(&json!({})))).unwrap();
    assert_eq!(r.status, 200, "{}", r.error_text());
    assert_eq!(r.json()["node_proto"], node_serve::NODE_PROTO);
}

#[test]
fn a_bad_setting_from_the_coordinator_is_refused_and_does_not_replace_the_good_ones() {
    let p = pair();
    let (c, fp) = code(&p);
    add(&p, &c, &fp, "b").unwrap();
    let client = NodeClient::new(
        lan::load_or_create_identity(&p.sa.data_dir).unwrap(),
        "127.0.0.1",
        p.port,
        &fp,
    )
    .unwrap();
    let good = json!({"max_jobs": 4});
    assert_eq!(
        run(&p, client.request("POST", "/node/status", Some(&good)))
            .unwrap()
            .status,
        200
    );
    for bad in [
        json!({"max_jobs": 0}),
        json!({"on_switch_off": "later"}),
        json!({"x": 1}),
    ] {
        let r = run(&p, client.request("POST", "/node/status", Some(&bad))).unwrap();
        assert_eq!(r.status, 400, "{bad}");
    }
    assert_eq!(p.b.node.effective_settings().max_jobs, 4);
}

#[test]
fn pairing_needs_a_node_that_serves_and_names_the_setting() {
    let s = Scratch::new("true", json!({"steps": {}}));
    let d = s.start();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let e = rt
        .block_on(node_serve::issue_code(State(d.clone())))
        .unwrap_err();
    assert_eq!(e.0.as_u16(), 409);
    assert!(e.1 .0["error"]
        .as_str()
        .unwrap()
        .contains("[node] serve = true"));
    // The switch is off by default, and a node that does not serve still says so.
    assert!(!node_serve::read_switch(&d).on);
}

#[test]
fn the_owners_switch_is_a_file_that_keeps_its_comments_and_leaves_a_log() {
    let p = pair();
    let path = node_serve::switch_path(&p.b);
    std::fs::write(&path, "# my note\naccept = false\n").unwrap();
    set_switch(&p, true);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("# my note") && text.contains("accept = true"),
        "{text}"
    );
    assert!(node_serve::read_switch(&p.b).on);
    set_switch(&p, false);
    let log = std::fs::read_to_string(p.sb.config_dir.join("node-changes.log")).unwrap();
    assert_eq!(log.lines().count(), 2, "{log}");
    assert!(log.contains("accept false -> true") && log.contains("accept true -> false"));
}

#[test]
fn use_on_and_off_edits_daemon_toml_live_and_keeps_the_rest() {
    let s = Scratch::new("true", json!({"steps": {}}));
    let d = s.start();
    std::fs::write(
        s.config_dir.join("daemon.toml"),
        "# mine\n[nodes]\nstatic = [\"studio.local:7425\"]\n",
    )
    .unwrap();
    assert!(!nodes::set_enabled(&d, true, "brian").unwrap());
    let text = std::fs::read_to_string(s.config_dir.join("daemon.toml")).unwrap();
    assert!(
        text.contains("# mine") && text.contains("studio.local:7425"),
        "{text}"
    );
    let cfg = mason_daemon::config::load_nodes(&s.config_dir).unwrap();
    assert!(cfg.enabled);
    assert!(nodes::set_enabled(&d, false, "brian").unwrap());
    assert!(
        !mason_daemon::config::load_nodes(&s.config_dir)
            .unwrap()
            .enabled
    );
    // Without a file, one is made.
    let fresh = Scratch::new("true", json!({"steps": {}}));
    let d2 = fresh.start();
    nodes::set_enabled(&d2, true, "brian").unwrap();
    assert!(
        mason_daemon::config::load_nodes(&fresh.config_dir)
            .unwrap()
            .enabled
    );
}
