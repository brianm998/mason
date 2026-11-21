// SPDX-License-Identifier: GPL-3.0-or-later

//! s705: a self-update that is waiting stops a red batch between runs instead of abandoning
//! it after an hour, and the next batch over the same base and task heads reuses the runs the
//! first one made.

use crate::support;
use support::lw::*;
use support::*;

const TOML: &str =
    "window = \"2s\"\nmax_size = 5\n[[tier]]\nname = \"batch\"\nsuites = [\"integration\"]\n";

/// Every red run is recorded in `reds`; the first one holds still (and says so in `first`)
/// until `go` exists, so the test can start the drain while that run is under way.
fn checks(gate: &std::path::Path) -> String {
    let g = gate.display();
    format!(
        r#"#!/bin/sh
n="$1"
echo "1..1"
if [ -n "$(find . -name "red.$n" -not -path "./.git/*")" ]; then
  echo "$PWD $$" >> {g}/reds
  if [ ! -f {g}/go ]; then echo x > {g}/first; while [ ! -f {g}/go ]; do sleep 0.1; done; fi
  echo "not ok 1 - $n"; exit 1
fi
echo "ok 1 - $n"
"#
    )
}

fn reds(gate: &std::path::Path) -> usize {
    std::fs::read_to_string(gate.join("reds")).map_or(0, |t| t.lines().count())
}

#[test]
fn a_draining_update_stops_a_red_batch_between_runs_and_the_next_batch_reuses_its_results() {
    let gate = tempfile::tempdir().unwrap();
    let script = checks(gate.path());
    let w = world(TOML, &[("checks.sh", &script)]);
    let t1 = w.task("one", &[("x/one.txt", "1")]);
    let t2 = w.task("two", &[("x/red.integration", "boom")]);
    let t3 = w.task("three", &[("x/three.txt", "3")]);
    wait_for("the first run to be under way", 120, || {
        gate.path().join("first").exists()
    });
    // The update is ready: it waits for the batch, which is in the middle of a run.
    w.d.landing().set_draining(true);
    std::fs::write(gate.path().join("go"), "").unwrap();
    wait_for("the batch to stop at the run boundary", 120, || {
        w.d.store
            .batches(Some("scratch"), 10)
            .unwrap()
            .iter()
            .any(|b| b.phase == "yielded" && b.finished_at.is_some())
    });
    let b =
        w.d.store
            .batches(Some("scratch"), 10)
            .unwrap()
            .into_iter()
            .find(|b| b.phase == "yielded")
            .unwrap();
    assert!(b.note.contains("stopped for an update"), "{}", b.note);
    // A red run is followed at once, inside the same run, by a rerun of its failing tests
    // (`rerun_failing` in suites.rs; this suite has no `rerun` template, so it is the whole
    // suite again). So one run shows as one red line or two; a later run of the batch would
    // add its own red and rerun on top of that.
    let n = reds(gate.path());
    assert!(
        (1..=2).contains(&n),
        "no run after the first one (and its rerun): {:?}",
        std::fs::read_to_string(gate.path().join("reds"))
    );
    for t in [t1, t2, t3] {
        assert!(!w.landed(t) && w.sent_back(t).is_none());
    }
    // The takeover failed (or came and went): the next batch carries on from what is known.
    w.d.landing().set_draining(false);
    wait_for("the innocent two to land", 180, || {
        w.landed(t1) && w.landed(t3)
    });
    wait_for("the culprit to go back", 60, || w.sent_back(t2).is_some());
    let notes: Vec<String> =
        w.d.store
            .batches(Some("scratch"), 10)
            .unwrap()
            .into_iter()
            .map(|b| b.note)
            .collect();
    assert!(
        notes
            .iter()
            .any(|n| n.contains("reused 1 run result(s) from an earlier batch")),
        "{notes:?}"
    );
}
