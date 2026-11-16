#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# An exit script that runs `swift test|build` must test MASON_SKIP_SWIFT, and one that runs
# `gradle` must test MASON_SKIP_COMPOSE: CI's Linux exit-scripts job sets both because the
# monitor clients have jobs of their own (decision 0443). Only commands count: a comment, or
# a name inside a grep or sed pattern, does not.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
fail=0
lead='(^|[(&;|`]|\$\()[[:space:]]*'
for f in scripts/exit/*.sh; do
  case "$f" in *-real.sh|*-manual.sh) continue ;; esac # run-exit-scripts.sh skips these
  code=$(grep -vE '^[[:space:]]*#' "$f" || true)
  if printf '%s\n' "$code" | grep -qE "${lead}swift (test|build)"; then
    printf '%s\n' "$code" | grep -q 'MASON_SKIP_SWIFT' \
      || { echo "$f: runs swift without testing MASON_SKIP_SWIFT" >&2; fail=1; }
  fi
  if printf '%s\n' "$code" | grep -qE "${lead}gradle "; then
    printf '%s\n' "$code" | grep -q 'MASON_SKIP_COMPOSE' \
      || { echo "$f: runs gradle without testing MASON_SKIP_COMPOSE" >&2; fail=1; }
  fi
done
[ "$fail" -eq 0 ] && echo "exit-script guards ok"
exit "$fail"
