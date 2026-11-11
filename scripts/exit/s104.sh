#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s104 exit test: `mason doctor` warns on a daemon with no notify.toml, and stops once one
# with [speech] is written.
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$(mktemp -d /tmp/mason-s104.XXXXXX)
trap 'rm -rf "$root"' EXIT
cargo build -q --workspace --bins
bin=$PWD/target/debug
d=$root/d; c=$root/c; mkdir -p "$c" "$d"
# With an empty data dir doctor only warns, so it must exit 0; any other status fails the script.
doctor() {
  local out rc=0
  out=$("$bin/mason" --data-dir "$d" --config-dir "$c" doctor 2>&1) || rc=$?
  if [ "$rc" -ne 0 ]; then echo "$out" >&2; echo "doctor exited $rc" >&2; return 99; fi
  printf '%s\n' "$out"
}

echo "== no notify.toml"
out=$(doctor)
echo "$out" | grep -q "No notification channel works with no monitor open" || { echo "$out" >&2; echo "no warning" >&2; exit 1; }
echo "$out" | grep -q 'command = \["say"\]' || { echo "$out" >&2; echo "no fix shown" >&2; exit 1; }

echo "== with [speech]"
printf '[speech]\ndaemon = true\ncommand = ["say"]\n' > "$c/notify.toml"
out=$(doctor)
if echo "$out" | grep -q "No notification channel works"; then echo "$out" >&2; echo "still warning" >&2; exit 1; fi
echo "s104 OK"
