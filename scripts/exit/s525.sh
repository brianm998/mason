#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s525 exit test: a fix that reaches the base is merged into the idle, clean worktree of a task
# whose queued check failed the test it fixed; a dirty or conflicting worktree is left as it was.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- fixsync::tests
grep -q 'self.queued(' crates/daemon/src/jobs.rs
grep -q -- '--signoff' crates/daemon/src/fixsync.rs
echo "s525 OK"
