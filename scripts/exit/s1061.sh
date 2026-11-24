#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1061 exit test: s759 clears the CI shard env and s664 does not rely on quiet cargo output.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q '^unset MASON_EXIT_SHARD' scripts/exit/s759.sh || { echo "FAILED: s759 keeps the shard env" >&2; exit 1; }
if grep -q 'cargo test -q' scripts/exit/s664.sh; then echo "FAILED: s664 uses cargo test -q" >&2; exit 1; fi
MASON_EXIT_SHARD=3/6 bash scripts/exit/s759.sh
echo "s1061 OK"
