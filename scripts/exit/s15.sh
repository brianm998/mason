#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 15 exit test: GitHub, CI and PR landing, against a local fake git host
# (a bare repository and a directory of files) and a fake CI (scripts over it).
#  1. All seven landing modes land a clean batch and park only the culprit of a
#     red; the CI modes also fold in a moved base and run CI again.
#  2. kill -9 in the middle of CI or a pull request leaves nothing on the host
#     (no candidate ref, no open PR) and lands exactly once.
#  3. The GitHub adapters pass their conformance kits on recorded responses,
#     including 404, 422, rate limit and outage; a permanent error stops the poll
#     at once; a webhook is acted on only if its signature verifies.
#  4. The tracker: outside issues are ignored until labeled and stay untrusted;
#     the tracker owns title, body, priority and closure; state goes back out best effort.
#  5. `mason doctor` finds a project whose CI cannot run on a candidate ref.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo build -q --workspace --bins

t() {  # PACKAGE TEST-FILE TEST-NAME
  # CI runs `cargo test --workspace` first, which runs every one of these tests;
  # running each again would only spend the job's 10-minute budget twice.
  [ -z "${MASON_TESTS_ALREADY_RAN:-}" ] || return 0
  cargo test -q -p "$1" --test "$2" -- --exact "$3" 2>&1 | tail -3 | grep -q 'test result: ok. 1 passed' \
    || { echo "FAILED: $2::$3" >&2; cargo test -p "$1" --test "$2" -- --exact "$3" 2>&1 | tail -30 >&2; exit 1; }
}
m() { t mason-daemon landing_modes "$1"; }

echo "== 1. the seven landing modes"
echo "-- direct          green batch, red culprit parked, moved base folded in"
m plain_direct_lands_a_batch_parks_a_culprit_and_folds_in_a_moved_base
echo "-- local-only      lands on the local base, nothing pushed, owner's later commit folded in"
m plain_local_only_lands_on_the_local_base_and_leaves_the_push_to_the_owner
echo "-- direct-ci       candidate ref pushed, CI runs on it, base fast-forwarded, ref deleted"
m direct_ci_pushes_a_candidate_runs_ci_and_fast_forwards_the_base
echo "                   CI red goes to the ladder: only the culprit is parked"
m direct_ci_red_goes_to_the_same_ladder_and_only_the_culprit_is_parked
echo "                   base moved during CI: new tip folded in, CI run again"
m direct_ci_folds_in_a_moved_base_and_runs_ci_again
echo "-- local-only-ci   only the candidate leaves the machine; the base is the owner's to push"
m local_only_ci_leaves_the_base_to_the_owner_and_only_the_candidate_leaves
echo "                   CI red parks the culprit; the owner's commit to the local base is folded in"
m local_only_ci_red_parks_the_culprit_and_a_moved_local_base_is_folded_in
echo "-- pr-each         one pull request per task, each merged"
m pr_each_opens_one_pull_request_per_task_and_merges_each
echo "-- pr-batch        one pull request for the batch; red goes to the ladder; waits for a required approval"
m pr_batch_opens_one_pull_request_for_the_batch
m pr_batch_red_checks_go_to_the_ladder_and_a_required_approval_is_waited_for
echo "-- host-queue      handed to the host's own merge queue; a red is found before it is queued"
m host_queue_hands_the_pull_request_to_the_hosts_own_queue
m host_queue_red_is_found_before_the_queue_and_never_queued
echo "-- a mode whose service is missing is refused, naming it"
m a_ci_mode_without_a_remote_or_ci_section_is_refused_naming_what_is_missing

echo "== 2. kill -9 during CI and during a pull request"
t mason-daemon landing_crash killing_the_daemon_during_ci_or_a_pull_request_leaves_nothing_behind_and_lands_once
echo "   direct-ci: ci_pushed ci_finished batch_red before/after promote; pr-batch: pr_pushed pr_opened pr_checked ...; host-queue: pr_opened ...: each recovered, nothing left on the host"

echo "== 3. the adapters on recorded responses"
if [ -z "${MASON_TESTS_ALREADY_RAN:-}" ]; then
cargo test -q -p mason-adapter-api 2>&1 | grep 'test result'
cargo test -q -p mason-adapters --test github 2>&1 | grep 'test result'
fi
echo "   conformance kits (host, tracker, build system), 404 stops the poll at once, rate limit waits as told, outage backs off, signatures verified"

echo "== 4. the tracker"
[ -n "${MASON_TESTS_ALREADY_RAN:-}" ] || cargo test -q -p mason-daemon --test tracker 2>&1 | grep 'test result'

echo "== 5. mason doctor"
if [ -z "${MASON_TESTS_ALREADY_RAN:-}" ]; then
cargo test -q -p mason-pipeline doctor 2>&1 | grep 'test result'
cargo test -q -p mason-landing config 2>&1 | grep 'test result' | head -1
fi

if [ -z "${MASON_TESTS_ALREADY_RAN:-}" ]; then
  cargo test -q -p mason-engine -p mason-store 2>&1 | grep 'test result' | head -3
fi
echo "s15 OK"
