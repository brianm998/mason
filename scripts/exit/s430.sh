#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s430 exit test: the Settings page's daemon API changes a notification route, a proposal rule
# and a schedule cadence in the files, and the live daemon uses them with no restart.
#  1. A refused value (per_day 0) and a stranger's request change nothing.
#  2. One change each to notify.toml, [proposals] and [schedule.*] through the API.
#  3. The files hold them, comments kept, and GET says the same.
#  4. masond's pid is the one it started with.
#  5. `conformance features clients/features.toml` passes.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s430.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
fail() { echo "FAILED: $*" >&2; tail -20 "$root/masond.log" >&2 || true; exit 1; }

git init -q -b main "$root/repo"
echo scratch > "$root/repo/README"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
mkdir -p "$root/c/projects" "$root/d"
cat > "$root/c/projects/scratch.toml" <<TOML
# my project file
path = "$root/repo"
base = "main"
test = "true"

[proposals]
per_day = 20 # keep this note

[schedule.nightly]
kind = "task"
every = "daily"
title = "Nightly sweep"
TOML
printf '# my routes\n[routing.failed]\nsound = "0"\n' > "$root/c/notify.toml"

MASON_CPU_FILE="$root/cpu" "$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" \
  --heartbeat-ms 100 --sweep-ms 3600000 2>>"$root/masond.log" &
daemon=$!
until [ -s "$root/d/daemon.json" ]; do kill -0 "$daemon" || fail "masond exited"; sleep 0.1; done
port=$(python3 -I -c "import json,sys; print(json.load(open(sys.argv[1]))['port'])" "$root/d/daemon.json")
pid_before=$(python3 -I -c "import json,sys; print(json.load(open(sys.argv[1]))['pid'])" "$root/d/daemon.json")
owner=${USER:-$(id -un)}
api() {  # WHO METHOD PATH [BODY]: prints the status code, body in $root/out
  curl -s -o "$root/out" -w '%{http_code}' -X "$2" -H "x-mason-as: $1" -H 'content-type: application/json' \
    ${4:+--data "$4"} "http://127.0.0.1:$port$3"
}
until [ "$(api "$owner" GET /projects/scratch/settings)" = 200 ]; do kill -0 "$daemon" || fail "masond exited"; sleep 0.2; done

echo "== 1. a refused value and a stranger change nothing"
before=$(cat "$root/c/projects/scratch.toml" "$root/c/notify.toml")
[ "$(api "$owner" POST /projects/scratch/settings '{"proposals":{"per_day":0}}')" = 400 ] || fail "per_day 0 was accepted"
[ "$(api stranger POST /projects/scratch/settings '{"proposals":{"per_day":5}}')" = 403 ] || fail "a stranger was let in"
[ "$(api stranger POST /notify/settings '{"quiet":{"sound":"22:00-07:00"}}')" = 403 ] || fail "a stranger changed notify"
[ "$(api "$owner" POST /projects/scratch/settings '{"tracker":{"token":"literal"}}')" = 400 ] || fail "a literal token was accepted"
[ "$before" = "$(cat "$root/c/projects/scratch.toml" "$root/c/notify.toml")" ] || fail "a refusal changed a file"
echo "   refused"

echo "== 2. one change each"
[ "$(api "$owner" POST /notify/settings '{"routing":{"paused":{"speech":"30s"}}}')" = 400 ] || fail "a monitor-only condition was routed"
grep -q 'monitor-only' "$root/out" || fail "the refusal does not say why: $(cat "$root/out")"
[ "$(api "$owner" POST /notify/settings '{"routing":{"failed":{"speech":"30s"}}}')" = 200 ] || fail "route: $(cat "$root/out")"
[ "$(api "$owner" POST /projects/scratch/settings '{"proposals":{"agent_normal":"queued","per_day":7}}')" = 200 ] || fail "proposals: $(cat "$root/out")"
[ "$(api "$owner" POST /projects/scratch/settings '{"schedules":{"nightly":{"every":"6h"}}}')" = 200 ] || fail "schedule: $(cat "$root/out")"
echo "   applied"

echo "== 3. the files and the live daemon say so"
grep -q 'speech = "30s"' "$root/c/notify.toml" || fail "notify.toml lacks the route"
grep -q '# my routes' "$root/c/notify.toml" || fail "notify.toml lost its comment"
grep -q 'agent_normal = "queued"' "$root/c/projects/scratch.toml" || fail "project file lacks the rule"
grep -q 'per_day = 7 # keep this note' "$root/c/projects/scratch.toml" || fail "project file lost its comment"
grep -q 'every = "6h"' "$root/c/projects/scratch.toml" || fail "project file lacks the cadence"
api "$owner" GET /projects/scratch/settings >/dev/null
python3 -I - "$root/out" <<'PY' || fail "GET does not show the changes"
import json, sys
v = json.load(open(sys.argv[1]))
assert v["proposals"]["agent_normal"] == "queued" and v["proposals"]["per_day"] == 7, v
assert v["schedules"][0]["every"] == "6h", v
PY
api "$owner" GET /notify/settings >/dev/null
python3 -I - "$root/out" <<'PY' || fail "GET /notify/settings does not show the route"
import json, sys
v = json.load(open(sys.argv[1]))
r = [x for x in v["routing"] if x["condition"] == "failed"][0]["rules"]
assert any(x["channel"] == "speech" and x["after_s"] == 30 for x in r), r
PY
echo "   reflected"

echo "== 4. no restart"
[ "$(python3 -I -c "import json,sys; print(json.load(open(sys.argv[1]))['pid'])" "$root/d/daemon.json")" = "$pid_before" ] || fail "the pid changed"
kill -0 "$pid_before" || fail "masond is gone"
echo "   same masond"

echo "== 5. features"
cargo run -q -p mason-conformance -- features clients/features.toml || fail "conformance features failed"

trap - EXIT; cleanup
echo "s430 OK"
