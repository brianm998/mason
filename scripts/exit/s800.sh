#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Develop CI on push is the fast jobs; the full matrix is dispatched (s800, decision 0793).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
echo "== 1. the CI watch tests (jobs absent from a push run, full-only reds and fixes)"
cargo test -q -p mason-core --lib -- ci_watch::
echo "== 2. the workflow's triggers"
cargo test -q -p mason-conformance --test ci_gates
f=.github/workflows/ci.yml
if grep -q 'cron:' "$f"; then echo "FAILED: ci.yml has a cron" >&2; exit 1; fi
grep -q '^  workflow_dispatch:' "$f" || { echo "FAILED: no workflow_dispatch" >&2; exit 1; }
grep -q "'CI: full'" "$f" || { echo "FAILED: no run-name CI: full" >&2; exit 1; }
grep -q 'schedule.full-ci' docs/examples/mason-self.project.toml || { echo "FAILED: no example schedule" >&2; exit 1; }
echo "s800 ok"
