#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Monitor answers (s40) exit test (the part that needs no Swift or Compose): answering and approving
# by clicking, as the daemon sees it.
#  1. Against a real daemon in-process (cargo tests): a question answered by option and
#     by text, a stale card refused (409), a non-owner refused (403); a self-hosting hold
#     shown in full, Approve with the diff hash shown (a different hash is refused, 409),
#     Reject cancels the task; the welcome says whether the connection is the owner.
#  2. Against `masond --demo`: the card offers its buttons to the owner and none to a
#     non-owner, in the daemon's own list of what a client must show (conformance).
#  3. The two clients are in parity on the feature list.
# The monitors themselves are driven against a real daemon by s40-real.sh.
set -euo pipefail
# A failing command under `set -e` would end the script with no word: say which one, and
# show what the two `conformance expected` runs said, if they ran.
show_captures() {
  local f
  for f in owner stranger; do
    [ -n "${root:-}" ] && [ -f "$root/$f.log" ] || continue
    echo "-- $f.log (tail)" >&2; tail -40 "$root/$f.log" >&2
  done
}
trap 'rc=$?; [ "$rc" -eq 0 ] || { echo "FAILED (exit $rc) at line $LINENO: $BASH_COMMAND" >&2; show_captures; }' ERR
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s40.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT

cargo build -q -p mason-daemon -p mason-conformance
bin=target/debug

# Run cargo test; on a failure (or a run that prints no result) show everything it said.
tests() {  # PATTERN CARGO-TEST-ARGS...
  local pat=$1 out; shift
  out=$(cargo test -q "$@" 2>&1) || { printf '%s\n' "$out" >&2; echo "FAILED: cargo test $*" >&2; exit 1; }
  printf '%s\n' "$out" | grep -E "$pat" | head -${TESTS_LINES:-20} || { printf '%s\n' "$out" >&2; echo "FAILED: cargo test $* printed no test result" >&2; exit 1; }
}

echo "== 1. answers, approvals and rejections through the protocol"
tests "test result|FAILED|panicked" -p mason-daemon --test it approvals::
tests "test result|FAILED|panicked" -p mason-daemon --test it an_answer_carries_the_card_hash
TESTS_LINES=1 tests "test result" -p mason-conformance

echo "== 2. the owner is offered the buttons and a non-owner sees the card and none"
rm -rf "$root/d"; mkdir -p "$root/d"
"$bin/masond" --port 0 --demo-manual --data-dir "$root/d" --config-dir "$root/c" 2>>"$root/masond.log" &
daemon=$!
until [ -s "$root/d/daemon.json" ]; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
port=$(sed -n 's/.*"port": *\([0-9]*\).*/\1/p' "$root/d/daemon.json")
owner=${MASON_AS:-${USER:-owner}}
"$bin/conformance" expected --port "$port" --actor "$owner" --out "$root/owner" >"$root/owner.log" 2>&1
kill -9 "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true
rm -rf "$root/d"; mkdir -p "$root/d"
"$bin/masond" --port 0 --demo-manual --data-dir "$root/d" --config-dir "$root/c" 2>>"$root/masond.log" &
daemon=$!
until [ -s "$root/d/daemon.json" ]; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
port=$(sed -n 's/.*"port": *\([0-9]*\).*/\1/p' "$root/d/daemon.json")
"$bin/conformance" expected --port "$port" --actor "not-$owner" --out "$root/stranger" >"$root/stranger.log" 2>&1
python3 -I - "$root" <<'PY'
import json, sys, glob
root = sys.argv[1]
def cards(d):
    out = {}
    for f in sorted(glob.glob(f"{d}/checkpoint-*.json")):
        for i in json.load(open(f)):
            if i["id"].startswith("card:"):
                out[(f.rsplit("-", 1)[1], i["id"])] = i
    return out
o, s = cards(root + "/owner"), cards(root + "/stranger")
assert o, "the demo never showed a card"
def quiet(who):
    return [line.rstrip() for line in open(f"{root}/{who}.log") if "quiet" in line]
assert set(o) == set(s), (
    "the same cards are shown to both\n"
    f"only in the owner capture: {sorted(set(o) - set(s))}\n"
    f"only in the stranger capture: {sorted(set(s) - set(o))}\n"
    f"owner quiet exits: {quiet('owner')}\n"
    f"stranger quiet exits: {quiet('stranger')}"
)
for k in o:
    assert o[k]["texts"] == s[k]["texts"], "a non-owner sees the same card"
    assert "other" in o[k]["actions"] and any(a.startswith("option:") for a in o[k]["actions"]), o[k]
    assert s[k]["actions"] == [], s[k]
print(f"{len(o)} card checkpoint(s): the owner gets option buttons and Other, a stranger gets none")
PY
if "$bin/conformance" check --expected "$root/owner" --actual "$root/stranger" >/dev/null 2>&1; then
  echo "a client showing no buttons passed against an owner's expectation" >&2; exit 1
fi
echo "a client that shows no buttons to the owner fails conformance"

echo "== 3. parity"
"$bin/conformance" features clients/features.toml | tail -1
echo "s40 OK"
