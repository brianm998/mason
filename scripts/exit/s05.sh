#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 5 exit test: the fake agent goes through accepted, rejected-then-fixed,
# no-result, timeout, usage-limit and crash scenarios; both adapters pass the
# conformance kit; and a real-address call fails in test mode.
# (The real-CLI run is a separate manual test: see crates/adapters/tests/real_claude.rs.)
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT

cargo test -p mason-adapter-api -p mason-adapters 2>&1 | tee "$tmp/out" | grep -E 'test result|FAILED|panicked' || true
grep -q FAILED "$tmp/out" && { echo "tests failed" >&2; exit 1; }

# Every named scenario must have run, and passed.
need=(
  "accepted_in_one_turn"
  "rejected_then_fixed_in_the_same_session"
  "no_result_is_nudged_with_a_note_then_fails"
  "timeout_fails_the_step_and_is_not_nudged"
  "usage_limit_is_a_hold_not_a_failure_and_keeps_the_figures"
  "a_crash_fails_the_step"
  "the_fake_agent_passes_the_conformance_kit"
  "the_claude_adapter_passes_the_conformance_kit_against_the_standin_cli"
  "a_real_address_call_fails_in_test_mode"
)
for t in "${need[@]}"; do
  grep -qE "^test ${t} \.\.\. ok$" "$tmp/out" || { echo "scenario did not run or did not pass: $t" >&2; exit 1; }
  echo "ok: $t"
done
echo "s05 OK"
