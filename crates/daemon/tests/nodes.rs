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
/// (`support::world_limit`), and tests that each held one while waiting for a second would wait
/// for each other for ever.
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
    std::env::set_var("MASON_NO_MDNS", "1");
    let _one_at_a_time = worlds();
    let sb = Scratch::new("true", json!({"steps": {}}));
    let b = sb.start_with(|c| {
        c.node.serve = true;
        c.node.port = 0;
        c.node.bind = vec![std::net::IpAddr::from([127, 0, 0, 1])];
        on_node(&mut c.node);
    });
    let sa = Scratch::new("true", json!({"steps": {}}));
    let a = sa.start();
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
    std::env::set_var(
        "MASON_NODE_TOOLCHAINS",
        r#"{"rust":["1.96.0"],"xcode":[],"java":[]}"#,
    );
    std::env::set_var("MASON_NODE_POLL_MS", "50");
    let p = pair_with(on_node);
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
    let green = run_check(&p.a, &spec("t0r1-green", "echo checked")).unwrap();
    assert!(green.passed, "{}", green.output);
    assert!(green.output.contains("ran on node b"), "{}", green.output);
    assert!(green.full.contains("built\nchecked"), "{}", green.full);
    let red = run_check(&p.a, &spec("t0r1-red", "echo nope; exit 2")).unwrap();
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

/// A batch of two tasks lands on the coordinator, which does every git step, while the suites of
/// its test run on the node. The node never gets a way to push.
#[test]
fn a_batch_lands_on_the_coordinator_while_its_suites_run_on_the_node() {
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
    mason_daemon::place::set_global(&w.d, mason_daemon::place::Mode::Remote, "test").unwrap();

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
            let h = sc.spawn(|| run_check(&p.a, &spec));
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
