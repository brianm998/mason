// SPDX-License-Identifier: GPL-3.0-or-later

//! Each suite a test step runs gets its own limit as its run budget (`MASON_RUN_BUDGET_S`), the
//! limit the daemon holds that suite to, not the shortest limit among the step's suites. Nothing
//! here waits for time to pass: the suites only write down the budget they were given.

use crate::support;
use mason_core::*;
use serde_json::json;
use support::*;

const DEF: &str = r#"schema = 1
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
suites = ["short", "long"]
uses = { compile = 1 }
on_red = "develop"
max_rounds = 2
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.short]
timeout = "2m"
command = "sh budget.sh short"
parser = "tap"
min_tests = 1
level = "unit"

[suite.long]
timeout = "90m"
command = "sh budget.sh long"
parser = "tap"
min_tests = 1
level = "unit"
"#;

/// Writes the budget it was given to `budget-<name>` beside the given directory, then passes.
fn budget_script(out: &std::path::Path) -> String {
    format!(
        "#!/bin/sh\necho \"${{MASON_RUN_BUDGET_S:-none}}\" > {}/budget-$1\necho 1..1\necho 'ok 1 - ran'\n",
        out.display()
    )
}

#[test]
fn each_suite_of_a_test_step_is_given_its_own_limit_as_its_run_budget() {
    let out = tempfile::tempdir().unwrap();
    let s = Scratch::new(
        "true",
        json!({"steps": {"develop": [[
            {"sh": "echo hi > hi.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm hi"},
            {"call": {"tool": "submit_change", "args": {"summary": "hi", "head": "$HEAD"}}}
        ]]}}),
    );
    s.write_project("scratch", "true", "");
    s.push_files(
        &[
            (".mason/mason.toml", DEF),
            ("budget.sh", &budget_script(out.path())),
        ],
        "setup",
    );
    let d = s.start();
    let t = file_task_as(
        &d,
        "Say hi",
        "Please do it.",
        true,
        TrustSource::Owner,
        false,
    );
    wait_state(&d, t, State::Landed, 120);
    let read = |n: &str| {
        std::fs::read_to_string(out.path().join(format!("budget-{n}")))
            .unwrap_or_else(|e| panic!("suite {n} did not run: {e}"))
            .trim()
            .to_string()
    };
    assert_eq!(read("short"), "120", "the 2 min suite's own limit");
    assert_eq!(
        read("long"),
        "5400",
        "the 90 min suite's own limit, not the step's shortest"
    );
}
