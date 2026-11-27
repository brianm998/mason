#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1144 exit test: a short network outage fails no landing batch and uses no test round
# (decision 1133). A scratch project's remote refuses pushes with a name-resolution error
# twice and then accepts: the batch lands and no task fails. A suite that fails only with
# "Could not resolve host" ends with class `outside` and no round.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1144.md
log=$(mktemp)
trap 'rm -f "$log"' EXIT
cargo test -q -p mason-daemon --test it net_outage:: 2>&1 | tee "$log"
# A filter that matched nothing would pass.
grep -q 'test result: ok\. 6 passed' "$log"
echo "OK s1144"
