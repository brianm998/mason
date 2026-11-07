#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Readable plan (s47) exit test: the daemon parses plan.json into PlanView, the clients only draw it.
#  1. The pure parser's unit tests pass (full plan, numbering from 0 and 1, 1) and bullets, multi-line
#     steps, non-list approach, missing field, extra string/object keys, invalid JSON, JSON array).
#  2. A real-size fixture parses to 8 files, 13 criteria and numbered steps.
#  3. A demo daemon's task detail carries plan_view and the old plan string, and a malformed plan
#     carries only raw and the note.
# The UI-level check (monitor-tool task-page against a real daemon) is described in docs/stages/s47.md.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib plan_view
cargo test -q -p mason-daemon --test protocol the_task_detail_carries_the_plan
echo "s47 ok"
