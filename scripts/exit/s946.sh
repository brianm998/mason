#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s946 exit test: base_builds names the compiler's first error.
set -uo pipefail
cd "$(dirname "$0")/../.."
if grep -q 'does not build ({first}{cause})' crates/daemon/src/landing.rs; then echo "PASS s946"; else echo "FAIL s946"; exit 1; fi
