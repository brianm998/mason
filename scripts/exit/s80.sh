#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s80 exit test: the holds tests set the reset relative to when the fake agent emits
# the limit, so they pass repeatedly regardless of load.
set -euo pipefail
cd "$(dirname "$0")/../.."

for i in 1 2 3; do
    echo "== holds run $i"
    out=$(cargo test -q -p mason-daemon --test holds 2>&1) || { echo "$out" >&2; echo "FAILED: run $i" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
done

echo "s80 OK"
