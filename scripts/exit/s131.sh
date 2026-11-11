#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s131 exit test: the dropped-line audit. Runs the rules (unit and git-backed), the batch case
# and the end-to-end scenario: the base lands a line, the task's resolution takes its own side,
# landing sends it back naming the line, the agent restores it, it lands.
# The daemon in the end-to-end test is in-process (the tests' Scratch harness: real git, a bare
# origin, real step hosts, fake-claude), not the masond binary.
set -uo pipefail
cd "$(dirname "$0")/../.."
fail=0
run() {
  echo "== $*"
  out=$(cargo test -q "$@" 2>&1)
  code=$?
  echo "$out" | tail -n 15
  [ "$code" -eq 0 ] || fail=1
}
run -p mason-landing audit
run -p mason-landing --test audit
run -p mason-landing --test batch
run -p mason-daemon --test audit_e2e
run -p mason-adapter-api contract
if [ "$fail" -eq 0 ]; then echo "s131: green"; else echo "s131: RED"; fi
[ "$fail" -eq 0 ]
