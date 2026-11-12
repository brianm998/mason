#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Test scripts/release-version.sh: good tag, wrong version, malformed tags.
set -uo pipefail
cd "$(dirname "$0")/../.."
have=$(awk '/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f && /^version *=/{gsub(/[" ]/,"",$0); sub(/^version=/,"",$0); print; exit}' Cargo.toml)
fail=0
out=$(bash scripts/release-version.sh "v$have" 2>/dev/null) || { echo "FAIL: good tag rejected"; fail=1; }
[ "$out" = "$have" ] || { echo "FAIL: printed '$out', want $have"; fail=1; }
for bad in v9.9.9 "$have" v1.2 v1.2.3-rc1 vx.y.z ""; do
  if bash scripts/release-version.sh "$bad" >/dev/null 2>&1; then echo "FAIL: '$bad' accepted"; fail=1; fi
done
[ "$fail" = 0 ] && echo "release-version: ok"
exit $fail
