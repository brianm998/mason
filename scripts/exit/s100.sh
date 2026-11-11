#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s100 exit test: a real masond with the free-disk reading below the floor and three queued
# tasks. `mason status` prints the STUCK line with the disk figures and a task count (and --json
# the same), `mason resources` names the disk for all three, `mason notify status` lists `disk`,
# and a delivery is recorded off the monitor. With room the line and alert go and the tasks land.
# The disk reading is injected through MASON_DISK_FILE; the real disk is never filled.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

run "a real daemon says the disk has stopped it, and stops saying so with room" -p mason-daemon --test stuck
run "every held request names the disk" -p mason-scheduler every_request
run "disk is a routed condition that sounds and speaks" -p mason-core a_full_disk
echo "s100 OK"
