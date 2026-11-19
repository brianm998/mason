#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s626 exit test: exit scripts name the iOS icon at its asset-catalog path.
set -euo pipefail
cd "$(dirname "$0")/../.."
for f in scripts/exit/s121.sh scripts/exit/s132.sh scripts/exit/s475.sh scripts/exit/s626.sh; do bash -n "$f"; done
test -f clients/swift/ios/MasonIOS/Assets.xcassets/AppIcon.appiconset/AppIcon-1024.png
if grep -rn 'MasonIOS/AppIcon-1024' scripts/exit | grep -v 's626.sh'; then
  echo "s626: stale icon path" >&2
  exit 1
fi
bash scripts/exit/s121.sh
echo "s626 OK"
