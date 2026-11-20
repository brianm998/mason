#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s704 exit test: the swift and gradle exit scripts skip when their skip variable is set.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
bash scripts/check-exit-guards.sh
for s in s645 s662; do
  out=$(MASON_SKIP_SWIFT=1 MASON_SKIP_COMPOSE=1 bash "scripts/exit/$s.sh")
  echo "$out" | grep -q skipped || { echo "$s did not skip: $out" >&2; exit 1; }
done
echo "s704 OK"
