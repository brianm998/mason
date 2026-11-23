#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1039 exit test: s249 and s782 hold under a shard (decision 1032).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1039.md
grep -q '^unset MASON_EXIT_SHARD' scripts/exit/s782.sh
export MASON_EXIT_SHARD=1/6
bash scripts/exit/s249.sh
bash scripts/exit/s782.sh
