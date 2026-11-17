#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s556 exit test: the docs name `mason project reload` and do not tell a reader to kickstart after an edit.
set -uo pipefail
cd "$(dirname "$0")/../.."
ok=0
for f in CLAUDE.md docs/self-hosting.md; do
  grep -q 'mason project reload' "$f" || { echo "s556: $f does not name mason project reload"; ok=1; }
done
if grep -n -i 'kickstart' CLAUDE.md | grep -v -i 'do not' ; then
  echo "s556: CLAUDE.md mentions kickstart other than as a prohibition"; ok=1
fi
[ "$ok" -eq 0 ] && echo "s556: ok"
exit "$ok"
