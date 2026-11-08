#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 11 exit test: the client protocol, views and parity tooling.
#  1. A test client connects, disconnects, resumes from its last sequence number
#     and receives only what it missed.
#  2. Both latency targets: a state change reaches a client in under 250 ms, and
#     when a step ends the next task is admitted in under 100 ms.
#  3. The command line runs over the same socket (and so can do all a monitor can).
#  4. The feature check fails on a planted imbalance, and the design tokens are
#     checked; the conformance runner catches a planted difference.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s11.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT

cargo build -q --workspace --bins
bin=target/debug
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
port() { sed -n 's/.*"port": *\([0-9]*\).*/\1/p' "$root/d/daemon.json"; }
start_demo() {
  rm -rf "$root/d"; mkdir -p "$root/d"
  "$bin/masond" --port 0 --demo-manual --data-dir "$root/d" --config-dir "$root/c" 2>>"$root/masond.log" &
  daemon=$!
  until [ -s "$root/d/daemon.json" ]; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
}
stop_demo() { kill -9 "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; daemon=""; }

echo "== 1. a client disconnects and resumes from its last sequence number"
start_demo
"$bin/conformance" resume --port "$(port)"
stop_demo

echo "== 2a. a state change reaches a local client in under 250 ms"
start_demo
"$bin/conformance" latency --port "$(port)" --limit-ms 250
stop_demo

echo "== 2b. when a step ends, the next task is admitted in under 100 ms"
out=$(cargo test -q -p mason-daemon --test protocol -- --nocapture when_a_step_ends 2>&1)
echo "$out" | grep 'admitted:'
ms=$(echo "$out" | sed -n 's/.*the next task is admitted: \([0-9]*\) ms.*/\1/p' | head -1)
[ -n "$ms" ] && [ "$ms" -lt 100 ] || { echo "the next task was not admitted within 100 ms" >&2; echo "$out" >&2; exit 1; }

echo "== 3. the command line runs over the same socket"
start_demo
for _ in 1 2 3 4; do curl -s -X POST "http://127.0.0.1:$(port)/demo/next" >/dev/null; done
m task list | head -6
m task show 2 | sed -n '/QUESTION/,/card  /p' | head -8
m task show 2 | grep -q 'QUESTION  Keep the old token format for one release?'
m status | head -3
# What a monitor can do, the command line can do: the same answer, hash and all.
msg=$(m task answer 2 1 --card 0000000000000000 2>&1 || true)
echo "$msg" | grep -q stale
m task answer 2 1
m task show 2 --json | grep -q '"state": "developing"'
echo "(the answer went over the socket; the task is back in development)"
stop_demo

echo "== 4. the parity tooling"
"$bin/conformance" features clients/features.toml
# A planted imbalance: done in one client, todo in the other, no reason given.
cat > "$root/imbalance.toml" <<'TOML'
[feature.one]
description = "a balanced feature"
swift = "todo"
compose = "todo"

[feature.two]
description = "an imbalanced one"
swift = "done"
compose = "todo"
TOML
if out=$("$bin/conformance" features "$root/imbalance.toml" 2>&1); then echo "the planted imbalance passed the feature check" >&2; exit 1; fi
echo "$out" | head -4
echo "$out" | grep -q 'no platform_reason'
# With a recorded platform reason it is allowed.
sed -i.bak 's/^swift = "done"/swift = "done"\nplatform_reason = "Compose has no equivalent on this platform"/' "$root/imbalance.toml"
"$bin/conformance" features "$root/imbalance.toml" | tail -1

"$bin/conformance" tokens --check
mkdir -p "$root/tok/clients"; cp clients/tokens.toml "$root/tok/clients/"
"$bin/conformance" tokens --write --root "$root/tok" >/dev/null
sed -i.bak 's/#D93A3A/#000000/' "$root/tok/clients/tokens.toml"
if "$bin/conformance" tokens --check --root "$root/tok" 2>/dev/null; then echo "a stale generated token file passed" >&2; exit 1; fi
echo "(a changed token with stale generated files fails the check)"

# The conformance runner: expected content from the demo's views at every
# checkpoint; a client that matches passes, one that differs does not.
start_demo
"$bin/conformance" expected --port "$(port)" --out "$root/expected" | tail -3
stop_demo
"$bin/conformance" check --expected "$root/expected" --actual "$root/expected"
mkdir -p "$root/actual"; cp "$root/expected"/*.json "$root/actual/"
# A planted difference: the client offers a button the task must not have
# ("delete" on the task that is testing). (Landed rows are collapsed away.)
python3 - "$root/actual/checkpoint-7.json" <<'PY'
import json, sys
p = sys.argv[1]
items = json.load(open(p))
for i in items:
    if i["id"] == "row:testing:1":
        i["actions"] = i["actions"] + ["delete"]
json.dump(items, open(p, "w"))
PY
if out=$("$bin/conformance" check --expected "$root/expected" --actual "$root/actual" 2>&1); then echo "the planted difference passed conformance" >&2; exit 1; fi
echo "$out" | head -3
echo "$out" | grep -q 'row:testing:1: offers .*"delete".* but may offer exactly'

# The rest: the schema and both encodings, the views and their sorting, allowed
# actions, the resume rules, and the parity tooling's own tests.
trap - EXIT; cleanup
if [ -z "${MASON_TESTS_ALREADY_RAN:-}" ]; then
  cargo test -q -p mason-protocol -p mason-conformance
  cargo test -q -p mason-daemon --test protocol
fi
echo "s11 OK"
