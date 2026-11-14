#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s251 exit test: the last round the test step will give runs on the escalation model, in a fresh
# session; earlier rounds keep their own model; an empty `escalate_model` turns it off. A fake
# agent and a test command that never passes drive the rounds.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -3
}

run "the round decision and the key" -p mason-daemon --lib -- last_round escalate_model
run "the last round is escalated, the earlier ones are not" -p mason-daemon --test escalate
echo "s251 OK"
