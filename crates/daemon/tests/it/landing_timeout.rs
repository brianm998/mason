// SPDX-License-Identifier: GPL-3.0-or-later

//! s702 (decision 0695): a batch run killed at its time limit is not a red to bisect. It is
//! rerun once whole; when it times out again the batch is held, nobody is blamed, and only a
//! test that was still running at both kills is searched for, on its own.
//!
//! The suite is a fake that blocks until it is killed, under a limit of a few seconds. Nothing
//! here asserts how long anything took.

use crate::support;
use serde_json::json;
use std::time::Duration;
use support::lw::*;
use support::*;

const TOML: &str =
    "window = \"2s\"\nmax_size = 5\n[[tier]]\nname = \"batch\"\nsuites = [\"integration\"]\n";

/// The suite. `hang.always.integration` anywhere in the tree makes it block, naming the test
/// `hangs_forever` as libtest does after 60 s; `hang.nameless.integration` blocks with no test
/// named; `hang.once.integration` blocks the first time only (a marker outside the tree).
fn check(marker: &str) -> String {
    format!(
        r#"#!/bin/sh
has() {{ [ -n "$(find . -name "$1" -not -path "./.git/*")" ]; }}
if has hang.always.integration; then
  echo "rerun asked for: $*"
  echo "test hangs_forever has been running for over 60 seconds"
  exec sleep 600
fi
if has hang.nameless.integration; then exec sleep 600; fi
if has hang.once.integration && [ ! -e "{marker}" ]; then
  touch "{marker}"
  echo "test sometimes_slow has been running for over 60 seconds"
  exec sleep 600
fi
echo "rerun asked for: $*"
echo "1..1"
echo "ok 1 - integration"
"#
    )
}

fn timeout_world(marker: &str) -> W {
    let s = Scratch::new("true", json!({"steps": {}}));
    s.write_project("scratch", "true", "");
    let definition = r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "land"

[step.land]
type = "landing"
category = "landing"
on_red = "develop"
max_rounds = 3

[suite.integration]
command = "sh checks.sh"
rerun = "sh checks.sh {tests}"
parser = "tap"
min_tests = 1
level = "unit"
timeout = "4s"
"#;
    let checks = check(marker);
    s.push_files(
        &[
            (".mason/mason.toml", definition),
            (".mason/landing.toml", TOML),
            ("checks.sh", &checks),
            ("shared.txt", "base\n"),
        ],
        "project setup",
    );
    sh(&s.repo, &["fetch", "-q", "origin"]);
    let d = s.start_with(|c| c.env_retry = Duration::from_millis(200));
    mason_daemon::attention::pause(&d, "", "claims", "test", 3600).unwrap();
    W { s, d }
}

fn batch_notes(w: &W) -> Vec<String> {
    w.d.store
        .batches(Some("scratch"), 10)
        .unwrap()
        .into_iter()
        .map(|b| b.note)
        .collect()
}

fn any_bisect_log(w: &W) -> bool {
    let b = w.d.store.batches(Some("scratch"), 10).unwrap();
    let Some(id) = b.iter().map(|b| b.id).min() else {
        return false;
    };
    let dir =
        w.d.cfg
            .task_dir(0)
            .parent()
            .unwrap()
            .join("batches")
            .join(id.to_string());
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .any(|e| e.file_name().to_string_lossy().contains("-bis"))
        })
        .unwrap_or(false)
}

#[test]
fn a_batch_that_times_out_once_lands_after_one_whole_rerun_and_no_bisect() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("hung-once");
    let w = timeout_world(marker.to_str().unwrap());
    let t1 = w.task("one", &[("x/hang.once.integration", "1")]);
    let t2 = w.task("two", &[("x/two.txt", "2")]);
    wait_for("both to land", 240, || w.landed(t1) && w.landed(t2));
    assert!(w.sent_back(t1).is_none() && w.sent_back(t2).is_none());
    assert!(
        batch_notes(&w).iter().any(|n| n.contains("timed out once")),
        "{:?}",
        batch_notes(&w)
    );
    assert!(!any_bisect_log(&w));
}

#[test]
fn a_batch_that_times_out_twice_with_no_test_named_is_held_and_nobody_is_blamed() {
    let dir = tempfile::tempdir().unwrap();
    let w = timeout_world(dir.path().join("unused").to_str().unwrap());
    let t1 = w.task("one", &[("x/hang.nameless.integration", "1")]);
    let t2 = w.task("two", &[("x/two.txt", "2")]);
    wait_for("the batch to be held", 240, || {
        batch_notes(&w)
            .iter()
            .any(|n| n.contains("timed out twice"))
    });
    assert!(!w.landed(t1) && !w.landed(t2));
    assert!(w.sent_back(t1).is_none() && w.sent_back(t2).is_none());
    assert!(!any_bisect_log(&w));
    assert!(
        w.d.landing()
            .attention()
            .iter()
            .any(|(_, _, text)| text.contains("timed out twice")),
        "{:?}",
        w.d.landing().attention()
    );
}

#[test]
fn a_batch_that_times_out_twice_on_the_same_test_bisects_on_that_test_alone() {
    let dir = tempfile::tempdir().unwrap();
    let w = timeout_world(dir.path().join("unused").to_str().unwrap());
    let t1 = w.task("one", &[("x/one.txt", "1")]);
    let t2 = w.task("two", &[("x/hang.always.integration", "1")]);
    let t3 = w.task("three", &[("x/three.txt", "3")]);
    wait_for("the innocent two to land", 300, || {
        w.landed(t1) && w.landed(t3)
    });
    wait_for("the task that hangs to go back", 60, || {
        w.sent_back(t2).is_some()
    });
    assert!(w.sent_back(t1).is_none() && w.sent_back(t3).is_none());
    assert!(!w.on_main("x/hang.always.integration"));
    assert!(
        batch_notes(&w)
            .iter()
            .any(|n| n.contains("timed out twice") && n.contains(&format!("#{}", t2.0))),
        "{:?}",
        batch_notes(&w)
    );
}
