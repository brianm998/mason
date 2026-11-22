// SPDX-License-Identifier: GPL-3.0-or-later

//! s943: a landing batch holds this machine's slots only for the suites that run on this machine.
//! Before, it took every suite's slots up front, so a batch whose suite ran on GitHub for most of
//! an hour kept a quarter of the local test capacity idle, and an exclusive request from another
//! project waited for the whole machine to drain, that is for the batch's GitHub run.
//!
//! The fake GitHub accepts connections and never answers, so a suite placed there stays out. No
//! duration is asserted: each check is made after the run has reached the fake GitHub, which is
//! after the batch has taken whatever it takes.

use crate::support;
use mason_daemon::sched::LeaseState;
use std::collections::BTreeMap;
use support::lw::*;
use support::wait_for;

const WORKFLOW: &str = "name: mason-run\non:\n  workflow_dispatch:\n";

/// `alpha` stays on this machine (`remote = false`) and uses a `test-suite` slot; `integration`
/// goes where the switch says and uses a `compile` slot.
const DEFINITION: &str = r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
suites = ["alpha", "integration"]
uses = { compile = 1 }
next = "land"
on_red = "develop"
max_rounds = 3

[step.land]
type = "landing"
category = "landing"
on_red = "develop"
max_rounds = 3

[resource.compile]
count = 1

[resource.test-suite]
count = 1

[impact]
"a/**" = ["alpha"]
"x/**" = ["integration"]

[suite.alpha]
command = "sh checks.sh alpha"
parser = "tap"
min_tests = 1
level = "unit"
remote = false
uses = { test-suite = 1 }

[suite.integration]
command = "sh checks.sh integration"
parser = "tap"
min_tests = 1
level = "unit"
uses = { compile = 1 }
"#;

fn github_batch(suites: &str) -> (W, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let (url, n) = crate::landing::silent_github();
    std::env::set_var("MASON_T943_TOKEN", "x");
    let extra = move |_: &support::Scratch| {
        format!(
            "[remote_runs]\nkind = \"github-actions\"\napi = \"{url}\"\nrepo = \"o/r\"\ntoken = \"env:MASON_T943_TOKEN\"\nruns_on = \"ubuntu-latest\"\n"
        )
    };
    let tier = format!("window = \"1s\"\n[[tier]]\nname = \"batch\"\nsuites = [{suites}]\n");
    let w = world_with(
        &tier,
        &extra,
        &[
            (".mason/mason.toml", DEFINITION),
            (".github/workflows/mason-run.yml", WORKFLOW),
        ],
    );
    mason_daemon::place::set_global(&w.d, mason_daemon::place::Mode::Remote, "test").unwrap();
    (w, n)
}

/// The batch's suite is out on GitHub: its run has written its hold record (`remote-holds`), the
/// last thing it does before it waits there, so whatever the batch holds of this machine is
/// already taken. (A request to the fake GitHub alone is not that: the daemon talks to it before
/// any batch.)
fn wait_until_out(w: &W) {
    let dir = mason_daemon::remote::holds_dir(&w.d.cfg.hosts_dir());
    wait_for("the batch's suite to be out on GitHub", 180, || {
        std::fs::read_dir(&dir).is_ok_and(|mut d| d.next().is_some())
    });
}

#[test]
fn a_batch_whose_only_suite_runs_on_github_holds_no_local_slot_while_it_is_out() {
    let (w, _github) = github_batch("\"integration\"");
    let _t = w.task("alpha", &[("x/c.txt", "1")]);
    wait_until_out(&w);
    assert_eq!(
        w.d.broker.used("compile"),
        0,
        "the batch holds a compile slot"
    );
    assert_eq!(w.d.broker.used("test-suite"), 0);
}

#[test]
fn an_exclusive_request_is_granted_while_a_batch_is_out_on_github() {
    let (w, _github) = github_batch("\"integration\"");
    let _t = w.task("alpha", &[("x/c.txt", "1")]);
    wait_until_out(&w);
    let star = w.d.broker.lease_open_as(
        "star exclusive",
        &BTreeMap::from([("exclusive".to_string(), 1)]),
        None,
    );
    wait_for("the exclusive request to be granted", 60, || {
        matches!(w.d.broker.lease_poll(star), LeaseState::Granted { .. })
    });
    w.d.broker.lease_close(star);
}

#[test]
fn a_batch_with_a_local_and_a_remote_suite_holds_only_the_local_suites_slots() {
    let (w, _github) = github_batch("\"alpha\", \"integration\"");
    // Both suites are impacted: `a/**` selects `alpha`, `x/**` `integration`.
    let _t = w.task("both", &[("a/c.txt", "1"), ("x/c.txt", "1")]);
    // `alpha` runs first, here; the batch's lease is still held while `integration` is out.
    wait_until_out(&w);
    assert_eq!(
        w.d.broker.used("test-suite"),
        1,
        "the local suite's slot is held"
    );
    assert_eq!(
        w.d.broker.used("compile"),
        0,
        "the remote suite's slot is not"
    );
}
