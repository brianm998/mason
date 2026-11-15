#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s456 exit test: the iOS connection code (PinnedSocket + RemoteDiscovery + RemoteSession) reaches
# a paired daemon. masond runs with --lan on loopback; a code is issued through POST /pair/codes
# and exchanged at POST /pair for a token; then `monitor-tool remote` connects through the pinned
# socket and the remote discovery, and receives a snapshot. Also: the Swift unit tests for attempt
# order, fingerprint filtering, 401 and resume-on-foreground. macOS only (it builds the Swift package).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
[ "$(uname)" = Darwin ] || { echo "s456 SKIP: needs macOS for the Swift package"; exit 0; }
root=$(mktemp -d /tmp/mason-s456.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT
fail() { echo "FAIL: $*" >&2; [ -f "$root/masond.log" ] && tail -20 "$root/masond.log" >&2; exit 1; }

cargo build -q --workspace --bins
bin=$PWD/target/debug
mkdir -p "$root/c" "$root/d"
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port 0 --lan --lan-port 0 2>"$root/masond.log" &
daemon=$!
until [ -s "$root/d/daemon.json" ]; do kill -0 "$daemon" 2>/dev/null || fail "masond did not start"; sleep 0.1; done
field() { python3 -c 'import json,sys; print(json.load(sys.stdin)[sys.argv[1]])' "$1"; }
port=$(field port < "$root/d/daemon.json")
lan=$(field lan_port < "$root/d/daemon.json")

issued=$(curl -fsS -X POST -H 'Content-Type: application/json' -d '{"role":"owner"}' "http://127.0.0.1:$port/pair/codes") || fail "no pairing code"
code=$(echo "$issued" | field code); fp=$(echo "$issued" | field fingerprint)
paired=$(curl -fsSk -X POST -H 'Content-Type: application/json' -d "{\"code\":\"$code\",\"name\":\"s456\"}" "https://127.0.0.1:$lan/pair") || fail "pairing refused"
token=$(echo "$paired" | field token)
[ -n "$token" ] || fail "no token"

cd clients/swift
swift test --filter RemoteTests
swift run monitor-tool remote --host 127.0.0.1 --port "$lan" --fp "$fp" --token "$token" | tee "$root/out.txt"
grep -q 'remote snapshot received' "$root/out.txt" || fail "no snapshot"
# A wrong fingerprint is never connected to.
if swift run monitor-tool remote --host 127.0.0.1 --port "$lan" --fp "$(printf 'cd%.0s' $(seq 32))" --token "$token" >/dev/null 2>&1; then
  fail "connected with the wrong fingerprint"
fi
echo "s456 OK"
