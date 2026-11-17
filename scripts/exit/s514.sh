#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s514 exit test: the syspolicyd reading and limit reach the views the monitors read (from a
# FakePolicy), and a hold past the threshold raises one alert. The client line tests run in each
# client's own suite (swift test, gradle); they are not run here.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

run "the views carry the syspolicyd reading and limit" -p mason-daemon --test limits the_views_carry_the_syspolicyd_reading_and_limit
run "a hold past the threshold raises the alert" -p mason-daemon --lib a_syspolicyd_hold_alerts
