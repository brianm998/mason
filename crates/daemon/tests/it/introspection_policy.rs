// SPDX-License-Identifier: GPL-3.0-or-later

//! Introspection findings are proposed or queued by the owner's `[proposals.introspection]`
//! table, per severity and cost (s295), and the analyst's task is known by the schedule row that
//! recorded it, not by its title.

use crate::support;
use mason_core::introspect::{Finding, Stats};
use mason_core::proposal::{Cost, Impact};
use mason_core::*;
use mason_daemon::daemon::now_ms;
use mason_daemon::{introspect, Daemon};
use std::sync::Arc;
use support::lw::*;
use support::*;

const SPLIT: &str = "[proposals.introspection]\nlow_small = \"queued\"\n";
const LANDING: &str = "mode = \"direct\"\nwindow = \"0s\"\n";

fn finding(title: &str, impact: Impact, cost: Cost) -> Finding {
    Finding {
        rule: "analyst".into(),
        subject: title.to_lowercase(),
        project: "scratch".into(),
        title: title.into(),
        body: format!("{title}: what is wrong and why it matters."),
        evidence: "crates/daemon/src/sched.rs:120 ranks reviews below develop".into(),
        metric: "wait_share".into(),
        before: None,
        impact,
        cost,
    }
}

fn state(d: &Arc<Daemon>, t: TaskId) -> State {
    d.world().unwrap().tasks[&t].state
}

#[test]
fn a_low_small_finding_is_queued_and_a_high_large_one_proposed_under_a_split_table() {
    let w = world_with(LANDING, &|_| SPLIT.into(), &[]);
    let d = &w.d;
    let stats = Stats::default();
    let small = introspect::file_finding(
        d,
        finding("Reviews wait for a slot", Impact::Low, Cost::Small),
        &stats,
    )
    .unwrap()
    .unwrap();
    let large = introspect::file_finding(
        d,
        finding("Landing idles between batches", Impact::High, Cost::Large),
        &stats,
    )
    .unwrap()
    .unwrap();
    assert_ne!(state(d, small), State::Proposed);
    assert_eq!(state(d, large), State::Proposed);
    let row = d.store.proposal(small.0).unwrap().unwrap();
    assert_eq!(row.severity, "low");
    assert_eq!(row.decided_by, "proposals.introspection.low_small");
    let row = d.store.proposal(large.0).unwrap().unwrap();
    assert_eq!(row.severity, "high");
    assert_eq!(row.decided_by, "proposals.introspection.high_large");
}

#[test]
fn with_no_table_every_finding_is_proposed() {
    let w = world_with(LANDING, &|_| String::new(), &[]);
    let t = introspect::file_finding(
        &w.d,
        finding("Reviews wait for a slot", Impact::Low, Cost::Small),
        &Stats::default(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(state(&w.d, t), State::Proposed);
}

#[test]
fn a_renamed_analyst_task_is_still_read_and_another_task_with_its_title_is_not() {
    let w = world_with(LANDING, &|_| SPLIT.into(), &[]);
    let d = &w.d;
    let pid = d.project_id_by_name("scratch").unwrap();
    let analyst = introspect::file_analyst_task(d, pid).unwrap();
    d.emit(EventKind::TrackerEdited {
        task: analyst,
        title: "Weekly look at the pipeline".into(),
        body: "x".into(),
        priority: Priority::Low,
    })
    .unwrap();
    let result = r#"{"findings":[{"title":"Reviews wait for a slot","body":"Review steps queue behind develop.","evidence":"crates/daemon/src/sched.rs:120 ranks review below develop","metric":"wait_share","severity":"low","cost":"small"}]}"#;
    let wt = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(wt.path().join(introspect::ANALYST_FILE), result).unwrap();
    introspect::ingest_analyst(d, analyst, "scratch", wt.path(), dir.path());
    assert!(!wt.path().join(introspect::ANALYST_FILE).exists());
    let rows = d.store.proposals().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].decided_by, "proposals.introspection.low_small");
    assert_ne!(state(d, TaskId(rows[0].task)), State::Proposed);

    // Another task that carries the analyst's old title is not the analyst.
    let other = d
        .emit(EventKind::TaskFiled {
            project: pid,
            title: introspect::ANALYST_TITLE.into(),
            body: "x".into(),
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            explored: true,
            proposed: false,
            blockers: vec![],
            not_before: None,
        })
        .unwrap()
        .effects
        .iter()
        .find_map(|e| match e {
            Effect::TaskCreated { task } => Some(*task),
            _ => None,
        })
        .unwrap();
    std::fs::write(wt.path().join(introspect::ANALYST_FILE), result).unwrap();
    introspect::ingest_analyst(d, other, "scratch", wt.path(), dir.path());
    assert!(wt.path().join(introspect::ANALYST_FILE).exists());
}

fn result_json(title: &str) -> String {
    format!(
        r#"{{"findings":[{{"title":"{title}","body":"Review steps queue behind develop.","evidence":"crates/daemon/src/sched.rs:120 ranks review below develop","metric":"wait_share","severity":"low","cost":"small"}}]}}"#
    )
}

#[test]
fn an_analyst_result_is_read_once_kept_in_the_log_dir_and_ends_the_develop_step() {
    let w = world_with(LANDING, &|_| SPLIT.into(), &[]);
    let d = &w.d;
    let pid = d.project_id_by_name("scratch").unwrap();
    let analyst = introspect::file_analyst_task(d, pid).unwrap();
    let wt = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();

    // No file yet: the develop step goes back, nothing is filed.
    let r = introspect::ingest_analyst(d, analyst, "scratch", wt.path(), dir.path());
    assert_eq!(r, introspect::Ingest::Missing);
    // An unreadable one goes back with the parse error.
    std::fs::write(wt.path().join(introspect::ANALYST_FILE), "not json").unwrap();
    let r = introspect::ingest_analyst(d, analyst, "scratch", wt.path(), dir.path());
    assert!(matches!(r, introspect::Ingest::Unreadable(_)));
    assert!(d.store.proposals().unwrap().is_empty());

    std::fs::write(
        wt.path().join(introspect::ANALYST_FILE),
        result_json("Reviews wait for a slot"),
    )
    .unwrap();
    let r = introspect::ingest_analyst(d, analyst, "scratch", wt.path(), dir.path());
    assert_eq!(r, introspect::Ingest::Done);
    assert_eq!(d.store.proposals().unwrap().len(), 1);
    // The worktree holds neither the file nor a `.read` copy; the log dir has the copy.
    assert!(!wt.path().join(introspect::ANALYST_FILE).exists());
    assert!(!wt
        .path()
        .join(format!("{}.read", introspect::ANALYST_FILE))
        .exists());
    assert!(dir.path().join(introspect::ANALYST_FILE).exists());

    // A later step of the same task, with the same or a reworded finding, files nothing.
    for title in [
        "Reviews wait for a slot",
        "Review steps wait for a free slot",
    ] {
        std::fs::write(wt.path().join(introspect::ANALYST_FILE), result_json(title)).unwrap();
        let r = introspect::ingest_analyst(d, analyst, "scratch", wt.path(), dir.path());
        assert_eq!(r, introspect::Ingest::Done);
        assert_eq!(d.store.proposals().unwrap().len(), 1);
        assert!(!wt.path().join(introspect::ANALYST_FILE).exists());
    }
}

#[test]
fn the_digest_counts_only_the_projects_own_flaky_suites() {
    let w = world_with(LANDING, &|_| String::new(), &[]);
    let d = &w.d;
    let pid = d.project_id_by_name("scratch").unwrap();
    let mine = d
        .emit(EventKind::TaskFiled {
            project: pid,
            title: "A task".into(),
            body: "x".into(),
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            explored: true,
            proposed: false,
            blockers: vec![],
            not_before: None,
        })
        .unwrap()
        .effects
        .iter()
        .find_map(|e| match e {
            Effect::TaskCreated { task } => Some(*task),
            _ => None,
        })
        .unwrap();
    d.store
        .record_flaky("own-suite", "a", mine.0, now_ms())
        .unwrap();
    // A flake from a task that belongs to no project of this name.
    d.store
        .record_flaky("cargo-workspace", "b", 999_999, now_ms())
        .unwrap();
    let dg = introspect::digest(d, Some("scratch"), 0, now_ms() + 1000);
    let text = dg.to_string();
    assert!(text.contains("own-suite::a"), "{text}");
    assert!(!text.contains("cargo-workspace::b"), "{text}");
}

/// The whole path of s22 stage 3: the analyst writes its findings, commits nothing as its task
/// says, and submits. The findings are read and the task ends at develop.
#[test]
fn an_analyst_task_that_commits_nothing_files_its_good_finding_and_ends() {
    let develop = serde_json::json!([
        {"sh": format!(
            "printf '%s' '{}' > {}",
            result_json("Reviews wait for a slot"),
            introspect::ANALYST_FILE
        )},
        {"call": {"tool": "submit_change", "args": {"summary": "analysed", "head": "$HEAD"}}}
    ]);
    let s = Scratch::new(
        "true",
        serde_json::json!({"steps": {"explore": [explore_turn()], "develop": [develop]}}),
    );
    let d = s.start();
    let pid = d.project_id_by_name("scratch").unwrap();
    let analyst = introspect::file_analyst_task(&d, pid).unwrap();
    // Closed with no change counts as landed; a refused submission ends in failed instead.
    wait_state(&d, analyst, State::Landed, 90);
    assert_eq!(
        d.world().unwrap().tasks[&analyst].landed,
        Some(LandedOutcome::ClosedNoChange)
    );
    let rows = d.store.proposals().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        d.world().unwrap().tasks[&TaskId(rows[0].task)].title,
        "Reviews wait for a slot"
    );
}
