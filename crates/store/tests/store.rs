// SPDX-License-Identifier: GPL-3.0-or-later

use mason_core::*;
use mason_store::{migrations, Store, StoreError};
use rusqlite::Connection;
use std::path::PathBuf;

fn db() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("mason.db");
    (d, p)
}

fn ev(at: u64, kind: EventKind) -> Event {
    Event {
        at: Timestamp(at),
        kind,
    }
}

fn file(project: u64, title: &str) -> EventKind {
    EventKind::TaskFiled {
        project: ProjectId(project),
        title: title.into(),
        priority: Priority::Normal,
        trust: TrustSource::Owner,
        explored: false,
        proposed: false,
        blockers: vec![],
        not_before: None,
    }
}

fn register(s: &Store) {
    s.append(ev(
        1,
        EventKind::ProjectRegistered {
            project: ProjectId(1),
            name: "p".into(),
        },
    ))
    .unwrap();
}

#[test]
fn sequence_numbers_are_contiguous_and_clients_resume() {
    let (_d, p) = db();
    let s = Store::open(&p).unwrap();
    register(&s);
    for i in 0..9 {
        let a = s.append(ev(10 + i, file(1, "t"))).unwrap();
        assert_eq!(a.seq, i + 2);
    }
    assert_eq!(s.last_seq().unwrap(), 10);
    let all = s.read_since(0, 100).unwrap();
    assert_eq!(
        all.iter().map(|e| e.seq).collect::<Vec<_>>(),
        (1..=10).collect::<Vec<_>>()
    );
    // A client that saw up to 7 gets only 8, 9, 10.
    let missed = s.read_since(7, 100).unwrap();
    assert_eq!(
        missed.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![8, 9, 10]
    );
    assert_eq!(s.read_since(10, 100).unwrap(), vec![]);
    assert_eq!(s.read_since(0, 3).unwrap().len(), 3, "limit applies");
    s.verify().unwrap();
}

#[test]
fn refused_events_write_nothing() {
    let (_d, p) = db();
    let s = Store::open(&p).unwrap();
    register(&s);
    let before = (
        s.last_seq().unwrap(),
        s.projection_dump().unwrap(),
        s.world().unwrap(),
    );
    let r = s.append(ev(5, EventKind::Approved { task: TaskId(99) }));
    assert!(matches!(r, Err(StoreError::Rejected(_))), "{r:?}");
    let r = s.append(ev(6, file(42, "no such project")));
    assert!(matches!(r, Err(StoreError::Rejected(_))));
    let after = (
        s.last_seq().unwrap(),
        s.projection_dump().unwrap(),
        s.world().unwrap(),
    );
    assert_eq!(before, after);
}

#[test]
fn effects_come_back_with_the_append() {
    let (_d, p) = db();
    let s = Store::open(&p).unwrap();
    register(&s);
    let a = s.append(ev(2, file(1, "t"))).unwrap();
    assert_eq!(
        a.effects,
        vec![
            Effect::TaskCreated { task: TaskId(0) },
            Effect::WakeScheduler { task: TaskId(0) }
        ]
    );
}

/// A long, mixed scenario driven by a fixed pseudo-random sequence; refused
/// events are simply skipped, as a real caller would.
fn run_scenario(s: &Store, attempts: u64) -> u64 {
    let mut seed = 0x9E3779B97F4A7C15u64;
    let mut rnd = move |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    register(s);
    let mut accepted = 0;
    for i in 0..attempts {
        let task = TaskId(rnd(12));
        let kind = match rnd(14) {
            0..=2 => file(1, &format!("task\t{i}\nline")),
            3 => EventKind::Approved { task },
            4..=5 => EventKind::Admitted { task },
            6..=8 => EventKind::StepSucceeded { task },
            9 => EventKind::SentBack {
                task,
                reason: "red".into(),
            },
            10 => EventKind::QuestionAsked {
                task,
                question: "which?".into(),
            },
            11 => EventKind::Answered {
                task,
                decision: DecisionId(rnd(4)),
                answer: "yes".into(),
            },
            12 => EventKind::StepFailed {
                task,
                reason: "x".into(),
            },
            _ => match rnd(3) {
                0 => EventKind::Retried {
                    task,
                    to: State::Testing,
                },
                1 => EventKind::Canceled {
                    task,
                    delete_branch: false,
                },
                _ => EventKind::Paused { task },
            },
        };
        if s.append(ev(100 + i, kind)).is_ok() {
            accepted += 1;
        }
    }
    accepted
}

#[test]
fn projections_live_equal_rebuilt_equal_reopened() {
    let (_d, p) = db();
    let s = Store::open(&p).unwrap();
    let accepted = run_scenario(&s, 1500);
    assert!(
        accepted > 300,
        "scenario should exercise the store, got {accepted}"
    );
    s.verify().unwrap();
    let live_dump = s.projection_dump().unwrap();
    let live_world = s.world().unwrap();
    assert!(live_dump.len() > 10);

    // Rebuilding from the log gives the same projections and world.
    let rebuilt = s.rebuild_projections().unwrap();
    assert_eq!(rebuilt, live_world);
    assert_eq!(s.projection_dump().unwrap(), live_dump);
    s.verify().unwrap();
    drop(s);

    // And so does reopening.
    let s = Store::open(&p).unwrap();
    assert_eq!(s.world().unwrap(), live_world);
    assert_eq!(s.projection_dump().unwrap(), live_dump);
    s.verify().unwrap();
}

#[test]
fn log_is_append_only() {
    let (_d, p) = db();
    let s = Store::open(&p).unwrap();
    register(&s);
    let raw = Connection::open(&p).unwrap();
    assert!(raw.execute("UPDATE events SET kind = 'tick'", []).is_err());
    assert!(raw.execute("DELETE FROM events", []).is_err());
    assert_eq!(s.last_seq().unwrap(), 1);
}

#[test]
fn a_write_that_never_committed_is_gone_after_reopen() {
    let (_d, p) = db();
    {
        let s = Store::open(&p).unwrap();
        register(&s);
        s.append(ev(2, file(1, "kept"))).unwrap();
    }
    {
        // A writer that dies mid-transaction: log row and projection row
        // written, never committed.
        let mut raw = Connection::open(&p).unwrap();
        let tx = raw.transaction().unwrap();
        tx.execute(
            "INSERT INTO events(at, kind, payload) VALUES (3, 'tick', '')",
            [],
        )
        .unwrap();
        tx.execute(
            "UPDATE meta SET value = '3' WHERE key = 'projected_seq'",
            [],
        )
        .unwrap();
        std::mem::forget(tx); // no commit, no rollback call: like a killed process
        drop(raw);
    }
    let s = Store::open(&p).unwrap();
    assert_eq!(s.last_seq().unwrap(), 2);
    s.verify().unwrap();
}

#[test]
fn half_applied_projections_are_detected_and_repaired() {
    let (_d, p) = db();
    let s = Store::open(&p).unwrap();
    register(&s);
    s.append(ev(2, file(1, "a"))).unwrap();
    s.append(ev(3, EventKind::Admitted { task: TaskId(0) }))
        .unwrap();
    s.verify().unwrap();
    // Damage a projection behind the store's back.
    let raw = Connection::open(&p).unwrap();
    raw.execute("UPDATE tasks SET state = 'landed'", [])
        .unwrap();
    assert!(matches!(s.verify(), Err(StoreError::Inconsistent(_))));
    s.rebuild_projections().unwrap();
    s.verify().unwrap();
}

#[test]
fn stale_projections_are_rebuilt_on_open() {
    let (_d, p) = db();
    {
        let s = Store::open(&p).unwrap();
        register(&s);
        s.append(ev(2, file(1, "a"))).unwrap();
    }
    let raw = Connection::open(&p).unwrap();
    raw.execute_batch("DELETE FROM tasks; DELETE FROM projects; UPDATE meta SET value='0' WHERE key='projected_seq';").unwrap();
    drop(raw);
    let s = Store::open(&p).unwrap();
    s.verify().unwrap();
    assert_eq!(s.projection_dump().unwrap().len(), 2);
}

#[test]
fn an_older_database_copy_migrates_and_the_original_is_untouched() {
    let (d, old_path) = db();
    // Build a genuine version-1 database (log only) holding events in the
    // on-disk text format. These literals also pin that format.
    {
        let mut c = Connection::open(&old_path).unwrap();
        migrations::migrate_to(&mut c, 1).unwrap();
        for (at, kind, payload) in [
            (1, "project_registered", "1\tdemo"),
            (2, "task_filed", "1\tfirst\\ttask\tnormal\towner\t0\t0\t\t-"),
            (3, "task_filed", "1\tsecond\thigh\toutside\t1\t1\t0\t-"),
            (4, "admitted", "0"),
        ] {
            c.execute(
                "INSERT INTO events(at, kind, payload) VALUES (?1, ?2, ?3)",
                rusqlite::params![at, kind, payload],
            )
            .unwrap();
        }
        assert_eq!(migrations::version(&c).unwrap(), 1);
    }
    let copy = d.path().join("copy.db");
    std::fs::copy(&old_path, &copy).unwrap();

    let s = Store::open(&copy).unwrap();
    assert_eq!(s.last_seq().unwrap(), 4);
    s.verify().unwrap();
    let dump = s.projection_dump().unwrap().join("\n");
    assert!(
        dump.contains("task 0|1|first\ttask|exploring|running"),
        "{dump}"
    );
    assert!(dump.contains("task 1|1|second|proposed|idle"), "{dump}");
    // New appends keep working after migration.
    s.append(ev(5, EventKind::Approved { task: TaskId(1) }))
        .unwrap();
    drop(s);

    let c = Connection::open(&copy).unwrap();
    assert_eq!(migrations::version(&c).unwrap(), migrations::SCHEMA_VERSION);
    let c = Connection::open(&old_path).unwrap();
    assert_eq!(
        migrations::version(&c).unwrap(),
        1,
        "the original stays at v1"
    );
}

#[test]
fn readers_see_consistent_progress_while_the_writer_works() {
    let (_d, p) = db();
    let s = std::sync::Arc::new(Store::open(&p).unwrap());
    register(&s);
    let r = {
        let s = s.clone();
        std::thread::spawn(move || {
            let mut seen = 0;
            while seen < 150 {
                for e in s.read_since(seen, 50).unwrap() {
                    assert_eq!(e.seq, seen + 1, "no gaps, no repeats");
                    seen = e.seq;
                }
            }
        })
    };
    for i in 0..149 {
        s.append(ev(2 + i, file(1, "t"))).unwrap();
    }
    r.join().unwrap();
}
