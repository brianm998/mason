// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 97: a history section sends at most the newest rows, with the true count.

mod support;
use mason_core::*;
use mason_daemon::views::{Ctx, HISTORY_ROW_CAP};
use support::*;

#[test]
fn a_history_section_is_capped_with_the_true_count_and_the_newest_kept() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let pid = d.project_id_by_name("scratch").unwrap();
    let n = HISTORY_ROW_CAP + 20;
    let mut ids = vec![];
    for i in 0..n {
        let a = d
            .emit(EventKind::TaskFiled {
                project: pid,
                title: format!("t{i}"),
                body: "x".into(),
                priority: Priority::Normal,
                trust: TrustSource::Owner,
                explored: false,
                proposed: false,
                blockers: vec![],
                // Far ahead, so nothing is admitted while the test runs.
                not_before: Some(Timestamp(u64::MAX / 4)),
            })
            .unwrap();
        let t = a
            .effects
            .iter()
            .find_map(|e| match e {
                Effect::TaskCreated { task } => Some(*task),
                _ => None,
            })
            .unwrap();
        d.emit(EventKind::Canceled {
            task: t,
            delete_branch: false,
        })
        .unwrap();
        ids.push(t.0);
    }
    let w = d.world().unwrap();
    let ctx = Ctx::new(&d, &w);
    let sidebar = ctx.sidebar("");
    let sec = sidebar
        .sections
        .iter()
        .find(|s| s.state == "canceled")
        .expect("a canceled section");
    assert_eq!(sec.count as usize, n, "the true total");
    assert_eq!(sec.rows.len(), HISTORY_ROW_CAP, "capped");
    // Newest canceled first: the last ones canceled are the ones kept.
    let want: Vec<u64> = ids.iter().rev().take(HISTORY_ROW_CAP).copied().collect();
    let got: Vec<u64> = sec.rows.iter().map(|r| r.id).collect();
    assert_eq!(got, want);
    // A task outside the cap still opens by id.
    let oldest = ids[0];
    assert!(!got.contains(&oldest));
    assert!(ctx.task_detail(oldest).is_some());
    // Sections that are not history are not capped.
    assert!(sidebar
        .sections
        .iter()
        .filter(|s| s.state != "canceled")
        .all(|s| s.rows.len() == s.count as usize));
}
