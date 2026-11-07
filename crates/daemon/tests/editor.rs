// SPDX-License-Identifier: GPL-3.0-or-later

//! The editor against a real daemon in-process: applying to the base branch (or by pull
//! request), refusing when the base moved, a draft that follows the base while unchanged,
//! and what a change affects ("2 tasks in this step will finish on the old version").

mod support;
use mason_core::*;
use mason_daemon::editor;
use serde_json::json;
use support::lw::*;
use support::*;

fn owner(w: &W) -> String {
    w.d.cfg.owner_names[0].clone()
}

fn pid(w: &W) -> ProjectId {
    w.d.project_id_by_name("scratch").unwrap()
}

fn insert(w: &W) {
    editor::operate(&w.d, pid(w), &json!({"op": "insert_step", "after": "develop", "name": "lint", "type": "command", "category": "developing", "fields": {"command": "true"}})).unwrap();
}

#[test]
fn applying_commits_to_the_base_branch_and_the_daemon_reloads() {
    let w = world("mode = \"direct\"\nwindow = \"30s\"\n", &[]);
    insert(&w);
    let m = editor::model(&w.d, pid(&w), "lint");
    assert_eq!(m["dirty"], true);
    assert_eq!(m["inspector"]["kind"], "command");
    assert!(
        m["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "adds step `lint`"),
        "{m}"
    );
    let v = editor::apply(&w.d, pid(&w), &owner(&w), "Add lint").unwrap();
    assert_eq!(v["applied"], true);
    assert!(sh(&w.s.origin, &["show", "main:.mason/mason.toml"]).contains("[step.lint]"));
    assert!(sh(&w.s.origin, &["log", "-1", "--format=%s%n%b", "main"]).contains("Add lint"));
    assert!(
        w.d.current_def(pid(&w)).step("lint").is_some(),
        "the daemon reloaded the definition"
    );
    assert_eq!(
        editor::model(&w.d, pid(&w), "")["dirty"],
        false,
        "the draft is the base again"
    );
}

#[test]
fn only_the_owner_applies_and_an_empty_or_broken_draft_is_refused() {
    let w = world("mode = \"direct\"\nwindow = \"30s\"\n", &[]);
    assert_eq!(
        editor::apply(&w.d, pid(&w), &owner(&w), "").unwrap_err().0,
        409,
        "nothing to apply"
    );
    insert(&w);
    assert_eq!(
        editor::apply(&w.d, pid(&w), "mallory", "x").unwrap_err().0,
        403
    );
    assert_eq!(editor::apply(&w.d, pid(&w), "", "x").unwrap_err().0, 403);
    assert!(!sh(&w.s.origin, &["show", "main:.mason/mason.toml"]).contains("lint"));
}

#[test]
fn a_draft_stops_when_the_base_changed_under_it_and_a_clean_one_follows_the_base() {
    let w = world("mode = \"direct\"\nwindow = \"30s\"\n", &[]);
    // A clean draft follows the base: somebody lands a change to the landing settings.
    assert_eq!(editor::model(&w.d, pid(&w), "")["landing"]["max_size"], 0);
    w.s.push_files(
        &[(
            ".mason/landing.toml",
            "mode = \"direct\"\nwindow = \"30s\"\nmax_size = 7\n",
        )],
        "somebody edits landing",
    );
    sh(&w.s.repo, &["fetch", "-q", "origin"]);
    assert_eq!(editor::model(&w.d, pid(&w), "")["landing"]["max_size"], 7);
    // A changed draft does not: it is refused at apply, with what to do.
    insert(&w);
    w.s.push_files(
        &[(
            ".mason/landing.toml",
            "mode = \"direct\"\nwindow = \"30s\"\nmax_size = 9\n",
        )],
        "and again",
    );
    let (code, why) = editor::apply(&w.d, pid(&w), &owner(&w), "x").unwrap_err();
    assert_eq!(code, 409);
    assert!(
        why.contains("changed on the base branch") && why.contains("discard"),
        "{why}"
    );
    editor::discard(&w.d, pid(&w));
    assert_eq!(editor::model(&w.d, pid(&w), "")["dirty"], false);
}

#[test]
fn where_landing_uses_pull_requests_apply_opens_one_and_changes_nothing_yet() {
    let w = world_with(
        "mode = \"pr-batch\"\nwindow = \"30s\"\n",
        &|s| {
            format!("{}[ci]\nkind = \"command\"\ntrigger = \"true\"\nstatus = \"echo green\"\npoll_s = 1\n", host_section(s, false, false))
        },
        &[],
    );
    insert(&w);
    let v = editor::apply(&w.d, pid(&w), &owner(&w), "Add lint").unwrap();
    assert_eq!(v["applied"], false);
    assert!(
        v["pull_request"]
            .as_str()
            .unwrap()
            .starts_with("fake://pr/"),
        "{v}"
    );
    assert!(
        !sh(&w.s.origin, &["show", "main:.mason/mason.toml"]).contains("lint"),
        "the base is untouched until the PR merges"
    );
    assert!(w.d.current_def(pid(&w)).step("lint").is_none());
    let pr: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(w.s.root.path().join("host-state/prs/1.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        (pr["state"].as_str(), pr["base"].as_str()),
        (Some("open"), Some("main"))
    );
    assert!(pr["head"]
        .as_str()
        .unwrap()
        .starts_with("mason/edit/scratch-"));
    assert_eq!(
        sh(
            &w.s.origin,
            &[
                "show",
                &format!("{}:.mason/mason.toml", pr["head"].as_str().unwrap())
            ]
        )
        .matches("[step.lint]")
        .count(),
        1
    );
}

#[test]
fn the_preview_says_which_tasks_will_finish_on_the_old_version() {
    let w = world("mode = \"direct\"\nwindow = \"600s\"\n", &[]);
    let t = w.task("waiting to land", &[("x/a.txt", "1")]);
    wait_for("the task to be in its landing step", 30, || {
        w.d.world().unwrap().open_run(t).is_some()
    });
    // Changing the `land` step affects the task in it.
    editor::operate(
        &w.d,
        pid(&w),
        &json!({"op": "set_field", "step": "land", "key": "max_rounds", "value": 5}),
    )
    .unwrap();
    let m = editor::model(&w.d, pid(&w), "");
    let affected: Vec<String> = m["affected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        affected,
        ["1 task(s) in `land` will finish on the old version"],
        "{m}"
    );
    // A change to a step nobody is in affects no one.
    editor::discard(&w.d, pid(&w));
    editor::operate(
        &w.d,
        pid(&w),
        &json!({"op": "set_field", "step": "develop", "key": "budget", "value": "2h"}),
    )
    .unwrap();
    assert!(editor::model(&w.d, pid(&w), "")["affected"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn undo_redo_and_discard_walk_the_history() {
    let w = world("mode = \"direct\"\nwindow = \"30s\"\n", &[]);
    let p = pid(&w);
    let m = |w: &W| editor::model(&w.d, p, "");
    assert_eq!(
        (m(&w)["can_undo"].clone(), m(&w)["can_redo"].clone()),
        (json!(false), json!(false))
    );
    insert(&w);
    editor::operate(
        &w.d,
        p,
        &json!({"op": "set_field", "step": "lint", "key": "command", "value": "cargo fmt --check"}),
    )
    .unwrap();
    assert_eq!(m(&w)["can_undo"], true);
    assert!(editor::undo(&w.d, p));
    assert!(m(&w)["text"]
        .as_str()
        .unwrap()
        .contains("command = \"true\""));
    assert_eq!(m(&w)["can_redo"], true);
    assert!(editor::redo(&w.d, p));
    assert!(m(&w)["text"]
        .as_str()
        .unwrap()
        .contains("cargo fmt --check"));
    // A new edit after an undo drops what could have been redone.
    assert!(editor::undo(&w.d, p));
    editor::operate(
        &w.d,
        p,
        &json!({"op": "set_field", "step": "lint", "key": "command", "value": "true; true"}),
    )
    .unwrap();
    assert!(!editor::redo(&w.d, p));
    editor::discard(&w.d, p);
    assert_eq!(
        (m(&w)["dirty"].clone(), m(&w)["can_undo"].clone()),
        (json!(false), json!(false))
    );
    assert!(!editor::undo(&w.d, p), "nothing before the base");
    // A refused edit is remembered for the view, and the next good one clears it.
    assert!(editor::operate(
        &w.d,
        p,
        &json!({"op": "set_edge", "from": "develop", "edge": "next", "to": "nowhere"})
    )
    .is_err());
    assert!(m(&w)["last_error"].as_array().unwrap()[0]
        .as_str()
        .unwrap()
        .contains("nowhere"));
    insert(&w);
    assert!(m(&w)["last_error"].as_array().unwrap().is_empty());
}
