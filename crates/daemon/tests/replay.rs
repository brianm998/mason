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
    let steps: Vec<State> = w.step_runs.values().map(|r| r.step).collect();
    assert_eq!(
        steps,
        vec![
            State::Exploring,
            State::Developing,
            State::Testing,
            State::Review,
            State::Landing
        ]
    );
    assert!(w
        .step_runs
        .values()
        .all(|r| r.outcome == Some(StepOutcome::Succeeded)));
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
    let outcomes: Vec<(State, StepOutcome)> = w
        .step_runs
        .values()
        .map(|r| (r.step, r.outcome.unwrap()))
        .collect();
    assert!(
        outcomes.contains(&(State::Testing, StepOutcome::SentBack)),
        "{outcomes:?}"
    );
    reencodes_identically(&text, &events);
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
    let (_, events) = load("red_then_green");
    for n in 0..=events.len() {
        replay(&events[..n]);
    }
}
