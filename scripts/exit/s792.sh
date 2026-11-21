#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 715 exit test: the row-redraw tests really draw rows on a headless runner.
# The RowRedrawTests pass and their s237 line shows a non-zero RowView count.
# Needs macOS and Swift; skipped elsewhere, and when MASON_SKIP_SWIFT is set.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [ -n "${MASON_SKIP_SWIFT:-}" ] || [ "$(uname)" != Darwin ] || ! command -v swift >/dev/null; then
  echo "s792 skipped: needs macOS and Swift (and is not run by the Rust job)"; exit 0
fi
log=$(mktemp /tmp/mason-s792.XXXXXX)
trap 'rm -f "$log"' EXIT
if ! (cd clients/swift && swift test --filter RowRedrawTests >"$log" 2>&1); then
  tail -30 "$log" >&2; echo "FAIL: swift test exited non-zero" >&2; exit 1
fi
line=$(grep '^s237: ' "$log" | tail -1 || true)
[ -n "$line" ] || { echo "FAIL: no s237 line" >&2; exit 1; }
echo "$line"
rows=$(echo "$line" | sed -n 's/.*; \([0-9]*\) rows drawn before.*/\1/p')
pv=$(echo "$line" | sed -n 's/.*, \([0-9]*\) progress views.*/\1/p')
[ -n "$rows" ] && [ "$rows" -gt 0 ] || { echo "FAIL: no rows were drawn" >&2; exit 1; }
[ -n "$pv" ] && [ "$pv" -gt 0 ] || { echo "FAIL: no progress view was drawn" >&2; exit 1; }
echo "s792 OK"
