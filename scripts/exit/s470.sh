#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s470 exit test: the TestFlight workflow lints clean and has the no-secrets path and the warning.
set -euo pipefail
cd "$(dirname "$0")/../.."
f=.github/workflows/ios-testflight.yml
if command -v actionlint >/dev/null 2>&1; then actionlint "$f"; else echo "actionlint not installed; skipped"; fi
grep -q '::warning::' "$f"
grep -q 'group: testflight' "$f"
grep -q 'allowProvisioningUpdates' "$f"
grep -q 'make ios' "$f"
echo "s470: ok"
