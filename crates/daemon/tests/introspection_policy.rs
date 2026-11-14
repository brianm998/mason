// SPDX-License-Identifier: GPL-3.0-or-later

//! Introspection findings are proposed or queued by the owner's `[proposals.introspection]`
//! table, per severity and cost (s295), and the analyst's task is known by the schedule row that
//! recorded it, not by its title.

mod support;
use mason_core::introspect::{Finding, Stats};
use mason_core::proposal::{Cost, Impact};
use mason_core::*;
use mason_daemon::{introspect, Daemon};
use std::sync::Arc;
use support::lw::*;

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
