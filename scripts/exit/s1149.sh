#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1149 exit test: the record of why pages/deploy was red still matches the workflow and the
# owner action it names.
set -euo pipefail
cd "$(dirname "$0")/../.."

fail() { echo "FAILED: s1149: $*" >&2; exit 1; }

grep -qE 'uses: actions/configure-pages@[0-9a-f]{40}( |$)' .github/workflows/pages.yml ||
  fail "pages.yml has no SHA-pinned configure-pages step"
grep -q '^Summary: ' docs/stages/s1149.md || fail "the stage note has no Summary line"
grep -qF 'Owner action: set Settings > Pages > Source to GitHub Actions.' docs/stages/s226.md ||
  fail "s226 no longer names the owner action"
echo "s1149 OK"
