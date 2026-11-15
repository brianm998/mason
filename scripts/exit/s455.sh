#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s455 exit test: the iOS workflow is valid, path-filtered, cancels superseded runs, time-boxed.
set -euo pipefail
cd "$(dirname "$0")/../.."
f=.github/workflows/ios.yml
grep -q 'clients/swift/\*\*' "$f"
grep -q 'cancel-in-progress: true' "$f"
grep -q 'group: ios-' "$f"
grep -q 'macos-15' "$f"
grep -q 'make ios' "$f"
t=$(sed -n 's/.*timeout-minutes: *\([0-9]*\).*/\1/p' "$f")
[ "$t" -le 25 ]
if command -v actionlint >/dev/null; then actionlint "$f" .github/actions/ios-setup/action.yml 2>/dev/null || actionlint "$f"; else echo "actionlint not installed: workflow not linted"; fi
echo "s455 ok (workflow not executed here)"
