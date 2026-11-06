// SPDX-License-Identifier: GPL-3.0-or-later

//! Event replay (Part 23): event logs recorded from real runs of the daemon
//! (`tests/fixtures/*.events`, regenerated with `UPDATE_FIXTURES=1`) must replay
//! to the same state after any code change, and keep decoding and re-encoding
//! byte for byte, which pins the on-disk format.

use mason_core::*;
use mason_engine::{reduce, World};
use mason_store::{decode_event, encode_event};

fn load(name: &str) -> (String, Vec<Event>) {
    let path = format!(
        "{}/tests/fixtures/{name}.events",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{path}: {e} (run with UPDATE_FIXTURES=1)"));
    let mut events = vec![];
    for line in text.lines() {
        let mut p = line.splitn(3, '\t');
        let at: u64 = p.next().unwrap().parse().unwrap();
        let (kind, payload) = (p.next().unwrap(), p.next().unwrap_or(""));
        events.push(Event {
            at: Timestamp(at),
            kind: decode_event(kind, payload).unwrap_or_else(|e| panic!("{line}: {e:?}")),
        });
    }
    (text, events)
}

fn replay(events: &[Event]) -> World {
    let mut w = World::new();
    for e in events {
        w = reduce(&w, e)
            .unwrap_or_else(|err| panic!("{e:?} was refused on replay: {err:?}"))
            .0;
        w.check_invariants().unwrap();
    }
    w
}

#[test]
fn a_recorded_clean_run_replays_to_a_landed_task() {
    let (text, events) = load("landed_task");
    let w = replay(&events);
    let t = &w.tasks[&TaskId(0)];
    assert_eq!(
        (t.state, t.landed, t.failures),
        (State::Landed, Some(LandedOutcome::Merged), 0)
    );
    let steps: Vec<(&str, State)> = w
        .step_runs
        .values()
        .map(|r| (r.name.as_str(), r.step))
        .collect();
    assert_eq!(
        steps,
        vec![
            ("explore", State::Exploring),
            ("develop", State::Developing),
            ("test", State::Testing),
            ("land", State::Landing)
        ]
    );
    assert!(w
        .step_runs
        .values()
        .all(|r| r.outcome == Some(StepOutcome::Succeeded)));
    assert_eq!(t.route, Some(Route::Explore));
    assert!(
        !t.def.is_empty(),
        "the task is pinned to the definition it started under"
    );
    assert_eq!(replay(&events), w, "replaying is deterministic");
    reencodes_identically(&text, &events);
}

#[test]
fn a_recorded_red_then_green_run_replays_through_the_loop_back() {
    let (text, events) = load("red_then_green");
    let w = replay(&events);
    let t = &w.tasks[&TaskId(0)];
    assert_eq!(
        (t.state, t.landed, t.failures),
        (State::Landed, Some(LandedOutcome::Merged), 0)
    );
    assert_eq!(
        (t.attempts[&State::Developing], t.attempts[&State::Testing]),
        (2, 2)
    );
    assert_eq!(
        (t.step_attempts["develop"], t.step_attempts["test"]),
        (2, 2)
    );
    let outcomes: Vec<(&str, StepOutcome)> = w
        .step_runs
        .values()
        .map(|r| (r.name.as_str(), r.outcome.unwrap()))
        .collect();
    assert!(
        outcomes.contains(&("test", StepOutcome::SentBack)),
        "{outcomes:?}"
    );
    reencodes_identically(&text, &events);
}

#[test]
fn a_recorded_flagged_run_skips_exploring_and_records_why() {
    let (text, events) = load("skipped_exploring");
    let w = replay(&events);
    let t = &w.tasks[&TaskId(0)];
    assert_eq!(
        (t.state, t.route, t.explore_returns),
        (State::Landed, Some(Route::Flag), 0)
    );
    let steps: Vec<&str> = w.step_runs.values().map(|r| r.name.as_str()).collect();
    assert_eq!(
        steps,
        vec!["develop", "test", "land"],
        "no exploring step ever ran"
    );
    reencodes_identically(&text, &events);
}

/// Logs written before steps had names, entries and pins (Stage 6) still decode
/// and replay, with the older fixed progression through the categories. They are
/// not re-encoded: the format has grown since.
#[test]
fn logs_recorded_before_definitions_existed_still_replay() {
    for name in ["stage6_landed_task", "stage6_red_then_green"] {
        let (_, events) = load(name);
        let w = replay(&events);
        let t = &w.tasks[&TaskId(0)];
        assert_eq!(
            (t.state, t.landed),
            (State::Landed, Some(LandedOutcome::Merged)),
            "{name}"
        );
        assert!(
            w.step_runs.values().all(|r| r.name.is_empty()),
            "no names in old logs"
        );
        let steps: Vec<State> = w
            .step_runs
            .values()
            .filter(|r| {
                r.outcome == Some(StepOutcome::Succeeded)
                    || r.outcome == Some(StepOutcome::SentBack)
            })
            .map(|r| r.step)
            .collect();
        assert!(
            steps.contains(&State::Review),
            "the old graph had a review step: {steps:?}"
        );
        assert_eq!(t.route, Some(Route::Explore));
        assert!(t.def.is_empty());
    }
}

fn reencodes_identically(text: &str, events: &[Event]) {
    let again: String = events
        .iter()
        .map(|e| {
            let (k, p) = encode_event(&e.kind);
            format!("{}\t{k}\t{p}\n", e.at.0)
        })
        .collect();
    assert_eq!(again, text, "the log's text format must not drift");
}

#[test]
fn every_prefix_of_a_recorded_run_is_a_valid_state() {
    // A crash can stop the log after any event. Every prefix must replay and keep its invariants.
    for name in [
        "red_then_green",
        "skipped_exploring",
        "stage6_red_then_green",
    ] {
        let (_, events) = load(name);
        for n in 0..=events.len() {
            replay(&events[..n]);
        }
    }
}
