// SPDX-License-Identifier: GPL-3.0-or-later

//! A suite with `pty = true` shows its output line by line while it runs, and is
//! judged exactly as without. Waits are on state (files, logs), never on a fixed sleep.

mod support;
use mason_core::*;
use serde_json::json;
use std::time::Duration;
use support::*;

fn def(extra: &str, go: &str) -> String {
    format!(
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
suites = ["xc"]
on_red = "develop"
max_rounds = 2
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.xc]
command = "echo \"Test Case 'A.a' started.\"; while [ ! -f {go} ]; do sleep 0.05; done; echo \"Test Case 'A.a' passed (0.1 seconds).\""
parser = "xctest"
min_tests = 1
level = "unit"
{extra}
"#
    )
}

fn run(extra: &str, expect_live: bool) {
    let go = std::env::temp_dir().join(format!(
        "mason-pty-go-{}-{}",
        std::process::id(),
        extra.len()
    ));
    let _ = std::fs::remove_file(&go);
    let turn = json!([
        {"sh": "echo x > u-$(basename \"$PWD\").txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm add"},
        {"call": {"tool": "submit_change", "args": {"summary": "add", "head": "$HEAD"}}}
    ]);
    let s = Scratch::new("false", json!({"steps": {"develop": [turn]}}));
    s.push_files(
        &[(".mason/mason.toml", &def(extra, &go.display().to_string()))],
        "setup",
    );
    let d = s.start_with(|c| c.env_retry = Duration::from_millis(200));
    let mut bus = d.progress_bus.subscribe();
    let t = file_task_as(&d, "Pty", "Please do it.", true, TrustSource::Owner, false);
    let log_has = |d: &std::sync::Arc<mason_daemon::Daemon>, needle: &str| {
        let mut found = false;
        let mut stack = vec![d.cfg.task_dir(t.0)];
        while let Some(p) = stack.pop() {
            for e in std::fs::read_dir(&p).into_iter().flatten().flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|x| x == "log")
                    && std::fs::read_to_string(&path).is_ok_and(|c| c.contains(needle))
                {
                    found = true;
                }
            }
        }
        found
    };
    let mut live_lines: Vec<String> = vec![];
    // The started line is in the suite log and pushed as a live log line while the
    // command is still blocked on the file.
    wait_for("the started line in the suite log", 120, || {
        log_has(&d, "Test Case 'A.a' started.")
    });
    if expect_live {
        wait_for("the started line in the progress log lines", 60, || {
            while let Ok(u) = bus.try_recv() {
                live_lines.extend(u.log);
            }
            live_lines.iter().any(|l| l == "Test Case 'A.a' started.")
        });
    }
    if expect_live {
        wait_for("the started line as the run's message", 60, || {
            d.progress_of_task(t)
                .iter()
                .any(|p| p.message == "Test Case 'A.a' started.")
        });
    }
    assert!(!go.exists(), "the command is still blocked");
    std::fs::write(&go, "go").unwrap();
    wait_state(&d, t, State::Landed, 120);
    let _ = std::fs::remove_file(&go);
    let (v, note) = d.store.closing(t.0).unwrap().expect("a closing verdict");
    assert_eq!(v, "verified", "{note}");
}

#[test]
fn a_pty_suite_shows_its_started_line_while_it_runs() {
    run("pty = true", true);
}

#[test]
fn without_pty_the_same_suite_gives_the_same_verdict() {
    run("", false);
}
