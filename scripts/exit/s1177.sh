#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1177 exit test: the diagnosis card on a failed task and the context field on every decision
# card (#1046). The daemon forwards `context`; MonitorCore and the Compose core build the answer
# request and choose the failed page; both clients' features lists name the new features.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

grep -q '^Summary: ' docs/stages/s1177.md
grep -q 'string context = 4;' schemas/mason.proto
grep -q 'feature.decision-card-context' clients/features.toml
grep -q 'feature.failed-task-diagnosis' clients/features.toml
cargo test -q -p mason-daemon --lib an_answer_carries_its_context_only_when_there_is_some
cargo run -q -p mason-conformance -- features clients/features.toml

if [ -n "${MASON_SKIP_SWIFT:-}" ] || [ "$(uname)" != Darwin ] || ! command -v swift >/dev/null; then
  echo "== (Swift tests skipped: needs macOS and Swift)"
else
  log=$(mktemp /tmp/mason-s1177.XXXXXX)
  trap 'rm -f "$log"' EXIT
  if ! (cd clients/swift && swift test --filter 'DiagnosisCardTests|FailedDiagnosisSnapshotTests' >"$log" 2>&1); then
    tail -30 "$log" >&2; echo "FAIL: swift test exited non-zero" >&2; exit 1
  fi
fi
if [ -n "${MASON_SKIP_COMPOSE:-}" ] || ! command -v gradle >/dev/null; then
  echo "== (Compose test skipped: MASON_SKIP_COMPOSE is set or gradle is missing)"
else
  (cd clients/compose && gradle --console=plain :core:test --tests 'mason.monitor.DiagnosisCardTest')
fi
echo "OK s1177"
