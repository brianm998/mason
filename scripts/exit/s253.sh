#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s253 exit test: a test that flaked in three different tasks within a week, and that its suite has
# been seen to pass, is quarantined for 14 days by `mason` with one task filed; a name never seen
# passing, one task flaking three times, a flake older than the window and a quarantine already in
# force are not. The lift of a quarantine whose task landed is checked on its selection.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -3
}

run "the threshold, the reason, the lift" -p mason-daemon --lib -- autoquarantine
run "a daemon quarantines, files once, and keeps the guards" -p mason-daemon --test auto_quarantine
echo "s253 OK"
