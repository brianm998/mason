#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Every `uses:` of an action must name a full 40-hex commit SHA: a tag or branch can be moved to
# other code, which then runs with the repository's token. Local `./` actions are exempt.
# Usage: check-action-pins.sh [file...]
# (default: .github/workflows/*.yml|yaml and every .github/actions/**/action.yml|yaml)
set -euo pipefail

if [ "$#" -eq 0 ]; then
  cd "$(dirname "$0")/.."
  files=()
  while IFS= read -r f; do files+=("$f"); done < <(
    find .github/workflows -type f \( -name '*.yml' -o -name '*.yaml' \) | sort
    if [ -d .github/actions ]; then
      find .github/actions -type f \( -name action.yml -o -name action.yaml \) | sort
    fi
  )
  if [ "${#files[@]}" -eq 0 ]; then
    echo "FAILED: no workflow files found" >&2
    exit 1
  fi
  set -- "${files[@]}"
fi

awk '
  /^[[:space:]]*#/ { next }
  # `uses` as a key: at the start of a line, after a list dash, or inside a flow mapping.
  match($0, /(^[[:space:]]*(-[[:space:]]+)?|[{,][[:space:]]*)["\047]?uses["\047]?[[:space:]]*:/) {
    value = substr($0, RSTART + RLENGTH)
    sub(/^[[:space:]]*["\047]?/, "", value)
    sub(/["\047[:space:],}].*$/, "", value)
    if (value ~ /^\.\//) next
    n = split(value, part, "@")
    # Counted by length: not every awk reads a {40} repetition.
    if (n == 2 && length(part[2]) == 40 && part[2] !~ /[^0-9a-f]/) next
    printf "FAILED: %s:%d: not pinned to a commit SHA: %s\n", FILENAME, FNR, $0 > "/dev/stderr"
    bad++
  }
  END { exit bad ? 1 : 0 }
' "$@"
echo "action pins ok"
