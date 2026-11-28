#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1308 exit test: every failed task reaches an agent outcome, with the fake agent: a card, a
# repair, a hand-off and an owner's rejection, a job that panicked before the task had a
# worktree, and the guard over the places that emit a failure.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1308.md
test -f docs/decisions/1298-recovery-session-without-a-worktree.md
cargo build -q --workspace --bins
cargo test -q -p mason-daemon --test it recovery::
echo "OK s1308"
