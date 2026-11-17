#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s529 exit test: a real masond whose free space is read from MASON_DISK_FILE (one volume per
# line, `name gb_free gb_total`) serves two volumes on /disk/history with their names and the
# free values as the file is rewritten twice, and a project whose path is on a second volume
# adds that volume. The real disk is never read.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

run "a real daemon serves the volumes of the disk file and a project adds one" -p mason-daemon --test disk_history
run "the ring keeps the last reading of a minute and round-trips" -p mason-daemon --lib diskhistory
run "paths on one volume make one entry" -p mason-daemon --lib disk::tests
echo "s529 OK"
