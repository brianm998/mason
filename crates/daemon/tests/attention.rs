// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 10 end to end: decision cards that fit or are made to, answers tied to
//! the card's hash and counted only from the owner, no question asked twice,
//! pauses, alerts that follow their conditions, and the intervention metric.

mod support;
use mason_adapter_api::card::{self, Origin};
use mason_core::*;
use mason_daemon::attention;
use mason_daemon::Daemon;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use support::*;

fn ask_turn(args: Value) -> Value {
    json!([{"call": {"tool": "ask", "args": args}}])
}

fn good_card(key: &str) -> Value {
    json!({
        "ask": "Should the token expire after one hour or one day?",
        "why": "The ticket does not say and the security notes are silent on it.",
        "options": [
            {"label": "One hour, with refresh tokens", "recommended": true},
            {"label": "One day, without refresh tokens"}
        ],
        "evidence": ["the ticket only says 'tokens expire'"],
        "key": key
    })
}

fn world(develop: Vec<Value>) -> Scratch {
    Scratch::new(
        "test -f hello.txt",
        json!({"steps": {"explore": [explore_turn()], "develop": develop}}),
    )
}

fn start(s: &Scratch) -> Arc<Daemon> {
    let who = "brian".to_string();
    s.start_with(move |c| {
        c.owner_names = vec![who];
        c.reload_interval = Duration::from_millis(100);
    })
}

fn file_it(d: &Arc<Daemon>, title: &str) -> TaskId {
    file_task_as(d, title, "Please do it.", true, TrustSource::Owner, false)
}

fn card_of(d: &Arc<Daemon>, t: TaskId) -> (card::Card, String) {
    attention::card_of(d, &task_of(d, t)).expect("an open question")
}

#[test]
fn a_card_that_fits_goes_through_untouched_and_an_answer_resumes_the_task() {
    let s = world(vec![ask_turn(good_card("")), develop_turn()]);
    let d = start(&s);
    let t = file_it(&d, "Asks well");
    wait_state(&d, t, State::NeedsAttention, 60);
    let (c, h) = card_of(&d, t);
    assert_eq!(c.origin, Origin::Agent);
    assert!(c.problems.is_empty());
    assert_eq!(
        task_of(&d, t).activity,
        Activity::Waiting(WaitReason::Owner)
    );
    // Option 1 is "One hour": the answer resumes the work, which then lands.
    let said = attention::answer(&d, t, "1", &h, "brian").unwrap();
    assert_eq!(said, "One hour, with refresh tokens");
    wait_state(&d, t, State::Landed, 90);
    // The resumed agent was told what the owner decided.
    let notes = std::fs::read_to_string(d.cfg.task_dir(t.0).join("decisions.txt")).unwrap();
    assert!(
        notes.contains("The owner answered: One hour, with refresh tokens"),
        "{notes}"
    );
}

#[test]
fn a_bad_card_is_rejected_and_rebuilt_so_no_question_is_lost() {
    // A pasted log as the question, one-word options, no why, nothing recommended.
    let log: String = (0..30)
        .map(|i| format!("error {i}: it broke"))
        .collect::<Vec<_>>()
        .join("\n");
    let bad = json!({"ask": format!("Why does the build fail?\n{log}"), "options": [{"label": "a"}, {"label": "b"}]});
    let s = world(vec![ask_turn(bad), develop_turn()]);
    let d = start(&s);
    let t = file_it(&d, "Asks badly");
    wait_state(&d, t, State::NeedsAttention, 60);
    let (c, _) = card_of(&d, t);
    assert_eq!(c.origin, Origin::Fallback, "{c:?}");
    assert_eq!(c.ask, "Why does the build fail?");
    assert!(c.ask.chars().count() <= 200);
    assert!(c.options.len() >= 2 && c.options.iter().filter(|o| o.recommended).count() == 1);
    assert!(c.evidence.len() <= 10);
    assert!(
        c.problems.iter().any(|p| p.contains("why: required")),
        "{:?}",
        c.problems
    );
    assert!(
        c.problems
            .iter()
            .any(|p| p.contains("exactly one must be recommended")),
        "{:?}",
        c.problems
    );
    // The rebuilt card is itself a card that fits.
    assert!(card::parse(&c.to_json()).is_ok());
    // The agent was told its card did not fit.
    let logs = mason_daemon::http::render_logs(&d, t.0);
    assert!(
        logs.contains("did not fit the decision-card shape"),
        "{logs}"
    );
}

#[test]
fn a_projects_cheap_rewriter_gets_the_first_try_at_a_bad_card() {
    let s = world(vec![
        ask_turn(json!({"ask": "tokens?", "options": [{"label": "a"}, {"label": "b"}]})),
        develop_turn(),
    ]);
    // The rewriter reads {card, problems} and prints a good card.
    let rewriter = s.root.path().join("rewrite.sh");
    std::fs::write(
        &rewriter,
        format!(
            "#!/bin/sh\ncat > /dev/null\ncat <<'JSON'\n{}\nJSON\n",
            good_card("")
        ),
    )
    .unwrap();
    let toml = s.config_dir.join("projects/scratch.toml");
    let text = std::fs::read_to_string(&toml).unwrap();
    std::fs::write(
        &toml,
        format!("card_rewriter = \"sh {}\"\n{text}", rewriter.display()),
    )
    .unwrap();
    let d = start(&s);
    let t = file_it(&d, "Rewritten");
    wait_state(&d, t, State::NeedsAttention, 60);
    let (c, _) = card_of(&d, t);
    assert_eq!(c.origin, Origin::Rewritten);
    assert_eq!(c.options[0].label, "One hour, with refresh tokens");
    assert!(!c.problems.is_empty(), "what was wrong is on the record");
}

#[test]
fn a_rewriter_that_fails_falls_back_deterministically() {
    let s = world(vec![
        ask_turn(json!({"ask": "tokens?", "options": [{"label": "a"}, {"label": "b"}]})),
        develop_turn(),
    ]);
    let toml = s.config_dir.join("projects/scratch.toml");
    let text = std::fs::read_to_string(&toml).unwrap();
    std::fs::write(&toml, format!("card_rewriter = \"exit 3\"\n{text}")).unwrap();
    let d = start(&s);
    let t = file_it(&d, "Rewriter broken");
    wait_state(&d, t, State::NeedsAttention, 60);
    assert_eq!(card_of(&d, t).0.origin, Origin::Fallback);
}

#[test]
fn a_stale_answer_is_refused_and_so_is_one_from_someone_else() {
    let s = world(vec![ask_turn(good_card("")), develop_turn()]);
    let d = start(&s);
    let t = file_it(&d, "Careful");
    wait_state(&d, t, State::NeedsAttention, 60);
    let (_, h) = card_of(&d, t);
    // A tap on a card that is not the current one.
    let stale = attention::answer(&d, t, "1", "0000000000000000", "brian").unwrap_err();
    assert_eq!(stale.0, 409);
    assert!(stale.1.contains("stale"), "{}", stale.1);
    // Not the owner's identity: refused, and never counted as an intervention.
    let other = attention::answer(&d, t, "1", &h, "mallory").unwrap_err();
    assert_eq!(other.0, 403);
    assert_eq!(
        state_of(&d, t),
        State::NeedsAttention,
        "still waiting for the owner"
    );
    // An option that is not there, and an empty answer.
    assert_eq!(
        attention::answer(&d, t, "7", &h, "brian").unwrap_err().0,
        400
    );
    assert_eq!(
        attention::answer(&d, t, "  ", &h, "brian").unwrap_err().0,
        400
    );
    // The owner, on the current card: it works, once.
    attention::answer(
        &d,
        t,
        "Other: use whatever the last service did",
        &h,
        "brian",
    )
    .unwrap();
    wait_state(&d, t, State::Landed, 90);
    let again = attention::answer(&d, t, "1", &h, "brian").unwrap_err();
    assert_eq!(again.0, 409, "the question is closed");
    let m = attention::metrics(&d, 0);
    assert_eq!(m["by_kind"]["answer"], 1, "{m}");
    assert_eq!(m["refused_answers"], 1);
    assert_eq!(
        m["interventions"], 1,
        "a refused answer is not an intervention: {m}"
    );
}

#[test]
fn free_text_is_the_other_box() {
    let s = world(vec![ask_turn(good_card("")), develop_turn()]);
    let d = start(&s);
    let t = file_it(&d, "Other");
    wait_state(&d, t, State::NeedsAttention, 60);
    let (_, h) = card_of(&d, t);
    let said =
        attention::answer(&d, t, "Thirty minutes, and log every refresh", &h, "brian").unwrap();
    assert_eq!(said, "Other: Thirty minutes, and log every refresh");
    wait_state(&d, t, State::Landed, 90);
}

#[test]
fn the_same_gate_failing_again_after_an_answer_is_acknowledged_not_asked_again() {
    // The agent asks about `gate-x`, is answered, asks about it again (the gate
    // failed again), and the second time is acknowledged without bothering anyone.
    let s = world(vec![
        ask_turn(good_card("gate-x")),
        ask_turn(good_card("gate-x")),
        develop_turn(),
    ]);
    let d = start(&s);
    let t = file_it(&d, "Loops");
    wait_state(&d, t, State::NeedsAttention, 60);
    let (_, h) = card_of(&d, t);
    attention::answer(&d, t, "1", &h, "brian").unwrap();
    wait_state(&d, t, State::Landed, 90);
    let w = d.world().unwrap();
    let decisions: Vec<_> = w.decisions.values().filter(|x| x.task == t).collect();
    assert_eq!(decisions.len(), 2, "two questions were raised");
    assert!(
        decisions[1]
            .answer
            .as_deref()
            .unwrap()
            .starts_with("(acknowledged, not asked again) One hour"),
        "{:?}",
        decisions[1].answer
    );
    let m = attention::metrics(&d, 0);
    assert_eq!(m["acknowledged"], 1);
    assert_eq!(m["interventions"], 1, "the owner was only asked once: {m}");
}

#[test]
fn pausing_claims_holds_the_queue_and_lifting_it_lets_work_go() {
    let s = world(vec![develop_turn()]);
    let d = start(&s);
    attention::pause(&d, "", "claims", "brian", 600).unwrap();
    let t = file_it(&d, "Held back");
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(
        state_of(&d, t),
        State::Queued,
        "a pause on claims keeps it queued"
    );
    assert!(attention::alerts_now(&d)
        .iter()
        .any(|a| a.0 == "paused::claims"));
    assert_eq!(attention::resume(&d, "", "claims").unwrap(), 1);
    wait_state(&d, t, State::Landed, 90);
    assert!(attention::alerts_now(&d)
        .iter()
        .all(|a| !a.0.starts_with("paused")));
}

#[test]
fn a_pause_runs_out_by_itself_and_one_project_can_be_paused_alone() {
    let s = world(vec![develop_turn()]);
    let d = start(&s);
    assert!(attention::pause(&d, "nowhere", "all", "brian", 5).is_err());
    assert!(attention::pause(&d, "", "sideways", "brian", 5).is_err());
    assert!(
        attention::pause(&d, "", "all", "brian", 0).is_err(),
        "an expiry is required"
    );
    attention::pause(&d, "scratch", "all", "brian", 1).unwrap();
    let t = file_it(&d, "Waits a second");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(state_of(&d, t), State::Queued);
    // It runs out, the watcher notices, and work goes.
    wait_state(&d, t, State::Landed, 90);
}

#[test]
fn pausing_all_work_stops_new_steps_at_the_boundary_and_says_so() {
    // The develop step takes a couple of seconds, so the pause lands mid-step.
    let slow = json!([
        {"sh": "sleep 2"},
        {"sh": TASK_SCRIPT_COMMIT},
        {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "$HEAD"}}}
    ]);
    let s = world(vec![slow]);
    let d = start(&s);
    let t = file_it(&d, "Mid-flight");
    wait_state(&d, t, State::Developing, 30);
    attention::pause(&d, "", "all", "brian", 600).unwrap();
    // Nothing new starts while the pause holds.
    wait_for("the next step to be held back", 60, || {
        matches!(
            task_of(&d, t).activity,
            Activity::Waiting(WaitReason::On(ref w)) if w.contains("paused by brian")
        )
    });
    // (Whether the pause caught the develop step as it was about to start or
    // the test step after it, nothing new runs while it holds.)
    let held = state_of(&d, t);
    assert!(
        matches!(held, State::Developing | State::Testing),
        "{held:?}"
    );
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(state_of(&d, t), held, "it stays held");
    attention::resume(&d, "", "all").unwrap();
    wait_state(&d, t, State::Landed, 90);
}

#[test]
fn alerts_follow_their_conditions_and_clear_when_they_clear() {
    let s = world(vec![ask_turn(good_card("")), develop_turn()]);
    let d = start(&s);
    let t = file_it(&d, "Needs you");
    wait_state(&d, t, State::NeedsAttention, 60);
    let key = format!("question:{}", t.0);
    attention::sync_alerts(&d);
    let active = d.store.alerts(true).unwrap();
    let a = active
        .iter()
        .find(|a| a.key == key)
        .expect("an alert for the question");
    assert_eq!(a.kind, "needs_you");
    assert!(a.text.contains("Should the token expire"), "{}", a.text);
    let (_, h) = card_of(&d, t);
    attention::answer(&d, t, "1", &h, "brian").unwrap();
    wait_state(&d, t, State::Landed, 90);
    attention::sync_alerts(&d);
    assert!(
        d.store.alerts(true).unwrap().iter().all(|a| a.key != key),
        "cleared"
    );
    let history = d.store.alerts(false).unwrap();
    let old = history
        .iter()
        .find(|a| a.key == key)
        .expect("kept in the history");
    assert!(old.cleared_at.is_some());
}

#[test]
fn interventions_per_landed_task_counts_what_the_owner_did() {
    let s = world(vec![ask_turn(good_card("")), develop_turn()]);
    let d = start(&s);
    let m0 = attention::metrics(&d, 0);
    assert!(
        m0["per_landed"].is_null(),
        "no landed tasks yet, so no ratio"
    );
    let t = file_it(&d, "Counted");
    wait_state(&d, t, State::NeedsAttention, 60);
    let (_, h) = card_of(&d, t);
    attention::intervene(&d, "open_session", Some(t), "brian", "", "");
    attention::answer(&d, t, "1", &h, "brian").unwrap();
    wait_state(&d, t, State::Landed, 90);
    let m = attention::metrics(&d, 0);
    assert_eq!(m["landed"], 1);
    assert_eq!(m["interventions"], 2);
    assert_eq!(m["per_landed"].as_f64(), Some(2.0));
    assert_eq!(m["by_kind"]["answer"], 1);
    assert_eq!(m["by_kind"]["open_session"], 1);
    // A window that excludes them counts none of them.
    let later = mason_daemon::daemon::now_ms() + 10_000;
    assert_eq!(attention::metrics(&d, later)["interventions"], 0);
}

#[test]
fn only_the_owner_identities_given_to_the_daemon_count() {
    let s = world(vec![develop_turn()]);
    let d = s.start_with(|c| c.owner_names = vec!["brian".into(), "brian@example.com".into()]);
    assert!(attention::is_owner(&d, "brian") && attention::is_owner(&d, "brian@example.com"));
    assert!(
        !attention::is_owner(&d, "")
            && !attention::is_owner(&d, "Brian")
            && !attention::is_owner(&d, "root")
    );
}

#[test]
fn opening_a_session_finds_the_agents_session_and_says_whether_the_step_is_idle() {
    let s = world(vec![develop_turn()]);
    let d = start(&s);
    let t = file_it(&d, "Resume me");
    wait_state(&d, t, State::Landed, 90);
    wait_for("the job to end", 30, || d.job_count() == 0);
    let v = attention::session_for(&d, t).unwrap();
    assert_eq!(v["idle"], true);
    assert!(v["session"].as_str().is_some_and(|s| !s.is_empty()), "{v}");
    assert!(
        v["worktree"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/{}", t.0)),
        "{v}"
    );
    assert!(attention::session_for(&d, TaskId(9999)).is_err());
}

#[test]
fn a_frozen_base_raises_an_alert_naming_the_fix_task_until_the_freeze_clears() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let find = || {
        mason_daemon::attention::alerts_now(&d)
            .into_iter()
            .find(|(k, _, _)| k == "frozen:scratch")
    };
    assert!(find().is_none());
    let fix = file_task(&d, "Fix the base");
    d.store
        .set_freeze(
            "scratch",
            "smoke went red",
            Some(fix.0),
            mason_daemon::daemon::now_ms() - 120_000,
        )
        .unwrap();
    let (_, kind, text) = find().expect("the freeze raises an alert");
    assert_eq!(kind, "landing");
    assert!(
        text.contains("smoke went red")
            && text.contains(&format!("Fix task #{}", fix.0))
            && text.contains("2 min"),
        "{text}"
    );
    assert!(!text.contains("will not lift"), "{text}");

    d.emit(EventKind::StepFailed {
        task: fix,
        reason: "r".into(),
        class: None,
    })
    .unwrap();
    let (_, _, text) = find().unwrap();
    assert!(
        text.contains("will not lift by itself")
            && text.contains(&format!("mason task retry {}", fix.0))
            && text.contains(&format!("mason task force {}", fix.0)),
        "{text}"
    );

    d.store.clear_freeze("scratch").unwrap();
    assert!(find().is_none(), "the alert clears with the freeze");
}
