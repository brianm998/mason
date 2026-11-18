#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s597 exit test: s476, s484 and s502 name the daemon tests by their `it` module.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'Summary: ' docs/stages/s597.md
grep -q 'crates/daemon/tests/it/testing.rs' scripts/exit/s476.sh
grep -q -- '--test it apns::' scripts/exit/s484.sh
grep -q -- '--test it -- limits::' scripts/exit/s502.sh
! grep -q -- '--test apns\|--test limits' scripts/exit/s484.sh scripts/exit/s502.sh
[ -e crates/daemon/tests/it/testing.rs ] && [ -e crates/daemon/tests/it/apns.rs ] && [ -e crates/daemon/tests/it/limits.rs ]
echo "s597: ok"
