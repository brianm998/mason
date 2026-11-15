#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s313 exit test: the Windows-only lint sites no longer have the patterns clippy rejected.
set -euo pipefail
cd "$(dirname "$0")/../.."
! grep -q 'return windows_free_memory_mb' crates/daemon/src/sched.rs
! grep -q 'drop(first)' crates/step-host/tests/portable.rs
echo "s313 ok"
