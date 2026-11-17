// SPDX-License-Identifier: GPL-3.0-or-later

//! A deliberately failing scenario for scripts/exit/s545.sh: a wait for a state the task
//! can no longer reach must fail with the early-exit message, not "timed out after 900s".
//! It does nothing unless `MASON_EARLY_EXIT_DEMO=1`, so normal runs never see it fail.

mod support;
use mason_core::{EventKind, State};
use serde_json::json;
use support::*;

#[test]
fn a_wait_for_an_unreachable_state_fails_at_once() {
    if std::env::var("MASON_EARLY_EXIT_DEMO").as_deref() != Ok("1") {
        return;
    }
    let s = Scratch::new("true", json!({"steps": {}}));
    let d = s.start();
    let t = file_task(&d, "demo");
    d.emit(EventKind::Canceled {
        task: t,
        delete_branch: false,
    })
    .unwrap();
    wait_state(&d, t, State::Landed, 1);
}
