// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 561: history rows leave their body out unless asked; a row says when it entered its state.

use crate::support;
use mason_core::*;
use mason_daemon::views::Ctx;
use support::*;

fn file(d: &std::sync::Arc<mason_daemon::Daemon>, title: &str, body: &str) -> TaskId {
    let pid = d.project_id_by_name("scratch").unwrap();
    let a = d
        .emit(EventKind::TaskFiled {
            project: pid,
            title: title.into(),
            body: body.into(),
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            explored: false,
            proposed: false,
            blockers: vec![],
            // Far ahead, so nothing is admitted while the test runs.
            not_before: Some(Timestamp(u64::MAX / 4)),
        })
        .unwrap();
    a.effects
        .iter()
        .find_map(|e| match e {
            Effect::TaskCreated { task } => Some(*task),
            _ => None,
        })
        .unwrap()
}

#[test]
fn history_rows_have_no_body_unless_asked_and_active_rows_keep_theirs() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let live = file(&d, "live", "live body");
    let gone = file(&d, "gone", "gone body");
    d.emit(EventKind::Canceled {
        task: gone,
        delete_branch: false,
    })
    .unwrap();
    let w = d.world().unwrap();
    let body_of = |ctx: &Ctx, id: TaskId| {
        ctx.sidebar("")
            .sections
            .iter()
            .flat_map(|s| s.rows.iter())
            .find(|r| r.id == id.0)
            .unwrap()
            .body
            .clone()
    };
    let mut ctx = Ctx::new(&d, &w);
    ctx.row_bodies = false;
    assert_eq!(body_of(&ctx, gone), "", "history row: no body");
    assert_eq!(body_of(&ctx, live), "live body", "active row keeps it");
    ctx.row_bodies = true;
    assert_eq!(body_of(&ctx, gone), "gone body");
    assert_eq!(body_of(&ctx, live), "live body");
}

#[test]
fn a_row_carries_when_it_entered_its_state_and_in_state_is_whole_minutes() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let t = file(&d, "one", "b");
    let w = d.world().unwrap();
    let ctx = Ctx::new(&d, &w);
    let row = ctx
        .sidebar("")
        .sections
        .into_iter()
        .flat_map(|s| s.rows)
        .find(|r| r.id == t.0)
        .unwrap();
    assert_eq!(row.state_since_ms, w.tasks[&t].state_entered_at.0);
    assert_eq!(row.in_state_ms % 60_000, 0);
}
