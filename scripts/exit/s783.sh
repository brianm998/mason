#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s783 exit test: the `it` binary lists one test per crash point (#779).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
list=$(cargo test -q -p mason-daemon --test it -- --list 2>/dev/null)
pts=$(sed -n '/^const POINTS/,/^];/p' crates/daemon/tests/it/crash.rs | grep -c '^    "')
n=$(printf '%s\n' "$list" | grep -c '^crash::at::.*: test$')
[ "$n" -eq "$pts" ] || { echo "FAILED: $n crash::at tests for $pts points" >&2; exit 1; }
c=$(printf '%s\n' "$list" | grep -c '^landing_crash::ci_at::.*: test$')
[ "$c" -eq 13 ] || { echo "FAILED: $c landing_crash::ci_at tests, want 13" >&2; exit 1; }
printf '%s\n' "$list" | grep -q 'every_crash_point_has_its_own_test' || { echo "FAILED: no list test" >&2; exit 1; }
echo "s783 passed"
