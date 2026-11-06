#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 12 exit test: the macOS monitor against `masond --demo`, through conformance.
#  1. The Swift package builds and its tests pass.
#  2. At every demo checkpoint, what the monitor shows (its visible items) matches
#     what the daemon's views require, via the same conformance runner as the other
#     client will use.
#  3. A planted difference is caught.
#  4. The application bundle (with the mason:// scheme) builds.
# Needs macOS and Swift; skipped elsewhere, and when MASON_SKIP_SWIFT is set (CI's
# Rust job: the Swift client has a job of its own).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [ -n "${MASON_SKIP_SWIFT:-}" ] || [ "$(uname)" != Darwin ] || ! command -v swift >/dev/null; then
  echo "s12 skipped: needs macOS and Swift (and is not run by the Rust job)"; exit 0
fi
root=$(mktemp -d /tmp/mason-s12.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT

cargo build -q -p mason-daemon -p mason-conformance
bin=target/debug
echo "== 1. the Swift package builds and its tests pass"
(cd clients/swift && swift build 2>&1 | tail -1 && swift test 2>&1 | grep -E "Executed .* tests" | head -1)
tool=clients/swift/.build/debug/monitor-tool

start_demo() {
  rm -rf "$root/d"; mkdir -p "$root/d"
  "$bin/masond" --demo-manual --data-dir "$root/d" --config-dir "$root/c" 2>>"$root/masond.log" &
  daemon=$!
  until [ -s "$root/d/daemon.json" ]; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
}
stop_demo() { kill -9 "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; daemon=""; }

echo "== 2. what the monitor shows matches what the views require, at every checkpoint"
start_demo; "$bin/conformance" expected --port "$(sed -n 's/.*"port": *\([0-9]*\).*/\1/p' "$root/d/daemon.json")" --out "$root/expected" | tail -2; stop_demo
start_demo; "$tool" conformance --data-dir "$root/d" --out "$root/actual" | tail -2; stop_demo
"$bin/conformance" check --expected "$root/expected" --actual "$root/actual"

echo "== 3. a planted difference is caught"
mkdir -p "$root/bad"; cp "$root/actual"/*.json "$root/bad/"
python3 - "$root/bad/checkpoint-7.json" <<'PY'
import json, sys
p = sys.argv[1]
items = json.load(open(p))
for i in items:
    if i["id"] == "tile:testing":
        i["texts"] = ["9", "testing"]          # a count the server did not say
    if i["id"] == "row:testing:1":
        i["actions"] = []                      # a button the server did allow, missing
json.dump(items, open(p, "w"))
PY
if out=$("$bin/conformance" check --expected "$root/expected" --actual "$root/bad" 2>&1); then echo "the planted difference passed" >&2; exit 1; fi
echo "$out" | head -3
echo "$out" | grep -q 'does not show "1"' && echo "$out" | grep -q 'row:testing:1: offers \[\] but may offer exactly'

echo "== 4. the application bundle builds, with the mason:// scheme"
bash scripts/bundle-monitor.sh "$root/build" | tail -1
test -x "$root/build/MASON.app/Contents/MacOS/MasonMonitor"
grep -q '<string>mason</string>' "$root/build/MASON.app/Contents/Info.plist"

if [ -z "${MASON_NO_GUI:-}" ]; then
  echo "== 5. the real app launches and connects to the daemon"
  start_demo
  clients/swift/.build/debug/MasonMonitor --data-dir "$root/d" >"$root/app.log" 2>&1 &
  app=$!
  n=0
  for _ in $(seq 1 30); do
    n=$(lsof -nP -iTCP -sTCP:ESTABLISHED -a -p "$app" 2>/dev/null | grep -c ESTABLISHED || true)
    [ "$n" -gt 0 ] && break; sleep 0.5
  done
  kill -9 "$app" 2>/dev/null || true
  [ "$n" -gt 0 ] || { cat "$root/app.log" >&2; echo "the app did not connect to the daemon" >&2; exit 1; }
  echo "the app is running and holds a connection to masond"
  stop_demo
fi

trap - EXIT; cleanup
echo "s12 OK"
