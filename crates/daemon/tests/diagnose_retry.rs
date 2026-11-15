// SPDX-License-Identifier: GPL-3.0-or-later

//! s397: a `retry` diagnosis of a failure outside the code is applied by itself for a project
//! that is not MASON's own; any other verdict stays a card. The project has `auto_retry` off,
//! so the retry loop is not the one retrying.

#![cfg(unix)]

mod support;
use mason_core::*;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use support::*;

fn auto_retries(d: &std::sync::Arc<mason_daemon::Daemon>, t: TaskId) -> usize {
    d.store
        .read_since(0, 100_000)
        .unwrap()
        .iter()
        .filter(
            |e| matches!(&e.event.kind, EventKind::Retried { task, auto: true, .. } if *task == t),
        )
        .count()
}

#[test]
fn a_retry_diagnosis_of_an_outside_failure_is_applied_by_itself_in_any_project() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    s.write_project("scratch", "test -f hello.txt", "auto_retry = false");
    let hook = s.origin.join("hooks/pre-receive");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(
        &hook,
        "#!/bin/sh\necho 'rejected by the hook' >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let d = s.start();
    let t = file_task(&d, "Push is rejected");
    wait_for("the outside failure", 180, || {
        let k = task_of(&d, t);
        k.state == State::Failed && k.failure_class == Some(FailClass::Outside)
    });
    let ask = json!({"verdict": "ask", "summary": "s", "question": "q?", "recommendation": "r"});
    mason_daemon::diagnose::record_verdict(&d, t, ask);
    assert_eq!(auto_retries(&d, t), 0, "an ask is a card");
    assert_eq!(d.store.diagnoses(Some(t.0)).unwrap()[0].status, "pending");

    let retry = json!({"verdict": "retry", "summary": "the push was refused", "note": "again"});
    mason_daemon::diagnose::record_verdict(&d, t, retry);
    assert_eq!(auto_retries(&d, t), 1, "the retry was applied by itself");
    let rows = d.store.diagnoses(Some(t.0)).unwrap();
    assert_eq!(rows.last().unwrap().status, "applied");
}
