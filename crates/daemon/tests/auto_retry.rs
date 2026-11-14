// SPDX-License-Identifier: GPL-3.0-or-later

//! s247: a failure outside the code is retried by the daemon itself, from the failed step; a
//! third one in the window stays failed; a failure about the code is never retried; three tasks
//! failing the same way are one alert. The retry loop's own tick is set far away and the tests
//! call one pass themselves, with zero delays, so nothing waits on a clock.

#![cfg(unix)]

mod support;
use mason_core::*;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;
use support::*;

fn hook(s: &Scratch, on: bool) {
    let p = s.origin.join("hooks/pre-receive");
    if on {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "#!/bin/sh\necho 'rejected by the hook' >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    } else {
        let _ = std::fs::remove_file(&p);
    }
}

fn start(s: &Scratch) -> Arc<mason_daemon::Daemon> {
    s.start_with(|c| {
        c.auto_retry_delays = vec![Duration::ZERO, Duration::ZERO];
        c.auto_retry_tick = Duration::from_secs(3600);
    })
}

fn auto_retries(d: &Arc<mason_daemon::Daemon>, t: TaskId) -> usize {
    d.store
        .read_since(0, 100_000)
        .unwrap()
        .iter()
        .filter(
            |e| matches!(&e.event.kind, EventKind::Retried { task, auto: true, .. } if *task == t),
        )
        .count()
}

fn failed_outside(d: &Arc<mason_daemon::Daemon>, t: TaskId) -> bool {
    let task = task_of(d, t);
    task.state == State::Failed && task.failure_class == Some(FailClass::Outside)
}

#[test]
fn an_outside_failure_is_retried_twice_and_the_third_stays_failed() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    hook(&s, true);
    let d = start(&s);
    let t = file_task(&d, "Push is rejected");
    wait_for("the first outside failure", 180, || failed_outside(&d, t));
    // First retry.
    mason_daemon::autoretry::pass(&d);
    assert_eq!(auto_retries(&d, t), 1);
    wait_for("the second outside failure", 180, || {
        failed_outside(&d, t)
            && steps_run(&d, t)
                .iter()
                .filter(|s| s.as_str() == "land")
                .count()
                >= 2
    });
    // Second retry.
    mason_daemon::autoretry::pass(&d);
    assert_eq!(auto_retries(&d, t), 2);
    wait_for("the third outside failure", 180, || {
        failed_outside(&d, t)
            && steps_run(&d, t)
                .iter()
                .filter(|s| s.as_str() == "land")
                .count()
                >= 3
    });
    // No retry is left in the window: it stays failed, and the usual alert is raised.
    mason_daemon::autoretry::pass(&d);
    assert_eq!(auto_retries(&d, t), 2);
    assert_eq!(state_of(&d, t), State::Failed);
    let (done, next) = mason_daemon::autoretry::status(&d, &task_of(&d, t)).unwrap();
    assert_eq!((done, next), (2, None));
    assert!(mason_daemon::attention::alerts_now(&d)
        .iter()
        .any(|a| a.0 == format!("failed:{}", t.0)));
}

#[test]
fn an_outside_failure_that_clears_lands_after_one_automatic_retry() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    hook(&s, true);
    let d = start(&s);
    let t = file_task(&d, "Push is rejected once");
    wait_for("the outside failure", 180, || failed_outside(&d, t));
    // While a retry is coming, the failure is not an alert yet.
    assert!(!mason_daemon::attention::alerts_now(&d)
        .iter()
        .any(|a| a.0 == format!("failed:{}", t.0)));
    hook(&s, false);
    mason_daemon::autoretry::pass(&d);
    wait_state(&d, t, State::Landed, 180);
    assert_eq!(auto_retries(&d, t), 1);
    assert_eq!(
        steps_run(&d, t)
            .iter()
            .filter(|s| s.as_str() == "develop")
            .count(),
        1,
        "the retry went to the failed step, not back to develop"
    );
}

#[test]
fn a_failure_about_the_code_is_never_retried() {
    // Tests that always fail: the task fails after its rounds, about the code.
    let s = Scratch::new("false", happy_script());
    let d = start(&s);
    let t = file_task(&d, "Red tests");
    wait_state(&d, t, State::Failed, 300);
    assert_eq!(task_of(&d, t).failure_class, Some(FailClass::Code));
    mason_daemon::autoretry::pass(&d);
    assert_eq!(auto_retries(&d, t), 0);
    assert_eq!(state_of(&d, t), State::Failed);
}

#[test]
fn three_tasks_failing_the_same_way_raise_one_alert() {
    let s = Scratch::new("test -f hello.txt", unique_script());
    hook(&s, true);
    let d = start(&s);
    let ts: Vec<TaskId> = (0..3)
        .map(|i| file_task(&d, &format!("Rejected {i}")))
        .collect();
    for t in &ts {
        wait_for("an outside failure", 300, || failed_outside(&d, *t));
    }
    let alerts: Vec<_> = mason_daemon::attention::alerts_now(&d)
        .into_iter()
        .filter(|a| a.0.starts_with("outside-burst:"))
        .collect();
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert!(
        alerts[0].2.starts_with("3 tasks failed outside the code"),
        "{}",
        alerts[0].2
    );
}
