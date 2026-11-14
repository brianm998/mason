#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s268 exit test: the release workflow lints, and its asset list equals docs/RELEASING.md.
set -euo pipefail
cd "$(dirname "$0")/../.."
if command -v actionlint >/dev/null 2>&1; then
  actionlint .github/workflows/release.yml
  echo "actionlint: release.yml ok"
else
  echo "s268: actionlint is not installed; SKIPPED linting release.yml"
fi
bash scripts/tests/release-assets.sh
# The Windows CLI jobs (the former WINDOWS-CLI-JOBS gap) landed in s275.
grep -q "^  windows-e2e:" .github/workflows/release.yml
echo "s268 OK"
