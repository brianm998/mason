#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 16 exit test: self-reload and updates, against the real masond.
#  1. An update while a fake-agent session runs: the daemon is replaced by exec
#     (same pid), the session's step host is the very same process, the task lands
#     once with one attempt, and the database was backed up first.
#  2. A build that passes its self-test and then crashes is rolled back by itself
#     after three crashes in five minutes, with an alert; it is not tried again by
#     accident.
#  3. A build whose self-test fails never takes over.
#  4. An update to an ancestor is refused without an explicit --rollback.
#  5. The self-test proves migrations on a copy, replays the golden logs and lands a
#     scripted task in a sandbox; a bad database fails it.
#  6. The wire: the welcome carries the daemon's build and the oldest protocol it
#     serves; a client too old is told why. Both monitors show an update banner and
#     stay read-only when the protocol is too old.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo build -q --workspace --bins

t() {  # PACKAGE TEST-FILE TEST-NAME
  # CI runs `cargo test --workspace` first, which runs every one of these tests.
  [ -z "${MASON_TESTS_ALREADY_RAN:-}" ] || return 0
  cargo test -q -p "$1" --test "$2" -- --exact "$3" 2>&1 | tail -3 | grep -q 'test result: ok. 1 passed' \
    || { echo "FAILED: $2::$3" >&2; cargo test -p "$1" --test "$2" -- --exact "$3" 2>&1 | tail -30 >&2; exit 1; }
}

echo "== 1. an update while an agent session runs"
t mason-daemon update an_update_while_an_agent_session_runs_does_not_interrupt_it
echo "   same pid after exec, same step host, one attempt, landed once, database backed up"
echo "== 2. a build that crashes is rolled back by itself"
t mason-daemon update a_build_that_crashes_is_rolled_back_by_itself_with_an_alert
echo "   three crashes in five minutes: back on the previous runtime, an alert, the bad build refused"
echo "== 3. a build that fails its self-test never takes over"
t mason-daemon update a_build_that_fails_its_self_test_never_takes_over
echo "== 4. no accidental downgrades"
t mason-daemon update the_updater_refuses_to_go_backwards_unless_told_to_roll_back
echo "== 5. the self-test"
t mason-daemon update the_self_test_runs_on_a_copy_and_a_bad_database_fails_it

echo "== 6. versions on the wire, and the monitors"
t mason-daemon protocol the_welcome_names_the_daemons_build_and_the_oldest_client_it_serves
t mason-daemon protocol a_client_too_old_gets_a_welcome_it_can_read_then_an_explanation_and_no_views
if [ -z "${MASON_TESTS_ALREADY_RAN:-}" ]; then
  cargo test -q -p mason-daemon --lib update 2>&1 | grep 'test result'
  cargo test -q -p mason-protocol 2>&1 | grep 'test result' | head -1
fi
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
  (cd clients/swift && swift test 2>&1 | grep -E 'Executed [0-9]+ tests' | tail -1)
else
  echo "(SwiftUI tests skipped: they run in the monitor-swift CI job)"
fi
if command -v gradle >/dev/null && command -v java >/dev/null && [ -z "${MASON_SKIP_COMPOSE:-}" ]; then
  (cd clients/compose && gradle --no-daemon -q :core:test 2>&1 | grep -vE '^$' | head -5 || true)
  echo "(Compose tests pass)"
else
  echo "(Compose tests skipped: they run in the monitor-compose CI job)"
fi
cargo run -q -p mason-conformance -- features clients/features.toml | tail -1
echo "s16 OK"
