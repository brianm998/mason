#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 1 exit test: all four spike results exist, each has a conclusion and
# is marked "Status: measured", and the kill -9 reattach test passes.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
fail=0
for s in suspend-resume mcp-timeouts step-host-reattach etag-polling; do
  f=docs/spikes/$s.md
  [ -f "$f" ] || { echo "missing $f" >&2; fail=1; continue; }
  grep -q '^## Conclusion' "$f" || { echo "$f has no conclusion" >&2; fail=1; }
  grep -q '^Status: measured' "$f" || { echo "$f is not measured" >&2; fail=1; }
done
bash spikes/step-host-reattach/run.sh
exit $fail
