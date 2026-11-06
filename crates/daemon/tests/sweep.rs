// SPDX-License-Identifier: GPL-3.0-or-later

//! The reconcile sweep: plant things the event path "missed" by writing to the
//! store behind the daemon's back, and check the sweep fixes them *and files
//! a bug*, but only once a condition has persisted across two passes.

mod support;
use mason_core::*;
use mason_daemon::sweep::sweep_once;
use serde_json::json;
use support::*;

fn raw(d: &std::sync::Arc<mason_daemon::Daemon>, kind: EventKind) -> Vec<Effect> {
    // Straight to the store: no effects are carried out, as if the event path dropped them.
    d.store
        .append(Event {
            at: Timestamp(1),
            kind,
        })
        .unwrap()
        .effects
}

fn plain_file(d: &std::sync::Arc<mason_daemon::Daemon>) -> TaskId {
    let pid = d.project_id_by_name("scratch").unwrap();
    let fx = raw(
        d,
        EventKind::TaskFiled {
            project: pid,
            title: "missed".into(),
            body: String::new(),
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            explored: false,
            proposed: false,
            blockers: vec![],
            not_before: None,
        },
    );
    fx.iter()
        .find_map(|e| {
            if let Effect::TaskCreated { task } = e {
                Some(*task)
            } else {
                None
            }
        })
        .unwrap()
}

#[test]
fn a_task_nobody_admitted_is_admitted_and_a_bug_is_filed() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    let d = s.start();
    let t = plain_file(&d); // queued, claimable, no admission happened
    assert_eq!(state_of(&d, t), State::Queued);

    assert!(
        sweep_once(&d).is_empty(),
        "first sighting: it might just be in flight"
    );
    assert_eq!(state_of(&d, t), State::Queued);
    assert_eq!(d.store.bugs().unwrap().len(), 0);

    assert_eq!(sweep_once(&d), vec!["missed_admit"], "second sighting: act");
    let bugs = d.store.bugs().unwrap();
    assert_eq!(bugs.len(), 1);
    assert_eq!(bugs[0].1, "missed_admit");
    assert!(bugs[0].2.contains(&format!("task {t}")), "{bugs:?}");
    wait_state(&d, t, State::Landed, 90); // and it then runs normally
}

#[test]
fn a_step_with_no_job_gets_one_and_a_bug_is_filed() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    let d = s.start();
    let t = plain_file(&d);
    raw(&d, EventKind::Admitted { task: t }); // the step opened, but the job was never started
    assert_eq!(state_of(&d, t), State::Exploring);
    assert_eq!(d.job_count(), 0);
    sweep_once(&d);
    assert_eq!(d.job_count(), 0, "not yet");
    assert_eq!(sweep_once(&d), vec!["missed_job"]);
    wait_state(&d, t, State::Landed, 90);
    assert_eq!(
        d.store
            .bugs()
            .unwrap()
            .iter()
            .filter(|b| b.1 == "missed_job")
            .count(),
        1
    );
}

#[test]
fn a_step_host_left_running_for_a_closed_step_is_stopped() {
    let develop = json!([{"sh": TASK_SCRIPT_COMMIT}, {"sleep_ms": 60000}]);
    let s = Scratch::new(
        "true",
        json!({"steps": {"explore": [explore_turn()], "develop": [develop]}}),
    );
    let d = s.start();
    let t = file_task(&d, "Orphan");
    wait_state(&d, t, State::Developing, 60);
    wait_for("the agent's host", 30, || {
        mason_step_host::registry::Registry::new(d.cfg.hosts_dir())
            .scan()
            .unwrap()
            .iter()
            .any(|h| h.state == mason_step_host::registry::HostState::Running)
    });
    // Close the step run behind the daemon's back, so its stop effect never happens.
    raw(
        &d,
        EventKind::Canceled {
            task: t,
            delete_branch: false,
        },
    );
    sweep_once(&d);
    assert_eq!(sweep_once(&d), vec!["orphan_host"]);
    wait_for("the orphan to be stopped", 20, || {
        mason_step_host::registry::Registry::new(d.cfg.hosts_dir())
            .scan()
            .unwrap()
            .iter()
            .all(|h| h.state != mason_step_host::registry::HostState::Running)
    });
}

#[test]
fn a_healthy_daemon_files_no_bugs_across_many_passes() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    let d = s.start();
    let t = file_task(&d, "Normal work");
    let mut passes = 0;
    wait_for("the task to land", 90, || {
        passes += 1;
        assert!(
            sweep_once(&d).is_empty(),
            "the sweep must not interfere with normal progress (pass {passes})"
        );
        state_of(&d, t) == State::Landed
    });
    for _ in 0..12 {
        assert!(sweep_once(&d).is_empty()); // includes the every-tenth-pass projection check
    }
    assert!(d.store.bugs().unwrap().is_empty());
}

#[test]
fn drifted_projections_are_detected_and_rebuilt() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    let d = s.start();
    let _ = plain_file(&d);
    // Damage a projection behind the store's back.
    let raw_db = rusqlite::Connection::open(d.cfg.store_path()).unwrap();
    raw_db
        .execute("UPDATE tasks SET state = 'landed'", [])
        .unwrap();
    drop(raw_db);
    let mut filed = vec![];
    for _ in 0..10 {
        filed.extend(sweep_once(&d));
    }
    assert!(filed.contains(&"projection_drift".to_string()), "{filed:?}");
    d.store.verify().unwrap();
}
