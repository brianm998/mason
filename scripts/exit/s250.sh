#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s250 exit test: a task that fails on its own code gets a read-only diagnosis session that ends in
# `submit_diagnosis`; the self project's `retry` is applied by itself once a day, every other
# verdict waits as a card the owner applies or dismisses; at most two sessions per task; and a
# retry's `--message` reaches the next round's prompt once.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -3
}

run "the decision, the cap, the card and the prompt" -p mason-daemon --lib -- diagnose
run "the retry note is shown once" -p mason-daemon --lib -- retry_note
run "submit_diagnosis is validated" -p mason-adapter-api --lib -- diagnosis
echo "s250 OK"
