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
        body: String::new(),
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
            4..=5 => EventKind::Admitted { task, entry: None },
            6..=8 => EventKind::StepSucceeded { task, next: None },
            9 => EventKind::SentBack {
                task,
                reason: "red".into(),
                to_step: String::new(),
            },
            10 => EventKind::QuestionAsked {
                task,
                question: "which?".into(),
            },
            11 => EventKind::Answered {
                task,
                decision: DecisionId(rnd(4)),
                answer: "yes".into(),
                step: String::new(),
            },
            12 => EventKind::StepFailed {
                task,
                reason: "x".into(),
            },
            _ => match rnd(3) {
                0 => EventKind::Retried {
                    task,
                    to: State::Testing,
                    step: String::new(),
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
    s.append(ev(
        3,
        EventKind::Admitted {
            task: TaskId(0),
            entry: None,
        },
    ))
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

#[test]
fn bugs_are_filed_and_listed_and_survive_reopen() {
    let (_d, p) = db();
    {
        let s = Store::open(&p).unwrap();
        register(&s);
        s.record_bug(10, "missed_job", "task 3 had no job").unwrap();
        s.record_bug(20, "missed_admit", "task 4").unwrap();
        assert_eq!(s.bugs().unwrap().len(), 2);
        s.verify().unwrap(); // bugs are not part of the log or the projections
    }
    let s = Store::open(&p).unwrap();
    assert_eq!(
        s.bugs().unwrap()[1],
        (20, "missed_admit".to_string(), "task 4".to_string())
    );
}

#[test]
fn definitions_are_remembered_by_hash() {
    let (_d, p) = db();
    {
        let s = Store::open(&p).unwrap();
        assert_eq!(s.definition("abc").unwrap(), None);
        s.put_definition("abc", "schema = 1\n", 5).unwrap();
        s.put_definition("abc", "something else", 6).unwrap(); // the first one wins: a hash names its text
        assert_eq!(
            s.definition("abc").unwrap().as_deref(),
            Some("schema = 1\n")
        );
    }
    let s = Store::open(&p).unwrap();
    assert_eq!(
        s.definition("abc").unwrap().as_deref(),
        Some("schema = 1\n")
    );
}

#[test]
fn runtime_overrides_need_an_owner_and_expire() {
    let (_d, p) = db();
    let s = Store::open(&p).unwrap();
    assert!(
        s.add_override("web", "step.develop.model", "opus", "", 100, 200)
            .is_err(),
        "no owner"
    );
    assert!(
        s.add_override("web", "step.develop.model", "opus", "brian", 100, 100)
            .is_err(),
        "no expiry in the future"
    );
    assert!(s
        .add_override("web", "step.develop.model", "opus", "brian", 100, 50)
        .is_err());
    s.add_override("web", "step.develop.model", "opus", "brian", 100, 200)
        .unwrap();
    s.add_override("web", "step.test.max_rounds", "5", "brian", 100, 400)
        .unwrap();
    s.add_override("other", "step.develop.model", "x", "brian", 100, 400)
        .unwrap();
    // Setting the same key again replaces it.
    s.add_override("web", "step.develop.model", "sonnet", "brian", 150, 300)
        .unwrap();

    let at = |now| {
        s.overrides("web", now)
            .unwrap()
            .into_iter()
            .map(|o| (o.key, o.value, o.owner))
            .collect::<Vec<_>>()
    };
    assert_eq!(at(160).len(), 2);
    assert!(at(160).contains(&(
        "step.develop.model".to_string(),
        "sonnet".to_string(),
        "brian".to_string()
    )));
    // After the first expires only the other remains; expired ones are not in force even before cleanup.
    assert_eq!(
        at(350),
        vec![(
            "step.test.max_rounds".to_string(),
            "5".to_string(),
            "brian".to_string()
        )]
    );
    // Only the one that expired at 300 is gone; the two expiring at 400 are still in force.
    assert_eq!(s.expire_overrides(350).unwrap(), 1);
    assert_eq!(s.overrides("web", 350).unwrap().len(), 1);
    assert_eq!(s.overrides("other", 350).unwrap().len(), 1);
}

#[test]
fn task_overrides_are_per_task() {
    let (_d, p) = db();
    let s = Store::open(&p).unwrap();
    s.set_task_override(3, "step.develop.model", "opus", "brian", 1)
        .unwrap();
    s.set_task_override(3, "skip", "test", "brian", 1).unwrap();
    s.set_task_override(4, "step.develop.model", "haiku", "brian", 1)
        .unwrap();
    assert_eq!(s.task_overrides(3).unwrap().len(), 2);
    assert_eq!(
        s.task_overrides(4).unwrap(),
        vec![("step.develop.model".to_string(), "haiku".to_string())]
    );
    assert_eq!(s.clear_task_override(3, "skip").unwrap(), 1);
    assert_eq!(s.task_overrides(3).unwrap().len(), 1);
}

#[test]
fn old_task_payloads_without_the_new_fields_still_replay() {
    // A v3-era database: the task events carry no step names, entries or pins.
    let (d, old_path) = db();
    {
        let mut c = Connection::open(&old_path).unwrap();
        migrations::migrate_to(&mut c, 3).unwrap();
        for (at, kind, payload) in [
            (1, "project_registered", "1\tdemo"),
            (2, "task_filed", "1\tt\tnormal\towner\t1\t0\t\t-\tbody"),
            (3, "admitted", "0"),
            (4, "step_succeeded", "0"),
            (5, "sent_back", "0\tred"),
            (6, "step_succeeded", "0"),
        ] {
            c.execute(
                "INSERT INTO events(at, kind, payload) VALUES (?1, ?2, ?3)",
                rusqlite::params![at, kind, payload],
            )
            .unwrap();
        }
    }
    let copy = d.path().join("copy.db");
    std::fs::copy(&old_path, &copy).unwrap();
    let s = Store::open(&copy).unwrap();
    s.verify().unwrap();
    let w = s.world().unwrap();
    let t = &w.tasks[&TaskId(0)];
    assert_eq!(t.state, State::Testing);
    assert_eq!(
        t.route,
        Some(Route::Flag),
        "explored + owner: the legacy rule"
    );
    let dump = s.projection_dump().unwrap().join("\n");
    assert!(
        dump.contains("|flag|"),
        "the projection has the route: {dump}"
    );
}

#[test]
fn reserved_numbers_start_above_the_floor_and_above_every_reservation() {
    let (_d, p) = db();
    let s = Store::open(&p).unwrap();
    // Above the base's highest (42), then above the other task's reservation.
    let a = s.reserve_number(1, "exit", 42, 99, 10).unwrap().unwrap();
    let b = s.reserve_number(2, "exit", 42, 99, 11).unwrap().unwrap();
    assert_eq!((a.number, b.number), (43, 44));
    // A floor above every reservation wins.
    let c = s.reserve_number(3, "exit", 60, 99, 12).unwrap().unwrap();
    assert_eq!(c.number, 61);
    // Kinds count separately.
    let d = s
        .reserve_number(1, "decision", 20, 9999, 13)
        .unwrap()
        .unwrap();
    assert_eq!(d.number, 21);
}

#[test]
fn a_task_keeps_its_number_and_a_released_number_is_never_reused() {
    let (_d, p) = db();
    let s = Store::open(&p).unwrap();
    let a = s.reserve_number(1, "exit", 42, 99, 10).unwrap().unwrap();
    // Asking again (a retry, a restart) returns the same row, even with a higher floor.
    let again = s.reserve_number(1, "exit", 70, 99, 20).unwrap().unwrap();
    assert_eq!(a, again);
    assert_eq!(s.release_numbers(1).unwrap(), 1);
    assert!(s.reservations(1).unwrap()[0].released);
    // The next task does not get the released number.
    let b = s.reserve_number(2, "exit", 42, 99, 30).unwrap().unwrap();
    assert_eq!(b.number, a.number + 1);
    // The ceiling is a refusal, not an overflow.
    assert_eq!(
        s.reserve_number(3, "exit", 98, 99, 40)
            .unwrap()
            .unwrap()
            .number,
        99
    );
    assert_eq!(s.reserve_number(4, "exit", 42, 99, 41).unwrap(), None);
}

#[test]
fn reservations_survive_a_restart_and_the_counter_goes_on_from_them() {
    let (_d, p) = db();
    {
        let s = Store::open(&p).unwrap();
        s.reserve_number(1, "exit", 42, 99, 10).unwrap();
        s.reserve_number(2, "exit", 42, 99, 11).unwrap();
        s.release_numbers(2).unwrap();
    }
    let s = Store::open(&p).unwrap();
    assert_eq!(s.all_reservations().unwrap().len(), 2);
    let c = s.reserve_number(3, "exit", 42, 99, 12).unwrap().unwrap();
    assert_eq!(c.number, 45);
}

#[test]
fn two_tasks_claimed_at_once_never_share_a_number() {
    let (_d, p) = db();
    let s = std::sync::Arc::new(Store::open(&p).unwrap());
    let hs: Vec<_> = (1..=8u64)
        .map(|t| {
            let s = s.clone();
            std::thread::spawn(move || {
                s.reserve_number(t, "exit", 42, 99, t)
                    .unwrap()
                    .unwrap()
                    .number
            })
        })
        .collect();
    let mut got: Vec<u32> = hs.into_iter().map(|h| h.join().unwrap()).collect();
    got.sort();
    got.dedup();
    assert_eq!(got.len(), 8);
}

#[test]
fn migration_12_applies_to_an_existing_database() {
    let (_d, p) = db();
    {
        let mut c = Connection::open(&p).unwrap();
        migrations::migrate_to(&mut c, 11).unwrap();
        assert_eq!(migrations::version(&c).unwrap(), 11);
    }
    let s = Store::open(&p).unwrap();
    s.verify().unwrap();
    assert!(s.progress_rows(None).unwrap().is_empty());
    drop(s);
    let c = Connection::open(&p).unwrap();
    assert_eq!(migrations::version(&c).unwrap(), migrations::SCHEMA_VERSION);
}

#[test]
fn live_progress_is_kept_per_run_and_dropped_when_not_open() {
    use mason_core::progress::{Figure, Progress, SubProgress};
    let (_d, p) = db();
    let s = Store::open(&p).unwrap();
    let mut a = Progress::new(7, "work", 100);
    a.output("t0r7-cmd", 0, b"x\ny\n", 150);
    a.offer_history_total(Some((0, 9)), 0);
    a.subs.push(SubProgress {
        name: "unit".into(),
        unit: "tests".into(),
        done: Figure::reported(3),
        total: Figure::reported(8),
    });
    s.put_progress(0, &a).unwrap();
    // The log lines waiting to be pushed and the unfinished line are not stored.
    a.pending_log.clear();
    a.partial.clear();
    s.put_progress(1, &Progress::new(8, "work", 100)).unwrap();
    let rows = s.progress_rows(Some(0)).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].1, a,
        "what was written comes back, marks and subs included"
    );
    // Written again, it is replaced, not added.
    a.output("t0r7-cmd", 4, b"z\n", 200);
    s.put_progress(0, &a).unwrap();
    assert_eq!(s.progress_rows(None).unwrap().len(), 2);
    assert_eq!(s.progress_rows(Some(0)).unwrap()[0].1.output.lines, 3);
    // At daemon start only the open runs keep theirs.
    assert_eq!(s.drop_progress_except(vec![8]).unwrap(), 1);
    let left = s.progress_rows(None).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].1.run, 8);
    s.drop_progress(8).unwrap();
    assert!(s.progress_rows(None).unwrap().is_empty());
}

#[test]
fn history_keeps_final_counts_per_project_and_step() {
    let (_d, p) = db();
    let s = Store::open(&p).unwrap();
    for (run, done) in [(1u64, 10u64), (2, 20), (3, 30)] {
        s.record_history("web", 0, run, "work", done, done + 1, run * 1000, 5)
            .unwrap();
    }
    s.record_history("api", 1, 4, "work", 99, 99, 0, 6).unwrap();
    s.record_history("web", 0, 5, "check", 7, 7, 0, 7).unwrap();
    assert_eq!(
        s.history_samples("web", "work").unwrap(),
        vec![(10, 11), (20, 21), (30, 31)]
    );
    assert_eq!(s.history_samples("api", "work").unwrap(), vec![(99, 99)]);
    assert_eq!(
        s.history_gaps("web", "work").unwrap(),
        vec![1000, 2000, 3000]
    );
    // A run recorded twice counts once.
    s.record_history("web", 0, 1, "work", 10, 11, 0, 9).unwrap();
    assert_eq!(s.history_samples("web", "work").unwrap().len(), 3);
}

#[test]
fn migration_14_applies_to_an_existing_database() {
    let (_d, p) = db();
    {
        let mut c = Connection::open(&p).unwrap();
        migrations::migrate_to(&mut c, 13).unwrap();
        assert_eq!(migrations::version(&c).unwrap(), 13);
    }
    let s = Store::open(&p).unwrap();
    assert!(s.launches(1).unwrap().is_empty());
    drop(s);
    let c = Connection::open(&p).unwrap();
    assert_eq!(migrations::version(&c).unwrap(), migrations::SCHEMA_VERSION);
}

#[test]
fn launches_are_kept_per_run_in_order_and_survive_reopen() {
    use mason_store::LaunchRow;
    let (_d, p) = db();
    let s = Store::open(&p).unwrap();
    let row = |run, task, kind: &str, model: &str| LaunchRow {
        run,
        task,
        step: "develop".into(),
        kind: kind.into(),
        tier: "deep".into(),
        level: "medium".into(),
        model: model.into(),
        provider: "claude".into(),
        ..Default::default()
    };
    assert_eq!(s.record_launch(row(7, 1, "launch", "opus")).unwrap(), 1);
    assert_eq!(s.record_launch(row(7, 1, "retry", "fable")).unwrap(), 2);
    assert_eq!(s.record_launch(row(8, 2, "launch", "haiku")).unwrap(), 1);
    s.set_reported_model(7, "claude-fable-5-1").unwrap();
    drop(s);
    let s = Store::open(&p).unwrap();
    let l = s.launches(7).unwrap();
    assert_eq!(
        l.iter()
            .map(|r| (r.seq, r.kind.as_str(), r.model.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, "launch", "opus"), (2, "retry", "fable")]
    );
    assert_eq!(l[0].reported_model, "");
    assert_eq!(l[1].reported_model, "claude-fable-5-1");
    assert_eq!(s.launches_for_task(2).unwrap().len(), 1);
    assert_eq!(s.launches_for_task(1).unwrap().len(), 2);
    s.verify().unwrap();
}

#[test]
fn host_losses_are_kept_per_task_and_survive_reopen() {
    use mason_store::HostLossRow;
    let (_d, p) = db();
    {
        let mut c = Connection::open(&p).unwrap();
        migrations::migrate_to(&mut c, 19).unwrap();
    }
    let s = Store::open(&p).unwrap();
    let row = |id: &str, task, at| HostLossRow {
        host_id: id.into(),
        task,
        run: 3,
        step: "test".into(),
        lost_at_ms: at,
        detail: "host pid 1 gone".into(),
        retried: true,
    };
    s.record_host_loss(row("t1r3-test", 1, 100)).unwrap();
    s.record_host_loss(row("t2r3-test", 2, 200)).unwrap();
    s.set_host_loss_retried("t2r3-test", false).unwrap();
    drop(s);
    let s = Store::open(&p).unwrap();
    assert_eq!(s.host_losses_for_task(1).unwrap().len(), 1);
    assert!(!s.host_losses_for_task(2).unwrap()[0].retried);
    assert_eq!(s.host_losses_since(150).unwrap().len(), 1);
    assert_eq!(s.host_losses_since(0).unwrap().len(), 2);
}

#[test]
fn migration_16_applies_on_15_and_usage_survives_reopening() {
    let (_d, p) = db();
    {
        let mut c = Connection::open(&p).unwrap();
        migrations::migrate_to(&mut c, 15).unwrap();
        assert_eq!(migrations::version(&c).unwrap(), 15);
    }
    let s = Store::open(&p).unwrap();
    s.put_usage_latest("main", 1000, "{\"a\":1}").unwrap();
    drop(s);
    let s = Store::open(&p).unwrap();
    assert_eq!(
        s.usage_latest().unwrap(),
        vec![("main".to_string(), 1000, "{\"a\":1}".to_string())]
    );
    drop(s);
    let c = Connection::open(&p).unwrap();
    assert_eq!(migrations::version(&c).unwrap(), migrations::SCHEMA_VERSION);
}

#[test]
fn migration_18_applies_on_17_and_holds_survive_reopening() {
    let (_d, p) = db();
    {
        let mut c = Connection::open(&p).unwrap();
        migrations::migrate_to(&mut c, 17).unwrap();
        assert_eq!(migrations::version(&c).unwrap(), 17);
    }
    let s = Store::open(&p).unwrap();
    let h = mason_store::AccountHold {
        account: "main".into(),
        kind: "limit".into(),
        why: "weekly limit".into(),
        since_ms: 5,
        until_ms: Some(900),
        backoff_ms: 0,
        probe_ms: 0,
        by: "pipeline".into(),
    };
    s.put_account_hold(h.clone()).unwrap();
    assert!(s.note_warning("main", "week", 900, 6).unwrap());
    assert!(!s.note_warning("main", "week", 900, 7).unwrap());
    assert!(s.note_warning("main", "week", 901, 8).unwrap());
    drop(s);
    let s = Store::open(&p).unwrap();
    assert_eq!(s.account_holds().unwrap(), vec![h]);
    assert_eq!(s.warnings().unwrap().len(), 2);
    assert!(s.clear_account_hold("main").unwrap());
    assert!(!s.clear_account_hold("main").unwrap());
    assert!(s.account_holds().unwrap().is_empty());
}

fn reading(at: u64, u: Option<f64>, resets: u64, state: &str) -> mason_store::UsageReading {
    mason_store::UsageReading {
        account: "main".into(),
        window: "5h".into(),
        at,
        utilization: u,
        resets_at: Some(resets),
        state: state.into(),
    }
}

#[test]
fn readings_are_thinned_to_one_per_five_minutes_and_pruned_after_eight_days() {
    let (_d, p) = db();
    let s = Store::open(&p).unwrap();
    let t0 = 10_000_000u64;
    assert!(s.record_reading(reading(t0, Some(0.1), 5, "ok")).unwrap());
    // Within five minutes, same state and reset: dropped.
    assert!(!s
        .record_reading(reading(t0 + 60, Some(0.2), 5, "ok"))
        .unwrap());
    // A changed state or reset is kept at once.
    assert!(s
        .record_reading(reading(t0 + 70, Some(0.2), 5, "warning"))
        .unwrap());
    assert!(s
        .record_reading(reading(t0 + 80, Some(0.0), 99, "warning"))
        .unwrap());
    // Five minutes after the last kept one.
    assert!(s
        .record_reading(reading(t0 + 80 + 300, Some(0.3), 99, "warning"))
        .unwrap());
    assert_eq!(s.readings_since(0).unwrap().len(), 4);
    // A write more than 8 days later drops the old ones.
    let late = t0 + 9 * 86400;
    assert!(s.record_reading(reading(late, None, 7, "ok")).unwrap());
    let left = s.readings_since(0).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].utilization, None);
}

#[test]
fn usage_runs_keep_only_reported_figures_and_each_attempt_apart() {
    let (_d, p) = db();
    let s = Store::open(&p).unwrap();
    let run = |attempt: &str, input: Option<u64>| mason_store::UsageRun {
        attempt: attempt.into(),
        task: 3,
        project: "demo".into(),
        step: "develop".into(),
        account: "main".into(),
        model: Some("opus".into()),
        input,
        at: 50,
        ..Default::default()
    };
    s.record_usage_run(run("k", Some(10))).unwrap();
    s.record_usage_run(run("ku1", None)).unwrap();
    let rows = s.usage_runs_since(0).unwrap();
    assert_eq!(rows.len(), 2);
    let ku1 = rows.iter().find(|r| r.attempt == "ku1").unwrap();
    assert_eq!(ku1.input, None);
    assert_eq!(ku1.cost_usd, None);
    assert_eq!(
        rows.iter().find(|r| r.attempt == "k").unwrap().input,
        Some(10)
    );
}

#[test]
fn migration_17_applies_to_a_version_16_database() {
    use mason_core::introspect::{signature, Attempt};
    let (_d, p) = db();
    {
        let mut c = Connection::open(&p).unwrap();
        migrations::migrate_to(&mut c, 16).unwrap();
        assert_eq!(migrations::version(&c).unwrap(), 16);
    }
    let s = Store::open(&p).unwrap();
    assert!(s.findings().unwrap().is_empty());
    assert!(s.schedules().unwrap().is_empty());
    assert!(s.triage().unwrap().is_empty());
    let at = |reason: &str, retry_n, at| Attempt {
        task: 3,
        project: "web".into(),
        step: "develop".into(),
        outcome: "fail".into(),
        reason: reason.into(),
        signature: signature(reason),
        retry_n,
        at,
    };
    s.record_attempt(&at("unknown model m-1", 1, 100)).unwrap();
    s.record_attempt(&at("unknown model m-2", 2, 200)).unwrap();
    let mut other = at("x", 1, 300);
    other.project = "api".into();
    s.record_attempt(&other).unwrap();
    drop(s);
    let s = Store::open(&p).unwrap();
    // the window is half open, and the project limits it
    assert_eq!(s.attempts_between(Some("web"), 0, 200).unwrap().len(), 1);
    assert_eq!(s.attempts_between(Some("web"), 0, 201).unwrap().len(), 2);
    assert_eq!(s.attempts_between(None, 0, 1000).unwrap().len(), 3);
    let both = s.attempts_between(Some("web"), 0, 1000).unwrap();
    assert_eq!(both[0].signature, both[1].signature);
    assert_eq!(s.attempt_projects().unwrap(), vec!["api", "web"]);
    s.verify().unwrap();
    drop(s);
    let c = Connection::open(&p).unwrap();
    assert_eq!(migrations::version(&c).unwrap(), migrations::SCHEMA_VERSION);
}

#[test]
fn findings_take_one_verdict_and_keep_it() {
    use mason_store::FindingRow;
    let (_d, p) = db();
    let s = Store::open(&p).unwrap();
    let id = s
        .add_finding(&FindingRow {
            rule: "retry_loop".into(),
            subject: "sig".into(),
            project: "web".into(),
            metric: "retry_loop_rate".into(),
            title: "t".into(),
            task: 9,
            before_num: 4.0,
            before_den: 5.0,
            before_n: 5,
            at: 10,
            ..Default::default()
        })
        .unwrap();
    assert!(s
        .close_finding(id, "worked", "this fix worked", (0.0, 8.0, 8), 50, 60)
        .unwrap());
    // a second verdict does not replace the first
    assert!(!s
        .close_finding(id, "didnt_help", "later", (4.0, 8.0, 8), 50, 70)
        .unwrap());
    let f = &s.findings().unwrap()[0];
    assert_eq!(
        (f.verdict.as_str(), f.after_n, f.landed_at, f.closed_at),
        ("worked", 8, 50, 60)
    );
    assert_eq!((f.before_num, f.before_den), (4.0, 5.0));
}

#[test]
fn schedules_remember_their_baseline_and_triage_waits_for_review() {
    use mason_store::{ScheduleRow, TriageRow};
    let (_d, p) = db();
    let s = Store::open(&p).unwrap();
    let mut row = ScheduleRow {
        name: "base".into(),
        project: "web".into(),
        kind: "base-health".into(),
        every_ms: 1000,
        ..Default::default()
    };
    s.put_schedule(&row).unwrap();
    assert_eq!(s.schedule("base").unwrap().unwrap().baseline, None);
    row.baseline = Some(vec![]);
    row.runs = 1;
    s.put_schedule(&row).unwrap();
    assert_eq!(s.schedule("base").unwrap().unwrap().baseline, Some(vec![]));
    row.baseline = Some(vec!["a::b".into(), "c".into()]);
    s.put_schedule(&row).unwrap();
    assert_eq!(
        s.schedule("base").unwrap().unwrap().baseline,
        Some(vec!["a::b".to_string(), "c".to_string()])
    );
    assert!(s.schedule("nope").unwrap().is_none());

    let id = s
        .add_triage(&TriageRow {
            task: 1,
            question: "q".into(),
            action: "answer".into(),
            answer: "a".into(),
            evidence: "e".into(),
            status: "queued".into(),
            at: 5,
            ..Default::default()
        })
        .unwrap();
    assert!(s.review_triage(id, false).unwrap());
    assert!(!s.review_triage(id, true).unwrap(), "reviewed once");
    let t = &s.triage().unwrap()[0];
    assert_eq!(
        (t.status.as_str(), t.verdict.as_str()),
        ("reviewed", "disagree")
    );
}

#[test]
fn force_skips_and_expired_switches_are_read_from_what_is_recorded() {
    let (_d, p) = db();
    let s = Store::open(&p).unwrap();
    s.add_debt(1, "web", "suite:slow", "late", "me", false, 100)
        .unwrap();
    s.add_debt(2, "web", "suite:slow", "late", "me", false, 200)
        .unwrap();
    s.add_debt(3, "web", "step:review", "late", "me", false, 200)
        .unwrap();
    s.add_debt(4, "api", "suite:slow", "late", "me", false, 200)
        .unwrap();
    assert_eq!(
        s.force_skips_since("web", 150).unwrap(),
        vec![("slow".to_string(), 1)]
    );
    assert_eq!(
        s.force_skips_since("web", 0).unwrap(),
        vec![("slow".to_string(), 2)]
    );
    assert_eq!(s.debts_between(0, 150).unwrap().len(), 1);
    s.add_override("web", "step.develop.model", "sonnet", "brian", 10, 20)
        .unwrap();
    assert_eq!(s.expired_switches(15).unwrap().len(), 0);
    assert_eq!(s.expired_switches(25).unwrap().len(), 1);
}

#[test]
fn tracker_overrides_and_the_relationship_are_kept_and_dropped() {
    let s = Store::open_in_memory().unwrap();
    assert_eq!(s.tracker_relationship("p", 7).unwrap(), None);
    s.set_tracker_relationship("p", 7, &[]).unwrap();
    assert_eq!(s.tracker_relationship("p", 7).unwrap(), Some(Default::default()));
    s.set_tracker_relationship("p", 7, &[3, 4]).unwrap();
    assert_eq!(
        s.tracker_relationship("p", 7).unwrap().unwrap().into_iter().collect::<Vec<_>>(),
        vec![3, 4]
    );
    s.put_tracker_override("p", 7, 30, "release", 1).unwrap();
    s.put_tracker_override("p", 7, 90, "add", 2).unwrap();
    assert_eq!(s.tracker_overrides("p").unwrap().len(), 2);
    s.drop_tracker_overrides("p", 7, Some("release"), None).unwrap();
    assert_eq!(s.tracker_overrides("p").unwrap(), vec![(7, 90, "add".to_string())]);
    s.drop_tracker_overrides("p", 7, None, Some(90)).unwrap();
    assert!(s.tracker_overrides("p").unwrap().is_empty());
}
