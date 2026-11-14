// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 234: a sidebar and a task view read the broker's wait reasons from its
//! published snapshot, taking the broker lock at most once per build.

mod support;
use mason_core::*;
use mason_daemon::sched::broker_locks_taken_by_this_thread as locks;
use mason_daemon::views::Ctx;
use mason_scheduler::{Needs, Rank};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use support::*;

#[test]
fn many_waiting_tasks_cost_at_most_one_broker_lock_per_build() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let pid = d.project_id_by_name("scratch").unwrap();
    d.broker.set_limit("widget", 1).unwrap();
    let alive = Arc::new(AtomicBool::new(true));
    let mut threads = vec![];
    let mut ids = vec![];
    for i in 0..21u64 {
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
        ids.push(t);
        let (b, alive) = (d.broker.clone(), alive.clone());
        threads.push(std::thread::spawn(move || {
            let rank = Rank::new(false, 3, 0, 1, 1000 + i, false);
            let lease = b.acquire(
                t,
                "step",
                rank,
                Needs::of(&[("widget", 1)]),
                &|| alive.load(Ordering::SeqCst),
                &mut |_| {},
            );
            while alive.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            drop(lease);
        }));
        // One at a time, so the first is the holder.
        let want = i as usize;
        let mut spins = 0;
        while ids.iter().filter(|t| d.broker.has_lease(**t)).count() < want + 1 {
            spins += 1;
            assert!(spins < 2000, "lease never opened");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    let waiting = ids
        .iter()
        .filter(|t| d.broker.explain_task(**t).is_some())
        .count();
    assert_eq!(waiting, 20);

    let w = d.world().unwrap();
    let before = locks();
    let ctx = Ctx::new(&d, &w);
    let sidebar = ctx.sidebar("");
    let detail = ctx.task_detail(ids[5].0).unwrap();
    let taken = locks() - before;
    assert!(taken <= 1, "{taken} broker locks for one build");
    assert!(
        detail
            .row
            .as_ref()
            .unwrap()
            .why_waiting
            .starts_with("Waiting for"),
        "{}",
        detail.row.as_ref().unwrap().why_waiting
    );
    let rows: Vec<_> = sidebar
        .sections
        .iter()
        .flat_map(|s| s.rows.iter())
        .collect();
    assert!(rows.len() >= 21);

    alive.store(false, Ordering::SeqCst);
    for t in threads {
        t.join().unwrap();
    }
}
