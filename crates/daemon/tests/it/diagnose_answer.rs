// SPDX-License-Identifier: GPL-3.0-or-later

//! s1116: a failed task holds its pending diagnosis' card, the owner answers it like any other,
//! and every answer may carry context. The fake agent records the prompts it is given. Nothing
//! here asserts a duration.

#![cfg(unix)]

use crate::support;
use mason_core::*;
use mason_daemon::attention;
use mason_daemon::views::Ctx;
use mason_daemon::Daemon;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use support::*;

const END: &str = "=====END-OF-PROMPT=====";

fn def(gate: &Path) -> String {
    format!(
        r#"schema = 1
start = "explore"

[step.explore]
type = "agent"
category = "exploring"
next = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
command = "test -f {}/ok"
on_red = "develop"
max_rounds = 1
next = "land"

[step.land]
type = "landing"
category = "landing"
"#,
        gate.display()
    )
}

/// A develop turn that records its prompt, commits and submits.
fn turn(rec: &Path) -> Value {
    json!([
        {"sh": format!("printf '%s\\n{END}\\n' \"$FAKE_PROMPT\" >> {}/dev.txt", rec.display())},
        {"sh": "echo $(date +%s%N) >> a.txt && git add a.txt && git -c user.name=t -c user.email=t@t commit -qm round"},
        {"call": {"tool": "submit_change", "args": {"summary": "round", "head": "$HEAD"}}}
    ])
}

/// An explore turn that records its prompt.
fn plan_turn(rec: &Path) -> Value {
    let mut t = explore_turn();
    let record = json!({"sh": format!("printf '%s\\n{END}\\n' \"$FAKE_PROMPT\" >> {}/explore.txt", rec.display())});
    t.as_array_mut().unwrap().insert(0, record);
    t
}

/// A scratch world whose test step is red until `gate/ok` exists, so a task fails after one
/// develop round.
struct World {
    s: Scratch,
    rec: tempfile::TempDir,
    gate: tempfile::TempDir,
}

fn world(extra: &str, mut steps: Value) -> World {
    let rec = tempfile::tempdir().unwrap();
    let gate = tempfile::tempdir().unwrap();
    if steps.get("explore").is_none() {
        steps["explore"] = json!(vec![plan_turn(rec.path()); 6]);
    }
    if steps.get("develop").is_none() {
        steps["develop"] = json!(vec![turn(rec.path()); 12]);
    }
    let s = Scratch::new("false", json!({"steps": steps}));
    s.write_project("scratch", "false", &format!("auto_retry = false\n{extra}"));
    s.push_files(&[(".mason/mason.toml", &def(gate.path()))], "project setup");
    World { s, rec, gate }
}

impl World {
    /// The prompts the develop sessions were given, oldest first.
    fn prompts(&self) -> Vec<String> {
        self.recorded("dev.txt")
    }
    fn recorded(&self, file: &str) -> Vec<String> {
        std::fs::read_to_string(self.rec.path().join(file))
            .unwrap_or_default()
            .split(END)
            .map(str::to_string)
            .filter(|p| !p.trim().is_empty())
            .collect()
    }
    fn open_gate(&self) {
        std::fs::write(self.gate.path().join("ok"), "").unwrap();
    }
}

/// Four options, one of each action; `cancel` is recommended so nothing applies by itself.
/// The card lists the recommended option first, so answer by label, not by number.
fn submission() -> Value {
    json!({"summary": "the gate file is missing", "options": [
        {"label": "Retry with the gate", "recommended": false, "action": "retry",
         "step": "develop", "note": "DIAG-NOTE write the gate"},
        {"label": "Plan it again", "recommended": false, "action": "explore",
         "reason": "DIAG-REASON the plan has no gate"},
        {"label": "Close it as done", "recommended": false, "action": "close",
         "kind": "already_done", "evidence": "DIAG-EVIDENCE it is on main"},
        {"label": "Cancel the task", "recommended": true, "action": "cancel"}]})
}

/// A task that has failed, with no diagnosis yet.
fn failed(d: &Arc<Daemon>, title: &str) -> TaskId {
    let t = file_task(d, title);
    wait_state(d, t, State::Failed, 600);
    t
}

/// Store a diagnosis of `t` and give back the hash of the card it now holds.
fn diagnose(d: &Arc<Daemon>, t: TaskId) -> String {
    mason_daemon::diagnose::record_verdict(d, t, submission());
    attention::card_of(d, &task_of(d, t))
        .expect("a diagnosis card")
        .1
}

fn call(d: &Arc<Daemon>, method: &str, path: String, body: Value) -> (u16, Value) {
    let d = d.clone();
    let method = method.to_string();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            use tower::ServiceExt;
            let req = axum::http::Request::builder()
                .method(method.as_str())
                .uri(path)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap();
            let resp = mason_daemon::http::api_router(d)
                .oneshot(req)
                .await
                .unwrap();
            let status = resp.status().as_u16();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap();
            (status, serde_json::from_slice(&bytes).unwrap_or_default())
        })
}

fn answer(
    d: &Arc<Daemon>,
    t: TaskId,
    answer: &str,
    hash: &str,
    context: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({"answer": answer, "hash": hash, "as": d.cfg.owner_names[0]});
    if let Some(c) = context {
        body["context"] = json!(c);
    }
    call(d, "POST", format!("/tasks/{}/answer", t.0), body)
}

/// The task JSON's card, and the task view's card options and allowed actions.
fn shown(d: &Arc<Daemon>, t: TaskId) -> (Value, usize, Vec<String>) {
    let (code, json) = call(d, "GET", format!("/tasks/{}", t.0), Value::Null);
    assert_eq!(code, 200, "{json}");
    let w = d.world().unwrap();
    let view = Ctx::new(d, &w).task_detail(t.0).expect("a task view");
    (
        json["card"].clone(),
        view.card.map_or(0, |c| c.options.len()),
        view.row.expect("a row").allowed_actions,
    )
}

fn decisions(d: &Arc<Daemon>, t: TaskId) -> String {
    std::fs::read_to_string(d.task_dir(t).join("decisions.txt")).unwrap_or_default()
}

fn status_of(d: &Arc<Daemon>, t: TaskId) -> String {
    let rows = d.store.diagnoses(Some(t.0)).unwrap();
    rows.last().expect("a diagnosis").status.clone()
}

/// The step of the owner's last retry of `t`.
fn last_retry(d: &Arc<Daemon>, t: TaskId) -> (State, String) {
    d.store
        .read_since(0, 100_000)
        .unwrap()
        .iter()
        .rev()
        .find_map(|e| match &e.event.kind {
            EventKind::Retried { task, to, step, .. } if *task == t => Some((*to, step.clone())),
            _ => None,
        })
        .expect("a retry")
}

#[test]
fn a_failed_task_holds_the_card_until_it_is_dismissed_or_retried() {
    let w = world("diagnose = false", json!({}));
    let d = w.s.start();
    let t = failed(&d, "Holds a card");
    let (card, options, _) = shown(&d, t);
    assert_eq!((card, options), (Value::Null, 0));
    // The running mark is one list per process, keyed by task number, and the other tests in
    // this binary have daemons of their own: wait out a diagnosis of theirs.
    let offered = [
        "retry",
        "cancel",
        "open_session",
        "diagnose",
        "skip_all_tests",
    ];
    wait_for("diagnose to be offered", 600, || shown(&d, t).2 == offered);

    let hash = diagnose(&d, t);
    let (card, options, actions) = shown(&d, t);
    assert_eq!(card["hash"], hash.as_str());
    assert_eq!(card["other"], true);
    assert_eq!(card["options"].as_array().unwrap().len(), 4);
    assert_eq!(options, 4);
    assert_eq!(
        actions,
        [
            "answer",
            "retry",
            "cancel",
            "open_session",
            "skip_all_tests"
        ]
    );
    let notices = attention::alerts_now(&d);
    let notice = notices
        .iter()
        .find(|a| a.0.starts_with("diagnosis:"))
        .expect("a notice");
    assert!(
        notice
            .2
            .contains("answer it on the task page or with `mason task answer"),
        "{}",
        notice.2
    );

    // A newer diagnosis supersedes the card: the same words, another hash.
    let newer = diagnose(&d, t);
    assert_ne!(newer, hash);
    let (code, body) = answer(&d, t, "4", &hash, None);
    assert_eq!(code, 409, "{body}");
    assert!(body["error"].as_str().unwrap().contains("stale"), "{body}");
    // Only the owner.
    let (code, body) = call(
        &d,
        "POST",
        format!("/tasks/{}/answer", t.0),
        json!({"answer": "4", "hash": newer, "as": "mallory"}),
    );
    assert_eq!(code, 403, "{body}");
    // Too long a context, and one that is not a string.
    let long = "x".repeat(attention::CONTEXT_MAX + 1);
    assert_eq!(answer(&d, t, "4", &newer, Some(&long)).0, 400);
    let (code, _) = call(
        &d,
        "POST",
        format!("/tasks/{}/answer", t.0),
        json!({"answer": "4", "hash": newer, "as": d.cfg.owner_names[0], "context": 7}),
    );
    assert_eq!(code, 400);
    assert_eq!(answer(&d, t, "9", &newer, None).0, 400, "no such option");
    assert_eq!(task_of(&d, t).state, State::Failed);
    assert_eq!(status_of(&d, t), "pending");
    assert_eq!(decisions(&d, t), "", "a refused answer is not kept");

    // Dismissed (both of them): no card.
    for row in d.store.diagnoses(Some(t.0)).unwrap() {
        mason_daemon::diagnose::dismiss(&d, &row).unwrap();
    }
    let (card, options, actions) = shown(&d, t);
    assert_eq!((card, options), (Value::Null, 0));
    assert_eq!(
        actions,
        ["retry", "cancel", "open_session", "skip_all_tests"]
    );
    assert_eq!(answer(&d, t, "4", &newer, None).0, 409);

    // Retried: no card, though its row is still pending.
    let hash = diagnose(&d, t);
    let (code, body) = call(&d, "POST", format!("/tasks/{}/retry", t.0), json!({}));
    assert_eq!(code, 200, "{body}");
    assert!(attention::card_of(&d, &task_of(&d, t)).is_none());
    assert_eq!(answer(&d, t, "4", &hash, None).0, 409);
}

#[test]
fn a_retry_option_retries_with_the_note_and_the_context() {
    let w = world("diagnose = false", json!({}));
    let d = w.s.start();
    let t = failed(&d, "Retried by the card");
    let hash = diagnose(&d, t);
    w.open_gate();
    let (code, body) = answer(
        &d,
        t,
        "Retry with the gate",
        &hash,
        Some("CTX-RETRY use the small fix"),
    );
    assert_eq!(code, 200, "{body}");
    let stored = "Retry with the gate\nOwner's context: CTX-RETRY use the small fix";
    assert_eq!(body["answer"], stored);
    assert_eq!(status_of(&d, t), "applied");
    assert_eq!(last_retry(&d, t), (State::Developing, "develop".into()));
    assert!(attention::card_of(&d, &task_of(&d, t)).is_none());
    assert!(
        decisions(&d, t).contains(&format!("The owner answered: {stored}")),
        "{}",
        decisions(&d, t)
    );
    assert_eq!(attention::metrics(&d, 0)["by_kind"]["answer"], 1);
    wait_state(&d, t, State::Landed, 600);
    let prompts = w.prompts();
    let next = &prompts[1];
    assert!(
        next.contains(
            "A note for this retry: DIAG-NOTE write the gate\nOwner's context: CTX-RETRY"
        ),
        "{next}"
    );
    assert!(
        next.contains(&format!("The owner answered: {stored}")),
        "{next}"
    );
    // Applied by the owner is not announced as applied by itself.
    assert!(!attention::alerts_now(&d)
        .iter()
        .any(|a| a.0.starts_with("diagnosis-applied:")));
}

#[test]
fn the_explore_close_and_cancel_options_do_what_they_say() {
    let w = world("diagnose = false", json!({}));
    let d = w.s.start();

    let t = failed(&d, "Explored again");
    let hash = diagnose(&d, t);
    // An option's label answers it too.
    let (code, body) = answer(&d, t, "plan it again", &hash, Some("CTX-EXPLORE"));
    assert_eq!(code, 200, "{body}");
    assert_eq!(last_retry(&d, t), (State::Exploring, "explore".into()));
    assert_eq!(status_of(&d, t), "applied");
    // The reason, with the context, is the next agent's note.
    wait_for("the second failure", 600, || {
        task_of(&d, t).state == State::Failed && w.prompts().len() >= 2
    });
    let plans = w.recorded("explore.txt");
    assert!(
        plans[1].contains(
            "A note for this retry: DIAG-REASON the plan has no gate\nOwner's context: CTX-EXPLORE"
        ),
        "{}",
        plans[1]
    );
    assert!(decisions(&d, t).contains("Plan it again\nOwner's context: CTX-EXPLORE"));

    let t = failed(&d, "Closed");
    let hash = diagnose(&d, t);
    let (code, body) = answer(&d, t, "Close it as done", &hash, Some("CTX-CLOSE"));
    assert_eq!(code, 200, "{body}");
    let k = task_of(&d, t);
    assert_eq!(k.state, State::Landed);
    assert_eq!(k.landed, Some(LandedOutcome::ClosedNoChange));
    assert_eq!(status_of(&d, t), "applied");
    let closed = d
        .store
        .read_since(0, 100_000)
        .unwrap()
        .iter()
        .find_map(|e| match &e.event.kind {
            EventKind::ClosedNoChange { task, reason } if *task == t => Some(reason.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        closed,
        "already_done: DIAG-EVIDENCE it is on main\nOwner's context: CTX-CLOSE"
    );

    let t = failed(&d, "Canceled");
    let hash = diagnose(&d, t);
    let (code, body) = answer(&d, t, "Cancel the task", &hash, None);
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["answer"], "Cancel the task");
    assert_eq!(task_of(&d, t).state, State::Canceled);
    assert_eq!(status_of(&d, t), "applied");
    assert!(decisions(&d, t).contains("The owner answered: Cancel the task"));
}

#[test]
fn other_words_retry_from_the_default_step_as_the_owner_wrote_them() {
    let w = world("diagnose = false", json!({}));
    let d = w.s.start();
    let t = failed(&d, "Other");
    let hash = diagnose(&d, t);
    w.open_gate();
    let words = "OTHER-WORDS keep the old name";
    let (code, body) = answer(&d, t, words, &hash, Some("CTX-OTHER"));
    assert_eq!(code, 200, "{body}");
    assert_eq!(
        body["answer"],
        format!("Other: {words}\nOwner's context: CTX-OTHER")
    );
    assert_eq!(status_of(&d, t), "answered");
    assert_eq!(last_retry(&d, t), (State::Developing, "develop".into()));
    assert!(decisions(&d, t).contains(words));
    wait_state(&d, t, State::Landed, 600);
    let next = &w.prompts()[1];
    assert!(
        next.contains(&format!(
            "A note for this retry: {words}\nOwner's context: CTX-OTHER"
        )),
        "{next}"
    );
}

#[test]
fn context_on_an_open_question_reaches_the_next_round() {
    let rec = tempfile::tempdir().unwrap();
    let ask = json!([{"call": {"tool": "ask", "args": {
        "ask": "Should the token expire after one hour or one day?",
        "why": "The ticket does not say and the security notes are silent on it.",
        "options": [
            {"label": "One hour, with refresh tokens", "recommended": true},
            {"label": "One day, without refresh tokens"}],
        "evidence": ["the ticket only says 'tokens expire'"]}}}]);
    let w = world(
        "diagnose = false",
        json!({"develop": [ask, turn(rec.path())]}),
    );
    w.open_gate();
    let d = w.s.start();
    let t = file_task(&d, "Asks");
    wait_state(&d, t, State::NeedsAttention, 600);
    let (_, hash) = attention::card_of(&d, &task_of(&d, t)).unwrap();
    let long = "x".repeat(attention::CONTEXT_MAX + 1);
    assert_eq!(answer(&d, t, "1", &hash, Some(&long)).0, 400);
    // The context is only kept: a number in it picks nothing.
    let (code, body) = answer(&d, t, "1", &hash, Some("2 CTX-ASK"));
    assert_eq!(code, 200, "{body}");
    let stored = "One hour, with refresh tokens\nOwner's context: 2 CTX-ASK";
    assert_eq!(body["answer"], stored);
    assert!(decisions(&d, t).contains(&format!("The owner answered: {stored}")));
    wait_state(&d, t, State::Landed, 600);
    let next = std::fs::read_to_string(rec.path().join("dev.txt")).unwrap();
    assert!(
        next.contains(&format!("The owner answered: {stored}")),
        "{next}"
    );
}

/// The exit test: a real daemon, a diagnosis session of the fake agent, and the CLI.
#[test]
fn the_cli_answers_a_failed_tasks_card_with_context() {
    let card = json!([{"call": {"tool": "submit_diagnosis", "args": {
        "summary": "the gate file is missing",
        "quote": "Answered from the shell",
        "options": [
            {"label": "Cancel the task", "recommended": true, "action": "cancel"},
            {"label": "Retry the work", "recommended": false, "action": "retry",
             "note": "write the gate"}]}}}]);
    let w = world("diagnose = true", json!({"diagnose": [card]}));
    let mut m = w.s.masond(&[], &[]);
    w.s.wait_daemon(&mut m);
    w.s.mason_ok(&["task", "add", "Answered from the shell"]);
    wait_for("the diagnosis card", 600, || {
        w.s.task_json(0)["card"]["options"].is_array()
    });
    w.open_gate();
    let said =
        w.s.mason_ok(&["task", "answer", "0", "2", "--context", "CTX-CLI"]);
    assert!(said.contains("Retry the work"), "{said}");
    assert_ne!(w.s.task_json(0)["state"], "failed");
    w.s.wait_task(0, "landed", 600);
    let next = &w.prompts()[1];
    assert!(next.contains("Owner's context: CTX-CLI"), "{next}");
}
