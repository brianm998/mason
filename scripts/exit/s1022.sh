#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1022 exit test: s249 and s782 hold under a shard (decision 1015).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1022.md
export MASON_EXIT_SHARD=1/6
bash scripts/exit/s249.sh
bash scripts/exit/s782.sh
