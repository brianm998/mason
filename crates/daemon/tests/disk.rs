// SPDX-License-Identifier: GPL-3.0-or-later

//! The disk floor (s85): below it a filed task waits and says why, the alert names the free space
//! and the floor, and raising the reading lets the task run and land and clears the alert.

mod support;
use mason_core::*;
use mason_daemon::attention::alerts_now;
use mason_daemon::disk::FakeDisk;
use std::time::Duration;
use support::*;

fn disk_alert(d: &std::sync::Arc<mason_daemon::daemon::Daemon>) -> Option<String> {
    alerts_now(d)
        .into_iter()
        .find(|(k, _, _)| k == "disk")
        .map(|(_, _, t)| t)
}

#[test]
fn below_the_floor_a_task_waits_and_says_why_and_it_lands_when_there_is_room() {
    let s = Scratch::new("true", happy_script());
    let disk = FakeDisk::new_gb(12);
    let d = s.start_with(|c| {
        c.disk = Some(disk.clone());
        c.disk_every = Duration::from_millis(50);
    });
    let t = file_task(&d, "Waits for room");
    wait_for("the task to wait for disk", 30, || {
        d.broker
            .explain_task(t)
            .is_some_and(|e| e == "Waiting: disk has 12 GB free, floor 20 GB")
    });
    let text = disk_alert(&d).expect("the alert is raised");
    assert!(text.contains("12 GB free, floor 20 GB"), "{text}");
    std::thread::sleep(Duration::from_millis(500));
    assert_ne!(
        d.world().unwrap().tasks[&t].state,
        State::Landed,
        "nothing ran below the floor"
    );

    disk.set_gb(40);
    wait_state(&d, t, State::Landed, 60);
    wait_for("the alert to clear", 15, || disk_alert(&d).is_none());
}

#[test]
fn at_the_floor_a_task_runs() {
    let s = Scratch::new("true", happy_script());
    let d = s.start_with(|c| {
        c.disk = Some(FakeDisk::new_gb(20));
        c.disk_every = Duration::from_millis(50);
    });
    let t = file_task(&d, "Runs");
    wait_state(&d, t, State::Landed, 60);
    assert!(disk_alert(&d).is_none());
}

#[test]
fn the_alert_and_the_status_count_the_tasks_the_disk_holds() {
    let s = Scratch::new("true", happy_script());
    let disk = FakeDisk::new_gb(12);
    let d = s.start_with(|c| {
        c.disk = Some(disk.clone());
        c.disk_every = Duration::from_millis(50);
    });
    let tasks = [
        file_task(&d, "One"),
        file_task(&d, "Two"),
        file_task(&d, "Three"),
    ];
    wait_for("every task to be held by the disk", 30, || {
        tasks.iter().all(|t| {
            d.broker
                .explain_task(*t)
                .is_some_and(|e| e.starts_with("Waiting: disk has"))
        })
    });
    assert_eq!(d.broker.disk_held(), 3);
    let text = disk_alert(&d).expect("the alert is raised");
    assert!(text.contains("3 tasks waiting"), "{text}");
    disk.set_gb(40);
    wait_for("the hold to clear", 30, || d.broker.disk_held() == 0);
    assert!(d.broker.disk_low().is_none());
}
