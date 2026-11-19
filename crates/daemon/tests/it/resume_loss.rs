// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg(unix)]
// Unix-only: signals and unix sockets.

//! After a lost step host, only the cargo test executables that had not finished run again, and
//! the step is judged on both runs' tests (#621). The fake suite prints cargo's `Running` blocks;
//! its first host is killed once the lost run's log holds two `test result:` lines (read from the
//! log, not a timer).

use crate::support;
use mason_core::*;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use support::*;

const RUN: &str = r#"#!/bin/sh
# A stand-in for `cargo test`: one block per executable, as cargo prints them.
emit() {
  echo "     Running $1 (target/debug/deps/x-0123456789abcdef)"
  echo "running 1 test"
  echo "test $2 ... ok"
  echo
  echo "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured"
  echo
}
echo "$*" >> "$RAN_LOG"
case "$*" in
  *--workspace*)
    emit "unittests crates/a/src/lib.rs" a_lib
    emit "crates/a/tests/one.rs" one_t
    echo "     Running crates/a/tests/two.rs (target/debug/deps/two-0123456789abcdef)"
    echo "running 1 test"
    sleep 120
    ;;
  *)
    case "$*" in *--doc*) echo "   Doc-tests a"; echo "running 1 test"; echo "test a_doc ... ok"; echo; echo "test result: ok. 1 passed; 0 failed"; echo;; esac
    case "$*" in *"--test two"*) emit "crates/a/tests/two.rs" two_t;; esac
    ;;
esac
"#;

fn log_of(data: &Path) -> Option<PathBuf> {
    let mut stack = vec![data.join("t")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.to_string_lossy().ends_with("-u-checks.log") {
                return Some(p);
            }
        }
    }
    None
}

fn commit(cmd: &str, msg: &str) -> Value {
    json!([
        {"sh": format!("{cmd} && git add -A && git -c user.name=t -c user.email=t@t commit -qm '{msg}'")},
        {"call": {"tool": "submit_change", "args": {"summary": msg, "head": "$HEAD"}}}
    ])
}

#[test]
fn a_lost_host_reruns_only_the_executables_that_had_not_finished() {
    let ran = tempfile::tempdir().unwrap();
    let ran_log = ran.path().join("ran");
    let definition = format!(
        r#"schema = 1
start = "develop"

[resource.compile]
count = 1

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
suites = ["checks"]
on_red = "develop"
max_rounds = 2
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.checks]
command = "RAN_LOG={l} sh run.sh cargo test --workspace"
dev_command = "RAN_LOG={l} sh run.sh cargo test {{tests_for_change}}"
parser = "regex"
pass_regex = '^test (.+) \.\.\. ok$'
fail_regex = '^test (.+) \.\.\. FAILED$'
min_tests = 4
level = "unit"
"#,
        l = ran_log.display()
    );
    let s = Scratch::new(
        "false",
        json!({"steps": {"develop": [commit("echo x > change.txt", "change")]}}),
    );
    let krate = "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
    s.push_files(
        &[
            (".mason/mason.toml", &definition),
            ("run.sh", RUN),
            (
                "Cargo.toml",
                "[workspace]\nresolver = \"2\"\nmembers = [\"crates/*\"]\n",
            ),
            ("crates/a/Cargo.toml", krate),
            ("crates/a/src/lib.rs", "\n"),
            ("crates/a/tests/one.rs", "\n"),
            ("crates/a/tests/two.rs", "\n"),
        ],
        "project setup",
    );
    let d = s.start_with(|c| c.env_retry = Duration::from_millis(200));
    let stop = Arc::new(AtomicBool::new(false));
    {
        let (hosts, data, stop) = (d.cfg.hosts_dir(), d.cfg.data_dir.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let done = log_of(&data)
                    .and_then(|l| std::fs::read_to_string(l).ok())
                    .is_some_and(|t| t.matches("test result:").count() >= 2);
                if done {
                    for e in std::fs::read_dir(&hosts).into_iter().flatten().flatten() {
                        let id = e.file_name().to_string_lossy().to_string();
                        if id.ends_with("-u-checks") && crate::host_loss::kill_host_only(&e.path())
                        {
                            return;
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
    }
    let t = file_task_as(
        &d,
        "Resume",
        "Please do it.",
        true,
        TrustSource::Owner,
        false,
    );
    wait_state(&d, t, State::Landed, 120);
    stop.store(true, Ordering::SeqCst);
    let calls = std::fs::read_to_string(&ran_log).unwrap_or_default();
    let lines: Vec<&str> = calls.lines().collect();
    assert!(lines[0].ends_with("cargo test --workspace"), "{calls}");
    let rest = lines[1..].join("\n");
    assert!(
        rest.contains("-p a --doc --test two")
            && !rest.contains("--lib")
            && !rest.contains("--test one"),
        "only the unfinished targets ran again: {calls}"
    );
    let (verdict, note) = d.store.closing(t.0).unwrap().expect("closing");
    assert_eq!(verdict, "verified", "{note}");
    assert!(
        note.contains("lost host after 2 of 4 executables; reran 2"),
        "{note}"
    );
    assert!(note.contains("green, 4 tests ran"), "{note}");
}
