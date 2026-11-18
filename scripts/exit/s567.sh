#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s567 exit test: s546 and s557 no longer fail on the Linux exit-scripts shard.
set -euo pipefail
cd "$(dirname "$0")/../.."
fail() { echo "s567: $1" >&2; exit 1; }
grep -q -- '--test it landing_split::' scripts/exit/s546.sh || fail "s546 does not run the it binary's landing_split module"
! grep -q -- '--test landing_split' scripts/exit/s546.sh || fail "s546 still names a landing_split test target"
[ -f crates/daemon/tests/it/landing_split.rs ] || fail "landing_split.rs is gone"
grep -q 'mod landing_split;' crates/daemon/tests/it/main.rs || fail "main.rs does not declare landing_split"
g=$(grep -n 'MASON_SKIP_SWIFT' scripts/exit/s557.sh | head -1 | cut -d: -f1)
d=$(grep -n 'Darwin' scripts/exit/s557.sh | head -1 | cut -d: -f1)
w=$(grep -n 'swift build\|swift test' scripts/exit/s557.sh | head -1 | cut -d: -f1)
[ -n "$g" ] && [ -n "$d" ] && [ -n "$w" ] && [ "$g" -lt "$w" ] && [ "$d" -lt "$w" ] || fail "s557 has no guard before its first swift call"
echo "s567: ok"
