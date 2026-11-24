#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s989 exit test: a red develop that gets fixed moves the tasks it held up. A real daemon on a
# scratch repository with sleeping agents: develop is red on a test and three tasks' checks name
# it; a fix lands; two clean branches get develop folded in and their agents are told, the
# conflicting one gets the conflict note and, on the test clock, the owner alert after 30
# minutes; a run_check on the old tree is stopped and made again; the same unexplained failure
# in three tasks is run at the tip and filed once, or folds the tasks when it passes there.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-store --test store -- partial_baseline red_sightings
cargo test -q -p mason-daemon --lib -- redwatch::tests knownred::tests rerun::tests fixsync::tests
cargo test -q -p mason-daemon --test it -- red_develop
grep -q 'spawn_red_watch' crates/daemon/src/daemon.rs
grep -q 'redwatch::alerts' crates/daemon/src/attention.rs
grep -q 'take_red_notes' crates/daemon/src/jobs.rs
echo "s989 OK"
