#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s897 exit test: the source-free note test keeps its note outside the worktree.
# The test itself runs in the cargo-workspace step; this checks the fixture shape.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q 'tempfile::tempdir().unwrap().keep().join("source-free.txt")' crates/daemon/src/jobs.rs
! grep -q 'dir.join("source-free.txt")' crates/daemon/src/jobs.rs
