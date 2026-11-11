#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s118 exit test: a real daemon (scripted agent, injected low disk reading) with `[stall] after`,
# `[reminder] after` and `repeat` all 1s: the stalled pipeline hands a claiming client the
# reminder and then a repeat, each once; acknowledging stops it.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

run "a lasting stall repeats to a claiming client until acknowledged" -p mason-daemon --test stall a_lasting_stall
run "the repeat rule" -p mason-core notify
run "reminder.repeat is read" -p mason-daemon --lib notify
echo "s118 OK"
