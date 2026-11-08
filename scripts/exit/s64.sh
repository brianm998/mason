#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s64 exit test: LLM usage is recorded and shown (windows with resets, overage,
# spend, tokens by project/task/step/model), kept across a restart with its age,
# and `mason usage --json` is the view. Then the usage page of both monitors is
# compared with the demo daemon's (when Swift and Gradle are installed).
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

run "the words: ages, resets, trends, overage, blocked" -p mason-core usage_text
run "the store: migration 15, thinned and pruned readings, per-attempt tokens" \
    -p mason-store --test store usage
run "a real daemon with the fake agent: windows, restart with age, no-percentage window, api spend, token attribution, mason usage --json" \
    -p mason-daemon --test usage
run "the demo daemon has accounts in several states and the usage items come out of them" \
    -p mason-daemon --test protocol demo_usage

echo "== the usage page of each monitor against the demo daemon"
root=$(mktemp -d /tmp/mason-s64.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
mkdir -p "$root/d" "$root/c"
"$bin/masond" --port 0 --demo-manual --data-dir "$root/d" --config-dir "$root/c" 2>>"$root/masond.log" &
daemon=$!
until [ -s "$root/d/daemon.json" ]; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
port=$(sed -n 's/.*"port": *\([0-9]*\).*/\1/p' "$root/d/daemon.json")
"$bin/mason" --data-dir "$root/d" --config-dir "$root/c" usage | head -12
"$bin/mason" --data-dir "$root/d" --config-dir "$root/c" usage --account api-main | grep -q 'spend:'
"$bin/conformance" usage-expected --port "$port" --out "$root/exp"
swift_ok=0; kt_ok=0
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then swift_ok=1; fi
if command -v java >/dev/null && command -v gradle >/dev/null && [ -z "${MASON_SKIP_COMPOSE:-}" ]; then kt_ok=1; fi
if [ $swift_ok = 1 ]; then
  (cd clients/swift && swift build 2>&1 | tail -1)
  clients/swift/.build/debug/monitor-tool usage --port "$port" --out "$root/sw"
  "$bin/conformance" check --expected "$root/exp" --actual "$root/sw"
else
  echo "Swift not run here"
fi
if [ $kt_ok = 1 ]; then
  (cd clients/compose && gradle --no-daemon -q :app:installDist 2>&1 | grep -vE '^$' | head -5 || true)
  clients/compose/app/build/install/app/bin/app usage --port "$port" --out "$root/kt"
  "$bin/conformance" check --expected "$root/exp" --actual "$root/kt"
else
  echo "Compose not run here"
fi

echo "s64 OK"
