// SPDX-License-Identifier: GPL-3.0-or-later

//! Task 989: the owner edits a task's title and body after it has started.

use crate::support::*;
use mason_core::*;
use mason_daemon::{attention, taskedit, Daemon};
use mason_store::TrackerItemRow;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

fn start() -> (Scratch, Arc<Daemon>) {
    let s = Scratch::new("test -f hello.txt", json!({"steps": {}}));
    let d = s.start_with(|c| {
        c.owner_names = vec!["brian".into()];
        c.reload_interval = Duration::from_millis(100);
    });
    (s, d)
}

fn proposed(d: &Arc<Daemon>) -> TaskId {
    file_task_as(
        d,
        "Old title",
        "old criteria",
        true,
        TrustSource::Owner,
        true,
    )
}

#[test]
fn the_owner_edits_the_text_and_the_edit_is_recorded_with_its_reason() {
    let (_s, d) = start();
    let t = proposed(&d);
    let r = taskedit::edit(
        &d,
        "brian",
        t.0,
        Some("New title"),
        Some("new criteria"),
        "split",
    )
    .unwrap();
    assert!(r["note"]
        .as_str()
        .unwrap()
        .contains("next step reads the new text"));
    let task = task_of(&d, t);
    assert_eq!(
        (task.title.as_str(), task.body.as_str()),
        ("New title", "new criteria")
    );
    let h = taskedit::history(&d, t.0);
    assert_eq!(h.len(), 1);
    assert_eq!(h[0]["reason"], "split");
    assert_eq!(h[0]["who"], "brian");
    assert_eq!(h[0]["title"], "Old title");
    assert_eq!(h[0]["body"], "old criteria");
    let rows = d.store.interventions(0).unwrap();
    assert!(rows
        .iter()
        .any(|r| r.kind == "task_edit" && r.task == Some(t.0) && r.reason == "split"));
}

#[test]
fn a_canceled_task_is_frozen() {
    let (_s, d) = start();
    let t = proposed(&d);
    d.emit(EventKind::Canceled {
        task: t,
        delete_branch: false,
    })
    .unwrap();
    let e = taskedit::edit(&d, "brian", t.0, Some("x"), None, "").unwrap_err();
    assert_eq!(e.0, 409);
    assert_eq!(task_of(&d, t).title, "Old title");
}

#[test]
fn someone_who_is_not_the_owner_is_refused() {
    let (_s, d) = start();
    let t = proposed(&d);
    assert!(!attention::is_owner(&d, "mallory"));
    let e = taskedit::edit(&d, "mallory", t.0, Some("x"), None, "").unwrap_err();
    assert_eq!(e.0, 403);
    assert_eq!(task_of(&d, t).title, "Old title");
    assert!(taskedit::history(&d, t.0).is_empty());
}

#[test]
fn a_task_from_a_tracker_issue_is_refused_because_the_tracker_owns_its_text() {
    let (_s, d) = start();
    let t = proposed(&d);
    d.store
        .put_tracker_item(&TrackerItemRow {
            project: "scratch".into(),
            number: 7,
            task: t.0,
            title: "Old title".into(),
            body: "old criteria".into(),
            priority: "normal".into(),
            open: true,
            mirrored: String::new(),
            labelled: None,
        })
        .unwrap();
    let e = taskedit::edit(&d, "brian", t.0, Some("x"), None, "").unwrap_err();
    assert_eq!(e.0, 409);
    assert!(e.1.contains("issue #7"));
}

#[test]
fn nothing_to_change_and_an_empty_body_are_refused() {
    let (_s, d) = start();
    let t = proposed(&d);
    assert_eq!(
        taskedit::edit(&d, "brian", t.0, None, None, "")
            .unwrap_err()
            .0,
        400
    );
    assert_eq!(
        taskedit::edit(&d, "brian", t.0, None, Some("  "), "")
            .unwrap_err()
            .0,
        400
    );
}
