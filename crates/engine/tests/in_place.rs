// SPDX-License-Identifier: GPL-3.0-or-later

mod common;
use common::*;
use mason_core::*;
use mason_engine::{reduce, reduce_in_place};

#[test]
fn in_place_equals_pure_and_a_refusal_changes_nothing() {
    let mut s = Sim::new();
    let a = s.file(false, false);
    let b = s.file_with(false, false, vec![a], None);
    let c = s.file(false, false);
    s.ok(EventKind::Admitted {
        task: c,
        entry: None,
    });

    // The same event, both ways.
    let ev = Event {
        at: Timestamp(500),
        kind: EventKind::StepSucceeded {
            task: c,
            next: None,
        },
    };
    let (pure, fx) = reduce(&s.w, &ev).unwrap();
    let mut w = s.w.clone();
    let (fx2, touched) = reduce_in_place(&mut w, &ev).unwrap();
    assert_eq!(pure, w);
    assert_eq!(fx, fx2);
    assert_eq!(touched.tasks, vec![c]);
    w.check_invariants().unwrap();

    // Refusals leave the world exactly as it was, even after a part-way edit.
    let refused = [
        EventKind::BlockersChanged {
            task: a,
            released: vec![],
            added: vec![b],
        },
        EventKind::Approved { task: a },
        EventKind::BlockerAdded {
            task: a,
            blocker: b,
        },
    ];
    for k in refused {
        let mut w = s.w.clone();
        let ev = Event {
            at: Timestamp(900),
            kind: k,
        };
        assert!(reduce_in_place(&mut w, &ev).is_err());
        assert_eq!(w, s.w);
    }
}

#[test]
fn the_open_run_index_matches_the_runs() {
    let mut s = Sim::new();
    let t = s.file(false, false);
    assert_eq!(s.w.open_run(t), None);
    s.ok(EventKind::Admitted {
        task: t,
        entry: None,
    });
    let run = s.w.open_run(t).expect("open run");
    s.w.check_invariants().unwrap();
    s.ok(EventKind::StepSucceeded {
        task: t,
        next: None,
    });
    assert_ne!(s.w.open_run(t), Some(run));
    s.w.check_invariants().unwrap();
}
