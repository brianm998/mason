#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Runs site/pick.js against fixture user agents and site/test/fixture.json, under node if present,
# else JavaScriptCore's jsc.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
jsc=/System/Library/Frameworks/JavaScriptCore.framework/Versions/Current/Helpers/jsc
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
{
  echo 'if (typeof print === "undefined") var print = console.log;'
  cat site/pick.js
  echo "var MasonPick = (typeof module !== 'undefined' && module.exports) ? module.exports : globalThis.MasonPick;"
  printf 'var FIXTURE = '; cat site/test/fixture.json; echo ';'
  cat site/test/pick.test.js
} > "$tmp/run.js"
if [ -z "${PICK_FORCE_JSC:-}" ] && command -v node >/dev/null; then node "$tmp/run.js" | tee "$tmp/out"
elif [ -x "$jsc" ]; then "$jsc" "$tmp/run.js" 2>&1 | tee "$tmp/out"
else echo "neither node nor jsc found" >&2; exit 1; fi
grep -q "pick tests passed" "$tmp/out"
