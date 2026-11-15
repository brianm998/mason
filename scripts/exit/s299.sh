#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s299 exit test: the offscreen editor tests and the owner's list of real checks.
#  1. Both clients' editor tests drive the editor's controls (add a step, connect it, edit a form
#     field, undo) and compare the operations they send with a stand-in draft. Nothing is shown:
#     SwiftUI is drawn into a hosting view that is never in a window, Compose runs in its test rule.
#  2. docs/real-checks.md names every scripts/exit/*-real.sh, each of them ends with a RESULT line,
#     and the page names no script that does not exist.
#  3. The device sample is built by CI jobs (build only).
# Nothing real is run: no token is spent and no mail is sent.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
  swift test --package-path clients/swift --filter 'EditorSnapshotTests'
else
  echo "swift not run here" >&2
fi
if command -v java >/dev/null && command -v gradle >/dev/null && [ -z "${MASON_SKIP_COMPOSE:-}" ]; then
  (cd clients/compose && gradle --no-daemon -q :app:test --tests 'mason.monitor.ui.EditorSnapshotTest')
else
  echo "compose not run here" >&2
fi

# The checklist lists every real script, and only real ones.
for f in scripts/exit/*-real.sh; do
  name=$(basename "$f")
  grep -q "$name" docs/real-checks.md || { echo "docs/real-checks.md does not list $name" >&2; exit 1; }
  tag="RESULT ${name%.sh}:"
  tail -n 3 "$f" | grep -q "echo \"$tag" || { echo "$name does not end with a '$tag' line" >&2; exit 1; }
done
for name in $(grep -o 's[0-9][0-9]*-real\.sh' docs/real-checks.md | sort -u); do
  [ -f "scripts/exit/$name" ] || { echo "docs/real-checks.md names $name, which does not exist" >&2; exit 1; }
done

# CI builds the device sample.
grep -q 'device-sample/ios' .github/workflows/ci.yml
grep -q 'device-sample/android' .github/workflows/ci.yml

# Every file added here starts with the license line.
bash scripts/check-spdx.sh
echo "s299 ok"
