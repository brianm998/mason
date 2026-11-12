// SPDX-License-Identifier: GPL-3.0-or-later

//! Scheduled CI runs (s175, decision 0170): the follow flow against a fake CI host. Time is a
//! parameter of every step, so the three-day give-up needs no waiting.

mod support;
use mason_adapters::github::{ActionsJob, WorkflowRun};
use mason_core::ci_schedule::{Follow, Phase, GIVE_UP_MS};
use mason_core::*;
use mason_daemon::ci_schedule::{decode, dispatch, encode, follow, Host, Repo};
use mason_daemon::schedule::CiSpec;
use mason_daemon::Daemon;
use mason_store::ScheduleRow;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use support::*;

/// A CI host whose runs and jobs the test edits. A rerun swaps in the second attempt's jobs.
#[derive(Default)]
struct Fake {
    runs: Mutex<Vec<WorkflowRun>>,
    jobs: Mutex<BTreeMap<u64, Vec<ActionsJob>>>,
    second: Mutex<BTreeMap<u64, Vec<ActionsJob>>>,
    calls: Mutex<Vec<String>>,
    refuse_rerun: Mutex<bool>,
}

fn job(id: u64, name: &str, c: &str) -> ActionsJob {
    ActionsJob {
        id,
        name: name.into(),
        conclusion: (!c.is_empty()).then(|| c.to_string()),
    }
}

impl Fake {
    fn run(&self, id: u64, status: &str, conclusion: &str, sha: &str, event: &str) {
        let mut runs = self.runs.lock().unwrap();
        runs.retain(|r| r.id != id);
        runs.insert(
            0,
            WorkflowRun {
                id,
                workflow: "CI".into(),
                sha: sha.into(),
                url: format!("https://github.com/o/r/actions/runs/{id}"),
                created_ms: id,
                status: status.into(),
                conclusion: (!conclusion.is_empty()).then(|| conclusion.to_string()),
                event: event.into(),
                title: "Full".into(),
                attempt: 1,
            },
        );
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Host for Fake {
    fn runs(&self, _w: &str, _b: &str) -> Result<Vec<WorkflowRun>, String> {
        self.calls.lock().unwrap().push("runs".into());
        Ok(self.runs.lock().unwrap().clone())
    }
    fn dispatch(&self, w: &str, r: &str, inputs: &[(String, String)]) -> Result<(), String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("dispatch {w} {r} {inputs:?}"));
        Ok(())
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
    fn rerun_failed(&self, run: u64) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("rerun {run}"));
        if *self.refuse_rerun.lock().unwrap() {
            return Err("refused".into());
        }
        if let Some(j) = self.second.lock().unwrap().remove(&run) {
            self.jobs.lock().unwrap().insert(run, j);
        }
        for r in self.runs.lock().unwrap().iter_mut().filter(|r| r.id == run) {
            r.attempt = 2;
        }
        Ok(())
    }
}

struct FakeRepo {
    tip: String,
    changed: Option<Vec<String>>,
    paths_file: Option<String>,
}

impl Repo for FakeRepo {
    fn fetch(&self) -> Result<(), String> {
        Ok(())
    }
    fn tip(&self) -> Result<String, String> {
        Ok(self.tip.clone())
    }
    fn file_at_tip(&self, _p: &str) -> Result<String, String> {
        self.paths_file.clone().ok_or_else(|| "no such file".into())
    }
    fn changed_since(&self, _sha: &str) -> Option<Vec<String>> {
        self.changed.clone()
    }
}

fn start() -> (Scratch, Arc<Daemon>, ProjectId) {
    let s = Scratch::new("true", happy_script());
    // Nothing is admitted, so a filed task stays open.
    let d = s.start_with(|c| c.max_concurrent = 0);
    let pid = d.project_id_by_name("scratch").unwrap();
    (s, d, pid)
}

fn spec(max: usize) -> CiSpec {
    CiSpec {
        workflow: "full.yml".into(),
        run_name: "Full".into(),
        inputs: vec![("features".into(), String::new())],
        group_by: mason_core::ci_schedule::GroupBy::parse("<group>/<leg>").unwrap(),
        max_tickets: max,
        ..Default::default()
    }
}

fn tasks(d: &Arc<Daemon>) -> Vec<Task> {
    d.world().unwrap().tasks.values().cloned().collect()
}

fn row() -> ScheduleRow {
    ScheduleRow {
        name: "scratch/nightly".into(),
        project: "scratch".into(),
        kind: "ci".into(),
        every_ms: 86_400_000,
        ..Default::default()
    }
}

/// The run `id` finished with these first-attempt jobs; the rerun will give `second`.
fn finished(f: &Fake, id: u64, first: &[(&str, &str)], second: &[(&str, &str)]) {
    f.run(id, "completed", "failure", "tipsha", "workflow_dispatch");
    let mk = |base: u64, v: &[(&str, &str)]| {
        v.iter()
            .enumerate()
            .map(|(i, (n, c))| job(base + i as u64, n, c))
            .collect::<Vec<_>>()
    };
    f.jobs.lock().unwrap().insert(id, mk(id * 100, first));
    f.second
        .lock()
        .unwrap()
        .insert(id, mk(id * 100 + 50, second));
}

fn following(id: u64, at: u64) -> String {
    encode(&Follow {
        run_id: id,
        phase: Phase::Await1,
        at,
        before: vec![],
        failures: vec![],
    })
}

/// Follow until the follow state is gone (at most a few ticks), returning each tick's words.
fn follow_all(
    d: &Arc<Daemon>,
    pid: ProjectId,
    ci: &CiSpec,
    f: &Fake,
    r: &mut ScheduleRow,
) -> Vec<String> {
    let mut said = vec![];
    for i in 0..6 {
        said.push(follow(d, pid, ci, "nightly", f, r, 1_000 + i));
        if r.follow.is_empty() {
            break;
        }
    }
    said
}

#[test]
fn red_twice_files_one_task_per_group_and_red_then_green_one_flaky_task() {
    let (_s, d, pid) = start();
    let f = Fake::default();
    finished(
        &f,
        7,
        &[
            ("alpha/linux", "failure"),
            ("alpha/mac", "failure"),
            ("beta/linux", "failure"),
            ("gamma/linux", "cancelled"),
            ("delta/linux", "success"),
        ],
        &[
            ("alpha/linux", "failure"),
            ("alpha/mac", "failure"),
            ("beta/linux", "success"),
            ("gamma/linux", "success"),
        ],
    );
    let mut r = row();
    r.follow = following(7, 500);
    let said = follow_all(&d, pid, &spec(6), &f, &mut r);
    assert!(said[0].contains("rerunning"), "{said:?}");
    assert_eq!(
        f.calls().iter().filter(|c| *c == "rerun 7").count(),
        1,
        "rerun once"
    );
    assert!(r.follow.is_empty());
    let t = tasks(&d);
    assert_eq!(
        t.len(),
        2,
        "{:?}",
        t.iter().map(|t| &t.title).collect::<Vec<_>>()
    );
    let red = t.iter().find(|t| t.title.contains("red twice")).unwrap();
    assert!(red.title.contains("CI/alpha/linux") && red.title.contains("CI/alpha/mac"));
    assert_eq!(red.priority, Priority::Normal);
    assert_eq!(red.trust, TrustSource::Owner);
    assert!(red.body.contains("FAILED test_x") && red.body.contains("actions/runs/7"));
    let flaky = t.iter().find(|t| t.title.contains("flaky")).unwrap();
    assert!(flaky.title.contains("CI/beta/linux"));
    assert_eq!(flaky.priority, Priority::Low);
    // The cancelled job was rerun and filed nothing; the green one was not rerun-judged at all.
    assert!(t
        .iter()
        .all(|t| !t.title.contains("gamma") && !t.title.contains("delta")));
}

#[test]
fn an_all_green_run_is_not_rerun_and_files_nothing() {
    let (_s, d, pid) = start();
    let f = Fake::default();
    f.run(3, "completed", "success", "tipsha", "workflow_dispatch");
    f.jobs.lock().unwrap().insert(
        3,
        vec![job(31, "a/x", "success"), job(32, "a/y", "skipped")],
    );
    let mut r = row();
    r.follow = following(3, 500);
    let said = follow_all(&d, pid, &spec(6), &f, &mut r);
    assert!(said[0].contains("green"), "{said:?}");
    assert!(f.calls().iter().all(|c| !c.starts_with("rerun")));
    assert!(tasks(&d).is_empty() && r.follow.is_empty());
}

#[test]
fn the_cap_keeps_reds_before_flakes() {
    let (_s, d, pid) = start();
    let f = Fake::default();
    // Listed flaky first; the cap of 2 must still go to the two reds.
    finished(
        &f,
        8,
        &[
            ("f1/l", "failure"),
            ("f2/l", "failure"),
            ("r1/l", "failure"),
            ("r2/l", "failure"),
            ("r3/l", "failure"),
        ],
        &[
            ("f1/l", "success"),
            ("f2/l", "success"),
            ("r1/l", "failure"),
            ("r2/l", "failure"),
            ("r3/l", "failure"),
        ],
    );
    let mut r = row();
    r.follow = following(8, 500);
    let said = follow_all(&d, pid, &spec(2), &f, &mut r);
    let titles: Vec<String> = tasks(&d).into_iter().map(|t| t.title).collect();
    assert_eq!(titles.len(), 2, "{titles:?}");
    assert!(titles.iter().all(|t| t.contains("red twice")), "{titles:?}");
    assert!(said.last().unwrap().contains("left unfiled"), "{said:?}");
}

#[test]
fn a_job_an_open_task_already_names_is_not_filed_again_and_does_not_use_the_cap() {
    let (_s, d, pid) = start();
    // What the base-branch CI watch files for the same job.
    d.emit(EventKind::TaskFiled {
        project: pid,
        title: "CI/alpha/linux red on main".into(),
        body: "x".into(),
        priority: Priority::Normal,
        trust: TrustSource::Owner,
        explored: false,
        proposed: false,
        blockers: vec![],
        not_before: None,
    })
    .unwrap();
    let f = Fake::default();
    finished(
        &f,
        9,
        &[("alpha/linux", "failure"), ("beta/linux", "failure")],
        &[("alpha/linux", "failure"), ("beta/linux", "failure")],
    );
    let mut r = row();
    r.follow = following(9, 500);
    let said = follow_all(&d, pid, &spec(1), &f, &mut r);
    let titles: Vec<String> = tasks(&d).into_iter().map(|t| t.title).collect();
    assert_eq!(titles.len(), 2, "{titles:?}");
    assert!(
        titles.iter().any(|t| t.contains("CI/beta/linux red twice")),
        "{titles:?}"
    );
    assert!(said.last().unwrap().contains("already named"), "{said:?}");
}

#[test]
fn a_run_that_never_completes_is_dropped_after_the_limit() {
    let (_s, d, pid) = start();
    let f = Fake::default();
    f.run(4, "in_progress", "", "tipsha", "workflow_dispatch");
    let mut r = row();
    r.follow = following(4, 1_000);
    let ci = spec(6);
    let say = follow(&d, pid, &ci, "nightly", &f, &mut r, 1_000 + GIVE_UP_MS);
    assert!(
        say.contains("following run 4") && !r.follow.is_empty(),
        "{say}"
    );
    let say = follow(&d, pid, &ci, "nightly", &f, &mut r, 1_000 + GIVE_UP_MS + 1);
    assert!(say.contains("dropped") && r.follow.is_empty(), "{say}");
    assert!(tasks(&d).is_empty());
    // A run that never showed up at all is dropped the same way.
    let mut r = row();
    r.follow = encode(&Follow {
        run_id: 0,
        phase: Phase::Locate,
        at: 1_000,
        before: vec![],
        failures: vec![],
    });
    let say = follow(
        &d,
        pid,
        &ci,
        "nightly",
        &Fake::default(),
        &mut r,
        1_000 + GIVE_UP_MS + 1,
    );
    assert!(say.contains("dropped") && r.follow.is_empty(), "{say}");
}

#[test]
fn a_rerun_that_is_refused_is_tried_again_and_a_run_rerun_by_hand_is_not_classified() {
    let (_s, d, pid) = start();
    let f = Fake::default();
    finished(&f, 5, &[("a/x", "failure")], &[("a/x", "failure")]);
    *f.refuse_rerun.lock().unwrap() = true;
    let mut r = row();
    r.follow = following(5, 500);
    let say = follow(&d, pid, &spec(6), "nightly", &f, &mut r, 1_000);
    assert!(
        say.contains("will try again") && !r.follow.is_empty(),
        "{say}"
    );
    assert_eq!(decode(&r.follow).unwrap().phase, Phase::Await1);
    // Somebody reran it: attempt 2 while we wait for attempt 1.
    f.runs.lock().unwrap()[0].attempt = 2;
    let say = follow(&d, pid, &spec(6), "nightly", &f, &mut r, 1_001);
    assert!(say.contains("by hand") && r.follow.is_empty(), "{say}");
    assert!(tasks(&d).is_empty());
}

#[test]
fn dispatch_follows_the_decision_and_the_new_run_is_found_on_a_later_tick() {
    let (_s, d, pid) = start();
    let f = Fake::default();
    let ci = spec(6);
    let repo = FakeRepo {
        tip: "tipsha".into(),
        changed: None,
        paths_file: None,
    };
    let mut r = row();
    // Never green: dispatch with the inputs, on the base branch.
    let say = dispatch(&d, pid, &ci, &f, &repo, &mut r, 100);
    assert!(say.contains("dispatched"), "{say}");
    assert!(f
        .calls()
        .iter()
        .any(|c| c.starts_with("dispatch full.yml main") && c.contains("features")));
    assert_eq!(decode(&r.follow).unwrap().phase, Phase::Locate);
    // The follow state is kept with the row and survives a store round trip.
    d.store.put_schedule(&r).unwrap();
    let back = d.store.schedule("scratch/nightly").unwrap().unwrap();
    assert_eq!(decode(&back.follow), decode(&r.follow));
    // Not in the list yet: wait. Then the new dispatch run appears.
    let mut r = back;
    assert!(
        follow(&d, pid, &ci, "nightly", &f, &mut r, 110).contains("waiting for the dispatched run")
    );
    f.run(50, "in_progress", "", "tipsha", "workflow_dispatch");
    f.run(40, "completed", "success", "older", "push");
    let say = follow(&d, pid, &ci, "nightly", &f, &mut r, 120);
    assert!(say.contains("following run 50"), "{say}");
    assert_eq!(decode(&r.follow).unwrap().run_id, 50);
    // While it is in flight a second schedule run dispatches nothing more.
    let mut again = row();
    let n = f
        .calls()
        .iter()
        .filter(|c| c.starts_with("dispatch"))
        .count();
    let say = dispatch(&d, pid, &ci, &f, &repo, &mut again, 130);
    assert!(
        say.contains("in flight") && again.follow.is_empty(),
        "{say}"
    );
    assert_eq!(
        f.calls()
            .iter()
            .filter(|c| c.starts_with("dispatch"))
            .count(),
        n
    );
}

#[test]
fn dispatch_reads_the_globs_from_the_workflow_at_the_tip_and_refuses_an_empty_list() {
    let (_s, d, pid) = start();
    let f = Fake::default();
    f.run(1, "completed", "success", "green", "workflow_dispatch");
    let mut ci = spec(6);
    ci.paths_from = Some(".github/workflows/smoke.yml".into());
    let yml = "on:\n  pull_request:\n    paths:\n      - 'src/**'\n";
    let mut repo = FakeRepo {
        tip: "tipsha".into(),
        changed: Some(vec!["docs/a.md".into()]),
        paths_file: Some(yml.into()),
    };
    let say = dispatch(&d, pid, &ci, &f, &repo, &mut row(), 100);
    assert!(
        say.contains("not dispatched") && say.contains("none match"),
        "{say}"
    );
    repo.changed = Some(vec!["docs/a.md".into(), "src/lib.rs".into()]);
    let mut r = row();
    assert!(dispatch(&d, pid, &ci, &f, &repo, &mut r, 100).contains("dispatched full.yml"));
    assert!(!r.follow.is_empty());
    // A workflow with no pull_request.paths list is an error, not "everything".
    repo.paths_file = Some("on:\n  push:\n".into());
    let mut r = row();
    let say = dispatch(&d, pid, &ci, &f, &repo, &mut r, 100);
    assert!(
        say.contains("could not dispatch") && r.follow.is_empty(),
        "{say}"
    );
}

#[test]
fn with_no_ci_schedule_configured_the_tick_makes_no_ci_call_and_adds_no_row() {
    let (_s, d, _pid) = start();
    mason_daemon::introspect::tick(&d);
    let rows = d.store.schedules().unwrap();
    assert!(rows.iter().all(|r| r.kind != "ci"), "{rows:?}");
    assert!(d.projects.values().all(|p| p.schedules().is_empty()));
}
