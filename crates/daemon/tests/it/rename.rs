// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason project rename`: a project keeps its id and its history under a new name.

use crate::support;
use mason_core::*;
use support::*;

fn file_and_cancel(d: &std::sync::Arc<mason_daemon::Daemon>, pid: ProjectId) -> TaskId {
    let a = d
        .emit(EventKind::TaskFiled {
            project: pid,
            title: "old work".into(),
            body: String::new(),
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            explored: false,
            proposed: true,
            blockers: vec![],
            not_before: None,
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
    t
}

fn count(d: &std::sync::Arc<mason_daemon::Daemon>, sql: &'static str, arg: String) -> i64 {
    d.store
        .read(|c| c.query_row(sql, [arg], |r| r.get::<_, i64>(0)))
        .unwrap()
}

#[test]
fn a_rename_keeps_the_id_and_every_row() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let pid = d.project_id_by_name("scratch").unwrap();
    let task = file_and_cancel(&d, pid);
    d.store.put_suite_green("scratch", "unit", "abc", 3, 1).unwrap();
    d.store.add_pause("scratch", "all", "owner", 1, u64::MAX / 4).unwrap();
    d.store
        .write(|c| {
            c.execute(
                "INSERT INTO schedules(name, project, kind, every_ms) VALUES ('analyst:scratch','scratch','analyst',1000)",
                [],
            )?;
            c.execute(
                "INSERT INTO alerts(key, kind, text, since) VALUES ('paused:scratch:all','paused','x',1)",
                [],
            )?;
            c.execute("INSERT OR REPLACE INTO meta(key, value) VALUES ('def:scratch','h')", [])
        })
        .unwrap();

    // Without yes: a plan, and nothing changes.
    let plan = d.rename_project("scratch", "renamed", false).unwrap();
    assert_eq!(plan["result"], "plan");
    assert!(d.project_id_by_name("scratch").is_some());
    assert!(s.config_dir.join("projects/scratch.toml").exists());

    let r = d.rename_project("scratch", "renamed", true).unwrap();
    assert_eq!(r["result"], "renamed");
    assert!(!s.config_dir.join("projects/scratch.toml").exists());
    assert!(s.config_dir.join("projects/renamed.toml").exists());
    assert_eq!(d.project_id_by_name("renamed"), Some(pid));
    assert!(d.project_id_by_name("scratch").is_none());
    let names: Vec<String> = d.projects().values().map(|p| p.id.clone()).collect();
    assert_eq!(names, vec!["renamed".to_string()]);
    let w = d.world().unwrap();
    assert_eq!(w.projects[&pid].name, "renamed");
    assert_eq!(w.tasks[&task].project, pid);

    assert_eq!(d.store.suite_green("renamed", "unit", "abc").unwrap(), Some(3));
    assert_eq!(d.store.suite_green("scratch", "unit", "abc").unwrap(), None);
    assert_eq!(count(&d, "SELECT count(*) FROM pauses WHERE project = ?1", "renamed".into()), 1);
    assert_eq!(count(&d, "SELECT count(*) FROM schedules WHERE name = ?1", "analyst:renamed".into()), 1);
    assert_eq!(count(&d, "SELECT count(*) FROM schedules WHERE project = ?1", "scratch".into()), 0);
    assert_eq!(count(&d, "SELECT count(*) FROM alerts WHERE key = ?1", "paused:renamed:all".into()), 1);
    assert_eq!(count(&d, "SELECT count(*) FROM meta WHERE key = ?1", "def:renamed".into()), 1);
    d.store.verify().unwrap();

    // And back again.
    let r = d.rename_project("renamed", "scratch", true).unwrap();
    assert_eq!(r["result"], "renamed");
    assert_eq!(d.project_id_by_name("scratch"), Some(pid));
    assert_eq!(d.store.suite_green("scratch", "unit", "abc").unwrap(), Some(3));
}

#[test]
fn a_rename_is_refused_when_something_is_in_the_way() {
    let s = Scratch::new("true", happy_script());
    s.write_project("other", "true", "");
    let d = s.start();
    let pid = d.project_id_by_name("scratch").unwrap();

    // NEW already registered.
    let e = d.rename_project("scratch", "other", true).unwrap_err();
    assert!(e.iter().any(|m| m.contains("already")), "{e:?}");
    // Not a valid id.
    assert!(d.rename_project("scratch", "Bad Name", true).is_err());
    // NEW.toml already there.
    std::fs::write(s.config_dir.join("projects/taken.toml"), "").unwrap();
    let e = d.rename_project("scratch", "taken", true).unwrap_err();
    assert!(e.iter().any(|m| m.contains("taken.toml")), "{e:?}");
    // An open task.
    let a = d
        .emit(EventKind::TaskFiled {
            project: pid,
            title: "open".into(),
            body: String::new(),
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            explored: false,
            proposed: true,
            blockers: vec![],
            not_before: None,
        })
        .unwrap();
    assert!(!a.effects.is_empty());
    let e = d.rename_project("scratch", "fresh", true).unwrap_err();
    assert!(e.iter().any(|m| m.contains("not landed or canceled")), "{e:?}");
    // Nothing moved.
    assert!(s.config_dir.join("projects/scratch.toml").exists());
    assert_eq!(d.project_id_by_name("scratch"), Some(pid));
}
