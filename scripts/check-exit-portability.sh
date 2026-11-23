#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Refuses constructs in exit scripts that behave differently under GNU and BSD tools, because
# the scripts run on macOS here and on Linux in CI (s669). Each refusal names file:line.
#   - `grep -E` (or egrep) with PCRE-only syntax: `(?:`, `(?=`, `(?!`, `(?<`, `\d`, or a quantifier
#     at the start of the expression (GNU grep: "? at start of expression");
#   - `sed -i ''` (the BSD form; GNU sed reads '' as the script);
#   - `cargo test` with more than one positional test name before `--` (cargo refuses it).
# Usage: check-exit-portability.sh [FILE...]   (default: scripts/exit/*.sh). Comment lines are skipped.
set -uo pipefail
cd "$(git rev-parse --show-toplevel 2>/dev/null || pwd)" || exit 1
if [ "$#" -gt 0 ]; then files=("$@"); else files=(scripts/exit/*.sh); fi
exec awk -v q="'" '
function bad(msg) { printf "error: %s:%d: %s\n", FILENAME, FNR, msg > "/dev/stderr"; failed = 1 }
/^[[:space:]]*#/ { next }
{
  line = $0
  if (line ~ /(^|[^[:alnum:]_-])(egrep|grep)[[:space:]]/ && (line ~ /egrep/ || line ~ /grep[^|;&]*[[:space:]](-[a-zA-Z]*E[a-zA-Z]*|--extended-regexp)([[:space:]]|$)/)) {
    if (line ~ /\(\?[:=!<]/) bad("grep -E pattern uses PCRE-only (?...) syntax; GNU grep rejects it")
    if (line ~ /\\d/) bad("grep -E pattern uses \\d; use [0-9]")
    if (line ~ ("[[:space:]][\"" q "][?*+]")) bad("grep -E pattern starts with a quantifier; GNU grep rejects it")
  }
  if (line ~ ("(^|[^[:alnum:]_-])sed[^|;&]*[[:space:]]-[a-zA-Z]*i[[:space:]]*(" q q "|\"\")")) bad("sed -i with an empty suffix argument is BSD-only; use sed -i.bak or a temp file")
  p = index(line, "cargo test")
  pre = substr(line, 1, p - 1)
  if (p > 0 && pre !~ ("[\"" q "#]") && pre !~ /(echo|printf|assert|mkproject|msgs)/) {
    rest = substr(line, p + 10)
    n = split(rest, t, /[[:space:]]+/)
    pos = 0; skip = 0
    for (i = 1; i <= n; i++) {
      w = t[i]
      if (w == "") continue
      if (w == "--" || w == "&&" || w == "||" || w == "|" || w == ";" || w ~ /^[0-9]*>/ || w == "\\") break
      if (skip) { skip = 0; continue }
      if (w ~ /^(-p|--package|--test|--bin|--features|-F|--target|--manifest-path|-j|--jobs|--profile|--exclude|--example|--bench|--color|--message-format)$/) { skip = 1; continue }
      if (w ~ /^-/) continue
      if (w ~ ("^[\"" q "]?\\$")) continue
      pos++
    }
    if (pos > 1) bad("cargo test takes one test name before --; pass the others after -- or run cargo test twice")
  }
}
END { exit failed }
' "${files[@]}"
