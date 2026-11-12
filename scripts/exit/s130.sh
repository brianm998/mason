#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s130 exit test: the waiting text is short. A real masond with `compile` limited to 1: an
# interactive run holds the slot, two more interactive runs with long commands queue, then a
# task waits for compile. Its wait (the activity in task list and task detail, after "waiting on ") starts with
# "Waiting for compile slot:", has "in line" and "held by", no "behind", neither queued command,
# and is under 200 characters.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
command -v python3 >/dev/null || { echo "s130 needs python3" >&2; exit 1; }
root=$(mktemp -d /tmp/mason-s130.XXXXXX); daemon=""
cleanup() {
  pkill -9 -f "mason-s130-" 2>/dev/null || true
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for f in "$root"/d/h/*/host.kv; do
    [ -e "$f" ] || continue
    for k in host_pid child_pid; do pid=$(sed -n "s/^$k=//p" "$f"); kill -9 "-$pid" 2>/dev/null || kill -9 "$pid" 2>/dev/null || true; done
  done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=$PWD/target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { set +e; echo "FAILED: $*" >&2; m task list >&2 2>&1; m resources >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }

echo "== setup: a repository whose definition limits compile to 1, the scripted agent"
mkdir -p "$root/c/projects" "$root/repo"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
echo base > "$root/repo/base.txt"
mkdir -p "$root/repo/.mason"
cat > "$root/repo/.mason/mason.toml" <<'TOML'
schema = 1
start = "develop"

[resource.compile]
count = 1

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
uses = { compile = 1 }
max_rounds = 2
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
TOML
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work
cat > "$root/script.json" <<'JSON'
{"steps": {"develop": [[{"sh": "f=$(mktemp ./f-XXXXXX) && echo x > $f && git add -A && git -c user.name=t -c user.email=t@t commit -qm change"}, {"call": {"tool": "submit_change", "args": {"summary": "done", "head": "$HEAD"}}}]]}}
JSON
cat > "$root/c/projects/mason.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$bin/fake-claude"
step_timeout_s = 300
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 4 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

pad=$(printf 'x%.0s' $(seq 1 300))
queued() { m resources --json | python3 -c "import json,sys; print(len(json.load(sys.stdin)['waiting']))"; }

echo "== an interactive run holds compile, two more with long commands queue"
m run --needs compile -- sh -c "sleep 300 # mason-s130-holder $pad" >/dev/null 2>&1 &
until m resources | grep -q 'held by interactive'; do sleep 0.2; done
m run --needs compile -- sh -c "true # mason-s130-first $pad" >/dev/null 2>&1 &
until [ "$(queued)" -ge 1 ]; do sleep 0.2; done
m run --needs compile -- sh -c "true # mason-s130-second $pad" >/dev/null 2>&1 &
until [ "$(queued)" -ge 2 ]; do sleep 0.2; done

echo "== a task waits for compile"
id=$(m task add "waits" --project mason --explored --body "x" | sed -n 's/.*#\([0-9]*\).*/\1/p')
end=$((SECONDS + 90)); list=""
until [ -n "$list" ]; do
  [ $SECONDS -lt $end ] || fail "task never showed a wait"
  list=$(m task list --json | ID="$id" python3 -c "
import json,os,sys
t=[x for x in json.load(sys.stdin)['tasks'] if str(x['id'])==os.environ['ID']]
a=t[0].get('activity','') if t else ''
print(a[len('waiting on '):] if a.startswith('waiting on ') else '')")
  sleep 0.2
done
detail=$(m task show "$id" --json | python3 -c "
import json,sys
def find(o):
    if isinstance(o,dict):
        a=o.get('activity')
        if isinstance(a,str) and a.startswith('waiting on '): return a[len('waiting on '):]
        for v in o.values():
            r=find(v)
            if r: return r
    return ''
print(find(json.load(sys.stdin)))")
for w in "$list" "$detail"; do
  echo "   $w"
  case "$w" in "Waiting for compile slot:"*) ;; *) fail "wrong start: $w";; esac
  case "$w" in *"in line"*) ;; *) fail "no position: $w";; esac
  case "$w" in *"held by"*) ;; *) fail "no holder: $w";; esac
  case "$w" in *behind*) fail "still names who is ahead: $w";; esac
  case "$w" in *mason-s130-first*|*mason-s130-second*|*xxxxxxxxxx*) fail "names a queued command: $w";; esac
  [ "${#w}" -lt 200 ] || fail "${#w} characters: $w"
done

trap - EXIT; cleanup
echo "s130 OK"
