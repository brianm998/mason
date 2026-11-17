// SPDX-License-Identifier: GPL-3.0-or-later

//! s512: reds of one test class on one base commit file one task, not one each.

mod support;
use mason_core::*;
use mason_daemon::basecheck::{record_red, Fix, Seen};
use serde_json::json;
use std::sync::Arc;
use support::*;

fn start() -> (Scratch, Arc<mason_daemon::Daemon>) {
    // Filed tasks stop at a question, so none ends while the test looks.
    let card = json!({
        "ask": "Should the token expire after one hour or one day?",
        "why": "The ticket does not say and the security notes are silent on it.",
        "options": [{"label": "One hour", "recommended": true}, {"label": "One day"}],
        "evidence": ["the ticket"],
        "key": "k"
    });
    let s = Scratch::new(
        "false",
        json!({"steps": {"explore": [[{"call": {"tool": "ask", "args": card}}]]}}),
    );
    let d = s.start();
    (s, d)
}

fn red(d: &Arc<mason_daemon::Daemon>, base: &str, tests: &[&str]) -> Vec<(String, Option<Fix>)> {
    let pid = d.project_id_by_name("scratch").unwrap();
    let seen = Seen {
        project: pid,
        project_name: "scratch",
        task: 0,
        step: "tests",
        base,
        cmd: "gradle test",
        log_tail: "tail",
        follows: None,
    };
    let t: Vec<String> = tests.iter().map(|s| s.to_string()).collect();
    record_red(d, "tests", &seen, &t)
}

fn fix_tasks(d: &Arc<mason_daemon::Daemon>) -> Vec<Task> {
    let mut v: Vec<Task> = d
        .world()
        .unwrap()
        .tasks
        .values()
        .filter(|t| t.title.contains(" fail on ") || t.title.contains(" fails on "))
        .cloned()
        .collect();
    v.sort_by_key(|t| t.id.0);
    v
}

#[test]
fn four_reds_in_one_class_file_one_task_listing_all_four() {
    let (_s, d) = start();
    let names = [
        "MascotTest > a",
        "MascotTest > b",
        "MascotTest > c",
        "MascotTest > d",
    ];
    let out = red(&d, "12cfa63f83aaaa", &names);
    let tasks = fix_tasks(&d);
    assert_eq!(
        tasks.len(),
        1,
        "{:?}",
        tasks.iter().map(|t| &t.title).collect::<Vec<_>>()
    );
    for n in names {
        assert!(
            tasks[0].body.contains(&format!("\n- `{n}`\n")),
            "{n} listed"
        );
    }
    assert!(out
        .iter()
        .all(|(_, f)| *f == Some(Fix::Task(tasks[0].id.0))));
}

#[test]
fn a_fifth_red_of_the_class_joins_and_another_class_files_its_own() {
    let (_s, d) = start();
    let base = "12cfa63f83aaaa";
    red(
        &d,
        base,
        &[
            "MascotTest > a",
            "MascotTest > b",
            "MascotTest > c",
            "MascotTest > d",
        ],
    );
    let first = fix_tasks(&d)[0].id.0;
    let out = red(&d, base, &["MascotTest > e"]);
    assert_eq!(out[0].1, Some(Fix::Task(first)));
    assert_eq!(fix_tasks(&d).len(), 1, "the fifth filed nothing");
    let out = red(&d, base, &["TopBarTest > x"]);
    assert_eq!(fix_tasks(&d).len(), 2);
    assert_ne!(out[0].1, Some(Fix::Task(first)));
    // The same class on another commit is its own task.
    red(&d, "bbbbbbbbbbbbbb", &["MascotTest > a"]);
    assert_eq!(fix_tasks(&d).len(), 3);
}
