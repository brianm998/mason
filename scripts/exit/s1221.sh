#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1221 exit test: an under-granted run with no local fallback waits for runner units (decision 1211).
set -euo pipefail
cd "$(dirname "$0")/../.."

cargo test -q -p mason-daemon --lib a_short_grant_with_no_fallback_waits_for_runners_instead_of_erroring
echo "OK s1221"
