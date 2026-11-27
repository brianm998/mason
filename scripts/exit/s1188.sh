#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1188 exit test: README has no generated status list, keeps the banner, and its relative links resolve.
set -euo pipefail
cd "$(dirname "$0")/../.."
! grep -q 'status:begin\|status:end' README.md
! grep -q '^## Stage notes' README.md
head -3 README.md | grep -q '^!\[.\+\](docs/images/readme_banner.png)'
for l in $(grep -o ']([^)]*)' README.md | sed 's/^](//; s/)$//; s/#.*//' | grep -v '^$' | grep -v '^[a-z]*://'); do
  test -e "$l" || { echo "missing link target: $l"; exit 1; }
done
bash scripts/exit/s127.sh
echo "s1188 ok"
