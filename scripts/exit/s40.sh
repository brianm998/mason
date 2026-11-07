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
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s40.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT

cargo build -q -p mason-daemon -p mason-conformance
bin=target/debug

echo "== 1. answers, approvals and rejections through the protocol"
cargo test -q -p mason-daemon --test approvals 2>&1 | grep -E "test result|FAILED|panicked"
cargo test -q -p mason-daemon --test protocol an_answer_carries_the_card_hash 2>&1 | grep -E "test result|FAILED|panicked"
cargo test -q -p mason-conformance 2>&1 | grep -E "test result" | head -1

echo "== 2. the owner is offered the buttons and a non-owner sees the card and none"
rm -rf "$root/d"; mkdir -p "$root/d"
"$bin/masond" --port 0 --demo-manual --data-dir "$root/d" --config-dir "$root/c" 2>>"$root/masond.log" &
daemon=$!
until [ -s "$root/d/daemon.json" ]; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
port=$(sed -n 's/.*"port": *\([0-9]*\).*/\1/p' "$root/d/daemon.json")
owner=${MASON_AS:-${USER:-owner}}
"$bin/conformance" expected --port "$port" --actor "$owner" --out "$root/owner" >/dev/null
kill -9 "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true
rm -rf "$root/d"; mkdir -p "$root/d"
"$bin/masond" --port 0 --demo-manual --data-dir "$root/d" --config-dir "$root/c" 2>>"$root/masond.log" &
daemon=$!
until [ -s "$root/d/daemon.json" ]; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
port=$(sed -n 's/.*"port": *\([0-9]*\).*/\1/p' "$root/d/daemon.json")
"$bin/conformance" expected --port "$port" --actor "not-$owner" --out "$root/stranger" >/dev/null
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
assert set(o) == set(s), "the same cards are shown to both"
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
