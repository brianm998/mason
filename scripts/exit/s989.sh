#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s989 exit test: a red develop that gets fixed moves the tasks it held up. Partial baselines take
# a fixed test off "red on develop"; the watch folds develop into clean branches and tells the
# agents; a conflicting branch gets the conflict note and an alert after the delay; the same
# unexplained failure in three tasks is looked at at the tip.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-store --test store -- partial_baseline red_sightings
cargo test -q -p mason-daemon --lib -- redwatch::tests knownred::tests rerun::tests fixsync::tests
grep -q 'spawn_red_watch' crates/daemon/src/daemon.rs
grep -q 'redwatch::alerts' crates/daemon/src/attention.rs
grep -q 'take_red_notes' crates/daemon/src/jobs.rs
echo "s989 OK"
