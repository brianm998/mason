#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s219 exit test: the workspace still type-checks for Windows with the AF_UNIX transport and the
# Job Object process-tree code (s193.sh does the check), the CI windows job runs the step-host and
# adapters tests, and the step host's transport tests pass here. Nothing runs on Windows from this
# script; CI's `windows` job does that after landing.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
bash scripts/exit/s193.sh
grep -q 'cargo test -p mason-platform -p mason-step-host -p mason-adapters' .github/workflows/ci.yml || {
  echo "FAILED: the windows CI job does not run the step-host and adapters tests" >&2
  exit 1
}
cargo test -q -p mason-step-host --test portable
cargo test -q -p mason-platform
echo "s219 ok"
