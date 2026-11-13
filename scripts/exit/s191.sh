#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s191 exit test: release identity.
set -uo pipefail
cd "$(dirname "$0")/../.."
fail=0
out=$(cargo run -q -p mason-pipeline --bin mason -- --version) || fail=1
case "$out" in "mason 0.1.0 ("*")") ;; *) echo "bad --version: $out"; fail=1;; esac
[ "$(bash scripts/release-version.sh v0.1.0)" = 0.1.0 ] || { echo "release-version v0.1.0"; fail=1; }
bash scripts/release-version.sh v9.9.9 2>/dev/null && { echo "v9.9.9 accepted"; fail=1; }
bash scripts/tests/release-version.sh || fail=1
if git grep -n 'mason-pipeline/mason' -- . ':!docs' ':!scripts/exit/s191.sh'; then fail=1; fi
[ "$fail" = 0 ] && echo "s191: ok"
exit $fail
