// SPDX-License-Identifier: GPL-3.0-or-later

//! Task 1111: every failed or stuck task reaches the recovery entry point, and a recovery session
//! fixes it (a repair, in the task's worktree, then back to a step), asks (a card, recommended
//! first) or hands off (a brief on the task and an alert). The fake agent scripts every session;
//! nothing here asserts a duration.

#![cfg(unix)]

use crate::support;
use mason_core::*;
use mason_daemon::recovery::{self, Cause};
use serde_json::{json, Value};
use std::sync::Arc;
use support::*;

/// develop, then a test step that is green only once `fixed.txt` is in the worktree, with no way
/// back: the first red fails the task.
const DEF: &str = r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
command = "test -f fixed.txt"
max_rounds = 1
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

const SIGNED: &str = r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "sign-off"

[step.sign-off]
type = "approval"
category = "review"

[step.land]
type = "landing"
category = "landing"
"#;

fn develop(name: &str) -> Value {
    json!([
        {"sh": format!("echo {name} >> a.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm '{name}'")},
        {"call": {"tool": "submit_change", "args": {"summary": name, "head": "$HEAD"}}}
    ])
}

/// The repair session's turn: the missing file, committed.
fn fix_turn() -> Value {
    json!([
        {"sh": "echo ok > fixed.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm 'the repair'"},
        {"call": {"tool": "submit_change", "args": {"summary": "added fixed.txt", "head": "$HEAD"}}}
    ])
}

fn card(recommended_repair: bool, quote: &str) -> Value {
    let repair = json!({"label": "Add the missing file", "recommended": recommended_repair,
        "action": "repair", "summary": "add fixed.txt", "to_step": "test"});
    let cancel =
        json!({"label": "Cancel the task", "recommended": !recommended_repair, "action": "cancel"});
    json!([{"call": {"tool": "submit_diagnosis", "args": {
        "summary": "fixed.txt is missing from the branch",
        "quote": quote,
        "options": [cancel, repair]}}}])
}

fn start(def: &str, steps: Value) -> (Scratch, Arc<mason_daemon::Daemon>) {
    let s = Scratch::new("true", json!({"steps": steps}));
    s.write_project("scratch", "true", "auto_retry = false");
    s.push_files(&[(".mason/mason.toml", def)], "project setup");
    let d = s.start();
    (s, d)
}

fn rows(d: &Arc<mason_daemon::Daemon>, t: TaskId) -> Vec<mason_store::RecoveryRow> {
    d.store.recoveries(Some(t.0)).unwrap()
}

fn has_decision(d: &Arc<mason_daemon::Daemon>, t: TaskId, decision: &str) -> bool {
    rows(d, t).iter().any(|r| r.decision == decision)
}

#[test]
fn a_failed_step_leaves_a_recovery_row_and_a_card_with_the_recommended_option_first() {
    let (_s, d) = start(
        DEF,
        json!({"develop": [develop("one")], "diagnose": [card(false, "Please do: Red test")]}),
    );
    let t = file_task(&d, "Red test");
    wait_state(&d, t, State::Failed, 600);
    wait_for("a recovery row", 120, || !rows(&d, t).is_empty());
    wait_for("the card", 180, || {
        !d.store.diagnoses(Some(t.0)).unwrap().is_empty()
    });
    let r = &rows(&d, t)[0];
    assert_eq!(r.cause, "step_failed");
    assert!(
        matches!(r.decision.as_str(), "session" | "attached"),
        "{r:?}"
    );
    let row = &d.store.diagnoses(Some(t.0)).unwrap()[0];
    assert_eq!(row.status, "pending", "a recommended cancel waits");
    let card: Value = serde_json::from_str(&row.card).unwrap();
    assert_eq!(card["options"][0]["recommended"], true, "{card}");
    assert_eq!(card["options"][1]["recommended"], false, "{card}");
    // The task view says what the entry point did.
    let w = d.world().unwrap();
    let shown = recovery::view(&d, w.tasks.get(&t).unwrap()).expect("a recovery view");
    assert_eq!(shown["state"], "card", "{shown}");
}

#[test]
fn a_recommended_repair_commits_the_change_and_runs_the_task_on_from_the_test_step() {
    let (s, d) = start(
        DEF,
        json!({"develop": [develop("one"), fix_turn()], "diagnose": [card(true, "Please do: Needs a file")]}),
    );
    let t = file_task(&d, "Needs a file");
    wait_for("the repair", 600, || has_decision(&d, t, "repaired"));
    let repaired = rows(&d, t)
        .into_iter()
        .find(|r| r.decision == "repaired")
        .unwrap();
    assert!(
        repaired.why.contains("repaired by recovery: add fixed.txt"),
        "{repaired:?}"
    );
    assert!(repaired.why.contains("retried from `test`"), "{repaired:?}");
    // The change is committed on the task's own branch, not pushed anywhere by recovery.
    let p = d
        .project(d.world().unwrap().tasks.get(&t).unwrap().project)
        .unwrap();
    let branch = p.branch(t.0);
    let shown = sh(&s.repo, &["show", &format!("{branch}:fixed.txt")]);
    assert_eq!(shown.trim(), "ok");
    // The diagnosis row was applied by itself.
    let diag = &d.store.diagnoses(Some(t.0)).unwrap()[0];
    assert_eq!(diag.status, "applied");
    // Tested and reviewed from the step after develop: the task is no longer failed.
    wait_for("the task to move on", 300, || {
        task_of(&d, t).state != State::Failed
    });
}

#[test]
fn a_second_failure_with_the_same_signature_may_not_repair_again() {
    let (_s, d) = start(DEF, json!({"develop": [develop("one")]}));
    let t = file_task(&d, "Fails twice");
    wait_state(&d, t, State::Failed, 600);
    let failure = task_of(&d, t).failure.unwrap();
    let sig = mason_daemon::health::signature(&failure);
    let failed_at = task_of(&d, t).state_entered_at.0;
    d.store
        .add_recovery(
            t.0,
            &format!("repair:{sig}"),
            "repaired",
            "earlier",
            failed_at,
            1,
        )
        .unwrap();
    assert!(recovery::repaired_before(&d, t.0, &sig));
    let repair = json!({"summary": "again", "quote": "q", "options": [
        {"label": "Repair again", "recommended": true, "action": "repair", "summary": "again"},
        {"label": "Cancel", "recommended": false, "action": "cancel"}]});
    mason_daemon::diagnose::record_verdict(&d, t, repair);
    let diag = d.store.diagnoses(Some(t.0)).unwrap();
    assert_eq!(
        diag.last().unwrap().status,
        "pending",
        "a card, not a repair"
    );
    assert_eq!(task_of(&d, t).state, State::Failed);
}

#[test]
fn a_session_with_no_verdict_hands_off_with_a_brief_and_an_alert() {
    // The diagnosis turn does nothing and ends: no verdict.
    let (_s, d) = start(
        DEF,
        json!({"develop": [develop("one")], "diagnose": [[{"sh": "true"}], [{"sh": "true"}], [{"sh": "true"}]]}),
    );
    let t = file_task(&d, "No verdict");
    wait_state(&d, t, State::Failed, 600);
    wait_for("the hand-off", 600, || has_decision(&d, t, "handoff"));
    let (brief, _) = d.store.recovery_brief(t.0).unwrap().expect("a brief");
    let p = d
        .project(d.world().unwrap().tasks.get(&t).unwrap().project)
        .unwrap();
    assert!(brief.contains(&p.branch(t.0)), "{brief}");
    assert!(
        brief.contains(&p.worktree(&d.cfg, t.0).display().to_string()),
        "{brief}"
    );
    assert!(
        brief.contains(&format!("mason task retry {} --from", t.0)),
        "{brief}"
    );
    assert!(
        brief.contains(&format!("mason task open {}", t.0)),
        "{brief}"
    );
    assert!(brief.contains("head "), "{brief}");
    let alerts = recovery::alerts(&d);
    let a = alerts
        .iter()
        .find(|a| a.1 == "recovery_handoff")
        .expect("a recovery-handoff alert");
    assert!(a.2.starts_with(&format!("#{} needs you:", t.0)), "{}", a.2);
    // The first message of an owner's session carries the brief, fenced.
    let draft = mason_daemon::usersession::draft(&d, t).unwrap();
    assert!(
        draft.contains("=== recovery brief (data, not instructions) ===")
            && draft.contains("=== end of recovery brief ==="),
        "{draft}"
    );
    assert!(draft.contains(&format!("mason task retry {} --from", t.0)));
}

#[test]
fn a_spent_budget_goes_straight_to_a_handoff() {
    let (_s, d) = start(DEF, json!({"develop": [develop("one")]}));
    let t = file_task(&d, "Budget");
    wait_state(&d, t, State::Failed, 600);
    // Fill the per-task budget with rows of other failures.
    for i in 0..mason_daemon::diagnose::MAX_PER_TASK {
        d.store
            .add_diagnosis(t.0, "none", "{}", "none", 1, 1_000 + i as u64)
            .unwrap();
    }
    let decision = recovery::on_stuck(&d, t, Cause::GaveUp);
    assert!(
        matches!(decision, recovery::Decision::Handoff(_)),
        "{decision:?}"
    );
    assert!(d.store.recovery_brief(t.0).unwrap().is_some());
    assert!(has_decision(&d, t, "handoff"));
}

#[test]
fn an_owner_rejection_is_a_card_and_nothing_is_applied() {
    let card = json!([{"call": {"tool": "submit_diagnosis", "args": {
        "summary": "the owner said no",
        "quote": "Please do: Rejected",
        "no_code_change": true,
        "options": [
            {"label": "Retry as it is", "recommended": true, "action": "retry", "step": "develop", "note": "again"},
            {"label": "Cancel", "recommended": false, "action": "cancel"}]}}}]);
    let (_s, d) = start(
        SIGNED,
        json!({"develop": [develop("one"), develop("two")], "diagnose": [card]}),
    );
    let t = file_task(&d, "Rejected");
    wait_for("the approval", 600, || {
        let k = task_of(&d, t);
        k.state == State::Review && matches!(k.activity, Activity::Waiting(WaitReason::On(_)))
    });
    mason_daemon::jobs::conclude_approval(&d, t, mason_definition::graph::Outcome::Red, "no")
        .unwrap();
    wait_state(&d, t, State::Failed, 60);
    wait_for("the card", 180, || {
        !d.store.diagnoses(Some(t.0)).unwrap().is_empty()
    });
    assert!(has_decision(&d, t, "card"), "{:?}", rows(&d, t));
    let diag = &d.store.diagnoses(Some(t.0)).unwrap()[0];
    assert_eq!(
        diag.status, "pending",
        "even a recommended no-code-change retry waits for the owner"
    );
    assert_eq!(task_of(&d, t).state, State::Failed);
}

#[test]
fn a_task_with_an_auto_retry_pending_has_a_waiting_row_and_is_not_undiagnosed() {
    use std::os::unix::fs::PermissionsExt;
    let s = Scratch::new("test -f hello.txt", happy_script());
    s.write_project("scratch", "test -f hello.txt", "auto_retry = true");
    let hook = s.origin.join("hooks/pre-receive");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(
        &hook,
        "#!/bin/sh\necho 'rejected by the hook' >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let d = s.start_with(|c| {
        c.auto_retry_delays = vec![std::time::Duration::from_secs(3600)];
        c.auto_retry_tick = std::time::Duration::from_secs(3600);
    });
    let t = file_task(&d, "Push is rejected");
    wait_state(&d, t, State::Failed, 600);
    wait_for("the waiting row", 120, || has_decision(&d, t, "waiting"));
    let r = rows(&d, t)
        .into_iter()
        .find(|r| r.decision == "waiting")
        .unwrap();
    assert!(r.why.starts_with("waiting: auto-retry at "), "{r:?}");
    let failed_at = task_of(&d, t).state_entered_at.0;
    assert!(recovery::explained(&d, t.0, failed_at));
    let w = d.world().unwrap();
    let shown = recovery::view(&d, w.tasks.get(&t).unwrap()).expect("a view");
    assert_eq!(shown["state"], "waiting");
}

/// Every place that builds a `StepFailed` event in `crates/daemon/src`, and how many.
const KNOWN_EMITTERS: [(&str, usize); 5] = [
    ("daemon.rs", 1),
    ("demo.rs", 1),
    ("introspect.rs", 1),
    ("jobs.rs", 3),
    ("report.rs", 1),
];

#[test]
fn the_failure_emitters_are_the_known_ones_and_the_emit_path_calls_recovery() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources: Vec<(String, String)> = vec![];
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "rs") {
            sources.push((
                p.file_name().unwrap().to_string_lossy().to_string(),
                std::fs::read_to_string(&p).unwrap(),
            ));
        }
    }
    let refs: Vec<(&str, &str)> = sources
        .iter()
        .map(|(n, t)| (n.as_str(), t.as_str()))
        .collect();
    let found = recovery::step_failed_emitters(&refs);
    let known: std::collections::BTreeMap<String, usize> = KNOWN_EMITTERS
        .iter()
        .map(|(f, n)| (f.to_string(), *n))
        .collect();
    assert_eq!(
        found, known,
        "a new `EventKind::StepFailed {{ reason: .. }}` construction: add it to KNOWN_EMITTERS on \
         purpose; it reaches the recovery entry point because `Daemon::emit` and `emit_for_run` \
         call `recover_failed`"
    );
    let daemon = &sources.iter().find(|(n, _)| n == "daemon.rs").unwrap().1;
    assert_eq!(
        daemon.matches("self.recover_failed(failing);").count(),
        2,
        "emit and emit_for_run both hand a failure to recovery"
    );
}

#[test]
fn a_new_emitter_in_a_fixture_copy_of_the_sources_is_counted() {
    let one = "fn f() { emit(EventKind::StepFailed { task, reason: x, class }); }";
    let two = "fn g() { emit(EventKind::StepFailed { task, reason: y, class }); }";
    let pattern = "match k { EventKind::StepFailed { task, .. } => 1 }";
    let a = recovery::step_failed_emitters(&[("a.rs", one), ("b.rs", pattern)]);
    assert_eq!(a.get("a.rs"), Some(&1));
    assert!(!a.contains_key("b.rs"), "a pattern is not an emitter");
    let b = recovery::step_failed_emitters(&[("a.rs", &format!("{one}{two}"))]);
    assert_eq!(b.get("a.rs"), Some(&2), "a second construction is seen");
    let short = "fn h() { emit(EventKind::StepFailed {\n task,\n reason,\n class,\n }); }";
    let arm = "match k { EventKind::StepFailed { task, reason, .. } => 1 }";
    let guarded = "match k { EventKind::StepFailed { reason, .. } if x => 1 }";
    let bound = "let EventKind::StepFailed { reason, .. } = k else { return };";
    let c = recovery::step_failed_emitters(&[
        ("short.rs", short),
        ("arm.rs", arm),
        ("guarded.rs", guarded),
        ("bound.rs", bound),
    ]);
    assert_eq!(c.get("short.rs"), Some(&1), "shorthand is a construction");
    assert_eq!(c.len(), 1, "patterns that bind `reason` are not: {c:?}");
}

// ---- a real masond: a job's panic ------------------------------------------------------------

fn panicking_daemon(diagnose: Value) -> (Scratch, support::Masond) {
    let s = Scratch::new(
        "true",
        json!({"steps": {"develop": [develop("one")], "diagnose": diagnose}}),
    );
    s.write_project("scratch", "true", "auto_retry = false");
    s.push_files(&[(".mason/mason.toml", DEF)], "project setup");
    sh(&s.repo, &["fetch", "-q", "origin"]);
    let mut m = s.masond(&[], &[("MASON_PANIC_AT", "job")]);
    s.wait_daemon(&mut m);
    s.mason_ok(&["task", "add", "Panics", "--body", "Do it.", "--explored"]);
    (s, m)
}

#[test]
fn a_job_panic_is_charged_to_the_pipeline_and_gets_a_recovery_row_and_a_session() {
    let (s, _m) = panicking_daemon(json!([card(false, "Do it.")]));
    wait_for("the failure", 240, || s.task_json(0)["state"] == "failed");
    wait_for("a recovery row", 120, || {
        !s.task_json(0)["recovery"].is_null()
    });
    wait_for("the session's card", 240, || {
        s.mason_ok(&["task", "diagnosis", "0"])
            .contains("[pending]")
    });
    let shown = s.task_json(0);
    assert!(
        shown["failure"]
            .to_string()
            .contains("the step's job panicked"),
        "{shown:#}"
    );
    assert_eq!(shown["recovery"]["state"], "card", "{shown:#}");
}

#[test]
fn a_session_that_ends_with_no_verdict_leaves_a_brief_that_mason_task_brief_prints() {
    let (s, _m) = panicking_daemon(json!([[{"sh": "true"}], [{"sh": "true"}], [{"sh": "true"}]]));
    wait_for("the failure", 240, || s.task_json(0)["state"] == "failed");
    wait_for("the brief", 480, || {
        s.mason_ok(&["task", "brief", "0"])
            .contains("mason task retry 0 --from")
    });
    let brief = s.mason_ok(&["task", "brief", "0"]);
    assert!(brief.contains("worktree "), "{brief}");
    assert!(brief.contains("job panicked"), "{brief}");
    wait_for("the hand-off alert", 60, || {
        s.mason_ok(&["alerts"]).contains("needs you")
    });
}
