#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s531 exit test: in both monitors a click shows at once that it was taken, and again when masond
# answers. Driven headless with a fake sender: the pending entry and its "Approving…" label exist
# before the command is sent; a failure shows the error on that task; a success keeps the controls
# disabled until a sidebar view with the task in a new state clears it.
# Skipped per client when swift/gradle are missing (MASON_SKIP_SWIFT / MASON_SKIP_COMPOSE).
set -euo pipefail
cd "$(dirname "$0")/../.."
log=$(mktemp "${TMPDIR:-/tmp}/s531.XXXXXX"); trap 'rm -f "$log"' EXIT
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
  (cd clients/swift && swift test --filter 'PendingCommandTests|PendingTaskPageTests' >"$log" 2>&1) \
    || { tail -40 "$log" >&2; exit 1; }
else
  echo "== (Swift tests skipped)"
fi
if [ -n "${MASON_SKIP_COMPOSE:-}" ] || ! command -v gradle >/dev/null; then
  echo "== (Compose tests skipped)"
else
  (cd clients/compose && gradle :core:test --tests 'mason.monitor.PendingCommandTest' --console=plain >"$log" 2>&1) \
    || { grep -E "FAILED|tests completed|error:" "$log" >&2; exit 1; }
fi
echo "s531 OK"
