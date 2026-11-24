#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Prints the runnable exit scripts (scripts/exit/*.sh, not -real/-manual) a change can break, one
# path per line (s669). Static, no timing. Usage: select-exit-scripts.sh BASE_COMMIT
# A script is selected when it was added or edited, or its text names a changed path (or the changed
# file's name when it is under scripts/), the `-p <package>` of a changed crate, or `--test <target>`
# of a changed integration-test target, except that a changed file under crates/<c>/tests/ selects
# only scripts naming its module stem as a whole word (s785). Docs-only changes select nothing.
# A changed crates/<c>/src/**/<x>.rs (or src/bin/<x>.rs) is scoped the same way (s669/s785): a
# script must run the crate (`-p <name>`) AND name the module stem as a whole word. Crate roots
# (lib.rs, main.rs, build.rs, Cargo.toml) select only scripts naming the path; the workspace suite
# builds everything. 2026-11-24: crate-wide -p selection picked ~300 of 648 scripts per change.
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
pick2() { # pick2 REGEX1 REGEX2: scripts matching both
  local s
  while IFS= read -r s; do
    grep -Eq -- "$1" "$s" && grep -Eq -- "$2" "$s" && echo "$s"
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
      case "$f" in
        crates/$c/tests/*)
          # A test file is matched by its module name only (s785): crate-wide and --test matches
          # would select hundreds of scripts; the cargo-workspace suite covers compile breaks.
          stem=$(basename "$f" .rs)
          case "$stem" in mod|main) stem=$(basename "$(dirname "$f")") ;; esac
          pick -E "(^|[^A-Za-z0-9_])$stem([^A-Za-z0-9_-]|$)"
          continue ;;
      esac
      # Crate roots: only the path pick above applies.
      case "$f" in crates/$c/Cargo.toml|crates/$c/build.rs|crates/$c/src/lib.rs|crates/$c/src/main.rs) continue ;; esac
      case "$f" in
        crates/$c/src/*.rs)
          stem=$(basename "$f" .rs)
          case "$stem" in mod) stem=$(basename "$(dirname "$f")") ;; esac
          name=$(sed -n 's/^name *= *"\(.*\)"/\1/p' "crates/$c/Cargo.toml" 2>/dev/null | head -n 1)
          [ -n "$name" ] && pick2 "(-p|--package)[ =]+$name([^A-Za-z0-9_-]|$)" "(^|[^A-Za-z0-9_-])$stem([^A-Za-z0-9_-]|$)"
          ;;
      esac
      ;;
  esac
done <<< "$changed"
sort -u "$sel" | while IFS= read -r s; do [ -e "$s" ] && echo "$s"; done
exit 0
