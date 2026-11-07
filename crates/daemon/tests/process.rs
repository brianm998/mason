// SPDX-License-Identifier: GPL-3.0-or-later

//! The real `masond` and `mason` binaries, talking over loopback HTTP.

mod support;
use serde_json::{json, Value};
use std::io::{Read, Write};
use support::*;

#[test]
fn the_cli_drives_a_task_from_filing_to_landing() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);

    let out = s.mason_ok(&[
        "task",
        "add",
        "Add a hello file",
        "--body",
        "Create hello.txt containing hello.",
    ]);
    assert!(out.contains("Filed task #0 in scratch"), "{out}");
    s.wait_task(0, "landed", 90);

    let list = s.mason_ok(&["task", "list"]);
    assert!(
        list.contains("landed") && list.contains("Add a hello file"),
        "{list}"
    );
    assert!(s
        .mason_ok(&["task", "list", "--state", "queued"])
        .contains("no tasks"));

    let show = s.mason_ok(&["task", "show", "0"]);
    assert!(
        show.contains("landed    merged") && show.contains("Create hello.txt containing hello."),
        "{show}"
    );
    for step in ["exploring", "developing", "testing", "landing"] {
        assert!(show.contains(step), "{step} in {show}");
    }
    let logs = s.mason_ok(&["logs", "0"]);
    assert!(
        logs.contains("mcp__mason__submit_plan") && logs.contains("result: accepted"),
        "{logs}"
    );
    assert!(
        logs.contains("(test)"),
        "the test run's output is in the logs too: {logs}"
    );

    let status = s.mason_ok(&["status"]);
    assert!(status.contains("1 landed"), "{status}");
    let j: Value = serde_json::from_str(&s.mason_ok(&["status", "--json"])).unwrap();
    assert_eq!(j["tasks"]["landed"], 1);
    assert_eq!(j["bugs"], 0);
    assert_eq!(j["event_loop_stalls"], 0);
    assert_eq!(sh(&s.origin, &["show", "main:hello.txt"]), "hello");
}

#[test]
fn cancel_retry_and_mistakes_are_reported_plainly() {
    let develop = serde_json::json!([{"sh": TASK_SCRIPT_COMMIT}, {"sleep_ms": 60000}]);
    let s = Scratch::new(
        "true",
        serde_json::json!({"steps": {"explore": [explore_turn()], "develop": [develop]}}),
    );
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    s.mason_ok(&["task", "add", "Long one"]); // one project: no --project needed
    s.wait_task(0, "developing", 60);

    // Retrying a task that is running is a legal "start this step again"; cancelling is final.
    let out = s.mason_ok(&["task", "cancel", "0"]);
    assert!(out.contains("its branch is kept"), "{out}");
    s.wait_task(0, "canceled", 30);

    for (args, want) in [
        (vec!["task", "show", "99"], "no task #99"),
        (vec!["task", "cancel", "0"], "not allowed"),
        (
            vec!["task", "add", "x", "--project", "nope"],
            "no project \"nope\"",
        ),
        (
            vec!["task", "add", "x", "--priority", "whenever"],
            "unknown priority",
        ),
        (
            vec!["task", "retry", "0", "--from", "sideways"],
            "unknown step",
        ),
        (vec!["task", "list", "--state", "nonsense"], "unknown state"),
        (vec!["task", "show", "abc"], "not a task number"),
        (vec!["frobnicate"], "unknown command"),
    ] {
        let o = s.mason(&args);
        assert!(!o.status.success(), "{args:?} should fail");
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains(want), "{args:?}: wanted {want:?} in {err:?}");
    }
}

#[test]
fn without_a_daemon_the_cli_says_so() {
    let s = Scratch::new("true", happy_script());
    let o = s.mason(&["status"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("masond is not running"));
}

#[test]
fn doctor_checks_the_setup_and_names_what_is_wrong() {
    let s = Scratch::new("true", happy_script());
    let out = s.mason_ok(&["doctor"]);
    assert!(
        out.contains("is a git repository") && out.contains("base ref \"origin/main\" exists"),
        "{out}"
    );
    assert!(out.contains("remote \"origin\" is configured"), "{out}");
    // Break the project: a base branch that does not exist, and a missing agent binary.
    let toml = std::fs::read_to_string(s.config_dir.join("projects/scratch.toml")).unwrap();
    std::fs::write(
        s.config_dir.join("projects/scratch.toml"),
        toml.replace("base = \"main\"", "base = \"no-such-branch\"")
            .replace("fake-claude", "missing-claude"),
    )
    .unwrap();
    let o = s.mason(&["doctor"]);
    assert!(!o.status.success(), "doctor fails when something is broken");
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("FAIL") && out.contains("no-such-branch") && out.contains("missing-claude"),
        "{out}"
    );
    std::fs::write(
        s.config_dir.join("projects/scratch.toml"),
        "path = \"/nope\"\n",
    )
    .unwrap();
    let out = String::from_utf8_lossy(&s.mason(&["doctor"]).stdout).to_string();
    assert!(out.contains("`base` is required"), "{out}");
}

fn http_post(port: u16, path: &str) -> String {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        s,
        "POST {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

#[test]
fn the_watchdog_notices_when_the_event_loop_is_blocked() {
    let s = Scratch::new("true", happy_script());
    let mut m = s.masond(&["--debug"], &[]);
    s.wait_daemon(&mut m);
    let before: Value = serde_json::from_str(&s.mason_ok(&["status", "--json"])).unwrap();
    assert_eq!(before["event_loop_stalls"], 0);

    let info: Value =
        serde_json::from_str(&std::fs::read_to_string(s.data_dir.join("daemon.json")).unwrap())
            .unwrap();
    let reply = http_post(
        info["port"].as_u64().unwrap() as u16,
        "/debug/stall?ms=1200",
    ); // blocks the loop on purpose
    assert!(reply.contains("blocked_ms"), "{reply}");
    wait_for("the watchdog to report", 10, || {
        let v: Value = serde_json::from_str(&s.mason_ok(&["status", "--json"])).unwrap();
        v["event_loop_stalls"].as_u64().unwrap_or(0) >= 1
            && v["worst_stall_ms"].as_u64().unwrap_or(0) >= 500
    });
    // Without --debug the endpoint does not exist.
    let s2 = Scratch::new("true", happy_script());
    let mut m2 = s2.masond(&[], &[]);
    s2.wait_daemon(&mut m2);
    let info: Value =
        serde_json::from_str(&std::fs::read_to_string(s2.data_dir.join("daemon.json")).unwrap())
            .unwrap();
    assert!(http_post(info["port"].as_u64().unwrap() as u16, "/debug/stall?ms=10").contains("404"));
}

#[test]
fn a_second_daemon_refuses_to_start_on_the_same_data() {
    let s = Scratch::new("true", happy_script());
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    let mut second = s.masond(&[], &[]);
    let st = {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if let Some(st) = second.child.try_wait().unwrap() {
                break st;
            }
            assert!(
                std::time::Instant::now() < end,
                "the second daemon should have exited"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    };
    assert!(!st.success());
    // The first is unharmed.
    assert!(s.mason(&["status"]).status.success());
}

// ---- Stage 7 through the real CLI ----

const DEF: &str = "schema = 1\nstart = \"explore\"\nexplored_entry = \"develop\"\n\n[step.explore]\ntype = \"agent\"\ncategory = \"exploring\"\nnext = \"develop\"\n\n[step.develop]\ntype = \"agent\"\ncategory = \"developing\"\nnext = \"test\"\n\n[step.test]\ntype = \"test\"\ncategory = \"testing\"\nmax_rounds = 3\non_red = \"develop\"\nnext = \"land\"\n\n[step.land]\ntype = \"landing\"\ncategory = \"landing\"\n";

#[test]
fn the_cli_files_a_task_as_explored_and_shows_why_exploring_was_skipped() {
    let s = Scratch::new("test -f hello.txt", unique_script());
    s.push_files(&[(".mason/mason.toml", DEF)], "pipeline");
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);

    s.mason_ok(&[
        "task",
        "add",
        "Already a plan",
        "--explored",
        "--body",
        "Do it.",
    ]);
    s.mason_ok(&[
        "task",
        "add",
        "From front matter",
        "--body",
        "---\nexplored: true\n---\nDo it too.",
    ]);
    s.mason_ok(&[
        "task",
        "add",
        "Needs exploring",
        "--body",
        "Make it better somehow.",
    ]);
    for id in 0..3 {
        s.wait_task(id, "landed", 90);
    }
    let a = s.mason_ok(&["task", "show", "0"]);
    assert!(
        a.contains("started   at develop (skipped exploring: filed as already explored)"),
        "{a}"
    );
    assert!(a.contains("mason task explore-anyway 0"), "{a}");
    let b = s.mason_ok(&["task", "show", "1"]);
    assert!(
        b.contains("skipped exploring: filed as already explored"),
        "front matter counts: {b}"
    );
    assert!(
        !s.mason_ok(&["task", "show", "1"])
            .contains("explored: true"),
        "the front matter was stripped from the body"
    );
    let c = s.mason_ok(&["task", "show", "2"]);
    assert!(
        c.contains("at explore (explored first)") && !c.contains("explore-anyway"),
        "{c}"
    );

    let st = s.mason_ok(&["status"]);
    assert!(
        st.contains("exploring: 1 explored, 2 skipped (flag 2, clarity 0, classifier 0)"),
        "{st}"
    );
    assert!(
        st.contains("definition for scratch:") && st.contains("base branch at"),
        "{st}"
    );
}

#[test]
fn approve_reject_and_explore_anyway_work_from_the_cli() {
    let def = DEF.replace("max_rounds = 3\non_red = \"develop\"\nnext = \"land\"", "max_rounds = 3\non_red = \"develop\"\nnext = \"sign-off\"\n\n[step.sign-off]\ntype = \"approval\"\ncategory = \"review\"\nnext = \"land\"");
    let s = Scratch::new(
        "test -f hello.txt",
        // The second develop turn (after "explore anyway") just resubmits the existing commit.
        json!({"steps": {"explore": [explore_turn()], "develop": [
            [{"sh": UNIQUE_COMMIT}, {"call": {"tool": "submit_change", "args": {"summary": "x", "head": "$HEAD"}}}],
            [{"call": {"tool": "submit_change", "args": {"summary": "again", "head": "$HEAD"}}}],
            [{"sh": UNIQUE_COMMIT}, {"call": {"tool": "submit_change", "args": {"summary": "x", "head": "$HEAD"}}}]
        ]}}),
    );
    s.push_files(&[(".mason/mason.toml", &def)], "pipeline");
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    s.mason_ok(&["task", "add", "Needs sign-off", "--explored"]);
    wait_for("it to wait for approval", 60, || {
        s.task_json(0)["activity"]
            .as_str()
            .is_some_and(|a| a.contains("waiting on approval by the owner"))
    });
    let o = s.mason(&["task", "explore-anyway", "0"]);
    assert!(
        o.status.success(),
        "a skipped task can still be sent to explore: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    // It went back to exploring, then comes around to the sign-off again.
    wait_for("it to wait for approval again", 90, || {
        let t = s.task_json(0);
        t["route"] == "explore_anyway"
            && t["activity"]
                .as_str()
                .is_some_and(|a| a.contains("approval"))
    });
    let t = s.task_json(0);
    assert_eq!(t["failures"], 0);
    s.mason_ok(&["task", "approve", "0"]);
    s.wait_task(0, "landed", 90);

    // Rejecting with no way back fails the task, with the reason in `task show`.
    s.mason_ok(&["task", "add", "Will be rejected", "--explored"]);
    wait_for("the second approval", 60, || {
        s.task_json(1)["activity"]
            .as_str()
            .is_some_and(|a| a.contains("approval"))
    });
    s.mason_ok(&["task", "reject", "1", "--reason", "not like this"]);
    s.wait_task(1, "failed", 30);
    assert!(s.mason_ok(&["task", "show", "1"]).contains("not like this"));
    for (args, want) in [
        (vec!["task", "approve", "0"], "is not in a step"),
        (vec!["task", "approve", "99"], "no task #99"),
        (vec!["task", "explore-anyway", "99"], "no task #99"),
    ] {
        let o = s.mason(&args);
        let err = String::from_utf8_lossy(&o.stderr).to_string();
        assert!(!o.status.success() && err.contains(want), "{args:?}: {err}");
    }
}

#[test]
fn definition_check_and_dry_run_need_no_daemon_and_say_what_is_wrong() {
    let s = Scratch::new("true", happy_script());
    let dir = s.root.path().join("defs");
    std::fs::create_dir_all(&dir).unwrap();
    let good = dir.join("good.toml");
    std::fs::write(&good, DEF).unwrap();
    let out = s.mason_ok(&["definition", "check", good.to_str().unwrap()]);
    assert!(
        out.contains("ok  4 steps")
            && out.contains("`test` red every time")
            && out.contains("still red after 3 rounds"),
        "{out}"
    );
    assert!(
        out.contains("flagged explored, everything green")
            && out.contains("develop > test > land  =>  landed"),
        "{out}"
    );

    let bad = dir.join("bad.toml");
    std::fs::write(
        &bad,
        DEF.replace("max_rounds = 3\n", "")
            .replace("start = \"explore\"", "strat = \"explore\""),
    )
    .unwrap();
    let o = s.mason(&["definition", "check", bad.to_str().unwrap()]);
    assert!(!o.status.success());
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(
        text.contains("unknown key `strat`; did you mean `start`?"),
        "{text}"
    );

    // Valid language that this version cannot run yet is called out separately.
    let gate = dir.join("gate.toml");
    std::fs::write(&gate, DEF.replace("type = \"test\"", "type = \"gate\"")).unwrap();
    let o = s.mason(&["definition", "check", gate.to_str().unwrap()]);
    assert!(
        !o.status.success()
            && String::from_utf8_lossy(&o.stdout).contains("cannot run yet  step.test.type"),
        "{}",
        String::from_utf8_lossy(&o.stdout)
    );

    // A dry run with a scenario file.
    let sc = dir.join("sc.toml");
    std::fs::write(&sc, "[[scenario]]\nname = \"red then fixed\"\nexplored = true\n[scenario.results]\ntest = [\"red\", \"green\"]\n").unwrap();
    let out = s.mason_ok(&[
        "definition",
        "dry-run",
        good.to_str().unwrap(),
        "--scenarios",
        sc.to_str().unwrap(),
    ]);
    assert!(
        out.contains("red then fixed")
            && out.contains("started by flag")
            && out.contains("develop > test (red) > develop > test > land  =>  landed"),
        "{out}"
    );
    assert!(
        s.mason(&[
            "definition",
            "check",
            dir.join("missing.toml").to_str().unwrap()
        ])
        .status
        .code()
            == Some(1)
    );
}

#[test]
fn help_filing_explains_every_way_to_say_a_task_is_explored() {
    let s = Scratch::new("true", happy_script());
    let out = s.mason_ok(&["help", "filing"]);
    for must in [
        "mason task add --explored",
        "explored: true",
        "mason:explored",
        "file_proposal",
        "Acceptance criteria",
        "ignored until you approve",
        "explore-anyway",
    ] {
        assert!(out.contains(must), "{must} in {out}");
    }
    assert!(s.mason_ok(&["help"]).contains("mason help filing"));
}

#[test]
fn config_set_show_and_unset_layer_settings_with_owner_and_expiry() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    s.push_files(&[(".mason/mason.toml", DEF)], "pipeline");
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);

    let show = s.mason_ok(&["config", "show"]);
    assert!(
        show.contains("develop (agent)")
            && show.contains("model       sonnet   <- built-in default"),
        "{show}"
    );
    assert!(
        show.contains("command     test -f hello.txt   <- the local overlay"),
        "{show}"
    );

    s.mason_ok(&[
        "config",
        "set",
        "step.develop.model",
        "opus",
        "--for",
        "2h",
        "--owner",
        "brian",
    ]);
    let show = s.mason_ok(&["config", "show"]);
    assert!(
        show.contains("model       opus   <- a runtime override by brian"),
        "{show}"
    );
    assert!(s
        .mason_ok(&["config", "list"])
        .contains("step.develop.model = opus   by brian"));

    // Rules: it must expire, must not change the graph, must name a real step.
    for (args, want) in [
        (
            vec!["config", "set", "step.develop.model", "x"],
            "must expire",
        ),
        (
            vec!["config", "set", "step.test.next", "develop", "--for", "1h"],
            "cannot be overridden",
        ),
        (
            vec!["config", "set", "step.ghost.model", "x", "--for", "1h"],
            "there is no step `ghost`",
        ),
        (
            vec!["config", "set", "step.test.max_rounds", "0", "--for", "1h"],
            "1 or more",
        ),
        (
            vec![
                "config",
                "set",
                "step.develop.budget",
                "soon",
                "--for",
                "1h",
            ],
            "not a duration",
        ),
        (
            vec!["config", "set", "step.develop.model", "x", "--for", "never"],
            "not a duration",
        ),
    ] {
        let o = s.mason(&args);
        assert!(
            !o.status.success() && String::from_utf8_lossy(&o.stderr).contains(want),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
    s.mason_ok(&["config", "unset", "step.develop.model"]);
    assert!(s
        .mason_ok(&["config", "show"])
        .contains("model       sonnet   <- built-in default"));
    assert!(s
        .mason_ok(&["config", "list"])
        .contains("no runtime overrides"));

    // Per-task override through the CLI.
    s.mason_ok(&["task", "add", "x", "--explored"]);
    s.mason_ok(&["task", "override", "0", "step.develop.model", "haiku"]);
    assert!(!s
        .mason(&["task", "override", "0", "step.develop.next", "x"])
        .status
        .success());
}

#[test]
fn definition_show_and_reload_report_a_refused_edit_and_keep_running() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    s.push_files(&[(".mason/mason.toml", DEF)], "pipeline");
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    let shown = s.mason_ok(&["definition", "show"]);
    assert!(
        shown.contains("skipping exploring starts at `develop`")
            && shown.contains("on red back to `develop` (3 rounds)"),
        "{shown}"
    );

    s.push_files(
        &[(".mason/mason.toml", &DEF.replace("max_rounds = 3\n", ""))],
        "remove the bound",
    );
    let o = s.mason(&["definition", "reload"]);
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stdout)
            .contains("Refused; the current definition keeps running"),
        "{}",
        String::from_utf8_lossy(&o.stdout)
    );
    assert!(s.mason_ok(&["definition", "show"]).contains("REFUSED"));
    assert!(s.mason_ok(&["status"]).contains("REFUSED"));
    let doctor = s.mason(&["doctor"]);
    assert!(
        !doctor.status.success()
            && String::from_utf8_lossy(&doctor.stdout).contains("FAIL")
            && String::from_utf8_lossy(&doctor.stdout).contains("max_rounds")
    );

    // Putting the same definition back is "nothing changed" (the old one never stopped running), but the refusal clears.
    s.push_files(&[(".mason/mason.toml", DEF)], "put it back");
    assert!(s
        .mason_ok(&["definition", "reload"])
        .contains("Nothing changed"));
    assert!(!s.mason_ok(&["definition", "show"]).contains("REFUSED"));
}

#[test]
fn the_cli_leaves_quietly_when_its_output_is_cut_off() {
    // `mason definition check FILE | head -1`: the reader goes away after one line.
    let s = Scratch::new("true", happy_script());
    let f = s.root.path().join("d.toml");
    std::fs::write(&f, DEF).unwrap();
    let out = std::process::Command::new("sh")
        .args([
            "-c",
            &format!(
                "'{}' --data-dir /nonexistent definition check '{}' | head -1",
                bin("mason-pipeline", "mason").display(),
                f.display()
            ),
        ])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !err.contains("panicked") && !err.contains("Broken pipe"),
        "{err}"
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).starts_with("ok  4 steps"),
        "{out:?}"
    );
}
