#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s317 exit test: the exit scripts that were red on develop now check what the code does today.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'fn llm_holders' crates/daemon/src/sched.rs
grep -q 'llm_holders' crates/daemon/src/views.rs
grep -q "failed on all 2 runs: alpha" scripts/exit/s165.sh
grep -q 'compile_jobs 1' scripts/exit/s08.sh
bash scripts/exit/s129.sh
echo "s317: ok"
