#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s269 exit test: home's compile holders carry the task's project and full title.
set -euo pipefail
cd "$(dirname "$0")/../.."
run() { out=$("$@" 2>&1) || { echo "$out" >&2; exit 1; }; echo "$out" | grep -E '^test result' | tail -3; }
run cargo test -q -p mason-daemon --test scheduling -- home_compile_holders
echo "s269 OK"
