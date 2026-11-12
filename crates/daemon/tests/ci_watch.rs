// SPDX-License-Identifier: GPL-3.0-or-later

//! The base-branch CI watch (s154, decision 0149) against recorded runs of the base branch:
//! what it files, when, and how it freezes landing. The clock is a parameter of a pass, so
//! the long-red rule needs no waiting.

mod support;
use mason_adapter_api::http::Recorded;
use mason_adapters::github::{ActionsJob, ActionsRun, GitHub, TokenRef};
use mason_core::ci_watch::Rules;
use mason_core::*;
use mason_daemon::ci_watch::{pass, Source};
use mason_daemon::Daemon;
use mason_store::TrackerItemRow;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use support::*;

/// The base branch as the host reports it, edited between passes.
#[derive(Default)]
struct Base {
    runs: Mutex<Vec<ActionsRun>>,
    jobs: Mutex<BTreeMap<u64, Vec<ActionsJob>>>,
}

impl Base {
    fn run(&self, id: u64, workflow: &str, conclusion: &str, jobs: &[(&str, &str)]) {
        self.runs.lock().unwrap().insert(
            0,
            ActionsRun {
                id,
                workflow: workflow.into(),
                sha: format!("{id:040}"),
                url: format!("https://github.com/o/r/actions/runs/{id}"),
                created_ms: id * 1000,
                conclusion: Some(conclusion.into()),
            },
        );
        self.jobs.lock().unwrap().insert(
            id,
            jobs.iter()
                .enumerate()
                .map(|(i, (n, c))| ActionsJob {
                    id: id * 10 + i as u64,
                    name: (*n).into(),
                    conclusion: Some((*c).into()),
                })
                .collect(),
        );
    }
}

impl Source for Base {
    fn runs(&self, _branch: &str) -> Result<Vec<ActionsRun>, String> {
        Ok(self.runs.lock().unwrap().clone())
    }
    fn jobs(&self, run: u64) -> Result<Vec<ActionsJob>, String> {
        Ok(self
            .jobs
            .lock()
            .unwrap()
            .get(&run)
            .cloned()
            .unwrap_or_default())
    }
    fn log_tail(&self, job: u64) -> Option<String> {
        Some(format!("log of job {job}\nFAILED test_x"))
    }
}

fn rules() -> Rules {
    Rules {
        workflows: vec![],
        exclude: vec!["Gherkin".into()],
        gate: Some("CI/pr-gate".into()),
        agreement: 2,
        red_ticket_after_ms: 3 * 3_600_000,
        max_open_auto: 2,
        freeze_on_gate: true,
    }
}

fn start() -> (Scratch, Arc<Daemon>, ProjectId) {
    let s = Scratch::new("true", happy_script());
    // Nothing is admitted, so a filed task stays open.
    let d = s.start_with(|c| c.max_concurrent = 0);
    let pid = d.project_id_by_name("scratch").unwrap();
    (s, d, pid)
}

fn tasks(d: &Arc<Daemon>) -> Vec<Task> {
    d.world().unwrap().tasks.values().cloned().collect()
}

fn titled(d: &Arc<Daemon>, what: &str) -> Vec<Task> {
    tasks(d)
        .into_iter()
        .filter(|t| t.title.contains(what))
        .collect()
}

fn freezes(d: &Arc<Daemon>) -> Vec<mason_store::FreezeRow> {
    d.store.freezes().unwrap()
}

#[test]
fn seeding_files_nothing_then_a_gate_red_files_urgent_and_freezes_until_green() {
    let (_s, d, pid) = start();
    let (b, r) = (Base::default(), rules());
    // A known red at the first look is the ledger, not a ticket; the first look only seeds.
    b.run(
        1,
        "CI",
        "failure",
        &[("pr-gate", "failure"), ("lint", "success")],
    );
    pass(&d, pid, &r, &b, 10_000).unwrap();
    assert!(tasks(&d).is_empty() && freezes(&d).is_empty());
    // Green, then red: the gate acts at once.
    b.run(
        2,
        "CI",
        "success",
        &[("pr-gate", "success"), ("lint", "success")],
    );
    pass(&d, pid, &r, &b, 20_000).unwrap();
    assert!(tasks(&d).is_empty());
    b.run(
        3,
        "CI",
        "failure",
        &[("pr-gate", "failure"), ("lint", "success")],
    );
    pass(&d, pid, &r, &b, 30_000).unwrap();
    let t = titled(&d, "CI/pr-gate red on main");
    assert_eq!(t.len(), 1, "{:?}", tasks(&d));
    assert_eq!(t[0].priority, Priority::Urgent);
    assert_eq!(t[0].trust, TrustSource::Owner);
    assert!(t[0].body.contains("https://github.com/o/r/actions/runs/3"));
    assert!(t[0].body.contains(&format!("{:040}", 3)));
    assert!(t[0].body.contains("FAILED test_x"), "{}", t[0].body);
    let f = freezes(&d);
    assert_eq!(f.len(), 1);
    assert_eq!(
        (f[0].source.as_str(), f[0].fix_task),
        ("ci", Some(t[0].id.0))
    );
    // The post-land tier going green does not lift it.
    assert!(!d.store.clear_freeze("scratch").unwrap());
    // Still red on the next run: no second task, still frozen.
    b.run(4, "CI", "failure", &[("pr-gate", "failure")]);
    pass(&d, pid, &r, &b, 40_000).unwrap();
    assert_eq!(titled(&d, "pr-gate").len(), 1);
    assert_eq!(freezes(&d).len(), 1);
    // Green on the base lifts it.
    b.run(5, "CI", "success", &[("pr-gate", "success")]);
    pass(&d, pid, &r, &b, 50_000).unwrap();
    assert!(freezes(&d).is_empty());
    assert_eq!(titled(&d, "pr-gate").len(), 1);
}

#[test]
fn a_gate_red_with_the_freeze_off_files_and_does_not_freeze() {
    let (_s, d, pid) = start();
    let (b, mut r) = (Base::default(), rules());
    r.freeze_on_gate = false;
    b.run(1, "CI", "success", &[("pr-gate", "success")]);
    pass(&d, pid, &r, &b, 10_000).unwrap();
    b.run(2, "CI", "failure", &[("pr-gate", "failure")]);
    pass(&d, pid, &r, &b, 20_000).unwrap();
    assert_eq!(titled(&d, "pr-gate").len(), 1);
    assert!(freezes(&d).is_empty());
}

#[test]
fn a_job_red_once_files_nothing_and_red_on_two_runs_files_one_task() {
    let (_s, d, pid) = start();
    let (b, r) = (Base::default(), rules());
    b.run(
        1,
        "CI",
        "success",
        &[("lint", "success"), ("unit", "success")],
    );
    pass(&d, pid, &r, &b, 10_000).unwrap();
    b.run(
        2,
        "CI",
        "failure",
        &[("lint", "failure"), ("unit", "failure")],
    );
    pass(&d, pid, &r, &b, 20_000).unwrap();
    assert!(tasks(&d).is_empty());
    // lint recovers, unit is red again on a different run.
    b.run(
        3,
        "CI",
        "failure",
        &[("lint", "success"), ("unit", "failure")],
    );
    pass(&d, pid, &r, &b, 30_000).unwrap();
    assert!(titled(&d, "CI/lint").is_empty());
    let t = titled(&d, "CI/unit red on main");
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].priority, Priority::Normal);
    assert!(freezes(&d).is_empty());
    b.run(4, "CI", "failure", &[("unit", "failure")]);
    pass(&d, pid, &r, &b, 40_000).unwrap();
    assert_eq!(tasks(&d).len(), 1);
}

#[test]
fn infrastructure_and_excluded_workflows_file_nothing() {
    let (_s, d, pid) = start();
    let (b, r) = (Base::default(), rules());
    b.run(1, "CI", "success", &[("pr-gate", "success")]);
    b.run(2, "Gherkin", "success", &[("nightly", "success")]);
    pass(&d, pid, &r, &b, 10_000).unwrap();
    for (i, c) in ["cancelled", "timed_out", "startup_failure"]
        .iter()
        .enumerate()
    {
        b.run(10 + i as u64, "CI", "failure", &[("pr-gate", c)]);
    }
    b.run(20, "Gherkin", "failure", &[("nightly", "failure")]);
    b.run(21, "Gherkin", "failure", &[("nightly", "failure")]);
    pass(&d, pid, &r, &b, 20_000).unwrap();
    pass(&d, pid, &r, &b, 100 * 3_600_000).unwrap();
    assert!(tasks(&d).is_empty(), "{:?}", tasks(&d));
    assert!(freezes(&d).is_empty());
}

#[test]
fn an_open_task_or_issue_naming_the_job_stops_a_second_filing() {
    let (_s, d, pid) = start();
    let (b, r) = (Base::default(), rules());
    let named = file_task(&d, "Look into CI/unit, it is flaky");
    b.run(
        1,
        "CI",
        "success",
        &[("unit", "success"), ("lint", "success")],
    );
    pass(&d, pid, &r, &b, 10_000).unwrap();
    d.store
        .put_tracker_item(&TrackerItemRow {
            project: "scratch".into(),
            number: 5,
            task: 0,
            title: "CI/lint fails".into(),
            body: String::new(),
            priority: "normal".into(),
            open: true,
            mirrored: String::new(),
            labelled: None,
        })
        .unwrap();
    for n in 2..4 {
        b.run(
            n,
            "CI",
            "failure",
            &[("unit", "failure"), ("lint", "failure")],
        );
        pass(&d, pid, &r, &b, n * 10_000).unwrap();
    }
    assert_eq!(tasks(&d).len(), 1, "only the one that was already there");
    assert_eq!(tasks(&d)[0].id, named);
}

#[test]
fn past_the_cap_a_red_is_filed_proposed() {
    let (_s, d, pid) = start();
    let (b, mut r) = (Base::default(), rules());
    r.max_open_auto = 1;
    b.run(
        1,
        "CI",
        "success",
        &[("pr-gate", "success"), ("unit", "success")],
    );
    pass(&d, pid, &r, &b, 10_000).unwrap();
    b.run(
        2,
        "CI",
        "failure",
        &[("pr-gate", "failure"), ("unit", "failure")],
    );
    pass(&d, pid, &r, &b, 20_000).unwrap();
    b.run(
        3,
        "CI",
        "failure",
        &[("pr-gate", "failure"), ("unit", "failure")],
    );
    pass(&d, pid, &r, &b, 30_000).unwrap();
    let gate = titled(&d, "pr-gate");
    let unit = titled(&d, "CI/unit");
    assert_eq!((gate.len(), unit.len()), (1, 1));
    assert_ne!(gate[0].state, State::Proposed);
    assert_eq!(unit[0].state, State::Proposed);
}

#[test]
fn a_long_red_is_filed_on_the_injected_clock_and_not_again_before_another_threshold() {
    let (_s, d, pid) = start();
    let (b, r) = (Base::default(), rules());
    let h = 3_600_000;
    b.run(1, "CI", "success", &[("unit", "success")]);
    pass(&d, pid, &r, &b, 0).unwrap();
    b.run(2, "CI", "failure", &[("unit", "failure")]);
    pass(&d, pid, &r, &b, 1_000).unwrap();
    pass(&d, pid, &r, &b, 2 * h).unwrap();
    assert!(tasks(&d).is_empty());
    pass(&d, pid, &r, &b, 3 * h + 2_000).unwrap();
    let t = titled(&d, "CI/unit");
    assert_eq!(t.len(), 1);
    // The ticket is closed by the owner; still red: not again until another threshold.
    d.emit(EventKind::Canceled {
        task: t[0].id,
        delete_branch: false,
    })
    .unwrap();
    pass(&d, pid, &r, &b, 4 * h).unwrap();
    assert_eq!(tasks(&d).len(), 1);
    pass(&d, pid, &r, &b, 6 * h + 3_000).unwrap();
    assert_eq!(tasks(&d).len(), 2);
}

#[test]
fn the_state_survives_a_restart_and_shows_in_the_view() {
    let (_s, d, pid) = start();
    let (b, r) = (Base::default(), rules());
    b.run(1, "CI", "success", &[("unit", "success")]);
    pass(&d, pid, &r, &b, 10_000).unwrap();
    b.run(2, "CI", "failure", &[("unit", "failure")]);
    pass(&d, pid, &r, &b, 20_000).unwrap();
    let saved =
        mason_daemon::ci_watch::decode(&d.store.ci_watch_state("scratch").unwrap().unwrap());
    assert_eq!(saved.state.workflows["CI"].jobs["unit"].pending, vec![2]);
    assert_eq!(
        mason_daemon::ci_watch::decode(&mason_daemon::ci_watch::encode(&saved)),
        saved
    );
}

#[test]
fn it_reads_recorded_github_responses_and_follows_the_log_redirect() {
    let (s, d, pid) = start();
    let dir = tempfile::tempdir().unwrap();
    let tok = dir.path().join("token");
    std::fs::write(&tok, "ghp_recorded_test_token\n").unwrap();
    let fx = |n: &str| {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../adapters/tests/fixtures/github")
                .join(n),
        )
        .unwrap()
    };
    let gh = |script: &str| {
        GitHub::new(
            Arc::new(Recorded::from_json(&fx(script)).unwrap()),
            "https://api.github.com",
            "o/r",
            TokenRef::File(tok.clone()),
        )
    };
    let mut r = rules();
    r.gate = Some("CI/pr-gate".into());
    pass(&d, pid, &r, &gh("base_runs_green.json"), 10_000).unwrap();
    assert!(tasks(&d).is_empty());
    pass(&d, pid, &r, &gh("base_runs_red.json"), 20_000).unwrap();
    let t = titled(&d, "CI/pr-gate red on main");
    assert_eq!(t.len(), 1);
    assert!(t[0].body.contains("FAILED test_x"), "{}", t[0].body);
    assert_eq!(freezes(&d).len(), 1);
    drop(s);
}
