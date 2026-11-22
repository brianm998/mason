#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s918 exit test: exit-scripts remote skip and base excuse (grep-only, no timing).
set -uo pipefail
cd "$(dirname "$0")/../.."
fail=0
chk() { grep -q "$1" "$2" || { echo "s918: missing '$1' in $2"; fail=1; }; }
chk MASON_SKIP_COMPOSE crates/daemon/src/remote.rs
chk 'FAILED: scripts/exit' crates/core/src/red_base.rs
chk '^Summary: ' docs/stages/s918.md
if [ -z "${MASON_TESTS_ALREADY_RAN:-}" ]; then
  cargo test -q -p mason-core red_base || fail=1
  cargo test -q -p mason-daemon run_json || fail=1
fi
[ "$fail" -eq 0 ] && echo "s918: ok"
exit "$fail"
