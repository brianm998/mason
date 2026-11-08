#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s72 exit test: banners compared without their clock, slots held outside the broker are
# counted, the conflict tests need no git identity, and no dependency has an advisory.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

run "a banner is compared without its clock" \
    -p mason-conformance --lib a_banner_is_compared_without_its_clock
run "a slot held outside the broker is not granted again" \
    -p mason-daemon --lib a_slot_held_outside_the_broker_is_not_granted_again
echo "== the conflict tests pass without a git identity (as on a CI runner)"
out=$(GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 cargo test -q -p mason-daemon --test e2e conflict 2>&1) \
    || { echo "$out" >&2; echo "FAILED: the conflict tests need a git identity" >&2; exit 1; }
echo "$out" | grep -E '^test result' | tail -1
if command -v cargo-deny >/dev/null; then
    echo "== no dependency has an advisory"
    cargo deny check advisories 2>&1 | tail -1
else
    echo "== (cargo-deny not installed: the advisories check is CI's lint job)"
fi

echo "s72 OK"
