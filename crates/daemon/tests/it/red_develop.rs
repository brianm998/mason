// SPDX-License-Identifier: GPL-3.0-or-later

//! s989 (#1023): a red develop that gets fixed moves the tasks it held up. A real daemon, a real
//! scratch repository and real worktrees; the agents sleep, so the tasks stay in develop. The
//! watch's pass is driven by hand on a clock the test gives it, so nothing here depends on how
//! long anything takes.

use crate::support;
use mason_core::*;
use mason_daemon::redwatch;
use mason_daemon::views::Ctx;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use support::*;

const DEFINITION: &str = r#"schema = 1
start = "develop"

[resource.compile]
count = 1

[resource.test-suite]
count = 1

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
uses = { compile = 1, test-suite = 1 }
suites = ["only"]
on_red = "develop"
max_rounds = 2
next = "land"

[suite.only]
command = "sh checks.sh"
parser = "tap"
min_tests = 1
level = "unit"

[binding.unit]
when = ["development", "test_step"]

[step.land]
type = "landing"
category = "landing"

[impact]
"hello.txt" = ["only"]
"#;

/// `alpha` fails while `state.txt` says `bad`; `gamma` fails while `gamma-bad.txt` exists. With
/// names as arguments it runs only those, as a `--exact` rerun does. The lines are libtest's, with
/// the `--report-time` suffix MASON's cargo suites print.
const CHECKS: &str = r#"#!/bin/sh
WANT="$*"
want() { [ -z "$WANT" ] || case " $WANT " in *" $1 "*) true;; *) false;; esac; }
if [ -n "$WANT" ] && want delta; then
  case "$PWD" in *redwatch*) echo x >> "$PWD/../probe-count";; esac
  exit 1
fi
r=0
if want alpha; then
  if [ "$(cat state.txt 2>/dev/null)" = bad ]; then echo "test alpha ... FAILED <0.054s>"; r=1; else echo "test alpha ... ok <0.001s>"; fi
fi
if want gamma; then
  if [ -f gamma-bad.txt ]; then echo "test gamma ... FAILED <0.054s>"; r=1; else echo "test gamma ... ok <0.001s>"; fi
fi
exit $r
"#;

const MIN30: u64 = 30 * 60 * 1000;

fn start(
    turn: serde_json::Value,
    files: &[(&str, &str)],
) -> (Scratch, Arc<mason_daemon::Daemon>, ProjectId) {
    let s = Scratch::new("false", json!({"steps": {"develop": [turn]}}));
    s.write_project(
        "scratch",
        "false",
        "rerun_filter = \"sh checks.sh {tests}\"",
    );
    let mut all: Vec<(&str, &str)> = vec![
        (".mason/mason.toml", DEFINITION),
        ("checks.sh", CHECKS),
        ("state.txt", "bad\n"),
    ];
    all.extend_from_slice(files);
    s.push_files(&all, "project setup");
    let d = s.start_with(|c| c.env_retry = Duration::from_millis(200));
    let pid = d.project_id_by_name("scratch").unwrap();
    (s, d, pid)
}

fn sleeping() -> serde_json::Value {
    json!([{"sleep_ms": 100_000}])
}

fn fetch(s: &Scratch) {
    sh(&s.repo, &["fetch", "-q", "origin"]);
}

fn tip(d: &Arc<mason_daemon::Daemon>, pid: ProjectId) -> String {
    let p = d.project(pid).unwrap();
    mason_landing::git::rev_parse(&p.path, &p.base_ref()).unwrap()
}

fn developing(d: &Arc<mason_daemon::Daemon>, title: &str) -> (TaskId, std::path::PathBuf) {
    let t = file_task_as(d, title, "Do it.", true, TrustSource::Owner, false);
    let pid = d.project_id_by_name("scratch").unwrap();
    let wt = d.project(pid).unwrap().worktree(&d.cfg, t.0);
    wait_for("the worktree", 90, || wt.join(".git").exists());
    wait_state(d, t, State::Developing, 90);
    (t, wt)
}

fn sighted(d: &Arc<mason_daemon::Daemon>, t: TaskId, test: &str) {
    d.store
        .put_red_sightings(t.0, "only", &[test.into()], mason_daemon::daemon::now_ms())
        .unwrap();
}

fn alert_text(d: &Arc<mason_daemon::Daemon>, key: &str) -> Option<String> {
    mason_daemon::attention::alerts_now(d)
        .into_iter()
        .find(|(k, _, _)| k == key)
        .map(|(_, _, t)| t)
}

fn wait_row(d: &Arc<mason_daemon::Daemon>, t: TaskId) -> Option<String> {
    let w = d.world().unwrap();
    let ctx = Ctx::new(d, &w);
    ctx.row(&w.tasks[&t]).wait.map(|i| i.resource)
}

#[test]
fn a_fixed_develop_folds_into_clean_branches_tells_the_agents_and_alerts_for_a_conflicting_one() {
    let (s, d, pid) = start(sleeping(), &[]);
    let a = tip(&d, pid);
    // The newest whole baseline of the suite is red on `alpha` at A.
    d.store
        .put_baseline(
            &a,
            "only",
            &["alpha".into()],
            &["alpha".into(), "gamma".into()],
            2,
            mason_daemon::daemon::now_ms(),
        )
        .unwrap();
    let (t1, wt1) = developing(&d, "one");
    let (t2, wt2) = developing(&d, "two");
    let (t3, wt3) = developing(&d, "three");
    wait_for("the checkout", 60, || wt3.join("state.txt").exists());
    // The third branch edits the same line the fix will: its merge conflicts.
    std::fs::write(wt3.join("state.txt"), "mine\n").unwrap();
    sh(&wt3, &["add", "-A"]);
    sh(
        &wt3,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "mine",
        ],
    );
    for t in [t1, t2, t3] {
        sighted(&d, t, "alpha");
    }
    // While develop is red the checks' failures are explained as develop's, and the tasks show
    // the wait class `base-red` instead of plain "developing".
    let (_, red) = d.store.latest_baseline("only").unwrap().unwrap();
    let header = mason_daemon::knownred::red_on_develop_header(
        "test alpha ... FAILED <0.054s>\n",
        Some(&(a.clone(), red)),
        &[],
    );
    assert!(header.unwrap().contains("alpha: red on develop at"));
    for t in [t1, t2, t3] {
        assert_eq!(wait_row(&d, t).as_deref(), Some("base-red"), "task {t:?}");
    }
    assert_eq!(
        alert_text(&d, "red-develop").as_deref(),
        Some("3 tasks held by a red develop (alpha)")
    );

    // Develop is fixed.
    s.push_files(&[("state.txt", "good\n")], "fix alpha");
    fetch(&s);
    let b = tip(&d, pid);
    assert_ne!(a, b);
    let t0 = mason_daemon::daemon::now_ms();
    redwatch::pass_at(&d, t0);

    // A partial baseline at B says alpha passes there; it is not a whole-suite baseline.
    let (shown, still) = d.store.latest_baseline("only").unwrap().unwrap_or_default();
    assert!(still.is_empty(), "{shown} {still:?}");
    assert!(d.store.baseline(&b, "only").unwrap().is_none());
    let (_, red) = d.store.latest_baseline("only").unwrap().unwrap_or_default();
    assert!(mason_daemon::knownred::red_on_develop_header(
        "test alpha ... FAILED <0.054s>\n",
        Some(&(b.clone(), red)),
        &[],
    )
    .is_none());

    // The two clean branches have develop folded in; each agent is told, once.
    for (t, wt) in [(t1, &wt1), (t2, &wt2)] {
        assert_eq!(
            std::fs::read_to_string(wt.join("state.txt")).unwrap(),
            "good\n"
        );
        let notes = d.store.take_red_notes(t.0).unwrap();
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("alpha red on develop at"), "{notes:?}");
        assert!(
            notes[0].contains(&format!("it passes at {}", &b[..10])),
            "{notes:?}"
        );
        assert!(
            notes[0].contains("develop is merged into your branch: run your check again."),
            "{notes:?}"
        );
        assert!(d.store.take_red_notes(t.0).unwrap().is_empty());
    }
    // The conflicting one is told plainly and left alone.
    assert_eq!(
        std::fs::read_to_string(wt3.join("state.txt")).unwrap(),
        "mine\n"
    );
    let notes = d.store.take_red_notes(t3.0).unwrap();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].contains(
            "develop has fixed alpha, which your branch still fails; merging it conflicts in \
             state.txt: merge develop and resolve them before anything else."
        ),
        "{notes:?}"
    );
    // The alert for it waits for the delay, on the clock the test gives.
    assert!(redwatch::alerts_at(&d, t0 + MIN30 - 1)
        .iter()
        .all(|(k, _, _)| k != &format!("red-fix:{}", t3.0)));
    let late = redwatch::alerts_at(&d, t0 + MIN30);
    let alert = late
        .iter()
        .find(|(k, _, _)| k == &format!("red-fix:{}", t3.0));
    assert!(
        alert.is_some_and(|(_, kind, _)| kind == "needs_you"),
        "{late:?}"
    );
    assert!(late
        .iter()
        .all(|(k, _, _)| k != &format!("red-fix:{}", t1.0)));
    // Once the branch has the base, the alert goes.
    sh(&wt3, &["merge", "-q", "-X", "theirs", "--no-edit", &b]);
    assert!(redwatch::alerts_at(&d, t0 + 2 * MIN30)
        .iter()
        .all(|(k, _, _)| k != &format!("red-fix:{}", t3.0)));
    // Nothing is held now, and no task is filed for a test that passes.
    assert!(alert_text(&d, "red-develop").is_none());
    assert!(d
        .world()
        .unwrap()
        .tasks
        .values()
        .all(|t| !t.title.contains("is red on")));
    // A second pass at the same tip does nothing more.
    redwatch::pass_at(&d, t0 + 1);
    assert!(d.store.take_red_notes(t1.0).unwrap().is_empty());
}

#[test]
fn the_same_unexplained_failure_in_three_tasks_is_run_at_the_tip_and_files_one_urgent_task() {
    // `gamma` fails at the tip: develop is red on it.
    let (s, d, pid) = start(sleeping(), &[("gamma-bad.txt", "x\n")]);
    let mut tasks = vec![];
    for n in 0..3 {
        tasks.push(developing(&d, &format!("task {n}")).0);
    }
    for t in &tasks {
        sighted(&d, *t, "gamma");
    }
    // Two tasks are not enough.
    d.store
        .clear_red_sightings(tasks[2].0, "only", None)
        .unwrap();
    redwatch::pass_at(&d, mason_daemon::daemon::now_ms());
    assert!(filed(&d).is_empty(), "{:?}", filed(&d));
    sighted(&d, tasks[2], "gamma");
    redwatch::pass_at(&d, mason_daemon::daemon::now_ms());
    let titles = filed(&d);
    assert_eq!(titles.len(), 1, "{titles:?}");
    let b = tip(&d, pid);
    assert_eq!(
        titles[0],
        format!("`only` is red on {}: test gamma fails", &b[..10])
    );
    let w = d.world().unwrap();
    let filed_task = w.tasks.values().find(|t| t.title == titles[0]).unwrap();
    assert_eq!(filed_task.priority, Priority::Urgent);
    // Now it is red on develop: the tasks' "red on develop" note and the wait class follow.
    let (_, red) = d.store.latest_baseline("only").unwrap().unwrap();
    assert_eq!(red, vec!["gamma".to_string()]);
    for t in &tasks {
        assert_eq!(wait_row(&d, *t).as_deref(), Some("base-red"));
    }
    // A pass again does not file a second one.
    redwatch::pass_at(&d, mason_daemon::daemon::now_ms());
    assert_eq!(filed(&d).len(), 1);
    drop(s);
}

#[test]
fn a_failure_that_passes_at_the_tip_files_nothing_and_folds_the_tasks_instead() {
    let (_s, d, _pid) = start(sleeping(), &[]);
    let mut tasks = vec![];
    for n in 0..3 {
        tasks.push(developing(&d, &format!("task {n}")));
    }
    // `gamma` was failing in their checks, and passes at the tip (there is no gamma-bad.txt).
    for (t, _) in &tasks {
        sighted(&d, *t, "gamma");
    }
    redwatch::pass_at(&d, mason_daemon::daemon::now_ms());
    assert!(filed(&d).is_empty(), "{:?}", filed(&d));
    for (t, _) in &tasks {
        let notes = d.store.take_red_notes(t.0).unwrap();
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].contains("gamma failed in your checks and passes on develop at"),
            "{notes:?}"
        );
    }
}

#[test]
fn a_known_red_test_never_counts_toward_a_red_develop() {
    let (_s, d, _pid) = start(sleeping(), &[("gamma-bad.txt", "x\n")]);
    let mut tasks = vec![];
    for n in 0..3 {
        tasks.push(developing(&d, &format!("task {n}")).0);
    }
    mason_daemon::knownred::add(&d.store, "only", "gamma", "task #1");
    assert!(mason_daemon::knownred::known_for(&d.store, "only")
        .iter()
        .any(|k| k.test == "gamma"));
    for t in &tasks {
        sighted(&d, *t, "gamma");
    }
    redwatch::pass_at(&d, mason_daemon::daemon::now_ms());
    assert!(filed(&d).is_empty(), "{:?}", filed(&d));
}

fn filed(d: &Arc<mason_daemon::Daemon>) -> Vec<String> {
    d.world()
        .unwrap()
        .tasks
        .values()
        .filter(|t| t.title.contains("is red on"))
        .map(|t| t.title.clone())
        .collect()
}

#[test]
fn a_run_check_on_the_old_tree_is_stopped_when_develop_is_folded_and_made_again() {
    let dir = tempfile::tempdir().unwrap();
    let g = dir.path().display().to_string();
    // The suite waits for `go` on its first run; the second finds it and ends.
    let checks = format!(
        "#!/bin/sh\ncase \"$PWD\" in *redwatch*) ;; *) echo x >> {g}/runs; while [ ! -f {g}/go ]; do sleep 0.1; done;; esac\n\
         if [ \"$(cat state.txt)\" = bad ]; then echo 'test alpha ... FAILED <0.05s>'; exit 1; fi\n\
         echo 'test alpha ... ok <0.01s>'\n"
    );
    let turn = json!([
        {"sh": "echo hello > hello.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm change"},
        {"call": {"tool": "run_check", "args": {"name": "tests"}}},
        {"sleep_ms": 100_000}
    ]);
    let s = Scratch::new("false", json!({"steps": {"develop": [turn]}}));
    s.write_project(
        "scratch",
        "false",
        "rerun_filter = \"sh checks.sh {tests}\"",
    );
    s.push_files(
        &[
            (".mason/mason.toml", DEFINITION),
            ("checks.sh", checks.as_str()),
            ("state.txt", "bad\n"),
        ],
        "project setup",
    );
    let d = s.start_with(|c| c.env_retry = Duration::from_millis(200));
    let pid = d.project_id_by_name("scratch").unwrap();
    let a = tip(&d, pid);
    d.store
        .put_baseline(
            &a,
            "only",
            &["alpha".into()],
            &["alpha".into()],
            1,
            mason_daemon::daemon::now_ms(),
        )
        .unwrap();
    let t = file_task_as(&d, "waits", "Do it.", true, TrustSource::Owner, false);
    let runs = || {
        std::fs::read_to_string(dir.path().join("runs"))
            .map(|r| r.lines().count())
            .unwrap_or(0)
    };
    wait_for("the first run starts", 120, || runs() == 1);
    sighted(&d, t, "alpha");
    s.push_files(&[("state.txt", "good\n")], "fix alpha");
    fetch(&s);
    // The run on the old tree is blocked. The watch folds develop in, the run is stopped, and the
    // check is made again on the folded tree; the agent sees one result, with the note.
    redwatch::pass_at(&d, mason_daemon::daemon::now_ms());
    wait_for("the check ran again", 120, || runs() >= 2);
    std::fs::write(dir.path().join("go"), "").unwrap();
    wait_for("the agent has the result", 120, || {
        mason_daemon::http::render_logs(&d, t.0)
            .contains("develop is merged into your branch: run your check again.")
    });
    assert_eq!(runs(), 2, "one run on the old tree, one on the folded tree");
    let logs = mason_daemon::http::render_logs(&d, t.0);
    assert!(!logs.contains("alpha ... FAILED"), "{logs}");
}

fn probed(d: &Arc<mason_daemon::Daemon>, tip: &str, tests: &[&str]) -> bool {
    let t: Vec<String> = tests.iter().map(|x| x.to_string()).collect();
    d.store.red_probed("scratch", "only", tip, &t).unwrap()
}

#[test]
fn a_baseline_of_pseudo_tests_only_is_never_probed() {
    let (s, d, pid) = start(sleeping(), &[]);
    let a = tip(&d, pid);
    let junk = vec![
        "** BUILD FAILED **".to_string(),
        "build".to_string(),
        "/Volumes/x/SIFTDetector.h:15:10: fatal error: 'opencv2/core.hpp' file not found"
            .to_string(),
    ];
    d.store
        .put_baseline(&a, "only", &junk, &junk, 0, mason_daemon::daemon::now_ms())
        .unwrap();
    s.push_files(&[("state.txt", "good\n")], "move develop");
    fetch(&s);
    let b = tip(&d, pid);
    redwatch::pass_at(&d, mason_daemon::daemon::now_ms());
    assert!(d.store.partial_baseline(&b, "only").unwrap().is_none());
    assert!(!probed(&d, &b, &["build"]));
    assert!(!probed(
        &d,
        &b,
        &junk.iter().map(String::as_str).collect::<Vec<_>>()
    ));
}

#[test]
fn an_inconclusive_probe_is_not_repeated_at_a_tip_and_is_tried_once_at_the_next() {
    let (s, d, pid) = start(sleeping(), &[]);
    let a = tip(&d, pid);
    // `delta` fails without printing its name when it is asked for: no answer.
    d.store
        .put_baseline(
            &a,
            "only",
            &["delta".into()],
            &["delta".into()],
            1,
            mason_daemon::daemon::now_ms(),
        )
        .unwrap();
    let count = d
        .cfg
        .worktrees_dir()
        .join(&d.project(pid).unwrap().id)
        .join("probe-count");
    let runs = || {
        std::fs::read_to_string(&count)
            .map(|t| t.lines().count())
            .unwrap_or(0)
    };
    s.push_files(&[("state.txt", "good\n")], "move develop");
    fetch(&s);
    let b = tip(&d, pid);
    redwatch::pass_at(&d, mason_daemon::daemon::now_ms());
    assert_eq!(runs(), 1);
    assert!(probed(&d, &b, &["delta"]));
    redwatch::pass_at(&d, mason_daemon::daemon::now_ms());
    redwatch::pass_at(&d, mason_daemon::daemon::now_ms());
    assert_eq!(runs(), 1, "the same tip is not probed again");
    // The tip moves: one more try.
    s.push_files(&[("state.txt", "better\n")], "move develop again");
    fetch(&s);
    let c = tip(&d, pid);
    assert_ne!(b, c);
    redwatch::pass_at(&d, mason_daemon::daemon::now_ms());
    redwatch::pass_at(&d, mason_daemon::daemon::now_ms());
    assert_eq!(runs(), 2);
    assert!(probed(&d, &c, &["delta"]));
}

#[test]
fn a_baseline_from_another_repository_is_not_probed() {
    let (s, d, pid) = start(sleeping(), &[]);
    // Same suite name, a base that is not a commit of this project.
    d.store
        .put_baseline(
            "0123456789abcdef0123456789abcdef01234567",
            "only",
            &["alpha".into()],
            &["alpha".into()],
            1,
            mason_daemon::daemon::now_ms(),
        )
        .unwrap();
    s.push_files(&[("state.txt", "good\n")], "move develop");
    fetch(&s);
    let b = tip(&d, pid);
    redwatch::pass_at(&d, mason_daemon::daemon::now_ms());
    assert!(!probed(&d, &b, &["alpha"]));
    assert!(d.store.partial_baseline(&b, "only").unwrap().is_none());
}
