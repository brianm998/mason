// SPDX-License-Identifier: GPL-3.0-or-later

//! Crash injection through a batch with a culprit: the real `masond` is killed
//! at each landing phase (including the red one), restarted, and the result
//! must be the same as if nothing had happened: the clean task landed exactly
//! once, the culprit was never landed, and the batch left no mess.

mod support;
use mason_core::*;
use mason_store::Store;
use serde_json::{json, Value};
use std::os::unix::process::ExitStatusExt;
use std::time::{Duration, Instant};
use support::lw::{ci_section, host_section};
use support::*;

const CHECKS: &str = r#"#!/bin/sh
echo "1..1"
if [ -f red.integration ]; then echo "not ok 1 - integration"; exit 1; fi
echo "ok 1 - integration"
"#;

const DEF: &str = r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "land"

[step.land]
type = "landing"
category = "landing"
on_red = "develop"
max_rounds = 2

[suite.integration]
command = "sh checks.sh"
parser = "tap"
min_tests = 1
level = "unit"
"#;

fn landing(mode: &str) -> String {
    format!("mode = \"{mode}\"\nwindow = \"4s\"\nmax_size = 2\n[[tier]]\nname = \"batch\"\nsuites = [\"integration\"]\n")
}

/// Task 0 adds a clean file; task 1 (its directory is named `1`) adds the red marker too.
fn script() -> Value {
    json!({"steps": {"develop": [[
        {"sh": "echo ok > file-$(basename \"$PWD\").txt; [ \"$(basename \"$PWD\")\" = 1 ] && echo x > red.integration; git add -A && git -c user.name=t -c user.email=t@t commit -qm \"work $(basename \"$PWD\")\""},
        {"call": {"tool": "submit_change", "args": {"summary": "done", "head": "$HEAD"}}}
    ]]}})
}

fn run(point: &str, mode: &str) -> Result<(), String> {
    let s = Scratch::new("true", script());
    let extra = match mode {
        "direct-ci" | "local-only-ci" => ci_section(&s).0,
        "direct" => String::new(),
        _ => format!(
            "{}[ci]\nkind = \"command\"\ntrigger = \"true\"\nstatus = \"echo green\"\npoll_s = 1\n",
            host_section(&s, false, mode == "host-queue")
        ),
    };
    s.write_project("scratch", "true", &extra);
    s.push_files(
        &[
            (".mason/mason.toml", DEF),
            (".mason/landing.toml", &landing(mode)),
            ("checks.sh", CHECKS),
        ],
        "setup",
    );
    sh(&s.repo, &["fetch", "-q", "origin"]);
    let mut first = s.masond(&[], &[("MASON_CRASH_AT", point)]);
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(60)
        && first.child.try_wait().ok().flatten().is_none()
        && !s.mason(&["status", "--json"]).status.success()
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    for t in ["clean", "culprit"] {
        let _ = s.mason(&["task", "add", t, "--body", "Do it.", "--explored"]);
    }
    let end = Instant::now() + Duration::from_secs(90);
    loop {
        if let Some(st) = first.child.try_wait().map_err(|e| e.to_string())? {
            if st.signal() == Some(9) {
                break;
            }
            return Err(format!("exited with {st} instead of being killed"));
        }
        if Instant::now() > end {
            return Err("the injected crash never happened".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _first = first;
    let mut second = s.masond(&[], &[]);
    let started = Instant::now();
    while !s.mason(&["status", "--json"]).status.success() {
        if started.elapsed() > Duration::from_secs(60) {
            return Err("the restarted daemon never answered".into());
        }
        if let Some(st) = second.child.try_wait().map_err(|e| e.to_string())? {
            return Err(format!("the restarted daemon exited: {st}"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // A crash before a task was recorded: the client files it again.
    let listed: Value = serde_json::from_str(&s.mason_ok(&["task", "list", "--json"]))
        .map_err(|e| e.to_string())?;
    let have = listed["tasks"].as_array().map_or(0, Vec::len);
    for t in ["clean", "culprit"].iter().skip(have) {
        s.mason_ok(&["task", "add", t, "--body", "Do it.", "--explored"]);
    }
    let end = Instant::now() + Duration::from_secs(150);
    loop {
        let v: Value = serde_json::from_slice(&s.mason(&["task", "show", "0", "--json"]).stdout)
            .unwrap_or(Value::Null);
        if v["state"].as_str() == Some("landed") {
            break;
        }
        if Instant::now() > end {
            return Err(format!("the clean task never landed: {v:#}"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // The culprit goes back for rework and, with a script that has one turn, ends failed
    // or waiting; what matters is that it never landed.
    let end = Instant::now() + Duration::from_secs(60);
    loop {
        let v: Value = serde_json::from_slice(&s.mason(&["task", "show", "1", "--json"]).stdout)
            .unwrap_or(Value::Null);
        if v["state"].as_str() != Some("landing") && v["state"].as_str() != Some("testing") {
            if v["state"].as_str() == Some("landed") {
                return Err("the culprit landed".into());
            }
            break;
        }
        if Instant::now() > end {
            return Err(format!("the culprit stayed in landing: {v:#}"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // The command line shows the batches: the clean task landed, the culprit was parked.
    let shown = s.mason_ok(&["landing"]);
    if !shown.contains("batch #") || !shown.contains("landed [0]") || !shown.contains("parked [1]")
    {
        return Err(format!("`mason landing` does not show the batch:\n{shown}"));
    }
    drop(second);
    kill_hosts(&s.data_dir);
    let log = s.origin_log();
    let clean = log.iter().filter(|m| *m == "work 0").count();
    if clean != 1 {
        return Err(format!(
            "the clean commit is on the remote {clean} times: {log:?}"
        ));
    }
    if log.iter().any(|m| m == "work 1") {
        return Err(format!("the culprit's commit reached the remote: {log:?}"));
    }
    if sh(&s.origin, &["ls-tree", "--name-only", "main"])
        .lines()
        .any(|f| f == "red.integration")
    {
        return Err("the red marker reached the base".into());
    }
    // Nothing of the batch is left on the host: no candidate branch, no open pull request.
    let end = Instant::now() + Duration::from_secs(40);
    loop {
        let refs = sh(&s.origin, &["for-each-ref", "--format=%(refname:short)", "refs/heads/mason"]);
        let open_prs = std::fs::read_dir(s.root.path().join("host-state/prs"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| std::fs::read_to_string(e.path()).is_ok_and(|t| t.contains("\"state\":\"open\"")))
            .count();
        if refs.trim().is_empty() && open_prs == 0 {
            break;
        }
        if Instant::now() > end {
            return Err(format!("left on the host: branches [{refs}], {open_prs} open pull request(s)"));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let store = Store::open(s.data_dir.join("store.db")).map_err(|e| e.to_string())?;
    store
        .verify()
        .map_err(|e| format!("log and projections disagree: {e}"))?;
    let w = store.world().map_err(|e| e.to_string())?;
    if w.tasks[&TaskId(0)].landed != Some(LandedOutcome::Merged) {
        return Err("the clean task is not recorded as merged".into());
    }
    let sent_back = store
        .read_since(0, 100_000)
        .map_err(|e| e.to_string())?
        .iter()
        .any(|e| matches!(&e.event.kind, EventKind::SentBack { task, .. } if task.0 == 1));
    if !sent_back {
        return Err("the culprit was not sent back".into());
    }
    // No integration branch is left behind once a batch is over.
    let left = sh(&s.repo, &["branch", "--list", "mason/int/*"]);
    let batches = store.batches(None, 50).map_err(|e| e.to_string())?;
    if batches.iter().all(|b| b.finished_at.is_some()) && !left.trim().is_empty() {
        return Err(format!("integration branches left behind: {left}"));
    }
    Ok(())
}

const POINTS: &[&str] = &[
    "batch_formed",
    "batch_merged",
    "batch_red",
    "batch_verified",
    "batch_before_promote",
    "batch_after_promote",
    "land_before_event",
];

#[test]
fn killing_the_daemon_at_each_landing_phase_of_a_batch_with_a_culprit_loses_and_repeats_nothing() {
    let results: Vec<(String, Result<(), String>)> = std::thread::scope(|sc| {
        let hs: Vec<_> = POINTS
            .iter()
            .map(|p| (p.to_string(), sc.spawn(move || run(p, "direct"))))
            .collect();
        hs.into_iter()
            .map(|(p, h)| {
                (
                    p,
                    h.join()
                        .unwrap_or_else(|_| Err("the scenario panicked".into())),
                )
            })
            .collect()
    });
    let failures: Vec<String> = results
        .into_iter()
        .filter_map(|(p, r)| r.err().map(|e| format!("{p}: {e}")))
        .collect();
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn killing_the_daemon_during_ci_or_a_pull_request_leaves_nothing_behind_and_lands_once() {
    let cases: &[(&str, &[&str])] = &[
        ("direct-ci", &["ci_pushed", "ci_finished", "batch_red", "batch_before_promote", "batch_after_promote"]),
        ("pr-batch", &["pr_pushed", "pr_opened", "pr_checked", "batch_before_promote", "batch_after_promote"]),
        ("host-queue", &["pr_opened", "batch_before_promote", "batch_after_promote"]),
    ];
    let jobs: Vec<(String, String)> = cases
        .iter()
        .flat_map(|(m, ps)| ps.iter().map(move |p| (m.to_string(), p.to_string())))
        .collect();
    let mut failures = vec![];
    for chunk in jobs.chunks(6) {
        let results: Vec<(String, Result<(), String>)> = std::thread::scope(|sc| {
            let hs: Vec<_> = chunk
                .iter()
                .map(|(m, p)| (format!("{m} @ {p}"), sc.spawn(move || run(p, m))))
                .collect();
            hs.into_iter()
                .map(|(n, h)| (n, h.join().unwrap_or_else(|_| Err("the scenario panicked".into()))))
                .collect()
        });
        failures.extend(results.into_iter().filter_map(|(n, r)| r.err().map(|e| format!("{n}: {e}"))));
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
