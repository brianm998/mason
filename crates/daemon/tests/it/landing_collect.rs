// SPDX-License-Identifier: GPL-3.0-or-later

//! #731: the landing queue waits out its batch window only while another task of the project
//! is still moving toward landing. With a 20 minute window, nothing here may take 20 minutes:
//! each test waits (with a bound far below that) for the batch to land, and none asserts a
//! duration.
//!
//! The "moving" task is a task filed and left queued (the scheduler's claims are paused in
//! these worlds), which is a task that has not reached landing yet.

use crate::support;
use mason_core::*;
use support::lw::*;
use support::*;

const WINDOW: &str = "window = \"20m\"\nmax_size = 5\n";

fn project(w: &W) -> ProjectId {
    w.d.project_id_by_name("scratch").unwrap()
}

#[test]
fn a_lone_task_in_the_queue_lands_without_waiting_out_the_window() {
    let w = world_collecting(WINDOW);
    let t = w.task("alone", &[("a/one.txt", "1")]);
    wait_state(&w.d, t, State::Landed, 120);
    assert!(w.on_main("a/one.txt"));
}

#[test]
fn a_batch_waits_for_a_task_that_is_still_moving_and_forms_when_it_joins() {
    let w = world_collecting(WINDOW);
    // Filed, not claimed: on its way to landing.
    let other = file_task_as(&w.d, "later", "x", true, TrustSource::Owner, false);
    let t = w.task("first", &[("a/one.txt", "1")]);
    wait_for("the queue to name the task it waits for", 60, || {
        w.d.landing().collecting(project(&w)) == vec![other.0]
    });
    assert!(!w.landed(t), "the batch must not form while #{other} moves");
    assert_eq!(w.d.landing().queued(project(&w)), vec![t]);
    // A second task reaches landing, then the moving one stops: the pair lands as one batch.
    let t2 = w.task("second", &[("b/two.txt", "2")]);
    w.d.emit(EventKind::Canceled {
        task: other,
        delete_branch: true,
    })
    .unwrap();
    wait_state(&w.d, t, State::Landed, 120);
    wait_state(&w.d, t2, State::Landed, 120);
    let batches = w.d.store.batches(Some("scratch"), 10).unwrap();
    assert!(
        batches.iter().any(|b| {
            let ids: Vec<&str> = b.tasks.split(',').collect();
            ids.contains(&t.0.to_string().as_str()) && ids.contains(&t2.0.to_string().as_str())
        }),
        "both tasks share a batch: {batches:?}"
    );
}

#[test]
fn a_moving_task_that_is_cancelled_lets_the_waiting_batch_form() {
    let w = world_collecting(WINDOW);
    let other = file_task_as(&w.d, "later", "x", true, TrustSource::Owner, false);
    let t = w.task("first", &[("a/one.txt", "1")]);
    wait_for("the queue to wait for the other task", 60, || {
        w.d.landing().collecting(project(&w)) == vec![other.0]
    });
    assert!(!w.landed(t));
    w.d.emit(EventKind::Canceled {
        task: other,
        delete_branch: true,
    })
    .unwrap();
    wait_state(&w.d, t, State::Landed, 120);
}

#[test]
fn a_task_held_behind_a_blocker_is_not_waited_for() {
    let w = world_collecting(WINDOW);
    let blocker = file_task_as(&w.d, "blocker", "x", true, TrustSource::Owner, false);
    let pid = project(&w);
    w.d.emit(EventKind::TaskFiled {
        project: pid,
        title: "held".into(),
        body: "x".into(),
        priority: Priority::Normal,
        trust: TrustSource::Owner,
        explored: true,
        proposed: false,
        blockers: vec![blocker],
        not_before: None,
    })
    .unwrap();
    // Cancel the blocker's own wait: only the held task would remain, and it does not count.
    w.d.emit(EventKind::Canceled {
        task: blocker,
        delete_branch: true,
    })
    .unwrap();
    let t = w.task("first", &[("a/one.txt", "1")]);
    wait_state(&w.d, t, State::Landed, 120);
}
