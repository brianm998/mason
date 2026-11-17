#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 432 exit test: pipeline self-correction.
#  1. On a healthy daemon `mason health pipeline --json` reports no findings.
#  2. Against a test daemon with the scripted agent (fake-claude): a seeded stuck task, one health
#     tick, a health session starts, `needs_owner` shows the banner and "Active sessions (1)" in
#     the views, `proposed` files a proposal, ending the session removes the banner.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
command -v python3 >/dev/null || { echo "s432 needs python3" >&2; exit 1; }
root=$(mktemp -d /tmp/mason-s432.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT
fail() { echo "FAIL: $*" >&2; tail -20 "$root/masond.log" >&2 || true; exit 1; }

echo "== 1. a healthy daemon reports no findings"
cargo build -q --workspace --bins
bin=target/debug
cpu="$root/cpu"; echo 0 > "$cpu"; export MASON_CPU_FILE="$cpu"
free() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
port=$(free)
mkdir -p "$root/c"
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port "$port" --owner "${USER:-owner}" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" 2>/dev/null || fail "masond did not start"; sleep 0.1; done
m health pipeline --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["healthy"] is True and d["findings"]==[], d' \
  || fail "a healthy daemon reported findings: $(m health pipeline --json)"
m health --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["pipeline"]["findings"]==[], d' \
  || fail "mason health --json has pipeline findings on a healthy daemon"
m health history --json >/dev/null || fail "mason health history failed"
echo "PASS: no findings on a healthy daemon, and history is readable"
kill "$daemon"; wait "$daemon" 2>/dev/null || true; daemon=""

echo "== 2. stuck task, one tick, session, banner, count, proposal, end"
out=$(cargo test -q -p mason-daemon --test it health:: 2>&1) || { echo "$out" >&2; fail "the health tests failed"; }
echo "$out" | grep -E '^test result' | tail -2
out=$(cargo test -q -p mason-core --lib -- health 2>&1) || { echo "$out" >&2; fail "the core health tests failed"; }
echo "$out" | grep -E '^test result' | tail -1
out=$(cargo test -q -p mason-adapter-api --lib -- health 2>&1) || { echo "$out" >&2; fail "the contract tests failed"; }
echo "$out" | grep -E '^test result' | tail -1
echo "s432: all checks passed"
