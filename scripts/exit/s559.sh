#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s559 exit test: seeding a task's build directory, and the decision record has its table and a recommendation.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- taskcache::tests
f=docs/decisions/0552-seed-task-build-dir.md
grep -q '^| task | seeded | wall s | CPU s | disk after | load |' "$f"
grep -q '^## Recommendation' "$f"
echo "s559 OK"
