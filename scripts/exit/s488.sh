#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s488 exit test: the agent's cargo is pointed at the task's build directory.
set -uo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib taskcache || { echo "s488: taskcache tests RED"; exit 1; }
note=docs/stages/s488.md
grep -q 'Cache hits (Rust) 14536' "$note" || { echo "s488: stage note lacks the sccache numbers"; exit 1; }
grep -q 'Worktree `target/` sizes before' "$note" || { echo "s488: stage note lacks the target sizes"; exit 1; }
echo "s488: ok"
