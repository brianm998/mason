#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Prints the runnable exit scripts (scripts/exit/*.sh, not -real/-manual) a change can break, one
# path per line (s669). Static, no timing. Usage: select-exit-scripts.sh BASE_COMMIT
# A script is selected when it was added or edited, or its text names a changed path (or the changed
# file's name when it is under scripts/), the `-p <package>` of a changed crate, or `--test <target>`
# of a changed integration-test target. Docs-only changes select nothing.
set -uo pipefail
cd "$(git rev-parse --show-toplevel)"
base=${1-}
# A remote run's checkout is shallow and has no base; MASON writes the change's files into the
# snapshot (s759), and that list is used instead of a diff.
list=.mason/changed-files.txt
[ -f "$list" ] || [ -n "$base" ] || { echo "usage: select-exit-scripts.sh BASE_COMMIT" >&2; exit 2; }
all=$(ls scripts/exit/*.sh 2>/dev/null | grep -Ev -- '-(real|manual)\.sh$' || true)
[ -n "$all" ] || exit 0
if [ -f "$list" ]; then changed=$(cat "$list"); else changed=$(git diff --name-only --diff-filter=ACDMRT "$base" HEAD); fi
sel=$(mktemp); trap 'rm -f "$sel"' EXIT
pick() { # pick FIXED_STRING | pick -E REGEX
  local s
  while IFS= read -r s; do
    if [ "$1" = "-E" ]; then grep -Eq -- "$2" "$s" && echo "$s"; else grep -Fq -- "$1" "$s" && echo "$s"; fi
  done <<< "$all" >> "$sel"
  return 0
}
while IFS= read -r f; do
  [ -n "$f" ] || continue
  case "$f" in
    docs/*|*.md) continue ;;
    # MASON's own snapshot files in a remote run (s759), not part of the change.
    .mason/run.json|.mason/changed-files.txt|.mason/base-sha) continue ;;
    scripts/exit/*.sh) echo "$f" | grep -Ev -- '-(real|manual)\.sh$' >> "$sel"; continue ;;
    scripts/exit/*) continue ;;
  esac
  pick "$f"
  case "$f" in scripts/*) pick "$(basename "$f")" ;; esac
  case "$f" in
    crates/*/*)
      c=${f#crates/}; c=${c%%/*}
      name=$(sed -n 's/^name *= *"\(.*\)"/\1/p' "crates/$c/Cargo.toml" 2>/dev/null | head -n 1)
      [ -n "$name" ] && pick -E "(-p|--package)[ =]+$name([^A-Za-z0-9_-]|$)"
      t=
      case "$f" in
        crates/$c/tests/*/*) t=${f#crates/$c/tests/}; t=${t%%/*} ;;
        crates/$c/tests/*.rs) t=$(basename "$f" .rs) ;;
      esac
      [ -n "$t" ] && pick -E "--test[ =]+$t([^A-Za-z0-9_-]|$)"
      ;;
  esac
done <<< "$changed"
sort -u "$sel" | while IFS= read -r s; do [ -e "$s" ] && echo "$s"; done
exit 0
