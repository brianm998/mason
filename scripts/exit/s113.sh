#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s113 exit test: the plugin kit tests all pass, including on fresh copies of the example.
set -euo pipefail
cd "$(dirname "$0")/../.."
out=$(cargo test -p mason-plugins --test kit -- --nocapture 2>&1) || { echo "$out" >&2; exit 1; }
echo "$out" | grep -F '9 passed; 0 failed' >/dev/null || { echo "$out" >&2; echo "kit tests did not all pass" >&2; exit 1; }
echo "s113 OK"
