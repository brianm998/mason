#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 120 exit test: a fetch that keeps failing is announced. A real masond with a short limit
# (--fetch-alert-after-ms) whose remote is moved away mid-run.
#  1. With the remote in place there is no fetch alert.
#  2. Moved away: `mason alerts` names the failed fetch and git's error.
#  3. Moved back: the next good fetch clears the alert.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s120.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() {
  echo "FAIL: $*" >&2
  if ! m alerts >&2; then echo "(mason alerts failed too)" >&2; fi
  tail -20 "$root/masond.log" >&2
  exit 1
}
wait_for() {  # SECONDS WHAT CMD...
  local end=$((SECONDS + $1)) what=$2; shift 2
  until "$@" >/dev/null 2>&1; do [ $SECONDS -lt $end ] || fail "timed out waiting for $what"; sleep 0.3; done
}
has_alert() { m alerts | grep -q -- "$1"; }
lacks_alert() { ! m alerts | grep -q -- "$1"; }

git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
echo hi > "$root/repo/README"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
mkdir -p "$root/c/projects"
cat > "$root/c/projects/mason.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
TOML
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 \
  --reload-ms 300 --fetch-interval-ms 300 --attention-ms 300 --fetch-alert-after-ms 2000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
wait_for 20 "masond" m status

echo "== 1. no alert while the remote answers"
sleep 3
lacks_alert "fetch has been failing" || fail "an alert with a healthy remote"

echo "== 2. the remote is moved away"
mv "$root/origin.git" "$root/origin.away"
wait_for 30 "the fetch alert" has_alert "git fetch has been failing"
m alerts | grep "git fetch has been failing" | grep -q "origin.git" || fail "git's error is not named"
echo "   $(m alerts | grep 'git fetch has been failing')"

echo "== 3. moved back, the alert clears"
mv "$root/origin.away" "$root/origin.git"
wait_for 30 "the fetch alert to clear" lacks_alert "git fetch has been failing"

trap - EXIT; cleanup
echo "s120 OK"
