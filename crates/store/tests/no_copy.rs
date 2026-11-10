// SPDX-License-Identifier: GPL-3.0-or-later

//! Replay, `world()` and an append do not deep-copy the world. Counted, not timed.
//! Alone in this file: the clone counter is per process, and tests share a process.

use mason_core::*;
use mason_engine::world_clones;
use mason_store::Store;

fn ev(s: &Store, at: u64, kind: EventKind) {
    s.append(Event {
        at: Timestamp(at),
        kind,
    })
    .expect("append");
}

fn fill(s: &Store, tasks: u64) {
    ev(
        s,
        1,
        EventKind::ProjectRegistered {
            project: ProjectId(1),
            name: "p".into(),
        },
    );
    for i in 0..tasks {
        let task = TaskId(i);
        ev(
            s,
            10 + i,
            EventKind::TaskFiled {
                project: ProjectId(1),
                title: "t".into(),
                body: "b".into(),
                priority: Priority::Normal,
                trust: TrustSource::Owner,
                explored: false,
                proposed: false,
                blockers: if i > 0 { vec![TaskId(i - 1)] } else { vec![] },
                not_before: None,
            },
        );
        if i == 0 {
            ev(s, 11, EventKind::Admitted { task, entry: None });
        }
    }
}

#[test]
fn replay_world_and_append_copy_nothing() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("mason.db");
    {
        let s = Store::open(&path).unwrap();
        fill(&s, 30);
    }

    // Replay on open.
    let before = world_clones();
    let s = Store::open(&path).unwrap();
    assert_eq!(world_clones(), before, "replay copied the world");

    // Snapshots are shared.
    let a = s.world().unwrap();
    let b = s.world().unwrap();
    assert!(std::sync::Arc::ptr_eq(&a, &b));
    assert_eq!(world_clones(), before, "world() copied the world");
    drop((a, b));

    // Appends, with no snapshot held, change the world in place.
    for i in 0..10 {
        ev(&s, 1000 + i, EventKind::Tick);
    }
    ev(
        &s,
        2000,
        EventKind::StepSucceeded {
            task: TaskId(0),
            next: None,
        },
    );
    assert_eq!(world_clones(), before, "an append copied the world");

    // A refused event changes nothing and copies nothing.
    let r = s.append(Event {
        at: Timestamp(3000),
        kind: EventKind::Approved { task: TaskId(0) },
    });
    assert!(r.is_err());
    assert_eq!(world_clones(), before);

    // verify() checks the projections against a fresh replay, and the index.
    s.verify().unwrap();
    s.world().unwrap().check_invariants().unwrap();
    assert!(s.world().unwrap().open_run(TaskId(0)).is_some());
}

#[test]
fn a_held_snapshot_is_not_changed_by_a_later_append() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path().join("mason.db")).unwrap();
    fill(&s, 3);
    let held = s.world().unwrap();
    let n = held.tasks.len();
    ev(
        &s,
        500,
        EventKind::TaskFiled {
            project: ProjectId(1),
            title: "more".into(),
            body: String::new(),
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            explored: false,
            proposed: false,
            blockers: vec![],
            not_before: None,
        },
    );
    assert_eq!(held.tasks.len(), n);
    assert_eq!(s.world().unwrap().tasks.len(), n + 1);
}
