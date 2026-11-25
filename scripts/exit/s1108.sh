#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 1108 exit test: the parser over the captured tool run and the compaction fixture
# reports the expected peak context fill and compaction count.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
out=$(cargo test -p mason-adapters --lib claude::tests::context_fill -- --nocapture --test-threads=1)
grep -q 'real_tool_suspend.jsonl: peak 3% compactions 0' <<<"$out"
grep -q 'synthetic_compaction.jsonl: peak 92% compactions 1' <<<"$out"
echo "OK s1108"
