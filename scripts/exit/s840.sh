#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s840 exit test: exit scripts name only packages that exist.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q '^Summary: ' docs/stages/s840.md
bash scripts/check-exit-guards.sh
if grep -n -e '-p mason-cli' scripts/exit/*.sh | grep -v 's840.sh'; then
  echo "mason-cli remains" >&2
  exit 1
fi
tmp=$(mktemp -d)
mkdir -p "$tmp/scripts/exit"
cp scripts/check-exit-guards.sh "$tmp/scripts/"
cp -R crates "$tmp/crates"
printf '#!/usr/bin/env bash\ncargo test -p mason-cli\n' > "$tmp/scripts/exit/s1.sh"
git -C "$tmp" init -q
if (cd "$tmp" && bash scripts/check-exit-guards.sh) >/dev/null 2>&1; then
  echo "guard accepted a missing package" >&2
  rm -rf "$tmp"
  exit 1
fi
rm -rf "$tmp"
echo "s840 OK"
