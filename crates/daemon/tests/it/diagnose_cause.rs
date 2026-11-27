// SPDX-License-Identifier: GPL-3.0-or-later

//! s1176 (decision 1166): a cause outside the tasks' code is diagnosed once, with the facts MASON
//! already has, and the card pairs a fix-now with a fix-later action. The fake agent records the
//! prompt it is given; the CI is a fake `Source`; nothing here asserts a duration.

#![cfg(unix)]

use crate::support;
use mason_adapters::github::{ActionsJob, ActionsRun};
use mason_core::*;
use mason_daemon::ci_watch::Source;
use mason_daemon::diagcause::{self, Started, Trigger};
use mason_daemon::{attention, Daemon};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use support::*;

const TIMEOUT: &str =
    "The job running on runner X has exceeded the maximum execution time of 360 minutes.";

/// The CI as it was on 2026-11-24: the batch's run never finishes with the windows job running,
/// and the base's newest run of the same job was cancelled at the time limit.
struct Fake;

fn run(id: u64, sha: &str, conclusion: Option<&str>) -> ActionsRun {
    ActionsRun {
        id,
        workflow: "v3-gate".into(),
        sha: sha.into(),
        url: format!("https://ci.example/runs/{id}"),
        created_ms: id,
        conclusion: conclusion.map(String::from),
        event: "push".into(),
    }
}

fn job(id: u64, name: &str, status: &str, conclusion: Option<&str>) -> ActionsJob {
    ActionsJob {
        id,
        name: name.into(),
        conclusion: conclusion.map(String::from),
        status: status.into(),
    }
}

impl Source for Fake {
    fn runs(&self, branch: &str) -> Result<Vec<ActionsRun>, String> {
        Ok(if branch == "main" {
            vec![
                run(200, "bbbbbbbb", Some("failure")),
                run(190, "deadbeefdeadbeef", Some("success")),
            ]
        } else {
            vec![run(100, "cccccccc", None)]
        })
    }
    fn jobs(&self, run: u64) -> Result<Vec<ActionsJob>, String> {
        Ok(match run {
            100 => vec![
                job(1001, "trd (windows-2022)", "in_progress", None),
                job(1002, "trd (ubuntu)", "completed", Some("success")),
            ],
            200 => vec![
                job(2001, "trd (windows-2022)", "completed", Some("cancelled")),
                job(2002, "trd (ubuntu)", "completed", Some("success")),
            ],
            190 => vec![
                job(1901, "trd (windows-2022)", "completed", Some("success")),
                job(1902, "trd (ubuntu)", "completed", Some("success")),
            ],
            _ => vec![],
        })
    }
    fn log_tail(&self, job: u64) -> Option<String> {
        self.log_long(job)
    }
    fn log_long(&self, job: u64) -> Option<String> {
        (job == 1001).then(|| "test scan_all ... running\nSCAN-HANG-LINE tests/scan.rs:41".into())
    }
    fn annotations(&self, job: u64) -> Option<String> {
        (job == 2001).then(|| TIMEOUT.into())
    }
}

fn reject_pushes(s: &Scratch) {
    let hook = s.origin.join("hooks/pre-receive");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(
        &hook,
        "#!/bin/sh\necho 'rejected by the hook' >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A diagnose turn that records its prompt, then submits `card`.
fn turn(rec: &Path, card: Value) -> Value {
    json!([
        {"sh": format!("printf '%s' \"$FAKE_PROMPT\" > {}/prompt-$$.txt", rec.display())},
        {"call": {"tool": "submit_diagnosis", "args": card}}
    ])
}

/// A turn that ends without a verdict.
fn silent() -> Value {
    json!([{"sh": "true"}])
}

fn replay_card() -> Value {
    json!({
        "summary": "The trd windows job hangs in tests/scan.rs on the base too since task 1000 landed.",
        "cause": "outside",
        "evidence_lines": ["tests/scan.rs:41 SCAN-HANG-LINE"],
        "options": [
            {"label": "File the scan fix, then land now", "recommended": true, "actions": [
                {"action": "file_task", "project": "scratch",
                 "title": "Fix the hanging scan tests on windows",
                 "body": "The scan tests hang.\n\n**Cause** tests/scan.rs:41",
                 "evidence": "tests/scan.rs:41 the trd (windows-2022) job hangs on the base since #1000"},
                {"action": "land_now", "reason": "the CI hangs on the base too"}]},
            {"label": "Retry later", "recommended": false, "action": "retry", "note": "try again"}]
    })
}

struct World {
    /// Kept: it owns the scratch repositories and the directories.
    #[allow(dead_code)]
    s: Scratch,
    rec: tempfile::TempDir,
    d: Arc<Daemon>,
}

fn world(cards: Vec<Value>, extra: &str) -> World {
    let rec = tempfile::tempdir().unwrap();
    let mut script = happy_script();
    script["steps"]["diagnose"] = json!(cards
        .into_iter()
        .map(|c| if c.is_array() { c } else { turn(rec.path(), c) })
        .collect::<Vec<_>>());
    let s = Scratch::new("test -f hello.txt", script);
    s.write_project(
        "scratch",
        "test -f hello.txt",
        &format!("auto_retry = true\ndiagnose = false\n{extra}"),
    );
    // What landed before: the merge of task 1000.
    s.push_files(&[("a.txt", "1")], "Merge branch 'mason/scratch/t1000'");
    reject_pushes(&s);
    let d = s.start_with(|c| {
        c.auto_retry_delays = vec![Duration::ZERO, Duration::ZERO];
        c.auto_retry_tick = Duration::from_secs(3600);
    });
    diagcause::set_source(&d, Arc::new(Fake));
    World { s, rec, d }
}

impl World {
    fn failed_task(&self, title: &str) -> TaskId {
        let t = file_task(&self.d, title);
        wait_for("the outside failure", 180, || {
            let k = task_of(&self.d, t);
            k.state == State::Failed && k.failure_class == Some(FailClass::Outside)
        });
        t
    }
    fn pid(&self) -> ProjectId {
        self.d.project_id_by_name("scratch").unwrap()
    }
    fn batch_for(&self, tasks: &[TaskId]) -> u64 {
        let ids: Vec<String> = tasks.iter().map(|t| t.0.to_string()).collect();
        let b = self
            .d
            .store
            .add_batch("scratch", &ids.join(","), now())
            .unwrap();
        self.d
            .landing()
            .ci
            .start(b, &format!("mason/ci/scratch-b{b}-1"), now());
        b
    }
    fn start(&self, t: TaskId) -> Result<Started, String> {
        let k = task_of(&self.d, t);
        self.start_keyed(
            t,
            diagcause::key_for_failure("scratch", &k.failure.unwrap_or_default()),
        )
    }
    fn start_keyed(&self, t: TaskId, key: String) -> Result<Started, String> {
        diagcause::start_cause(
            &self.d,
            key,
            self.pid(),
            vec![t.0],
            StepRunId(mason_daemon::http::latest_run(&self.d.world().unwrap(), t)),
            Trigger::Landing,
        )
    }
    fn rows(&self) -> Vec<mason_store::DiagnosisRow> {
        self.d.store.diagnoses(None).unwrap()
    }
    fn prompts(&self) -> Vec<String> {
        std::fs::read_dir(self.rec.path())
            .unwrap()
            .flatten()
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .collect()
    }
    fn retries(&self, t: TaskId, auto: bool) -> usize {
        self.d
            .store
            .read_since(0, 100_000)
            .unwrap()
            .iter()
            .filter(|e| matches!(&e.event.kind, EventKind::Retried { task, auto: a, .. } if *task == t && *a == auto))
            .count()
    }
    fn answer(&self, t: TaskId, n: &str) -> Result<String, (u16, String)> {
        let who = self.d.cfg.owner_names[0].clone();
        let (_, hash) = attention::card_of(&self.d, &task_of(&self.d, t)).expect("a card");
        attention::answer_with(&self.d, t, n, &hash, &who, None)
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[test]
fn the_1003_replay_holds_the_retry_and_the_card_files_the_fix_and_lands_now() {
    let w = world(vec![replay_card()], "");
    let t = w.failed_task("Trd batch waits on CI");
    w.batch_for(&[t]);
    assert_eq!(w.start(t).unwrap(), Started::Began);
    // The retry is held at once, before the session has said anything.
    assert!(diagcause::held(&w.d, t.0));
    wait_for("the card", 180, || {
        w.rows().iter().any(|r| r.status == "pending")
    });
    assert_eq!(w.rows().len(), 1, "one diagnosis");
    let prompt = w.prompts().join("\n");
    for want in [
        "SCAN-HANG-LINE",
        "[same on the base]",
        "(task #1000)",
        "trd (windows-2022)",
    ] {
        assert!(prompt.contains(want), "missing {want:?} in:\n{prompt}");
    }
    // The card names a fix-now and a fix-later action in one option.
    let card: Value = serde_json::from_str(&w.rows()[0].card).unwrap();
    let acts = card["options"][0]["action"]["actions"].as_array().unwrap();
    assert_eq!(acts[0]["action"], "file_task");
    assert_eq!(acts[1]["action"], "land_now");
    // It holds the retry while open, and shows on the task.
    mason_daemon::autoretry::pass(&w.d);
    assert_eq!(w.retries(t, true), 0, "the automatic retry is held");
    assert!(attention::card_of(&w.d, &task_of(&w.d, t)).is_some());
    // The alert: kind `diagnosis`, the one line.
    let alerts = attention::alerts_now(&w.d);
    let a = alerts
        .iter()
        .find(|a| a.1 == "diagnosis")
        .unwrap_or_else(|| panic!("no diagnosis alert in {alerts:?}"));
    assert!(a.2.starts_with(&format!("#{}: ", t.0)), "{}", a.2);
    assert!(
        a.2.ends_with("Recommended: File the scan fix, then land now."),
        "{}",
        a.2
    );
    // The owner's one choice files the ticket and lands now, with its debt.
    w.answer(t, "1").unwrap();
    let filed = w.d.store.proposals_in("scratch").unwrap();
    assert_eq!(filed.len(), 1, "one task filed");
    assert!(filed[0].title.contains("scan tests"));
    let debts = w.d.store.debts(Some(t.0), false).unwrap();
    assert_eq!(debts.len(), 1, "{debts:?}");
    assert!(debts[0].scope.contains("land_now"), "{debts:?}");
    assert_eq!(w.rows()[0].status, "applied");
}

#[test]
fn a_file_task_that_repeats_an_open_ticket_is_refused_and_the_card_says_which() {
    let w = world(vec![replay_card()], "");
    let t = w.failed_task("Trd batch waits on CI");
    w.batch_for(&[t]);
    // The ticket is already open.
    let row = mason_daemon::proposals::file(
        &w.d,
        &mason_daemon::proposals::Filing {
            from_task: None,
            project: w.pid(),
            title: "Fix the hanging scan tests on windows".into(),
            body: "The scan tests hang.".into(),
            evidence: "tests/scan.rs:41 the windows job hangs on the base".into(),
            blocks_task: false,
            explored: false,
            estimate: None,
        },
    )
    .unwrap();
    w.start(t).unwrap();
    wait_for("the card", 180, || {
        w.rows().iter().any(|r| r.status == "pending")
    });
    let err = w.answer(t, "1").unwrap_err();
    assert!(
        err.1.contains(&format!("duplicate of #{}", row.task.0)),
        "{err:?}"
    );
    assert_eq!(w.rows()[0].status, "pending", "nothing was applied");
    assert!(w.d.store.debts(Some(t.0), false).unwrap().is_empty());
}

#[test]
fn two_tasks_of_one_cause_get_one_diagnosis_and_one_card_that_acts_on_both() {
    let mut card = replay_card();
    // A card whose recommendation is a plain retry, so applying it acts on each task.
    card["options"] = json!([
        {"label": "Retry both", "recommended": true, "actions": [
            {"action": "retry", "note": "again"},
            {"action": "file_task", "project": "scratch", "title": "Make the push hook lenient",
             "body": "The hook refuses pushes.", "evidence": "hooks/pre-receive:1 refuses every push"}]},
        {"label": "Cancel", "recommended": false, "action": "cancel"}]);
    let w = world(vec![card], "");
    let a = w.failed_task("First rejected");
    let b = w.failed_task("Second rejected");
    // The two failures are one cause (the push hook's message carries a commit id, which a
    // signature does not fold, so the key is given).
    let key = "sig:scratch:push rejected by the hook".to_string();
    assert_eq!(w.start_keyed(a, key.clone()).unwrap(), Started::Began);
    // The second failure of the cause joins; it starts no session.
    assert!(matches!(w.start_keyed(b, key).unwrap(), Started::Joined(_)));
    wait_for("the card", 180, || {
        w.rows().iter().any(|r| r.status == "pending")
    });
    assert_eq!(w.rows().len(), 1);
    // The card is on each task, and lists both.
    for t in [a, b] {
        let (card, _) = attention::card_of(&w.d, &task_of(&w.d, t)).expect("the card on the task");
        assert!(card.ask.contains(&format!("#{}", a.0)) && card.ask.contains(&format!("#{}", b.0)));
    }
    w.answer(b, "1").unwrap();
    assert!(
        w.retries(a, false) >= 1 && w.retries(b, false) >= 1,
        "each task was retried"
    );
    assert_eq!(
        w.d.store.proposals_in("scratch").unwrap().len(),
        1,
        "one ticket for the card"
    );
}

#[test]
fn a_session_without_a_verdict_releases_the_held_retry() {
    let w = world(vec![silent()], "");
    let t = w.failed_task("Rejected");
    assert_eq!(w.start(t).unwrap(), Started::Began);
    assert!(diagcause::held(&w.d, t.0));
    wait_for("the row with no verdict", 180, || {
        w.rows().iter().any(|r| r.status == "none")
    });
    wait_for("the hold to lapse", 60, || !diagcause::held(&w.d, t.0));
    mason_daemon::autoretry::pass(&w.d);
    assert_eq!(w.retries(t, true), 1, "the automatic retry went ahead");
}

#[test]
fn a_recommended_retry_releases_the_retry_and_waits_for_the_loop() {
    let card = json!({
        "summary": "The hook refused the push once; trying again may work.",
        "cause": "outside",
        "options": [
            {"label": "Retry now", "recommended": true, "actions": [
                {"action": "retry", "note": "again"},
                {"action": "wait_for", "task": 999}]},
            {"label": "Cancel", "recommended": false, "action": "cancel"}]
    });
    // `wait_for` names a task that does not exist, so apply would refuse it; the card is only
    // checked for what it does to the held retry: a compound is the owner's to apply.
    let w = world(vec![card], "");
    let t = w.failed_task("Rejected");
    w.start(t).unwrap();
    wait_for("the card", 180, || {
        w.rows().iter().any(|r| r.status == "pending")
    });
    assert!(
        diagcause::held(&w.d, t.0),
        "a compound option waits for the owner, and the retry with it"
    );
    // A card whose recommended option is a lone retry does not hold it.
    let single = json!({
        "summary": "The hook refused the push once.",
        "options": [
            {"label": "Retry", "recommended": true, "action": "retry", "note": "again"},
            {"label": "Cancel", "recommended": false, "action": "cancel"}]
    });
    let u = w.failed_task("Rejected again");
    // Recorded as the session would have, with the cause fields.
    let mut a = single;
    a["kind"] = json!("cause");
    a["cause_key"] = json!("sig:scratch:other");
    a["project"] = json!("scratch");
    a["tasks"] = json!([u.0]);
    diagcause::record_cause(&w.d, u, a);
    assert!(
        !diagcause::held(&w.d, u.0),
        "a recommended retry releases the hold"
    );
    mason_daemon::autoretry::pass(&w.d);
    assert_eq!(w.retries(u, true), 1, "the loop made the retry");
}

#[test]
fn the_fifth_cause_diagnosis_of_a_day_is_refused() {
    let w = world(vec![silent()], "");
    let t = w.failed_task("Rejected");
    for i in 0..diagcause::CAUSE_PER_DAY {
        let args = json!({"kind": "cause", "cause_key": format!("other:{i}"),
                          "project": "scratch", "tasks": [9000 + i]});
        w.d.store
            .add_diagnosis(9000 + i as u64, "none", &args.to_string(), "none", now(), 0)
            .unwrap();
    }
    let why = w.start(t).unwrap_err();
    assert!(why.contains("the most there are"), "{why}");
    assert_eq!(w.rows().len(), diagcause::CAUSE_PER_DAY);
}

/// The landing world of `ci_stuck.rs`: the batch's CI stays pending with the same jobs.
fn stuck_world() -> support::lw::W {
    use support::lw::*;
    let toml = "mode = \"direct-ci\"\nwindow = \"30s\"\nmax_size = 2\nmax_fold = 4\n[[tier]]\nname = \"batch\"\nsuites = [\"alpha\"]\n";
    world_with(
        toml,
        &|_| {
            let status = "echo pending; echo 'running: trd (windows-2022)'; echo 'queued: 2'";
            format!(
                "[ci]\nkind = \"command\"\ntrigger = \"echo $MASON_SHA\"\nstatus = \"{}\"\npoll_s = 1\ntimeout_s = 300\nstuck_after = \"1s\"\n",
                q(status)
            )
        },
        &[],
    )
}

#[test]
fn a_ci_stuck_alert_starts_a_batch_diagnosis_while_the_batch_is_still_waiting() {
    let w = stuck_world();
    let rec = tempfile::tempdir().unwrap();
    let mut script = json!({"steps": {}});
    script["steps"]["diagnose"] = json!([turn(rec.path(), replay_card())]);
    std::fs::write(&w.s.script, script.to_string()).unwrap();
    diagcause::set_source(&w.d, Arc::new(Fake));
    let t = w.task("slow CI", &[("x/ok.txt", "1")]);
    wait_for("a diagnosis of the stuck batch", 120, || {
        diagcause::pass(&w.d);
        w.d.store
            .diagnoses(None)
            .unwrap()
            .iter()
            .any(|r| r.status == "pending")
    });
    // The batch is still waiting; the card is on its task, and it names the batch's job.
    assert_eq!(task_of(&w.d, t).state, State::Landing);
    let (card, _) = attention::card_of(&w.d, &task_of(&w.d, t)).expect("the card on the task");
    assert!(card.ask.contains(&format!("#{}", t.0)), "{}", card.ask);
    let row = w.d.store.diagnoses(None).unwrap().remove(0);
    let args: Value = serde_json::from_str(&row.args).unwrap();
    assert_eq!(args["kind"], "cause");
    assert!(args["cause_key"]
        .as_str()
        .unwrap()
        .starts_with("batch:scratch:"));
    // Asking again, as the loop does every tick, starts nothing more.
    diagcause::pass(&w.d);
    assert_eq!(w.d.store.diagnoses(None).unwrap().len(), 1);
}

#[test]
fn the_card_raises_a_diagnosis_alert_that_notify_toml_routes() {
    let w = world(vec![replay_card()], "");
    let t = w.failed_task("Trd batch waits on CI");
    w.batch_for(&[t]);
    w.start(t).unwrap();
    wait_for("the card", 180, || {
        w.rows().iter().any(|r| r.status == "pending")
    });
    let line = attention::alerts_now(&w.d)
        .into_iter()
        .find(|a| a.1 == "diagnosis")
        .map(|a| a.2)
        .expect("a diagnosis alert");
    // A test `notify.toml` routes it like any other condition: speech now, nothing else.
    let st = mason_daemon::notify::parse("[routing.diagnosis]\nspeech = \"0\"\n").unwrap();
    let rules = st.routing.rules("diagnosis");
    assert!(rules
        .iter()
        .any(|r| r.channel == mason_core::notify::Channel::Speech));
    assert!(rules
        .iter()
        .all(|r| !matches!(r.channel, mason_core::notify::Channel::Push)));
    // The line spoken is the one line, whole.
    let said = mason_core::notify::sentence("diagnosis", "MASON", &line, false);
    assert_eq!(said, line, "spoken as it is written");
    assert!(line.contains("Recommended:"), "{line}");
}
