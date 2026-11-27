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

/// A test that needs two worlds takes them one test at a time: the worlds are a limited set
/// (`support::world_limit`), and tests that each held one while another waits for its first
/// would push the machine past it.
static WORLDS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn worlds() -> std::sync::MutexGuard<'static, ()> {
    WORLDS.lock().unwrap_or_else(|e| e.into_inner())
}

struct Pair {
    rt: tokio::runtime::Runtime,
    /// The node's listener runs on a runtime of its own, so a test can take it away.
    rt_b: std::sync::Mutex<Option<tokio::runtime::Runtime>>,
    a: Arc<Daemon>,
    b: Arc<Daemon>,
    sa: Scratch,
    sb: Scratch,
    port: u16,
}

fn pair() -> Pair {
    pair_with(|_| {})
}

fn pair_with(on_node: impl FnOnce(&mut mason_daemon::config::NodeCfg)) -> Pair {
    pair_with_a(|_| {}, |_| {}, on_node)
}

/// [`pair_with`], and `on_coordinator` adjusts the coordinator's configuration too.
fn pair_with_a(
    on_coordinator: impl FnOnce(&mut mason_daemon::config::DaemonConfig),
    on_node_daemon: impl FnOnce(&mut mason_daemon::config::DaemonConfig),
    on_node: impl FnOnce(&mut mason_daemon::config::NodeCfg),
) -> Pair {
    std::env::set_var("MASON_NO_MDNS", "1");
    let _one_at_a_time = worlds();
    let sb = Scratch::new("true", json!({"steps": {}}));
    let b = sb.start_with(|c| {
        c.node.serve = true;
        c.node.port = 0;
        c.node.bind = vec![std::net::IpAddr::from([127, 0, 0, 1])];
        on_node(&mut c.node);
        on_node_daemon(c);
    });
    let sa = Scratch::new("true", json!({"steps": {}}));
    let a = sa.start_with(on_coordinator);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let rt_b = tokio::runtime::Runtime::new().unwrap();
    let port = rt_b.block_on(node_serve::start(b.clone())).unwrap();
    Pair {
        rt,
        rt_b: std::sync::Mutex::new(Some(rt_b)),
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

// ---- phase 2: running a job on the node ------------------------------------------------------

use mason_daemon::remote::{self, Fail, Outcome, StopWhy};

/// A coordinator with a project (`scratch`, a clone with an origin) paired with a node that
/// takes work, so that a run can be sent. The node says it has Rust; the machine need not.
fn working_pair(max_jobs: u32) -> Pair {
    working_pair_with(max_jobs, "", "", |_| {})
}

/// `top`: more of `[nodes]`; `default`: more of `[nodes.default]`.
fn working_pair_with(
    max_jobs: u32,
    top: &str,
    default: &str,
    on_node: impl FnOnce(&mut mason_daemon::config::NodeCfg),
) -> Pair {
    working_pair_full(max_jobs, top, default, |_| {}, |_| {}, on_node)
}

fn working_pair_full(
    max_jobs: u32,
    top: &str,
    default: &str,
    on_coordinator: impl FnOnce(&mut mason_daemon::config::DaemonConfig),
    on_node_daemon: impl FnOnce(&mut mason_daemon::config::DaemonConfig),
    on_node: impl FnOnce(&mut mason_daemon::config::NodeCfg),
) -> Pair {
    std::env::set_var(
        "MASON_NODE_TOOLCHAINS",
        r#"{"rust":["1.96.0"],"xcode":[],"java":[]}"#,
    );
    std::env::set_var("MASON_NODE_POLL_MS", "50");
    let p = pair_with_a(on_coordinator, on_node_daemon, on_node);
    std::fs::write(
        p.sa.config_dir.join("daemon.toml"),
        format!(
            "[nodes]\nenabled = true\n{top}\n[nodes.default]\nmax_jobs = {max_jobs}\n{default}\n"
        ),
    )
    .unwrap();
    let (c, fp) = code(&p);
    add(&p, &c, &fp, "b").unwrap();
    set_switch(&p, true);
    poll(&p);
    if p.b.cfg.node.proto == node_serve::NODE_PROTO {
        assert_eq!(listed(&p, "b")["usable"], true, "{}", listed(&p, "b"));
    }
    p
}

fn spec(id: &str, build: Option<&str>, cmd: &str) -> remote::RunSpec {
    let s = remote::spec_for(
        "scratch",
        id,
        None,
        cmd,
        std::time::Duration::from_secs(120),
        &[("MASON_X".to_string(), "from-the-coordinator".to_string())],
        "node",
        "",
    );
    remote::with_build(s, build)
}

fn run_on_b(
    p: &Pair,
    spec: &remote::RunSpec,
    stop: &dyn Fn() -> Option<StopWhy>,
) -> Result<Outcome, Fail> {
    let base = remote::base_ref_of(&p.a, "scratch").expect("the project has a base");
    let log = p.sa.root.path().join("logs");
    mason_daemon::node_exec::execute(
        &p.a,
        None,
        spec,
        &p.sa.repo,
        &base,
        &log,
        stop,
        &mut |_, _| {},
    )
}

fn done(r: Result<Outcome, Fail>) -> remote::Finished {
    match r {
        Ok(Outcome::Done(f)) => f,
        Ok(Outcome::Stopped(w)) => panic!("stopped: {w:?}"),
        Err(e) => panic!("failed: {e:?}"),
    }
}

/// What a local run of the same command in the same tree gives.
fn local(p: &Pair, build: Option<&str>, cmd: &str) -> (i32, String) {
    let script = match build {
        Some(b) => format!("({b}) && ({cmd})"),
        None => cmd.to_string(),
    };
    let o = std::process::Command::new("sh")
        .args(["-c", &script])
        .current_dir(&p.sa.repo)
        .env("MASON_X", "from-the-coordinator")
        .output()
        .unwrap();
    (
        o.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
    )
}

#[test]
fn a_run_goes_to_the_node_and_comes_back_as_a_local_run_would() {
    let p = working_pair(2);
    // What a local run sees: tracked, modified and untracked files, as they are in the worktree.
    std::fs::write(
        p.sa.repo.join("t.sh"),
        "echo \"tests start $MASON_X\"\nmkdir -p res\necho '<ok/>' > res/out.xml\necho \"worktree=$(basename $MASON_WORKTREE) node=$MASON_NODE\"\nexit 0\n",
    )
    .unwrap();
    let mut s = spec("run-green", None, "sh t.sh");
    s.results_file = Some("res/out.xml".into());
    let f = done(run_on_b(&p, &s, &|| None));
    assert_eq!((f.code, f.signal, f.timed_out), (0, 0, false));
    assert!(
        f.output.contains("tests start from-the-coordinator"),
        "{}",
        f.output
    );
    // The node's own facts, and not the coordinator's paths.
    assert!(
        f.output.contains("worktree=scratch-slot-1 node=1"),
        "{}",
        f.output
    );
    assert!(f.ran_on.starts_with("ran on node b"), "{}", f.ran_on);
    // The results file is where a local run leaves it.
    assert_eq!(
        std::fs::read_to_string(p.sa.repo.join("res/out.xml")).unwrap(),
        "<ok/>\n"
    );
    // The same command locally gives the same exit and, but for the node's own lines, the same output.
    let (code, out) = local(&p, None, "sh t.sh");
    assert_eq!(code, 0);
    assert!(out.starts_with("tests start from-the-coordinator\n"));
    // The coordinator keeps no record of a finished run, and the node deleted the job.
    assert!(
        !mason_daemon::node_exec::record_path(&p.sa.root.path().join("logs"), "run-green").exists()
    );
    let jobs = run(&p, node_serve::jobs(State(p.b.clone()))).unwrap().0;
    assert!(jobs["jobs"].as_array().unwrap().is_empty(), "{jobs}");
}

#[test]
fn a_red_run_and_a_failed_build_give_the_verdicts_a_local_run_gives() {
    let p = working_pair(2);
    std::fs::write(p.sa.repo.join("red.sh"), "echo 'not ok 1 - boom'\nexit 3\n").unwrap();
    let f = done(run_on_b(&p, &spec("run-red", None, "sh red.sh"), &|| None));
    let (code, out) = local(&p, None, "sh red.sh");
    assert_eq!((f.code, f.signal), (code, 0));
    assert_eq!(f.output, out);
    assert_eq!(f.phase.as_deref(), Some("run"));

    // A build that fails ends the run before the command, naming the phase.
    let f = done(run_on_b(
        &p,
        &spec(
            "run-build",
            Some("echo building; exit 4"),
            "echo SHOULD-NOT-RUN",
        ),
        &|| None,
    ));
    assert_eq!(f.code, 4);
    assert_eq!(f.phase.as_deref(), Some("build"));
    assert_eq!(f.output, "building\n");
    assert!(!f.output.contains("SHOULD-NOT-RUN"));

    // A build that passes runs the command, the build's output first.
    let f = done(run_on_b(
        &p,
        &spec("run-both", Some("echo building"), "echo testing; exit 1"),
        &|| None,
    ));
    assert_eq!((f.code, f.output.as_str()), (1, "building\ntesting\n"));
    assert_eq!(f.phase.as_deref(), Some("run"));
}

#[test]
fn a_stopped_run_is_cancelled_on_the_node_and_its_job_goes() {
    let p = working_pair(2);
    std::fs::write(p.sa.repo.join("slow.sh"), "echo started\nsleep 300\n").unwrap();
    let asked = std::sync::atomic::AtomicBool::new(false);
    // Stop it once the node has the job running.
    let b = p.b.clone();
    let stop = || {
        let running = !b.node.jobs.list().is_empty();
        if running {
            asked.store(true, std::sync::atomic::Ordering::SeqCst);
            Some(StopWhy::Skipped)
        } else {
            None
        }
    };
    let r = run_on_b(&p, &spec("run-stop", None, "sh slow.sh"), &stop);
    assert!(matches!(r, Ok(Outcome::Stopped(StopWhy::Skipped))), "{r:?}");
    support::wait_for("the node to end the cancelled job", 60, || {
        b.node.jobs.list().iter().all(|j| j["state"] != "running")
    });
    let jobs = run(&p, node_serve::jobs(State(p.b.clone()))).unwrap().0;
    assert_eq!(jobs["jobs"][0]["state"], "cancelled", "{jobs}");
}

#[test]
fn a_node_with_no_room_is_not_a_failure_of_the_code() {
    let p = working_pair(1);
    std::fs::write(p.sa.repo.join("slow.sh"), "sleep 300\n").unwrap();
    // The first run holds the node's only job slot...
    let b = p.b.clone();
    let release = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|sc| {
        let h = sc.spawn(|| {
            run_on_b(&p, &spec("run-one", None, "sh slow.sh"), &|| {
                release
                    .load(std::sync::atomic::Ordering::SeqCst)
                    .then_some(StopWhy::Skipped)
            })
        });
        support::wait_for("the first job to run on the node", 60, || {
            b.node.jobs.running() == 1
        });
        // ...so a second finds the node full (the coordinator's last look says it had room):
        // the run is environmental, which sends it to the next place.
        let second = run_on_b(&p, &spec("run-two", None, "echo hi"), &|| None);
        match second {
            Err(Fail::Environmental(why)) => {
                assert!(why.contains("no node had room"), "{why}");
                assert!(why.contains("job slots"), "{why}");
            }
            other => panic!("expected an environmental failure, got {other:?}"),
        }
        release.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(matches!(h.join().unwrap(), Ok(Outcome::Stopped(_))));
    });
}

#[test]
fn a_node_that_is_off_takes_nothing() {
    let p = working_pair(2);
    set_switch(&p, false);
    poll(&p);
    let r = run_on_b(&p, &spec("run-off", None, "echo hi"), &|| None);
    assert!(matches!(r, Err(Fail::Environmental(_))), "{r:?}");
}

#[test]
fn a_check_from_run_check_goes_to_the_node_when_placement_says_so() {
    use mason_daemon::hostedcheck::{run as run_check, CheckSpec};
    let p = working_pair(2);
    mason_daemon::place::set_global(&p.a, mason_daemon::place::Mode::Remote, "test").unwrap();
    let logs = tempfile::tempdir().unwrap();
    let spec = |id: &str, cmd: &str| CheckSpec {
        id_prefix: id.into(),
        log_dir: logs.path().to_path_buf(),
        cmd: cmd.into(),
        build: Some("echo built".into()),
        cwd: p.sa.repo.clone(),
        env: vec![],
        limit: std::time::Duration::from_secs(120),
        tree: format!("tree-{id}"),
        stop: None,
        holds: Default::default(),
        label: String::new(),
        place: None,
        // A check no task owns follows the global switch.
        global: true,
    };
    assert!(mason_daemon::hostedcheck::goes_remote(
        &p.a,
        &spec("g", "true")
    ));
    let green = run_check(
        &p.a,
        &spec("t0r1-green", "echo checked"),
        &mason_daemon::remote::Grant::attached(),
    )
    .unwrap();
    assert!(green.passed, "{}", green.output);
    assert!(green.output.contains("ran on node b"), "{}", green.output);
    assert!(green.full.contains("built\nchecked"), "{}", green.full);
    let red = run_check(
        &p.a,
        &spec("t0r1-red", "echo nope; exit 2"),
        &mason_daemon::remote::Grant::attached(),
    )
    .unwrap();
    assert!(!red.passed && red.full.contains("nope"), "{}", red.full);
    assert!(red.output.contains("ran on node b"));
    // A node that is switched off sends the check where a local run would be: here.
    set_switch(&p, false);
    poll(&p);
    assert!(!mason_daemon::hostedcheck::goes_remote(
        &p.a,
        &spec("g2", "true")
    ));
}

/// #1150: a project with no `[remote_runs]` and mode `remote` is nodes only: a check waits
/// while no node takes work, never running here, and runs on the node once one does.
#[test]
fn a_nodes_only_projects_check_waits_for_a_node_and_never_runs_here() {
    use mason_daemon::hostedcheck::{self, CheckSpec};
    use mason_daemon::place::{self, Mode};
    let p = working_pair(2);
    let set = place::set_project_mode(&p.a, "scratch", Mode::Remote).unwrap();
    assert_eq!(set["mode"], "remote", "{set}");
    let status = place::status(&p.a);
    let scratch = &status["projects"][0];
    assert_eq!(
        (&scratch["nodes_only"], &scratch["mode"], &scratch["source"]),
        (&json!(true), &json!("remote"), &json!("project")),
        "{status}"
    );
    set_switch(&p, false);
    poll(&p);
    let logs = tempfile::tempdir().unwrap();
    let spec = CheckSpec {
        id_prefix: "t0r1-only".into(),
        log_dir: logs.path().to_path_buf(),
        cmd: "echo node=$MASON_NODE".into(),
        build: None,
        cwd: p.sa.repo.clone(),
        env: vec![],
        limit: std::time::Duration::from_secs(120),
        tree: "tree-only".into(),
        stop: None,
        holds: Default::default(),
        label: String::new(),
        place: None,
        // The project's mode, not the global switch (which is local here).
        global: true,
    };
    assert!(hostedcheck::goes_remote(&p.a, &spec));
    // It is a node run: it holds no slot of this machine, and no node unit while none has room.
    assert!(hostedcheck::node_units(&p.a, &spec).is_some());
    assert!(matches!(
        mason_daemon::jobs::want_of(&p.a, &spec),
        mason_daemon::jobs::Want::Node(_)
    ));
    // A stop ends the wait.
    let base = remote::base_ref_of(&p.a, "scratch").unwrap();
    let r = mason_daemon::node_exec::execute(
        &p.a,
        Some(&place::wait_nid("")),
        &self::spec("only-stop", None, "echo no"),
        &p.sa.repo,
        &base,
        logs.path(),
        &|| Some(StopWhy::Skipped),
        &mut |_, _| {},
    );
    assert!(matches!(r, Ok(Outcome::Stopped(StopWhy::Skipped))), "{r:?}");
    std::thread::scope(|sc| {
        let h = sc.spawn(|| hostedcheck::run(&p.a, &spec, &remote::Grant::attached()));
        set_switch(&p, true);
        poll(&p);
        let out = h.join().unwrap().unwrap();
        assert!(out.passed, "{}", out.output);
        assert!(out.output.contains("ran on node b"), "{}", out.output);
        assert!(out.full.contains("node=1"), "{}", out.full);
    });
    // Cleared, it always runs here again.
    place::clear_project_mode(&p.a, "scratch").unwrap();
    assert!(!hostedcheck::goes_remote(&p.a, &spec));
}

/// A batch of two tasks lands on the coordinator, which does every git step, while the suites of
/// its test run on the node. The node never gets a way to push.
#[test]
fn a_batch_lands_on_the_coordinator_while_its_suites_run_on_the_node() {
    batch_on_the_node(|d| {
        mason_daemon::place::set_global(d, mason_daemon::place::Mode::Remote, "test").unwrap();
    });
}

/// #1150: the same for a project with no `[remote_runs]` whose own mode is `remote`.
#[test]
fn a_nodes_only_projects_suites_run_on_the_node() {
    batch_on_the_node(|d| {
        let name = d.projects().values().next().unwrap().id.clone();
        mason_daemon::place::set_project_mode(d, &name, mason_daemon::place::Mode::Remote).unwrap();
    });
}

fn batch_on_the_node(set_mode: impl FnOnce(&Arc<Daemon>)) {
    use support::lw::world;
    std::env::set_var(
        "MASON_NODE_TOOLCHAINS",
        r#"{"rust":["1.96.0"],"xcode":[],"java":[]}"#,
    );
    std::env::set_var("MASON_NODE_POLL_MS", "50");
    std::env::set_var("MASON_NO_MDNS", "1");
    let _one_at_a_time = worlds();
    let marks = tempfile::tempdir().unwrap();
    let ran = marks.path().join("ran-on-node");
    let checks = format!(
        "#!/bin/sh\nn=\"$1\"\nif [ -n \"$MASON_NODE\" ]; then echo \"$n\" >> {}; fi\necho 1..1\necho \"ok 1 - $n\"\n",
        ran.display()
    );
    let w = world(
        "window = \"1s\"\n[[tier]]\nname = \"batch\"\nsuites = [\"alpha\", \"beta\"]\n",
        &[("checks.sh", &checks)],
    );
    // The node: its own data and configuration, serving on loopback.
    let sb = Scratch::new("true", json!({"steps": {}}));
    let b = sb.start_with(|c| {
        c.node.serve = true;
        c.node.port = 0;
        c.node.bind = vec![std::net::IpAddr::from([127, 0, 0, 1])];
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let port = rt.block_on(node_serve::start(b.clone())).unwrap();
    std::fs::write(
        w.s.config_dir.join("daemon.toml"),
        "[nodes]\nenabled = true\n",
    )
    .unwrap();
    let v = rt
        .block_on(node_serve::issue_code(State(b.clone())))
        .unwrap()
        .0;
    let _ = rt
        .block_on(nodes::add(
            State(w.d.clone()),
            Json(json!({
                "host": format!("127.0.0.1:{port}"), "fp": v["fingerprint"], "code": v["code"],
                "name": "b",
            })),
        ))
        .map_err(|e| e.1 .0["error"].clone())
        .unwrap();
    let _ = rt
        .block_on(node_serve::accept(
            State(b.clone()),
            axum::http::HeaderMap::new(),
            Json(json!({"on": true})),
        ))
        .unwrap();
    rt.block_on(nodes::poll_once(&w.d));
    set_mode(&w.d);

    let t1 = w.task("one", &[("a/x1.txt", "1")]);
    let t2 = w.task("two", &[("b/x2.txt", "1")]);
    support::wait_for("both tasks to land", 240, || w.landed(t1) && w.landed(t2));
    // Landed on the coordinator's origin: the merge, the promotion and the push were its own.
    assert!(w.on_main("a/x1.txt") && w.on_main("b/x2.txt"));
    // The suites ran on the node (the marker is written only where `MASON_NODE` is set).
    let text = std::fs::read_to_string(&ran).unwrap_or_default();
    assert!(
        text.lines().any(|l| l == "alpha"),
        "ran on the node: {text:?}"
    );
    assert!(
        text.lines().any(|l| l == "beta"),
        "ran on the node: {text:?}"
    );
    // The node has no way to push: no remote in its mirror or its slots.
    let node_dir = sb.data_dir.join("node");
    let mut remotes = vec![];
    for dir in std::fs::read_dir(node_dir.join("mirrors"))
        .unwrap()
        .flatten()
        .flat_map(|c| std::fs::read_dir(c.path()).unwrap().flatten())
    {
        // `git config --get-regexp` exits 1 when nothing matches: there is no remote.
        let o = std::process::Command::new("git")
            .args(["config", "--get-regexp", "^remote\\."])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_eq!(
            o.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&o.stdout)
        );
        remotes.push(String::from_utf8_lossy(&o.stdout).into_owned());
    }
    assert!(!remotes.is_empty(), "the node has a mirror");
    assert!(remotes.iter().all(|r| r.trim().is_empty()), "{remotes:?}");
    let mut hits = vec![];
    files_with(&node_dir, &w.s.origin.display().to_string(), &mut hits);
    assert!(
        hits.iter().all(|h| !h.ends_with("config")),
        "the node's git config names the coordinator's origin: {hits:?}"
    );
}

// ---- phase 3: what happens when things go wrong ------------------------------------------------

fn coord_of_a(p: &Pair) -> String {
    let (cert, _) = lan::load_or_create_identity(&p.sa.data_dir).unwrap();
    mason_daemon::node_run::fp16(&lan::sha256_hex(&cert))
}

fn job_dirs_on_b(p: &Pair) -> usize {
    std::fs::read_dir(p.sb.data_dir.join("node/jobs").join(coord_of_a(p)))
        .map(|r| r.flatten().count())
        .unwrap_or(0)
}

fn flag_script(p: &Pair, name: &str, flag: &Path) -> String {
    // Waits for the flag file, then ends well; the file is made by the test.
    std::fs::write(
        p.sa.repo.join(name),
        format!(
            "while [ ! -f '{}' ]; do sleep 0.1; done\necho finished\n",
            flag.display()
        ),
    )
    .unwrap();
    format!("sh {name}")
}

/// (drain, the default) A job is running on the node; the owner turns the switch off. The node
/// takes no new job, the running one finishes there, and the coordinator collects it green.
#[test]
fn turning_the_switch_off_lets_a_running_job_finish_and_takes_no_new_one() {
    let p = working_pair(2);
    let flag = p.sa.root.path().join("go");
    let cmd = flag_script(&p, "wait.sh", &flag);
    let b = p.b.clone();
    std::thread::scope(|sc| {
        let h = sc.spawn(|| run_on_b(&p, &spec("run-drain", None, &cmd), &|| None));
        support::wait_for("the job to run on the node", 60, || {
            b.node.jobs.running() == 1
        });
        set_switch(&p, false);
        poll(&p);
        let n = listed(&p, "b");
        assert_eq!(n["accepting"], false, "{n}");
        assert!(
            n["state"]
                .as_str()
                .unwrap()
                .contains("draining: 1 job left"),
            "{n}"
        );
        // No new job: the coordinator will not even try.
        let r = run_on_b(&p, &spec("run-new", None, "echo hi"), &|| None);
        assert!(matches!(r, Err(Fail::Environmental(_))), "{r:?}");
        // The running one is still running, and finishes when its command does.
        assert_eq!(b.node.jobs.running(), 1);
        std::fs::write(&flag, "go").unwrap();
        let f = done(h.join().unwrap());
        assert_eq!((f.code, f.output.as_str()), (0, "finished\n"));
        assert!(f.ran_on.starts_with("ran on node b"));
    });
    assert_eq!(
        job_dirs_on_b(&p),
        0,
        "the finished job was collected and deleted"
    );
}

/// (cancel) `on_switch_off = "cancel"`, or `mason node stop-jobs`: the job is killed, the
/// coordinator's run is placed again, and here that is this machine: green. The node's job
/// directory is cleaned up.
#[test]
fn a_cancelled_job_is_run_again_here_and_the_node_cleans_up() {
    use mason_daemon::hostedcheck::{run as run_check, CheckSpec};
    for how in ["switch-off-with-cancel", "stop-jobs"] {
        let p = working_pair_with(
            2,
            "",
            if how == "stop-jobs" {
                ""
            } else {
                "on_switch_off = \"cancel\""
            },
            |_| {},
        );
        mason_daemon::place::set_global(&p.a, mason_daemon::place::Mode::Remote, "test").unwrap();
        let logs = tempfile::tempdir().unwrap();
        let spec = CheckSpec {
            id_prefix: "t0r1-cancel".into(),
            log_dir: logs.path().to_path_buf(),
            // On the node this hangs; anywhere else it ends at once.
            cmd: "if [ -n \"$MASON_NODE\" ]; then sleep 300; else echo local-ok; fi".into(),
            build: None,
            cwd: p.sa.repo.clone(),
            env: vec![],
            limit: std::time::Duration::from_secs(120),
            tree: "tree-cancel".into(),
            stop: None,
            holds: Default::default(),
            label: String::new(),
            place: None,
            global: true,
        };
        let b = p.b.clone();
        std::thread::scope(|sc| {
            let h = sc.spawn(|| run_check(&p.a, &spec, &mason_daemon::remote::Grant::attached()));
            support::wait_for("the job to run on the node", 60, || {
                b.node.jobs.running() == 1
            });
            if how == "stop-jobs" {
                let n = run(&p, node_serve::stop_jobs(State(p.b.clone())))
                    .unwrap()
                    .0;
                assert_eq!(n["cancelled"], 1);
            } else {
                set_switch(&p, false);
                let _ = run(
                    &p,
                    node_serve::accept(
                        State(p.b.clone()),
                        axum::http::HeaderMap::new(),
                        Json(json!({"on": false})),
                    ),
                )
                .unwrap();
            }
            let r = h.join().unwrap().unwrap();
            assert!(r.passed, "{how}: {}", r.full);
            assert!(
                r.full.contains("local-ok"),
                "{how}: it ran here: {}",
                r.full
            );
        });
        support::wait_for("the node to clean up", 60, || {
            b.node.jobs.list().is_empty() && job_dirs_on_b(&p) == 0
        });
    }
}

/// #1149: a check holds the machine's compile slots while a node has it. The node gives it
/// back, and it runs here under those slots: with every unit held and a step start waiting, each
/// check still finishes. Before, it queued for a second slot behind its own.
#[test]
fn a_check_the_node_gives_back_runs_under_the_slot_its_task_holds() {
    use mason_daemon::hostedcheck::{run as run_check, CheckSpec};
    use mason_daemon::remote::Grant;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
    for n in [1u32, 2] {
        let p = working_pair(2);
        mason_daemon::place::set_global(&p.a, mason_daemon::place::Mode::Remote, "test").unwrap();
        p.a.broker.set_limit("compile", n).unwrap();
        let logs = tempfile::tempdir().unwrap();
        let step_lease = |a: &std::sync::Arc<mason_daemon::daemon::Daemon>, task: u64| {
            a.broker
                .acquire(
                    mason_core::TaskId(task),
                    &format!("#{task}"),
                    mason_scheduler::Rank::new(false, 1, 0, task, task, false),
                    mason_scheduler::Needs::of(&[("compile", 1)]),
                    &|| true,
                    &mut |_| {},
                )
                .expect("a lease")
        };
        let grants: Vec<Grant> = (1..=n)
            .map(|t| Grant::of(step_lease(&p.a, u64::from(t))))
            .collect();
        // Another task's step start waits for a slot.
        let started = std::sync::Arc::new(AtomicBool::new(false));
        let start = {
            let (a, started) = (p.a.clone(), started.clone());
            std::thread::spawn(move || {
                let l = step_lease(&a, 99);
                started.store(true, SeqCst);
                l
            })
        };
        let finished = std::sync::Arc::new(AtomicUsize::new(0));
        // One at a time: two first jobs for the same repository race to create the node's
        // mirror of it, which is not what this test is about.
        let mut checks = vec![];
        for (grant, t) in grants.iter().zip(1u64..) {
            let spec = CheckSpec {
                id_prefix: format!("t{t}r1-check"),
                log_dir: logs.path().to_path_buf(),
                // On the node this hangs; anywhere else it ends at once.
                cmd: "if [ -n \"$MASON_NODE\" ]; then sleep 300; else echo local-ok; fi".into(),
                build: None,
                cwd: p.sa.repo.clone(),
                env: vec![],
                limit: std::time::Duration::from_secs(120),
                tree: format!("tree-borrow-{t}"),
                stop: None,
                holds: [("compile".to_string(), 1)].into(),
                label: format!("#{t} check"),
                place: None,
                global: true,
            };
            let (a, grant, finished) = (p.a.clone(), grant.clone(), finished.clone());
            checks.push(std::thread::spawn(move || {
                let r = run_check(&a, &spec, &grant);
                finished.fetch_add(1, SeqCst);
                r
            }));
            support::wait_for("the job to run on the node", 60, || {
                p.b.node.jobs.running() == t as u32
            });
        }
        support::wait_for("the step start to queue", 60, || {
            p.a.broker.snapshot()["waiting"].as_array().unwrap().len() == 1
        });
        let _ = run(&p, node_serve::stop_jobs(State(p.b.clone()))).unwrap();
        support::wait_for("every check to finish here", 60, || {
            finished.load(SeqCst) == n as usize
        });
        for c in checks {
            let r = c.join().unwrap().unwrap();
            assert!(r.passed && r.full.contains("local-ok"), "{}", r.full);
        }
        assert!(!started.load(SeqCst), "the step start is still waiting");
        let snap = p.a.broker.snapshot();
        let waiting = snap["waiting"].as_array().unwrap();
        assert_eq!(waiting.len(), 1, "only the step start: {waiting:?}");
        drop(grants);
        drop(start.join().unwrap());
    }
}

/// A node that goes away mid-job: after `lost_after_polls` polls in a row the run is
/// environmental (placed again, no task charged), the coordinator alerts, and the node's lease
/// then expires and it kills the orphan.
#[test]
fn a_node_that_goes_away_is_given_up_on_and_its_orphan_is_killed_when_its_lease_runs_out() {
    let p = working_pair_with(2, "lost_after_polls = 2", "", |_| {});
    let flag = p.sa.root.path().join("never");
    let cmd = flag_script(&p, "wait.sh", &flag);
    let b = p.b.clone();
    let r = std::thread::scope(|sc| {
        let h = sc.spawn(|| run_on_b(&p, &spec("run-lost", None, &cmd), &|| None));
        // The node counts the job before the coordinator has its answer: wait for the
        // coordinator to be following the run on b, or the listener goes while it is placing it.
        support::wait_for("the coordinator to follow the job on the node", 60, || {
            mason_daemon::remote_runs::view(&p.a, &|_| None, "")
                .runs
                .iter()
                .any(|r| r.id == "run-lost" && r.runs_on == "node b" && r.state == "running")
        });
        support::wait_for("the job to run on the node", 60, || {
            b.node.jobs.running() == 1
        });
        // The node's listener goes away (its machine slept, the network broke).
        p.rt_b.lock().unwrap().take().unwrap().shutdown_background();
        h.join().unwrap()
    });
    match r {
        Err(Fail::Environmental(why)) => assert!(why.contains("went away"), "{why}"),
        other => panic!("expected an environmental failure, got {other:?}"),
    }
    // No record is left to resume, and the coordinator says so.
    assert!(
        !mason_daemon::node_exec::record_path(&p.sa.root.path().join("logs"), "run-lost").exists()
    );
    for _ in 0..2 {
        poll(&p);
    }
    let n = listed(&p, "b");
    assert!(n["state"].as_str().unwrap().contains("lost"), "{n}");
    let alerts = nodes::alerts(&p.a);
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert!(alerts[0].2.contains("node b went away"), "{alerts:?}");
    // On the node the job is still running until nobody has asked after it for its lease.
    assert_eq!(b.node.jobs.running(), 1);
    b.node
        .jobs
        .tick(mason_daemon::daemon::now_ms() + 10_000_000);
    support::wait_for("the orphan to be killed", 60, || b.node.jobs.running() == 0);
    let jobs = b.node.jobs.list();
    assert_eq!(jobs[0]["state"], "cancelled", "{jobs:?}");
    assert!(
        jobs[0]["why"].as_str().unwrap().contains("lease"),
        "{jobs:?}"
    );
}

/// A coordinator that restarts finds the record of a run it sent and asks the node for that job:
/// nothing is sent again, and the result is the one the node has.
#[test]
fn a_restarted_coordinator_picks_up_the_job_its_record_names() {
    let p = working_pair(2);
    let flag = p.sa.root.path().join("go");
    let cmd = flag_script(&p, "wait.sh", &flag);
    let s = spec("run-restart", None, &cmd);
    let base = remote::base_ref_of(&p.a, "scratch").unwrap();
    let log = p.sa.root.path().join("logs");
    // The first coordinator sends the job and dies.
    let rec = mason_daemon::node_exec::start(&p.a, None, &s, &p.sa.repo, &base, &log).unwrap();
    assert!(mason_daemon::node_exec::record_path(&log, "run-restart").exists());
    assert_eq!(p.b.node.jobs.running(), 1);
    // Its successor runs the same step: no second job, the same one is followed to its end.
    std::fs::write(&flag, "go").unwrap();
    let f = done(mason_daemon::node_exec::execute(
        &p.a,
        None,
        &s,
        &p.sa.repo,
        &base,
        &log,
        &|| None,
        &mut |_, _| {},
    ));
    assert_eq!((f.code, f.output.as_str()), (0, "finished\n"));
    assert_eq!(rec.job, "run-restart");
    assert_eq!(job_dirs_on_b(&p), 0);
}

/// A node that restarts still has a finished job on disk, and gives the result from it.
#[test]
fn a_finished_job_survives_a_restart_of_the_node() {
    let p = working_pair(2);
    let s = spec("run-kept", None, "echo kept");
    let base = remote::base_ref_of(&p.a, "scratch").unwrap();
    let log = p.sa.root.path().join("logs");
    mason_daemon::node_exec::start(&p.a, None, &s, &p.sa.repo, &base, &log).unwrap();
    support::wait_for("the job to end on the node", 60, || {
        p.b.node.jobs.list().iter().all(|j| j["state"] == "done")
    });
    // A new daemon's view of the same directory.
    let fresh = mason_daemon::node_run::Jobs::new(&p.sb.data_dir);
    let coord = coord_of_a(&p);
    let a = fresh.artifact(&coord, "run-kept").unwrap();
    assert_eq!(a["result"]["code"], 0, "{a}");
    assert_eq!(
        String::from_utf8(fresh.log(&coord, "run-kept", 0)).unwrap(),
        "kept\n"
    );
    // And the coordinator, following it, collects as usual.
    let f = done(mason_daemon::node_exec::execute(
        &p.a,
        None,
        &s,
        &p.sa.repo,
        &base,
        &log,
        &|| None,
        &mut |_, _| {},
    ));
    assert_eq!(f.output, "kept\n");
}

/// A node whose mirror was cleared is sent the whole history again; a start for a commit the
/// node does not have is `412`.
#[test]
fn a_node_that_lost_its_mirror_is_sent_everything_again_and_a_missing_commit_is_412() {
    let p = working_pair(2);
    std::fs::write(p.sa.repo.join("t.sh"), "echo again\n").unwrap();
    let f = done(run_on_b(&p, &spec("run-m1", None, "sh t.sh"), &|| None));
    assert_eq!(f.output, "again\n");
    // The node clears its mirrors and slots.
    std::fs::remove_dir_all(p.sb.data_dir.join("node/mirrors")).unwrap();
    std::fs::remove_dir_all(p.sb.data_dir.join("node/work")).unwrap();
    let f = done(run_on_b(&p, &spec("run-m2", None, "sh t.sh"), &|| None));
    assert_eq!(f.output, "again\n");
    // A start for a commit that is not in the mirror, over the wire.
    let (_, fp) = (
        0,
        lan::sha256_hex(&lan::load_or_create_identity(&p.sb.data_dir).unwrap().0),
    );
    let client = NodeClient::new(
        lan::load_or_create_identity(&p.sa.data_dir).unwrap(),
        "127.0.0.1",
        p.port,
        &fp,
    )
    .unwrap();
    let r = run(
        &p,
        client.request(
            "POST",
            "/node/jobs",
            Some(&json!({
                "id": "run-m3", "project": "scratch", "sha": "1".repeat(40),
                "run": {"command": "true"}, "limit_s": 60, "lease_s": 60,
            })),
        ),
    )
    .unwrap();
    assert_eq!(r.status, 412, "{}", r.error_text());
    assert_eq!(r.json()["missing"], "1".repeat(40));
    // A bad id, project or sha is refused before anything is touched.
    for bad in [
        json!({"id": "../x", "project": "scratch", "sha": "1".repeat(40), "run": {"command": "true"}}),
        json!({"id": "a", "project": "a/b", "sha": "1".repeat(40), "run": {"command": "true"}}),
        json!({"id": "a", "project": "scratch", "sha": "zz", "run": {"command": "true"}}),
    ] {
        let r = run(&p, client.request("POST", "/node/jobs", Some(&bad))).unwrap();
        assert_eq!(r.status, 400, "{bad}");
    }
}

/// A node on another protocol is listed as incompatible, never used, and raises an alert.
#[test]
fn a_node_on_another_protocol_is_incompatible_and_never_used() {
    let p = working_pair_with(2, "", "", |n| n.proto = 2);
    let n = listed(&p, "b");
    assert!(n["state"].as_str().unwrap().contains("incompatible"), "{n}");
    assert_eq!(n["usable"], false);
    let r = run_on_b(&p, &spec("run-old", None, "echo hi"), &|| None);
    assert!(matches!(r, Err(Fail::Environmental(_))), "{r:?}");
    let alerts = nodes::alerts(&p.a);
    assert!(
        alerts.iter().any(|a| a.2.contains("incompatible")),
        "{alerts:?}"
    );
}

/// A run on a node is a row of the monitor's "Remote runs": running while it runs, then its
/// outcome.
#[test]
fn a_run_on_a_node_is_a_row_of_remote_runs() {
    let p = working_pair(2);
    let flag = p.sa.root.path().join("go");
    let cmd = flag_script(&p, "wait.sh", &flag);
    let rows = || {
        mason_daemon::remote_runs::view(&p.a, &|_| None, "")
            .runs
            .into_iter()
            .filter(|r| r.id == "run-row")
            .collect::<Vec<_>>()
    };
    std::thread::scope(|sc| {
        let h = sc.spawn(|| run_on_b(&p, &spec("run-row", None, &cmd), &|| None));
        support::wait_for("the run to be listed", 60, || !rows().is_empty());
        let r = rows().remove(0);
        assert_eq!(r.runs_on, "node b");
        assert_eq!(r.state, "running");
        assert!(r.url.is_empty() && r.jobs.is_empty());
        std::fs::write(&flag, "go").unwrap();
        let _ = done(h.join().unwrap());
    });
    let r = rows().remove(0);
    assert_eq!(r.state, "green", "{r:?}");
    assert!(mason_daemon::remote_runs::view(&p.a, &|_| None, "").enabled);
}

// ---- #1155: what runs where ---------------------------------------------------------------------

/// A suite of `name` for a check's place.
fn named_suite(name: &str) -> Box<mason_testing::Suite> {
    Box::new(mason_testing::Suite::new(
        name,
        "true",
        mason_testing::ParserKind::Tap,
    ))
}

/// The hosts of `/status` (as `mason status --json` prints it) that have a run of task `t`.
fn hosts_running(p: &Pair, t: u64) -> Vec<String> {
    let s: Value = serde_json::from_str(&p.sa.mason_ok(&["status", "--json"])).unwrap();
    let mut out = vec![];
    for h in s["hosts"].as_array().expect("status has hosts") {
        for r in h["runs"].as_array().unwrap() {
            if r["task"] == t {
                assert_eq!(r["state"], "proposed", "the run carries the task: {r}");
                assert_eq!(r["title"], "Watched", "{r}");
                out.push(r["host"].as_str().unwrap().to_string());
            }
        }
    }
    out
}

/// The hosts `/tasks/{id}` says task `t` is running on.
fn running_on(p: &Pair, t: u64) -> Vec<String> {
    p.sa.task_json(t)["running_on"]
        .as_array()
        .expect("the task has running_on")
        .iter()
        .map(|r| r["host"].as_str().unwrap().to_string())
        .collect()
}

/// The lines `mason hosts` prints under the host whose line starts with `head`.
fn under(text: &str, head: &str) -> Vec<String> {
    let mut lines = text.lines().skip_while(|l| !l.starts_with(head));
    assert!(lines.next().is_some(), "no host {head} in:\n{text}");
    lines
        .take_while(|l| l.starts_with("  "))
        .map(str::to_string)
        .collect()
}

/// A check is listed under the node while it runs there, under this Mac once the node gives it
/// back, and nowhere once it has ended; the placement decision is still on record. A GitHub run
/// is listed from its hold.
#[test]
fn a_check_in_flight_is_listed_under_its_host_until_it_ends() {
    use mason_daemon::hostedcheck::{run as run_check, CheckSpec};
    let p = working_pair_full(
        2,
        "",
        "on_switch_off = \"cancel\"",
        |c| c.port = 0,
        |_| {},
        |_| {},
    );
    mason_daemon::place::set_global(&p.a, mason_daemon::place::Mode::Remote, "test").unwrap();
    let served = p.a.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _ = rt.block_on(mason_daemon::http::serve(served));
    });
    wait_for("the daemon to be served", 60, || {
        p.a.cfg.info_path().exists()
    });
    let t = file_task_as(
        &p.a,
        "Watched",
        "Please watch.",
        false,
        mason_core::TrustSource::Owner,
        true,
    )
    .0;
    assert!(hosts_running(&p, t).is_empty());

    let logs = tempfile::tempdir().unwrap();
    let flag = p.sa.root.path().join("finish");
    let spec = CheckSpec {
        id_prefix: format!("t{t}r1-check"),
        log_dir: logs.path().to_path_buf(),
        // On the node this hangs; here it waits for the flag.
        cmd: format!(
            "if [ -n \"$MASON_NODE\" ]; then sleep 300; else while [ ! -f '{}' ]; do sleep 0.1; done; fi",
            flag.display()
        ),
        build: None,
        cwd: p.sa.repo.clone(),
        env: vec![],
        limit: std::time::Duration::from_secs(120),
        tree: "tree-watched".into(),
        stop: None,
        holds: Default::default(),
        label: String::new(),
        place: Some((Some(mason_core::TaskId(t)), named_suite("unit"))),
        global: false,
    };
    std::thread::scope(|sc| {
        let h = sc.spawn(|| run_check(&p.a, &spec, &mason_daemon::remote::Grant::attached()));
        wait_for("the check to be listed on the node", 60, || {
            p.b.node.jobs.running() == 1 && running_on(&p, t) == ["node:b"]
        });
        assert_eq!(hosts_running(&p, t), ["node:b"]);
        let listed = p.sa.mason_ok(&["hosts"]);
        let on_b = under(&listed, "node:b");
        assert!(
            on_b.iter()
                .any(|l| l.contains(&format!("#{t} proposed")) && l.contains("check unit")),
            "{listed}"
        );
        let shown = p.sa.mason_ok(&["task", "show", &t.to_string()]);
        assert!(shown.contains("running on: node b (check unit"), "{shown}");

        // The node gives the job back: it runs here, and is listed here only.
        set_switch(&p, false);
        let _ = run(
            &p,
            node_serve::accept(
                State(p.b.clone()),
                axum::http::HeaderMap::new(),
                Json(json!({"on": false})),
            ),
        )
        .unwrap();
        wait_for("the check to be listed here", 60, || {
            running_on(&p, t) == ["local"]
        });
        assert_eq!(hosts_running(&p, t), ["local"]);
        std::fs::write(&flag, "go").unwrap();
        let out = h.join().unwrap().unwrap();
        assert!(out.passed, "{}", out.full);
    });
    assert!(hosts_running(&p, t).is_empty());
    assert!(running_on(&p, t).is_empty());
    assert!(!p.sa.mason_ok(&["hosts"]).contains(&format!("#{t} ")));
    assert!(!p
        .sa
        .mason_ok(&["task", "show", &t.to_string()])
        .contains("running on:"));
    assert!(
        mason_daemon::place::seen(Some(t))
            .iter()
            .any(|s| s["where"] == "node"),
        "the decision stays on record"
    );

    // A run on GitHub, as the executor records it while it is out.
    let rs = {
        let mut rs = remote::spec_for(
            "scratch",
            &format!("t{t}r2-check0"),
            None,
            "true",
            std::time::Duration::from_secs(60),
            &[],
            "macos-15",
            "",
        );
        rs.suite = "unit".into();
        rs
    };
    let dir = logs.path();
    mason_step_host::registry::write_kv(
        &dir.join(format!("{}.remote", rs.id)),
        &[
            ("sha", "abc".to_string()),
            ("collected", "0".to_string()),
            ("url", "https://github.com/o/r/actions/runs/9".to_string()),
            ("runs_on", "macos-15".to_string()),
        ],
    )
    .unwrap();
    let hold = remote::HoldGuard::new(
        &p.a.cfg.hosts_dir(),
        &rs,
        &rs.id,
        &rs.id,
        1,
        dir,
        "check",
        t,
    );
    assert_eq!(running_on(&p, t), ["github:macos-15"]);
    assert_eq!(hosts_running(&p, t), ["github:macos-15"]);
    let listed = p.sa.mason_ok(&["hosts"]);
    assert!(
        under(&listed, "GitHub")
            .iter()
            .any(|l| l.contains("check unit on macos-15") && l.contains("actions/runs/9")),
        "{listed}"
    );
    drop(hold);
    assert!(running_on(&p, t).is_empty());
    assert!(hosts_running(&p, t).is_empty());
}

// ---- §3.14: privacy prompts, signing and the doctor ---------------------------------------------

/// The doctor checks the node and keeps what it found; a blocking problem is why the node takes
/// no work, in words the owner can act on, and it is gone when the problem is.
#[test]
fn what_the_doctor_finds_stops_the_node_taking_work_until_it_is_fixed() {
    use mason_daemon::node_doctor::{Check, Report};
    let p = working_pair(2);
    // The check itself, on this machine: the listener is up and the data directory writable.
    let r = run(&p, node_serve::doctor(State(p.b.clone()))).unwrap().0;
    let by = |name: &str| {
        r["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .cloned()
    };
    assert_eq!(by("listener").unwrap()["ok"], true, "{r}");
    assert!(
        r["checks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["name"] == "path")
            .all(|c| c["ok"] == true),
        "{r}"
    );
    // A test's daemon does not keep a report of a machine it does not measure.
    assert!(p.b.node.doctor.lock().unwrap().is_none());
    // A problem found at start-up or by the periodic check:
    *p.b.node.doctor.lock().unwrap() = Some(Report {
        at_ms: 1,
        checks: vec![Check {
            name: "signature".into(),
            ok: false,
            detail: "masond is signed ad hoc; privacy grants will not hold: install the runtime from the coordinator (`mason node export-runtime`)".into(),
            blocks: true,
        }],
    });
    poll(&p);
    let n = listed(&p, "b");
    assert_eq!(n["accepting"], false, "{n}");
    assert!(
        n["state"]
            .as_str()
            .unwrap()
            .contains("masond is signed ad hoc"),
        "{n}"
    );
    assert_eq!(n["usable"], false);
    let r = run_on_b(&p, &spec("run-doctor", None, "echo hi"), &|| None);
    assert!(matches!(r, Err(Fail::Environmental(_))), "{r:?}");
    // The node's own status carries the report.
    let local = run(&p, node_serve::local_status(State(p.b.clone())))
        .unwrap()
        .0;
    assert_eq!(local["doctor"]["ok"], false, "{local}");
    // Fixed: the next check finds nothing, and the node takes work again.
    *p.b.node.doctor.lock().unwrap() = None;
    poll(&p);
    assert_eq!(listed(&p, "b")["usable"], true);
}

/// The doctor looks for tools where a job does: a `cargo` in `$HOME/.cargo/bin` counts although
/// the `PATH` it was started with (`sudo -H`, a LaunchDaemon) does not name that folder.
#[cfg(unix)]
#[test]
fn the_doctor_finds_cargo_on_the_path_a_job_gets() {
    use mason_daemon::node_doctor::{evaluate, gather_in, Check, Mdns};
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let bin = home.join(".cargo/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let base = |k: &str| match k {
        "HOME" => Some(home.to_string_lossy().into_owned()),
        "PATH" => Some("/usr/bin:/bin".to_string()),
        _ => None,
    };
    let cargo = || -> Check {
        let data = t.path().join("data");
        let i = gather_in(&base, &data, &data, true, Mdns::Off, true);
        evaluate(&i, 1)
            .checks
            .into_iter()
            .find(|c| c.name == "cargo")
            .unwrap()
    };
    let stub = |body: &str| {
        let f = bin.join("cargo");
        std::fs::write(&f, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    // A machine with a cargo of its own in a system folder cannot be made to have none.
    let system = mason_daemon::node_run::job_path(None, "", "/usr/bin:/bin");
    if !std::env::split_paths(&system).any(|d| d.join("cargo").exists()) {
        let c = cargo();
        assert!(!c.ok && c.blocks, "{c:?}");
        assert!(c.detail.contains("does not run as this user"), "{c:?}");
    }
    // It runs from the home folder.
    stub(r#"[ "$(pwd -P)" = "$(cd "$HOME" && pwd -P)" ] || exit 1; echo "cargo 9.9.9 (stub)""#);
    let c = cargo();
    assert!(c.ok, "{c:?}");
    assert_eq!(c.detail, "cargo: cargo 9.9.9 (stub)");
    stub(
        r#"echo "info: syncing" >&2
echo "error: rustup could not choose a version of cargo to run, because one wasn't specified explicitly" >&2
exit 1"#,
    );
    let c = cargo();
    assert!(!c.ok && c.blocks, "{c:?}");
    assert!(c.detail.contains("could not choose a version"), "{c:?}");
    assert!(c.detail.contains("rustup default"), "{c:?}");
}

/// A runtime waiting for the node's jobs to end holds the node, and says so.
#[test]
fn a_node_that_is_to_take_over_a_runtime_takes_no_new_job_and_says_why() {
    let p = working_pair(2);
    *p.b.node.pending_runtime.lock().unwrap() = Some("a".repeat(40));
    poll(&p);
    let n = listed(&p, "b");
    assert_eq!(n["accepting"], false, "{n}");
    assert!(
        n["state"]
            .as_str()
            .unwrap()
            .contains("updating to runtime aaaaaaaaaa"),
        "{n}"
    );
    *p.b.node.pending_runtime.lock().unwrap() = None;
    poll(&p);
    assert_eq!(listed(&p, "b")["usable"], true);
}

/// The stall of 2026-11-26: a runtime that arrived made the node refuse every job until the
/// running ones ended (some run an hour), with free slots idle. A running job now does not hold
/// the takeover up: the node says it takes over now, and the job carries on and is collected.
#[test]
fn a_runtime_waiting_to_take_over_does_not_wait_for_a_running_job_and_the_job_survives_it() {
    let p = working_pair(2);
    let flag = p.sa.root.path().join("go");
    let cmd = flag_script(&p, "wait.sh", &flag);
    let s = spec("run-over", None, &cmd);
    let base = remote::base_ref_of(&p.a, "scratch").unwrap();
    let log = p.sa.root.path().join("logs");
    let b = p.b.clone();
    // The coordinator sends the job and does not collect it yet, so nothing deletes it below.
    mason_daemon::node_exec::start(&p.a, None, &s, &p.sa.repo, &base, &log).unwrap();
    support::wait_for("the job's command to run under its host", 60, || {
        b.node.jobs.running() == 1 && b.node.jobs.settling() == 0
    });
    *b.node.pending_runtime.lock().unwrap() = Some("a".repeat(40));
    poll(&p);
    let n = listed(&p, "b");
    let state = n["state"].as_str().unwrap();
    assert!(
        state.contains("taking over now; 1 running job carries on"),
        "{n}"
    );
    assert!(!state.contains("waiting for"), "{n}");
    // The takeover itself (the exec) cannot run in a test, and a second daemon cannot follow
    // the same host beside this one without the two fighting over its record; what is checked is
    // that nothing in the pending state touches the job: it runs on and the coordinator collects.
    assert_eq!(b.node.jobs.running(), 1);
    std::fs::write(&flag, "go").unwrap();
    // And the coordinator collects the result as usual.
    let f = done(mason_daemon::node_exec::execute(
        &p.a,
        None,
        &s,
        &p.sa.repo,
        &base,
        &log,
        &|| None,
        &mut |_, _| {},
    ));
    assert_eq!((f.code, f.output.as_str()), (0, "finished\n"));
}

/// A takeover that could not start the new runtime does not leave the node refusing work: it
/// serves as it was, with free slots, until the retry.
#[test]
fn a_node_whose_takeover_failed_keeps_taking_jobs_under_the_runtime_it_has() {
    let p = working_pair(2);
    let flag = p.sa.root.path().join("go");
    let cmd = flag_script(&p, "wait.sh", &flag);
    let b = p.b.clone();
    *b.node.pending_runtime.lock().unwrap() = Some("a".repeat(40));
    b.node
        .takeover_failed_ms
        .store(1, std::sync::atomic::Ordering::SeqCst);
    std::thread::scope(|sc| {
        let h = sc.spawn(|| run_on_b(&p, &spec("run-held", None, &cmd), &|| None));
        support::wait_for("the job to run on the node", 60, || {
            b.node.jobs.running() == 1
        });
        poll(&p);
        let n = listed(&p, "b");
        assert_eq!(n["accepting"], true, "a free slot is taken: {n}");
        assert!(b.node.pending_runtime.lock().unwrap().is_some());
        std::fs::write(&flag, "go").unwrap();
        let f = done(h.join().unwrap());
        assert_eq!(f.output, "finished\n");
    });
    // And a second job is accepted while the runtime is still pending.
    let f = done(run_on_b(&p, &spec("run-next", None, "echo next"), &|| None));
    assert_eq!(f.output, "next\n");
}

fn put_runtime(p: &Pair, bytes: Vec<u8>) -> mason_daemon::node_client::Response {
    let fp = lan::sha256_hex(&lan::load_or_create_identity(&p.sb.data_dir).unwrap().0);
    let client = NodeClient::new(
        lan::load_or_create_identity(&p.sa.data_dir).unwrap(),
        "127.0.0.1",
        p.port,
        &fp,
    )
    .unwrap();
    run(
        p,
        client.send(
            "PUT",
            "/node/runtime",
            Some("application/octet-stream"),
            bytes,
        ),
    )
    .unwrap()
}

/// A node installs a runtime only from a coordinator that was signed by a team when it paired,
/// and only if every executable of the package verifies and is that team's.
#[test]
fn a_node_refuses_a_runtime_it_cannot_check_and_installs_nothing_of_one_that_does_not_verify() {
    use mason_daemon::node_runtime::{pack, File};
    let p = working_pair(2);
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let file = |name: &str| File {
        path: name.into(),
        mode: 0o755,
        bytes: format!("not a Mach-O file: {name}").into_bytes(),
    };
    let package = pack(sha, &[file("masond"), file("step-host")]);
    // This test's coordinator is not signed by a team, so none was pinned at pairing.
    let r = put_runtime(&p, package.clone());
    assert_eq!(r.status, 412, "{}", r.error_text());
    assert!(
        r.error_text()
            .contains("not signed by a team when it paired"),
        "{}",
        r.error_text()
    );
    // A team pinned: the package is now looked at, and a file that is no signed program fails.
    let (cert, _) = lan::load_or_create_identity(&p.sa.data_dir).unwrap();
    p.b.node
        .add_coordinator(
            &lan::sha256_hex(&cert),
            &lan::hostname(),
            1,
            Some("TEAM111111".into()),
        )
        .unwrap();
    let r = put_runtime(&p, b"not a package at all".to_vec());
    assert_eq!(r.status, 422, "{}", r.error_text());
    assert!(
        r.error_text().contains("not a MASON runtime package"),
        "{}",
        r.error_text()
    );
    #[cfg(target_os = "macos")]
    {
        let r = put_runtime(&p, package);
        assert_eq!(r.status, 422, "{}", r.error_text());
        assert!(
            r.error_text().contains("does not verify"),
            "{}",
            r.error_text()
        );
    }
    // Nothing was installed, whole or part, and no takeover is waiting.
    let rt = p.sb.data_dir.join("runtimes");
    let left: Vec<String> = std::fs::read_dir(&rt)
        .map(|r| {
            r.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(left.iter().all(|n| !n.starts_with(sha)), "{left:?}");
    assert!(p.b.node.pending_runtime.lock().unwrap().is_none());
    // The pinned team is on disk beside the fingerprint, and only there.
    let text = std::fs::read_to_string(p.sb.data_dir.join("node/coordinators.json")).unwrap();
    assert!(text.contains("TEAM111111"), "{text}");
}

/// The data directory of `docs/nodes.md` made every job id of any length too long a socket path
/// for its step host. A node whose data directory is as long as that one starts its hosts and
/// finishes its jobs, and its doctor passes the socket check.
#[test]
fn a_node_with_the_documented_length_of_data_dir_runs_jobs_with_long_ids() {
    use mason_daemon::{config::NodeSettings, node_doctor, node_run};
    const DOCUMENTED: &str = "/Users/masonnode/Library/Application Support/mason";
    let p = pair();
    // Under /tmp: $TMPDIR on macOS is already too long for a socket with this much after it.
    let tmp = tempfile::Builder::new()
        .prefix("mn")
        .tempdir_in("/tmp")
        .unwrap();
    let used = tmp.path().as_os_str().len() + 1;
    let data = tmp.path().join("d".repeat(DOCUMENTED.len() - used));
    assert_eq!(data.as_os_str().len(), DOCUMENTED.len());
    let jobs = Arc::new(node_run::Jobs::new(&data));
    let coord = "ad7507fdc188cdd0";
    let mirror = jobs.mirror_path(coord, "scratch");
    std::fs::create_dir_all(mirror.parent().unwrap()).unwrap();
    sh(
        &p.sa.repo,
        &["clone", "-q", "--bare", ".", mirror.to_str().unwrap()],
    );
    let sha = sh(&p.sa.repo, &["rev-parse", "HEAD"]).trim().to_string();
    for id in [
        "t1113r6691-check-cargo-workspace-build3".to_string(),
        format!("b421-r1-bis6-7-{}", "x".repeat(45)),
    ] {
        let req = node_run::StartReq {
            id: id.clone(),
            project: "scratch".into(),
            sha: sha.clone(),
            build: Some("echo built".into()),
            command: "echo ran".into(),
            results_file: None,
            env: vec![],
            limit_s: 120,
            lease_s: 300,
            base: None,
        };
        jobs.start(&p.b, coord, req, &NodeSettings::default(), &|_| None)
            .unwrap();
        support::wait_for("the job to end", 60, || {
            jobs.list().iter().all(|j| j["state"] != "running")
        });
        let a = jobs.artifact(coord, &id).unwrap();
        assert_eq!(a["result"]["state"], "done", "{a}");
        assert_eq!(a["result"]["code"], 0, "{a}");
        assert_eq!(
            String::from_utf8(jobs.log(coord, &id, 0)).unwrap(),
            "built\nran\n"
        );
        // The folder is named by the key; what it is for is written in it.
        for phase in ["build", "run"] {
            let dir = data
                .join("node/hosts")
                .join(node_run::host_key(coord, &id, phase));
            assert!(dir.join("host.kv").exists(), "{}", dir.display());
            assert_eq!(
                std::fs::read_to_string(dir.join("name")).unwrap(),
                format!("{coord}-{id}-{phase}\n")
            );
        }
        assert!(jobs.ack(coord, &id));
    }
    // What `mason node doctor` says of the socket paths, here and under the documented one.
    for dir in [data.as_path(), Path::new(DOCUMENTED)] {
        let report = node_doctor::evaluate(
            &node_doctor::Inputs {
                signature: node_doctor::Signature::Unchecked,
                paths: vec![],
                host_socket: node_doctor::longest_host_socket(dir),
                listener_bound: true,
                mdns: node_doctor::Mdns::Off,
                tools: vec![],
                xctest: None,
            },
            1,
        );
        let check = report
            .checks
            .iter()
            .find(|c| c.name == "socket path")
            .expect("the doctor checks the socket path");
        assert!(check.ok, "{}", check.detail);
    }
}

// ---- s1122: the fleet update ----------------------------------------------------------------

mod fleet {
    use super::*;
    use mason_daemon::node_fleet::{self, Fleet, FleetIo, Phase};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Condvar, Mutex};

    /// The commit this coordinator runs after the update, and the one the node runs.
    pub const NEW: &str = "1111111111111111111111111111111111111111";
    pub const OLD: &str = "2222222222222222222222222222222222222222";

    /// Stands in for building and sending: a package is the commit's name, and a send does what a
    /// node's `PUT /node/runtime` leaves behind (an installed, complete runtime directory) without
    /// needing a Developer ID to sign with. Nothing is executable, so nothing can be exec'd.
    pub struct FakeIo {
        node_data: std::path::PathBuf,
        pub fail: AtomicBool,
        pub built: Mutex<Vec<String>>,
        pub sends: AtomicUsize,
        /// While held, a build waits (so that a test can look at "building").
        hold: (Mutex<bool>, Condvar),
    }

    impl FakeIo {
        pub fn new(p: &Pair) -> Arc<FakeIo> {
            let io = Arc::new(FakeIo {
                node_data: p.sb.data_dir.clone(),
                fail: AtomicBool::new(false),
                built: Mutex::new(vec![]),
                sends: AtomicUsize::new(0),
                hold: (Mutex::new(false), Condvar::new()),
            });
            p.a.nodes.set_io(io.clone());
            io
        }

        pub fn hold(&self, on: bool) {
            *self.hold.0.lock().unwrap() = on;
            self.hold.1.notify_all();
        }
    }

    impl FleetIo for FakeIo {
        fn package(
            &self,
            _: &Arc<Daemon>,
            _: &mason_daemon::update::UpdateCfg,
            sha: &str,
            arch: &str,
            _: bool,
        ) -> Result<Vec<u8>, String> {
            let mut held = self.hold.0.lock().unwrap();
            while *held {
                held = self.hold.1.wait(held).unwrap();
            }
            self.built.lock().unwrap().push(arch.to_string());
            Ok(sha.as_bytes().to_vec())
        }

        fn send(&self, _: &Arc<Daemon>, _: &str, package: &[u8]) -> Result<(), String> {
            self.sends.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                return Err("the node said no".into());
            }
            let dir = self
                .node_data
                .join("runtimes")
                .join(String::from_utf8_lossy(package).as_ref());
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("READY"), "").unwrap();
            Ok(())
        }
    }

    pub fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// A coordinator on `NEW` paired with a node that is `proto` and runs `node_runs`.
    pub fn pair_on(proto: u32, node_runs: Option<&str>, port: u16) -> Pair {
        let node_runs = node_runs.map(String::from);
        let p = working_pair_full(
            2,
            "",
            "",
            |c| c.runtime_sha = Some(NEW.into()),
            move |c| c.runtime_sha = node_runs,
            move |n| {
                n.proto = proto;
                n.port = port;
            },
        );
        std::fs::write(
            p.sa.config_dir.join("update.toml"),
            "repo = \"/nonexistent\"\nsign_identity = \"Test Identity\"\n",
        )
        .unwrap();
        p
    }

    pub fn ucfg(p: &Pair) -> mason_daemon::update::UpdateCfg {
        mason_daemon::update::UpdateCfg::load(&p.sa.config_dir).unwrap()
    }

    pub fn nid(p: &Pair) -> String {
        listed(p, "b")["nid"].as_str().unwrap().to_string()
    }

    pub fn state(p: &Pair) -> Fleet {
        node_fleet::states(&p.a)
            .into_iter()
            .find(|s| s.1 == "b")
            .unwrap()
            .2
    }

    /// `GET /update`'s `nodes`, as `mason update status` prints it.
    pub fn update_status_line(p: &Pair) -> String {
        node_fleet::states_json(&p.a)
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["name"] == "b")
            .map(|n| n["text"].as_str().unwrap().to_string())
            .unwrap()
    }

    pub fn deliver_now(p: &Pair, h: &node_fleet::Handle) -> usize {
        node_fleet::deliver(&p.a, &ucfg(p), NEW, h, std::time::Duration::from_secs(120))
    }

    /// The contract `PUT /node/runtime` and the status it relies on keep across protocols.
    #[test]
    fn the_runtime_route_and_its_status_keys_do_not_depend_on_the_node_protocol() {
        assert_eq!(node_serve::RUNTIME_ROUTE, "/node/runtime");
        assert_eq!(
            node_serve::RUNTIME_STATUS_KEYS,
            ["node_proto", "arch", "runtime_sha", "runtimes"]
        );
        let other = node_serve::NODE_PROTO + 1;
        let p = pair_on(other, Some(OLD), 0);
        // The node reports every key the contract names, whatever protocol it speaks, and the
        // coordinator keeps that status although it calls the node incompatible.
        let status = p.a.nodes.view(&nid(&p)).unwrap().status.unwrap();
        for k in node_serve::RUNTIME_STATUS_KEYS {
            assert!(status.get(k).is_some(), "{k} is missing from {status}");
        }
        assert_eq!(status["node_proto"], other);
        assert_eq!(status["runtime_sha"], OLD);
        assert!(listed(&p, "b")["state"]
            .as_str()
            .unwrap()
            .contains("incompatible"));
        // And the route answers a coordinator on another protocol: it reaches the handler, which
        // refuses for want of a pinned team (412), not for the protocol.
        let r = put_runtime(&p, b"anything".to_vec());
        assert_eq!(r.status, 412, "{}", r.error_text());
        let (cert, _) = lan::load_or_create_identity(&p.sa.data_dir).unwrap();
        p.b.node
            .add_coordinator(
                &lan::sha256_hex(&cert),
                &lan::hostname(),
                1,
                Some("TEAM111111".into()),
            )
            .unwrap();
        let r = put_runtime(&p, b"anything".to_vec());
        assert_eq!(r.status, 422, "{}", r.error_text());
        assert!(
            r.error_text().contains("not a MASON runtime package"),
            "{}",
            r.error_text()
        );
    }

    /// An update that bumps the protocol still delivers: the old node is incompatible, is sent the
    /// runtime anyway, and its update goes building, sending, sent, installed.
    #[test]
    fn an_update_with_a_protocol_bump_still_delivers_the_runtime_to_the_old_node() {
        let p = pair_on(node_serve::NODE_PROTO + 1, Some(OLD), 0);
        let io = FakeIo::new(&p);
        let node = nid(&p);
        assert!(listed(&p, "b")["state"]
            .as_str()
            .unwrap()
            .contains("incompatible"));
        assert_eq!(
            listed(&p, "b")["update"],
            "will be sent the new runtime at the next poll"
        );
        // The update has started (building): the node is asked for nothing yet.
        let h = node_fleet::begin(&p.a, &ucfg(&p), NEW, true);
        io.hold(true);
        let sent = std::thread::scope(|sc| {
            let d = sc.spawn(|| deliver_now(&p, &h));
            support::wait_for("the runtime to be building", 60, || {
                matches!(state(&p), Fleet::Building(_))
            });
            assert!(update_status_line(&p).starts_with("building for "));
            io.hold(false);
            d.join().unwrap()
        });
        assert_eq!(sent, 1);
        assert_eq!(io.sends.load(Ordering::SeqCst), 1);
        // Sent, and the node lists it: installed, taking over when its jobs end.
        poll(&p);
        assert_eq!(state(&p), Fleet::Installed(0));
        assert_eq!(update_status_line(&p), "installed; taking over now");
        let row = listed(&p, "b");
        assert_eq!(row["update"], "installed; taking over now");
        assert_eq!(row["update_state"], "installing");
        assert_eq!(row["node_proto"], node_serve::NODE_PROTO + 1);
        assert_eq!(row["runtime_sha"], OLD);
        assert_eq!(node_fleet::record_of(&node).unwrap().phase, Phase::Sent);
        // Nothing more is sent to a node that holds it.
        // Nothing more is sent to a node that holds it: the poll finds no candidate.
        poll(&p);
        assert!(node_fleet::candidates(&p.a, NEW).is_empty());
        assert_eq!(io.sends.load(Ordering::SeqCst), 1);
    }

    /// A node that already runs the coordinator's commit is up to date in every view.
    #[test]
    fn a_node_on_the_coordinators_commit_is_up_to_date_in_the_node_list_and_the_update_status() {
        let p = pair_on(node_serve::NODE_PROTO, Some(NEW), 0);
        let io = FakeIo::new(&p);
        let row = listed(&p, "b");
        assert_eq!(row["runtime_sha"], NEW);
        assert_eq!(row["update"], "up to date");
        assert_eq!(row["update_state"], "up-to-date");
        assert_eq!(state(&p), Fleet::UpToDate);
        assert_eq!(update_status_line(&p), "up to date");
        assert_eq!(node_fleet::candidates(&p.a, NEW).len(), 0);
        let h = node_fleet::begin(&p.a, &ucfg(&p), NEW, true);
        assert_eq!(deliver_now(&p, &h), 0);
        assert_eq!(io.sends.load(Ordering::SeqCst), 0);
    }

    /// A failed send is recorded, raised as an alert, and not retried by the poll for ten minutes;
    /// `mason node update` sends at once all the same.
    #[test]
    fn a_failed_push_is_an_alert_and_node_update_all_sends_inside_the_retry_window() {
        let p = pair_on(node_serve::NODE_PROTO, Some(OLD), 0);
        let io = FakeIo::new(&p);
        io.fail.store(true, Ordering::SeqCst);
        let h = node_fleet::begin(&p.a, &ucfg(&p), NEW, true);
        assert_eq!(deliver_now(&p, &h), 0);
        assert_eq!(
            state(&p),
            Fleet::Failed("the node said no".into()),
            "{}",
            update_status_line(&p)
        );
        let alerts = nodes::alerts(&p.a);
        assert!(
            alerts.iter().any(|a| a.0.ends_with(":update")
                && a.2.contains("could not be updated")
                && a.2.contains("the node said no")),
            "{alerts:?}"
        );
        // Inside the retry window the poll does not try again.
        let c = node_fleet::candidates(&p.a, NEW).pop().unwrap();
        let why = node_fleet::start_push(&p.a, &ucfg(&p), &c, NEW, false).unwrap_err();
        assert!(why.contains("tried a moment ago"), "{why}");
        assert_eq!(io.sends.load(Ordering::SeqCst), 1);
        // `mason node update --all` does, and it works this time.
        io.fail.store(false, Ordering::SeqCst);
        let r = run(
            &p,
            nodes::update(State(p.a.clone()), Json(json!({"all": true}))),
        )
        .unwrap()
        .0;
        assert_eq!(r["nodes"][0]["started"], true, "{r}");
        support::wait_for("the runtime to be sent", 60, || {
            io.sends.load(Ordering::SeqCst) == 2
                && node_fleet::record_of(&nid(&p)).unwrap().phase == Phase::Sent
        });
        poll(&p);
        assert_eq!(state(&p), Fleet::Installed(0));
        assert!(
            nodes::alerts(&p.a)
                .iter()
                .all(|a| !a.0.ends_with(":update")),
            "the alert goes with the failure"
        );
        // Named, or all: a name that is not paired, and neither, are refused.
        let e = run(
            &p,
            nodes::update(State(p.a.clone()), Json(json!({"name": "nobody"}))),
        )
        .unwrap_err();
        assert_eq!(e.0.as_u16(), 404);
        let e = run(&p, nodes::update(State(p.a.clone()), Json(json!({})))).unwrap_err();
        assert_eq!(e.0.as_u16(), 400);
    }

    /// A node that is off when the update runs is "will update when it returns", and is sent the
    /// runtime on the first poll that finds it.
    #[test]
    fn a_node_that_is_off_during_the_update_is_pending_and_gets_the_runtime_on_its_next_poll() {
        let port = free_port();
        let p = pair_on(node_serve::NODE_PROTO, Some(OLD), port);
        let io = FakeIo::new(&p);
        // The node goes away.
        p.rt_b.lock().unwrap().take().unwrap().shutdown_background();
        poll(&p);
        assert_eq!(state(&p), Fleet::WillUpdateWhenBack);
        assert_eq!(update_status_line(&p), "will update when it returns");
        // The update runs while it is off: nothing is built or sent for it, and it stays pending.
        let h = node_fleet::begin(&p.a, &ucfg(&p), NEW, true);
        assert_eq!(deliver_now(&p, &h), 0);
        assert_eq!(state(&p), Fleet::WillUpdateWhenBack);
        assert_eq!(io.sends.load(Ordering::SeqCst), 0);
        assert!(io.built.lock().unwrap().is_empty());
        // It comes back: the poll that finds it sends the runtime, with no one asking.
        let rt_b = tokio::runtime::Runtime::new().unwrap();
        support::wait_for("the node to listen again", 60, || {
            rt_b.block_on(node_serve::start(p.b.clone())).is_ok()
        });
        *p.rt_b.lock().unwrap() = Some(rt_b);
        poll(&p);
        support::wait_for("the runtime to be sent", 60, || {
            node_fleet::record_of(&nid(&p)).is_some_and(|r| r.phase == Phase::Sent)
        });
        assert_eq!(io.sends.load(Ordering::SeqCst), 1);
        poll(&p);
        assert_eq!(state(&p), Fleet::Installed(0));
    }

    /// A source repository with two commits, and the `update.toml` that builds them into
    /// ad-hoc signed copies of this test's own `masond`. Returns the commits (oldest first).
    fn source_repo(root: &Path) -> Vec<String> {
        let repo = root.join("source");
        std::fs::create_dir_all(&repo).unwrap();
        sh(&repo, &["init", "-q", "-b", "main"]);
        let mut shas = vec![];
        for i in 1..=2 {
            std::fs::write(repo.join("version"), format!("c{i}")).unwrap();
            sh(&repo, &["add", "-A"]);
            sh(
                &repo,
                &[
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "-qm",
                    &format!("c{i}"),
                ],
            );
            shas.push(sh(&repo, &["rev-parse", "HEAD"]));
        }
        shas
    }

    fn write_update_toml(p: &Pair, repo: &Path) {
        let masond = bin("mason-daemon", "masond");
        bin("mason-step-host", "step-host");
        bin("mason-adapters", "mason-mcp");
        bin("mason-adapters", "fake-claude");
        let bindir = masond.parent().unwrap();
        // The build copies this test's own binaries and signs the copies ad hoc, as a copied
        // binary must be; the self-test runs the copied `masond`.
        let build = format!(
            "mkdir -p target/release\nfor b in masond step-host mason-mcp fake-claude; do cp '{}'/$b target/release/$b\n! command -v codesign >/dev/null || codesign -s - -f target/release/$b\ndone\n",
            bindir.display()
        );
        std::fs::write(
            p.sa.config_dir.join("update.toml"),
            format!(
                "repo = \"{}\"\nchannel = \"main\"\nbuild = {build:?}\nbins = [\"masond\", \"step-host\", \"mason-mcp\", \"fake-claude\"]\nsign_identity = \"-\"\n",
                repo.display()
            ),
        )
        .unwrap();
    }

    /// `mason update` itself: the coordinator builds and proves the new commit, the node of an
    /// older protocol is sent the same commit before the takeover, and a node that cannot be
    /// sent it neither fails nor holds the update.
    #[test]
    fn an_update_sends_every_node_the_new_commit_before_it_takes_over_and_a_failure_does_not_stop_it(
    ) {
        let src = tempfile::tempdir().unwrap();
        let shas = source_repo(src.path());
        let (old, new) = (shas[0].clone(), shas[1].clone());
        let a_runs = old.clone();
        let p = working_pair_full(
            2,
            "",
            "",
            move |c| c.runtime_sha = Some(a_runs),
            |c| c.runtime_sha = Some(OLD.into()),
            |n| n.proto = node_serve::NODE_PROTO + 1,
        );
        write_update_toml(&p, &src.path().join("source"));
        let io = FakeIo::new(&p);
        let ran = |p: &Pair, req: &mason_daemon::update::Request| {
            *p.a.update_status.lock().unwrap() = Default::default();
            mason_daemon::update::run(&p.a, req)
        };
        // A check builds and tests only: no node is sent anything.
        let checked = ran(
            &p,
            &mason_daemon::update::Request {
                check_only: true,
                ..Default::default()
            },
        );
        assert_eq!(checked.unwrap(), new);
        assert_eq!(io.sends.load(Ordering::SeqCst), 0);
        // A node that refuses the runtime does not fail the update, and is named in its log.
        io.fail.store(true, Ordering::SeqCst);
        let moved = ran(&p, &Default::default()).unwrap();
        assert_eq!(moved, new);
        assert_eq!(io.sends.load(Ordering::SeqCst), 1);
        let log = p.a.update_status.lock().unwrap().log.clone();
        assert!(
            log.iter()
                .any(|l| l.starts_with("node b: failed (the node said no)")),
            "{log:#?}"
        );
        // And one that takes it: sent after the self-test, before "taking over".
        io.fail.store(false, Ordering::SeqCst);
        node_fleet::forget(&nid(&p));
        ran(&p, &Default::default()).unwrap();
        assert_eq!(io.sends.load(Ordering::SeqCst), 2);
        let s = p.a.update_status.lock().unwrap();
        let at = |what: &str| {
            s.log
                .iter()
                .position(|l| l.contains(what))
                .unwrap_or_else(|| panic!("no {what:?} in {:#?}", s.log))
        };
        let (test, node, take) = (at("self-test"), at("node b: sent"), at("taking over"));
        assert!(test < node && node < take, "{:#?}", s.log);
        assert_eq!(s.phase, "taking over");
    }
}

/// A check as `run_check` makes one for a task with no suite: placed by the global switch.
fn check_spec(p: &Pair, id: &str, cmd: &str, logs: &Path) -> mason_daemon::hostedcheck::CheckSpec {
    mason_daemon::hostedcheck::CheckSpec {
        id_prefix: id.into(),
        log_dir: logs.to_path_buf(),
        cmd: cmd.into(),
        build: None,
        cwd: p.sa.repo.clone(),
        env: vec![],
        limit: std::time::Duration::from_secs(120),
        tree: format!("tree-{id}"),
        stop: None,
        holds: std::collections::BTreeMap::from([("test-suite".to_string(), 1)]),
        label: "#7 check".into(),
        place: None,
        global: true,
    }
}

/// A check placed on a node asked the broker for this Mac's `test-suite` slot before it went out
/// (`run_place` read a node as a run here), so while the machine was held for heat it never
/// started, though it would have used nothing of this Mac. It now asks for its unit of the node
/// only, which no slot limit and no heat hold applies to; the same check placed here still waits.
#[test]
fn a_check_bound_for_a_node_takes_no_slot_here_and_no_heat_hold_applies_to_it() {
    use mason_daemon::jobs::{want_of, Want};
    use mason_daemon::sched::LeaseState;
    let p = working_pair(2);
    let logs = tempfile::tempdir().unwrap();
    let uses = std::collections::BTreeMap::from([("test-suite".to_string(), 1)]);
    let b = &p.a.broker;
    // Room for two, one taken by other work: the heat hold, which only holds more work while
    // some machine work runs, is then the only thing that can stop a second.
    b.declare("test-suite", 2);
    b.set_limit("test-suite", 2).unwrap();
    let hot = mason_daemon::thermal::FakeThermal::new(100);
    hot.set_die(70.0);
    b.set_thermal(hot.clone(), 35);
    let busy = b.lease_open_as("busy", &uses, None);
    assert!(matches!(b.lease_poll(busy), LeaseState::Granted { .. }));
    hot.set_die(98.0);

    mason_daemon::place::set_global(&p.a, mason_daemon::place::Mode::Remote, "test").unwrap();
    let spec = check_spec(&p, "t7r1-check", "true", logs.path());
    let want = want_of(&p.a, &spec);
    assert!(matches!(want, Want::Node(_)), "a node-bound check");
    let asked = want.asks(&uses);
    assert!(
        !asked.is_empty() && asked.keys().all(|k| k.starts_with("node:")),
        "only the node's unit: {asked:?}"
    );
    let on_node = b.lease_open_as("check on the node", &asked, None);
    assert!(
        matches!(b.lease_poll(on_node), LeaseState::Granted { .. }),
        "the heat hold does not stop it"
    );
    assert_eq!(b.used("test-suite"), 1, "it holds none of this Mac's");

    // Placed here, the same check asks for the slot and waits under the hold.
    mason_daemon::place::set_global(&p.a, mason_daemon::place::Mode::Local, "test").unwrap();
    let want = want_of(&p.a, &spec);
    assert!(matches!(want, Want::Slots));
    assert_eq!(want.asks(&uses), uses);
    let here = b.lease_open_as("check here", &want.asks(&uses), None);
    assert!(
        matches!(b.lease_poll(here), LeaseState::Waiting { .. }),
        "held for heat"
    );
    hot.set_die(70.0);
    assert!(matches!(b.lease_poll(here), LeaseState::Granted { .. }));
    b.lease_close(here);
    b.lease_close(on_node);
    b.lease_close(busy);
}

/// A node that has no room gives a check back; it then runs here, and asks for this Mac's slot at
/// that point, so the heat hold applies to the local run (#1149's fallback lease, with no slot
/// taken up front).
#[test]
fn a_check_the_node_gives_back_runs_here_only_after_getting_a_slot_and_waits_under_heat() {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    let p = working_pair(1);
    std::fs::write(p.sa.repo.join("slow.sh"), "sleep 300\n").unwrap();
    mason_daemon::place::set_global(&p.a, mason_daemon::place::Mode::Remote, "test").unwrap();
    let logs = tempfile::tempdir().unwrap();
    let b = &p.a.broker;
    b.declare("test-suite", 2);
    b.set_limit("test-suite", 2).unwrap();
    let hot = mason_daemon::thermal::FakeThermal::new(100);
    hot.set_die(70.0);
    b.set_thermal(hot.clone(), 35);
    // Other work runs here, then the die heats: the hold applies to more work, not to that.
    let busy = b.lease_open_as(
        "busy",
        &std::collections::BTreeMap::from([("test-suite".to_string(), 1)]),
        None,
    );
    assert!(matches!(
        b.lease_poll(busy),
        mason_daemon::sched::LeaseState::Granted { .. }
    ));
    hot.set_die(98.0);
    let release = AtomicBool::new(false);
    let done = AtomicBool::new(false);
    std::thread::scope(|sc| {
        // The node's only job slot is taken.
        let slow = sc.spawn(|| {
            run_on_b(&p, &spec("run-one", None, "sh slow.sh"), &|| {
                release.load(SeqCst).then_some(StopWhy::Skipped)
            })
        });
        support::wait_for("the first job to run on the node", 60, || {
            p.b.node.jobs.running() == 1
        });
        let check = sc.spawn(|| {
            let r = mason_daemon::hostedcheck::run(
                &p.a,
                &check_spec(&p, "t7r1-back", "echo ran-here", logs.path()),
                &mason_daemon::remote::Grant::attached(),
            );
            done.store(true, SeqCst);
            r
        });
        support::wait_for("the check to ask for its slot here", 60, || {
            b.snapshot().to_string().contains("run here")
        });
        assert!(!done.load(SeqCst), "held for heat: it has not run here");
        assert_eq!(
            b.used("test-suite"),
            1,
            "only the other work's: {}",
            b.snapshot()
        );
        // Cooler: it gets the slot and runs.
        hot.set_die(70.0);
        let out = check.join().unwrap().unwrap();
        assert!(out.passed && out.full.contains("ran-here"), "{}", out.full);
        release.store(true, SeqCst);
        assert!(matches!(slow.join().unwrap(), Ok(Outcome::Stopped(_))));
    });
    b.lease_close(busy);
    assert_eq!(b.used("test-suite"), 0);
}

/// The test step's own request for a suite placed on a node: this Mac's slots are replaced by the
/// node's unit (`run_place` read a node as a run here, so the step queued for `test-suite` and
/// the heat hold first). A suite placed here keeps its slots.
#[test]
fn a_test_step_for_a_suite_on_a_node_asks_for_the_nodes_unit_and_none_of_this_macs() {
    use mason_daemon::remote::{run_place, suite_pool_needs, Kind, RunPlace};
    let p = working_pair(2);
    let suite = mason_testing::Suite::new("alpha", "true", mason_testing::ParserKind::Tap);
    let needs = mason_scheduler::Needs::of(&[("compile", 1), ("test-suite", 1)]);
    mason_daemon::place::set_global(&p.a, mason_daemon::place::Mode::Remote, "test").unwrap();
    let place = run_place(
        &p.a,
        "scratch",
        None,
        Some(&suite),
        &suite.command,
        Kind::Suite,
    );
    assert!(matches!(place, RunPlace::Node { .. }), "{place:?}");
    let n = suite_pool_needs(&p.a, "scratch", None, &needs, &suite, true)
        .expect("a suite off this machine asks for something else");
    assert!(
        !n.amounts.is_empty() && n.amounts.keys().all(|k| k.starts_with("node:")),
        "{n:?}"
    );
    mason_daemon::place::set_global(&p.a, mason_daemon::place::Mode::Local, "test").unwrap();
    assert!(suite_pool_needs(&p.a, "scratch", None, &needs, &suite, true).is_none());
}

/// A commit on a side branch, made without moving the worktree, and the worktree's own base
/// tracking ref set to it: a base tip that is not an ancestor of what a job runs, and that a
/// node has never seen.
fn move_base_to_a_side_commit(p: &Pair, file: &str, parent: &str) -> String {
    let b = sh(
        &p.sa.repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit-tree",
            "-p",
            parent,
            "-m",
            file,
            &format!("{parent}^{{tree}}"),
        ],
    )
    .trim()
    .to_string();
    sh(&p.sa.repo, &["update-ref", "refs/remotes/origin/main", &b]);
    b
}

#[test]
fn a_node_job_sees_origin_base_even_when_the_base_is_not_an_ancestor_of_the_snapshot() {
    let p = working_pair(2);
    let m0 = sh(&p.sa.repo, &["rev-parse", "HEAD"]).trim().to_string();
    let b1 = move_base_to_a_side_commit(&p, "base one", &m0);
    assert_ne!(b1, m0);
    std::fs::write(
        p.sa.repo.join("t.sh"),
        "git merge-base HEAD origin/main && git rev-parse origin/main\n",
    )
    .unwrap();
    let f = done(run_on_b(&p, &spec("run-b1", None, "sh t.sh"), &|| None));
    assert_eq!((f.code, f.signal), (0, 0), "{}", f.output);
    assert_eq!(f.output, format!("{m0}\n{b1}\n"), "{}", f.output);
    // Another base tip, a descendant of the first and again unknown to the node: the job sees
    // its own, not the one a job before it left.
    let b2 = move_base_to_a_side_commit(&p, "base two", &b1);
    let f = done(run_on_b(&p, &spec("run-b2", None, "sh t.sh"), &|| None));
    assert_eq!((f.code, f.signal), (0, 0), "{}", f.output);
    assert_eq!(f.output, format!("{m0}\n{b2}\n"), "{}", f.output);
}

#[test]
fn a_job_body_without_a_base_runs_as_before_and_leaves_origin_alone() {
    let p = working_pair(2);
    let m0 = sh(&p.sa.repo, &["rev-parse", "HEAD"]).trim().to_string();
    let b1 = move_base_to_a_side_commit(&p, "base one", &m0);
    std::fs::write(p.sa.repo.join("t.sh"), "echo first\n").unwrap();
    let f = done(run_on_b(&p, &spec("run-o1", None, "sh t.sh"), &|| None));
    assert_eq!(f.output, "first\n");
    // The node's mirror now holds the snapshot. A job of an older coordinator names no base.
    let mirror =
        p.sb.data_dir
            .join("node/mirrors")
            .read_dir()
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
            .join("scratch.git");
    let sha = sh(
        &mirror,
        &[
            "for-each-ref",
            "--format=%(objectname)",
            "refs/mason/node/run-o1",
        ],
    )
    .trim()
    .to_string();
    assert_eq!(sha.len(), 40, "the mirror holds the snapshot");
    let (_, fp) = (
        0,
        lan::sha256_hex(&lan::load_or_create_identity(&p.sb.data_dir).unwrap().0),
    );
    let client = NodeClient::new(
        lan::load_or_create_identity(&p.sa.data_dir).unwrap(),
        "127.0.0.1",
        p.port,
        &fp,
    )
    .unwrap();
    let r = run(
        &p,
        client.request(
            "POST",
            "/node/jobs",
            Some(&json!({
                "id": "run-o2", "project": "scratch", "sha": sha,
                "run": {"command": "git rev-parse --verify -q origin/main || echo none"},
                "limit_s": 60, "lease_s": 60,
            })),
        ),
    )
    .unwrap();
    assert!(r.status == 200 || r.status == 201, "{}", r.error_text());
    support::wait_for("the job to end", 60, || {
        run(&p, client.request("GET", "/node/jobs/run-o2", None))
            .ok()
            .is_some_and(|r| r.json()["job"]["state"] == "done")
    });
    let r = run(
        &p,
        client.request("GET", "/node/jobs/run-o2/log?from=0", None),
    )
    .unwrap();
    // The ref the first job set is the shared mirror's; this one neither set nor moved it.
    assert_eq!(
        String::from_utf8_lossy(&r.body).trim(),
        b1,
        "an older coordinator's job sees whatever the mirror has, and changes nothing"
    );
    assert_eq!(
        sh(&mirror, &["rev-parse", "refs/remotes/origin/main"]).trim(),
        b1
    );
}

#[test]
fn origin_base_is_set_exactly_for_a_lone_job_and_only_forward_beside_others() {
    use mason_daemon::node_run::set_origin_base;
    let t = tempfile::tempdir().unwrap();
    let m = t.path().join("m.git");
    sh(t.path(), &["init", "-q", "--bare", m.to_str().unwrap()]);
    let commit = |parents: &[&str], msg: &str| {
        let empty = sh(&m, &["hash-object", "-t", "tree", "-w", "--stdin"]);
        let mut a = vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit-tree"];
        for p in parents {
            a.push("-p");
            a.push(p);
        }
        a.extend(["-m", msg, empty.trim()]);
        sh(&m, &a).trim().to_string()
    };
    let (a, b) = {
        let a = commit(&[], "a");
        (a.clone(), commit(&[&a], "b"))
    };
    let side = commit(&[&a], "side");
    let at = || {
        sh(&m, &["rev-parse", "refs/remotes/origin/feature/x"])
            .trim()
            .to_string()
    };
    // Nothing there yet: set, whether alone or not.
    set_origin_base(&m, "feature/x", &a, false).unwrap();
    assert_eq!(at(), a);
    // Beside others: forward only. A descendant moves it; an older or an unrelated commit does not.
    set_origin_base(&m, "feature/x", &b, false).unwrap();
    assert_eq!(at(), b);
    set_origin_base(&m, "feature/x", &a, false).unwrap();
    assert_eq!(at(), b);
    set_origin_base(&m, "feature/x", &side, false).unwrap();
    assert_eq!(at(), b);
    // Alone: exactly the job's own, a force-pushed base included.
    set_origin_base(&m, "feature/x", &side, true).unwrap();
    assert_eq!(at(), side);
    // A commit the mirror lacks, and a name that is no branch, change nothing and say why.
    assert!(set_origin_base(&m, "feature/x", &"1".repeat(40), true).is_err());
    assert!(set_origin_base(&m, "a..b", &a, true).is_err());
    assert_eq!(at(), side);
}

#[test]
fn a_node_jobs_path_has_a_mason_that_runs_and_finds_no_daemon() {
    let mason = support::bin("mason-pipeline", "mason");
    std::env::set_var("MASON_NODE_CLI", &mason);
    let p = working_pair(2);
    std::fs::write(
        p.sa.repo.join("t.sh"),
        "command -v mason\nmason --help | head -1\nmason task list 2>&1 | head -2\nfile=$(mktemp -d)/m.toml; mason definition check \"$file\" 2>&1 | head -1\nexit 0\n",
    )
    .unwrap();
    let f = done(run_on_b(&p, &spec("run-cli", None, "sh t.sh"), &|| None));
    std::env::remove_var("MASON_NODE_CLI");
    assert_eq!((f.code, f.signal), (0, 0), "{}", f.output);
    let lines: Vec<&str> = f.output.lines().collect();
    assert!(lines[0].ends_with("/shims/mason"), "{}", f.output);
    assert!(lines[1].to_lowercase().contains("mason"), "{}", f.output);
    // The node's own daemon is out of reach of a job's `mason`.
    assert!(f.output.contains("not running"), "{}", f.output);
}

/// Serve `d`'s loopback API, as `masond` does, so that the `mason` command line finds it.
fn serve_api(d: &Arc<Daemon>) {
    let d2 = d.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let _ = rt.block_on(mason_daemon::http::serve(d2));
    });
    let info = d.cfg.info_path();
    support::wait_for("the API to be served", 60, || {
        std::fs::read_to_string(&info)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .is_some_and(|v| v["port"].is_u64())
    });
}

/// #1197: a node whose mirror cannot take `origin/<base>` says so, and the run is a node failure
/// that names the cause; `mason node sync` puts the ref back and `mason node show` lists it.
#[test]
fn a_node_without_origin_base_is_a_node_failure_and_node_sync_restores_it() {
    use mason_daemon::hostedcheck;
    use mason_daemon::place::{self, Mode};
    let p = working_pair(2);
    let tip = sh(&p.sa.repo, &["rev-parse", "origin/main"])
        .trim()
        .to_string();
    std::fs::write(p.sa.repo.join("t.sh"), "git rev-parse origin/main\n").unwrap();
    // A first run makes the mirror, and says the base it set.
    let f = done(run_on_b(&p, &spec("run-first", None, "sh t.sh"), &|| None));
    assert!(
        f.ran_on
            .ends_with(&format!("base: set main@{}", &tip[..10])),
        "{}",
        f.ran_on
    );
    let mirror =
        p.sb.data_dir
            .join("node/mirrors")
            .join(coord_of_a(&p))
            .join("scratch.git");
    // The ref is gone, and a lock left behind keeps any job from setting it again.
    sh(&mirror, &["update-ref", "-d", "refs/remotes/origin/main"]);
    let lock = mirror.join("refs/remotes/origin/main.lock");
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    std::fs::write(&lock, "").unwrap();

    match run_on_b(&p, &spec("run-locked", None, "sh t.sh"), &|| None) {
        Err(Fail::Environmental(why)) => {
            assert!(why.starts_with("origin/main not set on node b"), "{why}");
            assert!(why.contains("base: failed: "), "{why}");
            assert!(why.contains("mason node sync b --project scratch"), "{why}");
        }
        other => panic!("expected a node failure, got {other:?}"),
    }
    // A check that can run here does, and its result says why it left the node.
    place::set_global(&p.a, Mode::Remote, "test").unwrap();
    let logs = tempfile::tempdir().unwrap();
    let out = hostedcheck::run(
        &p.a,
        &check_spec(&p, "t7r1-back", "echo ran-here", logs.path()),
        &remote::Grant::attached(),
    )
    .unwrap();
    assert!(out.passed && out.full.contains("ran-here"), "{}", out.full);
    assert!(
        out.output.contains("origin/main not set on node b"),
        "{}",
        out.output
    );
    // A nodes-only project has nowhere else: no result, as an outage, not a red.
    place::set_project_mode(&p.a, "scratch", Mode::Remote).unwrap();
    let why = hostedcheck::run(
        &p.a,
        &check_spec(&p, "t7r2-only", "echo never", logs.path()),
        &remote::Grant::attached(),
    )
    .map(|o| o.output)
    .unwrap_err();
    assert!(hostedcheck::is_outage(&why), "{why}");
    assert!(why.contains("origin/main not set on node b"), "{why}");

    serve_api(&p.a);
    let sync = ["node", "sync", "b", "--project", "scratch"];
    // While the cause is there, the sync says what it is.
    let o = p.sa.mason(&sync);
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("main.lock"),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let shown = p.sa.mason_ok(&["node", "show", "b"]);
    assert!(shown.contains("origin/main not set"), "{shown}");
    std::fs::remove_file(&lock).unwrap();
    let said = p.sa.mason_ok(&sync);
    assert!(said.contains(&format!("origin/main = {tip}")), "{said}");
    assert_eq!(
        sh(&mirror, &["rev-parse", "refs/remotes/origin/main"]).trim(),
        tip
    );
    let shown = p.sa.mason_ok(&["node", "show", "b"]);
    assert!(shown.contains("scratch.git"), "{shown}");
    assert!(shown.contains(&format!("origin/main {tip}")), "{shown}");
    let f = done(run_on_b(&p, &spec("run-after", None, "sh t.sh"), &|| None));
    assert_eq!(f.output, format!("{tip}\n"));
    // A project nobody registered is refused.
    assert!(!p
        .sa
        .mason(&["node", "sync", "b", "--project", "nope"])
        .status
        .success());
}

const SPILL_REMOTE_RUNS: &str =
    "[remote_runs]\nkind = \"github-actions\"\nrepo = \"o/r\"\ntoken = \"env:MASON_NODE_SPILL_TOKEN\"\n";

/// A pair whose project also has `[remote_runs]`, with the node switched off so that placement
/// at queue time finds no room and picks GitHub, the way it did for checks queued while the node
/// was full.
fn github_project_with_a_node_off() -> Pair {
    let p = working_pair(1);
    p.sa.write_project("scratch", "true", SPILL_REMOTE_RUNS);
    p.a.reload_registration("scratch").unwrap();
    mason_daemon::place::set_global(&p.a, mason_daemon::place::Mode::Remote, "test").unwrap();
    set_switch(&p, false);
    poll(&p);
    p
}

fn gha_units(n: u32) -> std::collections::BTreeMap<String, u32> {
    std::collections::BTreeMap::from([("gha".to_string(), n)])
}

/// A check queued for GitHub while no node had room waited for a runner unit and never looked at
/// a node again, though the node freed up at once: it waited for GitHub's pool (full of other
/// repositories' runs) until GitHub gave it back. It now takes the node's unit as soon as a node
/// has room, holds that and no runner unit, and runs on the node.
#[test]
fn a_check_waiting_for_a_full_runner_pool_takes_a_node_that_has_room_and_runs_there() {
    use mason_daemon::hostedcheck;
    use mason_daemon::jobs::{pool_or_node, want_of, Want};
    use mason_daemon::remote::Grant;
    let p = github_project_with_a_node_off();
    let logs = tempfile::tempdir().unwrap();
    let b = &p.a.broker;
    b.declare("gha", 1);
    b.set_limit("gha", 1).unwrap();
    let other_repo = b.lease_open_as("another repository's run", &gha_units(1), None);
    let spec = check_spec(&p, "t9r1-check", "echo node=$MASON_NODE", logs.path());
    let want = want_of(&p.a, &spec);
    let Want::PoolOrNode(units, hop) = want else {
        panic!("a check for a full pool may take a node");
    };
    assert_eq!(units, gha_units(1));
    assert!(hop().is_none(), "no node has room yet");

    let (tx, rx) = std::sync::mpsc::channel();
    let waiter = {
        let (b, hop) = (b.clone(), hop.clone());
        std::thread::spawn(move || {
            let rank = || mason_scheduler::Rank::new(false, 1, 0, 9, 9, false);
            let got = pool_or_node(
                &*hop,
                std::time::Duration::from_millis(20),
                |leave| {
                    b.acquire(
                        mason_core::TaskId(9),
                        "#9 check",
                        rank(),
                        mason_scheduler::Needs::of(&[("gha", 1)]).flexible(),
                        &|| !leave(),
                        &mut |_| {},
                    )
                },
                |units| {
                    b.try_acquire_expecting(
                        mason_core::TaskId(9),
                        "#9",
                        rank(),
                        mason_daemon::sched::needs_of(&units, None),
                        None,
                    )
                },
            );
            tx.send(got.map(Grant::of)).unwrap();
        })
    };
    // The node takes work again: the waiting check takes its unit.
    set_switch(&p, true);
    poll(&p);
    let grant = rx
        .recv_timeout(std::time::Duration::from_secs(120))
        .expect("the check took the node's unit")
        .expect("granted");
    waiter.join().unwrap();
    assert_eq!(
        grant.node().as_deref(),
        listed(&p, "b")["nid"].as_str(),
        "it holds the node's unit"
    );
    assert!(grant.units().is_empty(), "and no runner unit");
    assert_eq!(b.used("gha"), 1, "the runner pool is still the other run's");

    // With that unit held the node reads as full, so placement alone would send the check to
    // GitHub; the held unit keeps it on the node.
    let out = hostedcheck::run(&p.a, &spec, &grant).unwrap();
    assert!(out.passed, "{}", out.output);
    assert!(out.output.contains("ran on node b"), "{}", out.output);
    drop(grant);
    b.lease_close(other_repo);
}

/// The rules every node run answers to hold for a check that left the runner queue: a suite that
/// says `remote = false`, uses `exclusive` or leases a device never takes a node's unit, and
/// stays with its own placement.
#[test]
fn a_suite_that_must_not_leave_this_mac_never_takes_a_nodes_unit_while_it_waits() {
    use mason_daemon::jobs::{want_of, Want};
    let p = github_project_with_a_node_off();
    let logs = tempfile::tempdir().unwrap();
    let suite_of =
        |name: &str| mason_testing::Suite::new(name, "true", mason_testing::ParserKind::Tap);
    let mut stays = suite_of("stays");
    stays.remote = false;
    let mut exclusive = suite_of("alone");
    exclusive.uses.insert("exclusive".into(), 1);
    let free = suite_of("free");
    let spec_for = |suite: &mason_testing::Suite| {
        let mut s = check_spec(&p, &format!("t8-{}", suite.name), "true", logs.path());
        s.global = false;
        s.place = Some((None, Box::new(suite.clone())));
        s
    };

    // The suite that may leave: placed on GitHub with the node off, it may hop to the node.
    assert!(
        matches!(want_of(&p.a, &spec_for(&free)), Want::PoolOrNode(..)),
        "a suite with no rules of its own may take a node"
    );
    // The node now has room.
    set_switch(&p, true);
    poll(&p);
    assert!(
        mason_daemon::place::node_room_for(&p.a, "scratch", None, &free).is_some(),
        "the node has room for it"
    );
    assert!(
        mason_daemon::place::node_room_for(&p.a, "scratch", None, &stays).is_none(),
        "remote = false never goes to a node"
    );
    assert!(
        mason_daemon::place::node_room_for(&p.a, "scratch", None, &exclusive).is_none(),
        "exclusive never goes to a node"
    );
    // And such a suite waits for its own placement: this Mac's slots, no runner or node unit.
    assert!(matches!(want_of(&p.a, &spec_for(&stays)), Want::Slots));
    assert!(matches!(want_of(&p.a, &spec_for(&exclusive)), Want::Slots));
}

/// A project with `[remote_runs]` in `remote` mode (fallback none) whose node gave a check back
/// ("no room": its one job slot was taken) ran the check on this Mac, under this Mac's slots and
/// heat hold, labelled "no GitHub runner, run here". It now waits for a node and runs there, and
/// never takes this Mac's slot.
#[test]
fn a_check_the_node_gives_back_waits_for_a_node_when_the_project_may_not_run_it_here() {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    let p = working_pair(1);
    p.sa.write_project("scratch", "true", SPILL_REMOTE_RUNS);
    p.a.reload_registration("scratch").unwrap();
    std::fs::write(p.sa.repo.join("slow.sh"), "sleep 300\n").unwrap();
    mason_daemon::place::set_global(&p.a, mason_daemon::place::Mode::Remote, "test").unwrap();
    let logs = tempfile::tempdir().unwrap();
    let b = &p.a.broker;
    b.declare("test-suite", 2);
    b.set_limit("test-suite", 2).unwrap();
    let release = AtomicBool::new(false);
    std::thread::scope(|sc| {
        // The node's only job slot is taken.
        let slow = sc.spawn(|| {
            run_on_b(&p, &spec("run-one", None, "sh slow.sh"), &|| {
                release.load(SeqCst).then_some(StopWhy::Skipped)
            })
        });
        support::wait_for("the first job to run on the node", 60, || {
            p.b.node.jobs.running() == 1
        });
        let check = sc.spawn(|| {
            mason_daemon::hostedcheck::run(
                &p.a,
                &check_spec(
                    &p,
                    "t7r1-wait",
                    "if [ -n \"$MASON_NODE\" ]; then echo on-the-node; else echo ran-here; fi",
                    logs.path(),
                ),
                &mason_daemon::remote::Grant::attached(),
            )
        });
        // It never asks for this Mac's slot while it waits for the node.
        for _ in 0..20 {
            assert!(
                !b.snapshot().to_string().contains("run here"),
                "{}",
                b.snapshot()
            );
            std::thread::yield_now();
        }
        release.store(true, SeqCst);
        assert!(matches!(slow.join().unwrap(), Ok(Outcome::Stopped(_))));
        let out = check.join().unwrap().unwrap();
        assert!(
            out.passed && out.full.contains("on-the-node"),
            "{}",
            out.full
        );
        assert!(out.output.contains("ran on node b"), "{}", out.output);
    });
    assert_eq!(b.used("test-suite"), 0);
}

/// A job whose command starts a long-running grandchild: the script writes the grandchild's pid
/// to a file of the test's and waits for it.
fn grandchild_script(p: &Pair, name: &str, pidfile: &Path) -> String {
    std::fs::write(
        p.sa.repo.join(name),
        format!(
            "sleep 3000 &\necho $! > '{}.tmp'\nmv '{}.tmp' '{}'\nwait\n",
            pidfile.display(),
            pidfile.display(),
            pidfile.display()
        ),
    )
    .unwrap();
    format!("sh {name}")
}

fn grandchild_pid(pidfile: &Path) -> Option<u32> {
    std::fs::read_to_string(pidfile).ok()?.trim().parse().ok()
}

/// Cleanup for a test that fails before the grandchild goes: it is a `sleep 3000`.
struct KillOnDrop(std::path::PathBuf);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(pid) = grandchild_pid(&self.0) {
            let _ =
                mason_platform::process::signal_process(pid, mason_platform::process::Signal::Kill);
        }
    }
}

/// A job that ends early must not leave its processes running. These were found on the node
/// hours after the job was cancelled or timed out: test binaries holding scratch-world slots.
/// Nothing here is a timing assertion: each test waits, up to the harness's limit, for the
/// grandchild to be gone.
#[test]
fn a_cancelled_node_job_leaves_no_grandchild_behind() {
    let p = working_pair(2);
    let pidfile = p.sa.root.path().join("grandchild-cancel.pid");
    let _guard = KillOnDrop(pidfile.clone());
    let cmd = grandchild_script(&p, "gc-cancel.sh", &pidfile);
    let stop = || {
        // Cancel once the grandchild is running.
        grandchild_pid(&pidfile).map(|_| StopWhy::Skipped)
    };
    let r = run_on_b(&p, &spec("run-gc-cancel", None, &cmd), &stop);
    assert!(matches!(r, Ok(Outcome::Stopped(StopWhy::Skipped))), "{r:?}");
    let pid = grandchild_pid(&pidfile).expect("the job started its grandchild");
    support::wait_for("the cancelled job's grandchild to be gone", 60, || {
        !mason_step_host::registry::pid_alive(pid)
    });
}

/// The host of a running job dies (killed, or the machine stalled it past its heartbeat): the
/// job is lost, and its command, which the host no longer watches, is killed with its group.
#[test]
fn a_node_job_whose_host_dies_leaves_no_grandchild_behind() {
    let p = working_pair(2);
    let pidfile = p.sa.root.path().join("grandchild-lost.pid");
    let _guard = KillOnDrop(pidfile.clone());
    let cmd = grandchild_script(&p, "gc-lost.sh", &pidfile);
    let hosts = p.b.cfg.data_dir.join("node").join("hosts");
    let r = std::thread::scope(|sc| {
        let h = sc.spawn(|| run_on_b(&p, &spec("run-gc-lost", None, &cmd), &|| None));
        support::wait_for("the job's grandchild to run", 60, || {
            grandchild_pid(&pidfile).is_some()
        });
        let found = mason_step_host::registry::Registry::new(&hosts)
            .scan()
            .unwrap();
        assert_eq!(found.len(), 1, "one host runs the job");
        // The host goes; the command and its grandchild are left running.
        mason_platform::process::signal_process(
            found[0].info.host_pid,
            mason_platform::process::Signal::Kill,
        )
        .unwrap();
        h.join().unwrap()
    });
    assert!(r.is_err(), "a lost host is a failed run: {r:?}");
    let pid = grandchild_pid(&pidfile).expect("the job started its grandchild");
    support::wait_for("the lost job's grandchild to be gone", 60, || {
        !mason_step_host::registry::pid_alive(pid)
    });
}
