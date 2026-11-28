#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1255 exit test (#1261): the monitors' runner limit.
# A scratch masond. `conformance runner-limit --set gha=N --as OWNER` sends SetLimit the way a monitor
# does and prints the home view's runner pools.
#  1. The view shows the gha pool with its default limit and the ceiling.
#  2. SetLimit gha=18 as the owner: the view and `mason resources` both say 18.
#  3. SetLimit as somebody else is refused with the reason, and the limit stays 18.
#  4. `mason resources limit gha 12` (the CLI) shows in the view as well.
#  5. After a restart the view and `mason resources` still say 12.
# Not covered here: the monitors' own drawing (their model tests and snapshots do that).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s1255.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { set +e; echo "FAILED: $*" >&2; m resources >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
start() {
  "$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" \
    --heartbeat-ms 100 --sweep-ms 3600000 2>>"$root/masond.log" &
  daemon=$!
  until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
}
port() { python3 -I -c "import json,sys; print(json.load(open(sys.argv[1]))['port'])" "$root/d/daemon.json"; }
conf() { cargo run -q -p mason-conformance -- runner-limit --port "$(port)" "$@"; }
view_limit() { conf | awk '$1=="gha" {print $2}'; }
cli_limit() { m resources --json | python3 -I -c "
import json,sys
print([r['limit'] for r in json.load(sys.stdin)['resources'] if r['name']=='gha'][0])"; }
owner=${USER:-$(id -un)}

mkdir -p "$root/c" "$root/d"
start

echo "== 1. the view shows the gha pool with a limit and a ceiling"
conf | awk '$1=="gha" && $2>0 && $3>0 {ok=1} END {exit !ok}' || fail "no gha pool with a limit and a ceiling in the home view: $(conf)"
conf | awk '$1=="gha-macos" {ok=1} END {exit !ok}' || fail "no gha-macos pool in the home view"
echo "   shown"

echo "== 2. SetLimit gha=18 as the owner: the view and mason resources say 18"
conf --set gha=18 --as "$owner" >/dev/null || fail "the owner's SetLimit was refused"
[ "$(view_limit)" = 18 ] || fail "the view says $(view_limit), not 18"
[ "$(cli_limit)" = 18 ] || fail "mason resources says $(cli_limit), not 18"
echo "   18"

echo "== 3. somebody else is refused, with the reason"
if conf --set gha=3 --as somebody-else >/dev/null 2>"$root/refused"; then fail "a non-owner changed the limit"; fi
grep -q "only the owner" "$root/refused" || fail "the refusal does not say why: $(cat "$root/refused")"
[ "$(view_limit)" = 18 ] || fail "the refused change moved the limit to $(view_limit)"
echo "   refused"

echo "== 4. a change from the CLI shows in the view"
m resources limit gha 12 >/dev/null || fail "mason resources limit gha 12"
[ "$(view_limit)" = 12 ] || fail "the view says $(view_limit), not 12"

echo "== 5. after a restart both still say 12"
kill -9 "$daemon"; wait "$daemon" 2>/dev/null || true; daemon=""
start
[ "$(view_limit)" = 12 ] || fail "after a restart the view says $(view_limit)"
[ "$(cli_limit)" = 12 ] || fail "after a restart mason resources says $(cli_limit)"
echo "   12"
echo "s1255 ok"
