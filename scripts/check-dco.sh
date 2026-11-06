#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# DCO check. Usage:
#   check-dco.sh --message FILE   check one commit message file
#   check-dco.sh BASE..HEAD       check every non-merge commit in a range
set -euo pipefail
has_signoff() { grep -Eq '^Signed-off-by: .+ <[^<>@ ]+@[^<>@ ]+>$' "$1"; }
if [ "${1:-}" = "--message" ]; then
  has_signoff "$2" || { echo "missing Signed-off-by (use git commit -s)" >&2; exit 1; }
  exit 0
fi
range="${1:?usage: check-dco.sh BASE..HEAD | --message FILE}"
bad=0
for sha in $(git rev-list --no-merges "$range"); do
  tmp=$(mktemp); git log -1 --format=%B "$sha" > "$tmp"
  if ! has_signoff "$tmp"; then echo "commit $sha lacks Signed-off-by" >&2; bad=1; fi
  rm -f "$tmp"
done
exit $bad
