// SPDX-License-Identifier: GPL-3.0-or-later

//! Finished step hosts leave the registry (s98): a closed run's hosts move after the grace
//! period, an open run's stay, the logs stay readable, and old ones are deleted after the
//! retention period. The clock is injected: `pass` takes the time.

mod support;
use mason_core::*;
use mason_daemon::daemon::now_ms;
use mason_daemon::hostarchive::pass;
use mason_daemon::http::render_logs;
use mason_step_host::registry::{HostState, Registry};
use serde_json::json;
use support::*;

const DAY: u64 = 86_400_000;

fn live_ids(d: &std::sync::Arc<mason_daemon::Daemon>, t: TaskId) -> Vec<String> {
    let p = format!("t{}r", t.0);
    Registry::new(d.cfg.hosts_dir())
        .scan_where(|n| n.starts_with(&p))
        .unwrap()
        .into_iter()
        .map(|h| h.info.id)
        .collect()
}

fn landed(s: &Scratch) -> (std::sync::Arc<mason_daemon::Daemon>, TaskId) {
    let d = s.start_with(|c| {
        c.host_grace = std::time::Duration::from_secs(24 * 3600);
        c.retention_days = 10;
        c.sweep_interval = std::time::Duration::from_secs(3600);
    });
    let t = file_task(&d, "Lands");
    wait_state(&d, t, State::Landed, 60);
    wait_for("the job", 15, || d.job_count() == 0);
    (d, t)
}

#[test]
fn a_closed_runs_hosts_move_after_a_day_and_the_logs_still_show_them() {
    let s = Scratch::new("true", happy_script());
    let (d, t) = landed(&s);
    let ids = live_ids(&d, t);
    assert!(!ids.is_empty(), "the task ran hosts");
    let before = render_logs(&d, t.0);
    assert!(before.contains("=== step run"));
    let w = d.world().unwrap();

    // Recent: stays.
    let r = pass(&d.cfg, &w, now_ms() + 3_600_000);
    assert!(r.moved.is_empty(), "{r:?}");
    assert_eq!(live_ids(&d, t), ids);

    // A day on: moves, with the input files.
    let r = pass(&d.cfg, &w, now_ms() + DAY + 60_000);
    assert_eq!(r.failed, 0, "{r:?}");
    assert!(!r.moved.is_empty(), "{r:?}");
    assert!(live_ids(&d, t).is_empty(), "{:?}", live_ids(&d, t));
    let arch = d.cfg.archived_hosts_dir(t.0);
    for id in &ids {
        assert!(arch.join(id).join("journal").exists(), "{id}");
    }
    let leftovers: Vec<_> = std::fs::read_dir(d.cfg.hosts_dir())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with(&format!("t{}r", t.0)))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // The log reads the same from the new place.
    assert_eq!(render_logs(&d, t.0), before);
    let sess = mason_daemon::attention::session_for(&d, t).unwrap();
    assert!(sess["worktree"].is_string());

    // Nothing moves twice, nothing fails.
    let r = pass(&d.cfg, &w, now_ms() + DAY + 120_000);
    assert!(r.moved.is_empty() && r.failed == 0, "{r:?}");
}

#[test]
fn retention_deletes_moved_hosts_and_keeps_the_task_logs() {
    let s = Scratch::new("true", happy_script());
    let (d, t) = landed(&s);
    let w = d.world().unwrap();
    let t0 = now_ms();
    pass(&d.cfg, &w, t0 + 2 * DAY);
    let arch = d.cfg.archived_hosts_dir(t.0);
    assert!(arch.exists());
    std::fs::write(d.cfg.task_dir(t.0).join("keep.log"), "x").unwrap();

    // Inside retention (10 days): kept.
    let r = pass(&d.cfg, &w, t0 + 8 * DAY);
    assert!(r.deleted.is_empty(), "{r:?}");
    assert!(arch.exists());

    // Past it: deleted, the task's own directory stays.
    let r = pass(&d.cfg, &w, t0 + 12 * DAY);
    assert!(!r.deleted.is_empty(), "{r:?}");
    assert!(!arch.exists());
    assert!(d.cfg.task_dir(t.0).join("keep.log").exists());
}

#[test]
fn retention_zero_keeps_them_for_ever() {
    let s = Scratch::new("true", happy_script());
    let d = s.start_with(|c| {
        c.host_grace = std::time::Duration::from_secs(24 * 3600);
        c.retention_days = 0;
        c.sweep_interval = std::time::Duration::from_secs(3600);
    });
    let t = file_task(&d, "Lands");
    wait_state(&d, t, State::Landed, 60);
    wait_for("the job", 15, || d.job_count() == 0);
    let w = d.world().unwrap();
    let t0 = now_ms();
    pass(&d.cfg, &w, t0 + 2 * DAY);
    let r = pass(&d.cfg, &w, t0 + 5000 * DAY);
    assert!(r.deleted.is_empty(), "{r:?}");
    assert!(d.cfg.archived_hosts_dir(t.0).exists());
}

#[test]
fn an_open_runs_hosts_stay_where_reattach_looks() {
    let develop = json!([{"sh": TASK_SCRIPT_COMMIT}, {"sleep_ms": 60000}]);
    let s = Scratch::new(
        "true",
        json!({"steps": {"explore": [explore_turn()], "develop": [develop]}}),
    );
    let d = s.start_with(|c| {
        c.host_grace = std::time::Duration::from_secs(24 * 3600);
        c.sweep_interval = std::time::Duration::from_secs(3600);
    });
    let t = file_task(&d, "Long running");
    wait_state(&d, t, State::Developing, 60);
    wait_for("a running host", 30, || {
        Registry::new(d.cfg.hosts_dir())
            .scan()
            .unwrap()
            .iter()
            .any(|h| h.state == HostState::Running && h.info.id.starts_with(&format!("t{}r", t.0)))
    });
    let open = d.world().unwrap().open_run(t).unwrap();
    let key = format!("t{}r{}", t.0, open.0);
    let before = live_ids(&d, t);
    // A session directory made after the run began belongs to it, however old the clock says.
    let a = d.cfg.agent_state_dir().join("1-1");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::write(a.join("mcp.json"), "{}").unwrap();

    // Earlier runs of this task (the explore step) have closed; this one has not.
    let w = d.world().unwrap();
    let r = pass(&d.cfg, &w, now_ms() + 30 * DAY);
    let after = live_ids(&d, t);
    assert!(
        after.iter().any(|i| i.starts_with(&key)),
        "{after:?} lost the open run's host: {r:?}"
    );
    assert!(
        r.moved.iter().all(|i| !i.starts_with(&key)),
        "moved an open run's host: {r:?}"
    );
    assert!(
        before.iter().filter(|i| i.starts_with(&key)).count()
            == after.iter().filter(|i| i.starts_with(&key)).count()
    );
    // What reattach uses still finds it.
    assert!(mason_adapters::hosted::latest_invocation(&d.cfg.hosts_dir(), &key).is_some());
    assert!(
        a.exists(),
        "a session directory of the open run was removed"
    );
    kill_hosts(&s.data_dir);
}

#[test]
fn ended_sessions_config_directories_go() {
    let s = Scratch::new("true", happy_script());
    let (d, _t) = landed(&s);
    let w = d.world().unwrap();
    let a = d.cfg.agent_state_dir();
    std::fs::create_dir_all(a.join("4242-7")).unwrap();
    std::fs::create_dir_all(a.join("notes")).unwrap();
    let r = pass(&d.cfg, &w, now_ms() + 3_600_000);
    assert!(r.sessions.is_empty(), "inside the grace period: {r:?}");
    let r = pass(&d.cfg, &w, now_ms() + 2 * DAY);
    assert!(r.sessions.contains(&"4242-7".to_string()), "{r:?}");
    assert!(!a.join("4242-7").exists());
    assert!(a.join("notes").exists(), "only <pid>-<n> directories go");
}

#[test]
fn a_move_that_fails_is_counted_and_retried() {
    let s = Scratch::new("true", happy_script());
    let (d, t) = landed(&s);
    let w = d.world().unwrap();
    // `hosts` is a file where a directory should be: nothing can be moved there.
    std::fs::write(d.cfg.task_dir(t.0).join("hosts"), "in the way").unwrap();
    let r = pass(&d.cfg, &w, now_ms() + 2 * DAY);
    assert!(r.moved.is_empty() && r.failed > 0, "{r:?}");
    assert!(!live_ids(&d, t).is_empty(), "left where they were");
    std::fs::remove_file(d.cfg.task_dir(t.0).join("hosts")).unwrap();
    let r = pass(&d.cfg, &w, now_ms() + 2 * DAY);
    assert!(!r.moved.is_empty() && r.failed == 0, "{r:?}");
}

#[test]
fn retention_days_is_read_from_daemon_toml() {
    use mason_daemon::config::{parse_daemon_port, parse_retention_days};
    assert_eq!(parse_retention_days("").unwrap(), None);
    assert_eq!(
        parse_retention_days("[daemon]\nretention_days = 30\n").unwrap(),
        Some(30)
    );
    assert_eq!(
        parse_retention_days("[daemon]\nretention_days = 0\n").unwrap(),
        Some(0)
    );
    assert!(parse_retention_days("[daemon]\nretention_days = -1\n").is_err());
    assert!(parse_retention_days("[daemon]\nretention_days = \"x\"\n").is_err());
    // The port reader accepts the key rather than calling it unknown.
    assert_eq!(
        parse_daemon_port("[daemon]\nport = 9\nretention_days = 5\n").unwrap(),
        Some(9)
    );
}

fn fake_host(root: &std::path::Path, id: &str, journal: &str) {
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("host.kv"),
        format!(
            "id={id}\nhost_pid=1\nchild_pid=1\nsock=/nonexistent\nstarted_ms=1\nheartbeat_ms=1\n"
        ),
    )
    .unwrap();
    std::fs::write(dir.join("journal"), journal).unwrap();
}

#[test]
fn log_signature_names_only_the_tasks_hosts_and_follows_its_journal() {
    use mason_daemon::http::log_signature;
    use std::os::unix::fs::PermissionsExt;
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let live = d.cfg.hosts_dir();
    fake_host(&live, "t7r0-1", "a");
    fake_host(&live, "t70r0-1", "b");
    fake_host(&live, "t700r0-1", "c");
    let arch = d.cfg.archived_hosts_dir(7);
    fake_host(&arch, "t7r1-1", "dd");

    let ids = |t| -> Vec<String> { log_signature(&d, t).into_iter().map(|x| x.0).collect() };
    assert_eq!(ids(7), vec!["t7r0-1", "t7r1-1"]);
    assert_eq!(ids(70), vec!["t70r0-1"]);

    // Another task's host that cannot be read is never opened.
    let other = live.join("t8r0-1");
    fake_host(&live, "t8r0-1", "x");
    std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o000)).unwrap();
    let before = log_signature(&d, 7);
    std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(before.len(), 2);

    // The journal growing changes the signature.
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(live.join("t7r0-1/journal"))
        .unwrap()
        .write_all(b"more")
        .unwrap();
    assert_ne!(log_signature(&d, 7), before);
}
