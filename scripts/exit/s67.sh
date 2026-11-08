#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s67 exit test: MASON acts on provider usage. A usage limit with a reset holds the
# whole account (waiting tasks start no session until the reset, then all resume
# with no failure and no time charged); a restart during the hold keeps it; a hold
# with no readable reset backs off and uses one probe at a time; an outage holds
# and shows as an outage; overage allow carries on and shows extra billing; warn_at
# raises one item per window per reset; an api budget reached holds the account
# until the owner releases it. Everything runs against real daemons with the fake agent.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

run "reset texts: epoch, clock with a zone, try again at, garbage" -p mason-core reset_text
run "the store: migration 18, holds and warnings survive reopening" \
    -p mason-store --test store migration_18
run "the hold model: backoff doubling to the cap, projection, waiting text" \
    -p mason-daemon --lib holds
run "real daemons with the fake agent: hold until the reset (three tasks, no session until then, none fail), \
backoff with one probe at a time, outage, overage hold and allow, warn_at once per window per reset, \
api budget held until released, owner hold/release recorded with who, kill -9 during a hold, mason account hold|release" \
    -p mason-daemon --test holds

echo "s67 OK"
