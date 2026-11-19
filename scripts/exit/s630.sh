#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s630 exit test: s602 passes under a CI shard value, because its nested runner ignores the outer shard.
set -euo pipefail
cd "$(dirname "$0")/../.."
fail() { echo "s630: $1" >&2; exit 1; }
grep -q '^unset MASON_EXIT_SHARD' scripts/exit/s602.sh || fail "s602 does not clear MASON_EXIT_SHARD"
out=$(MASON_EXIT_SHARD=4/6 bash scripts/exit/s602.sh 2>&1) || fail "s602 failed under shard 4/6: $out"
echo "$out" | tail -1 | grep -qx 's602: ok' || fail "s602 prints no ok verdict"
echo "s630: ok"
