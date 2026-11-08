#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 19 exit test: project extensions, against a real masond.
#  1. A plugin written in Python (docs/examples/plugins/migration-check) provides a step type.
#     It is in the editor's palette, with a form generated from its config_schema.
#  2. It runs in a graph: refused before approval (and it did not run), then approved
#     by the owner (a stranger is refused), then it runs and its result is used.
#  3. Changing any file in its folder changes the hash and needs a new approval.
#  4. An extension that exists only on a task branch never runs and is not offered.
#  5. A plugin that provides a parser is refused as not yet supported; the rest loads.
#  6. Script steps get MASON_* variables and a JSON result file; the conformance kit
#     passes the example and names what is wrong with a broken one.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if ! command -v python3 >/dev/null; then echo "s19 needs python3 (the plugin is a Python program)" >&2; exit 1; fi
root=$(mktemp -d /tmp/mason-s19.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for f in "$root"/d/h/*/host.kv; do
    [ -e "$f" ] || continue
    hp=$(sed -n "s/^host_pid=//p" "$f"); cp=$(sed -n "s/^child_pid=//p" "$f"); if [ -n "$hp" ] && ps -o command= -p "$hp" 2>/dev/null | grep -qF "step-host --dir $(dirname "$f")"; then kill -9 "-$cp" 2>/dev/null || true; kill -9 "$hp" 2>/dev/null || true; fi  # only a live step host: a stale pid may belong to someone else now
  done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_for_task() {  # TASK PATTERN SECONDS: until `mason task show` mentions PATTERN
  local end=$((SECONDS + $3))
  until m task show "$1" 2>/dev/null | grep -q -- "$2"; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1: $2" >&2; m task show "$1" >&2 || true; exit 1; }
    sleep 0.2
  done
}
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1 to be $2" >&2; m task show "$1" >&2 || true; exit 1; }
    sleep 0.2
  done
}
lines() { wc -l < "$root/ran.log" | tr -d ' '; }
add() { m task add "$1" --project web --body "x" | sed -n 's/^Filed task #\([0-9]*\) .*/\1/p'; }
ci="git -C $root/pusher -c user.name=t -c user.email=t@t"
publish() {  # MESSAGE: commit what is in the pusher clone to origin's main, and let the project see it
  $ci add -A; $ci commit -qm "$1"; $ci push -q origin HEAD:main
  git -C "$root/repo" fetch -q origin
}

# ---- the project: a pipeline with a script step, the plugin's step, and an approval ----
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
mkdir -p "$root/repo/.mason/extensions" "$root/repo/migrations"
cp -R docs/examples/plugins/migration-check "$root/repo/.mason/extensions/migration-check"
# An extension that provides a parser: not supported yet, so refused (and harmless).
mkdir -p "$root/repo/.mason/extensions/junit-parser"
cat > "$root/repo/.mason/extensions/junit-parser/manifest.toml" <<'TOML'
name = "junit-parser"
protocol = 1
provides = ["parser:junit"]
run = "./parse.sh"
TOML
printf '#!/bin/sh\nexit 0\n' > "$root/repo/.mason/extensions/junit-parser/parse.sh"; chmod +x "$root/repo/.mason/extensions/junit-parser/parse.sh"
printf -- '-- reversible\nalter table a add column b int;\n' > "$root/repo/migrations/001.sql"
cat > "$root/repo/.mason/mason.toml" <<TOML
schema = 1
start = "prepare"

# Level 2, a script step: MASON_* variables in, a JSON result file out.
[step.prepare]
type = "command"
category = "developing"
next = "check"
command = '''
env | grep '^MASON_' | sort > mason-env.txt
echo "work of task \$MASON_TASK" > work.txt
# A task branch that tries to take over the extension and to add one of its own:
if [ -f "$root/evil-on" ]; then
  printf '#!/bin/sh\ntouch "$root/evil-ran"\n' > .mason/extensions/migration-check/check.py
  mkdir -p .mason/extensions/sneaky
  printf 'name = "sneaky"\nprotocol = 1\nprovides = ["step-type:sneaky-check"]\nrun = "./go.sh"\n' > .mason/extensions/sneaky/manifest.toml
  printf '#!/bin/sh\ntouch "$root/evil-ran"\n' > .mason/extensions/sneaky/go.sh; chmod +x .mason/extensions/sneaky/go.sh
fi
git add -A && git -c user.name=t -c user.email=t@t commit -qm "work of task \$MASON_TASK"
printf '{"passed": true, "summary": "prepared"}' > "\$MASON_RESULT_FILE"
'''

# Level 3, a process plugin in .mason/extensions/migration-check
[step.check]
type = "migration-check"
category = "testing"
directory = "migrations"
log = "$root/ran.log"
next = "sign-off"

[step.sign-off]
type = "approval"
category = "review"
next = "land"

[step.land]
type = "landing"
category = "landing"
TOML
printf 'mode = "direct"\nwindow = "20m"\n' > "$root/repo/.mason/landing.toml"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm "pipeline with extensions"
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work
git clone -q "$root/origin.git" "$root/pusher" 2>/dev/null
mkdir -p "$root/c/projects"
cat > "$root/c/projects/web.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
TOML
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== 1. the plugin is in the editor's palette, with a form generated from its config_schema"
m extension list
m extension list | grep -q '^migration-check  unapproved'
j=$(m editor show --json)
echo "$j" | grep -q '"type": "migration-check"'
echo "$j" | grep -q '"plugin": "migration-check"'
echo "$j" | grep -q '"title": "Migrations directory"'
echo "$j" | grep -q '"description": "Where the .sql files are, relative to the worktree."'
echo "   palette entry:"
echo "$j" | python3 -c '
import json, sys
e = [p for p in json.load(sys.stdin)["palette"] if p["type"] == "migration-check"][0]
print("    type", e["type"], "| categories", e["categories"], "| new step starts with", e["fields"])
for k in e["schema"]["x-order"]:
    p = e["schema"]["properties"][k]
    print("    form field %-12s %-8s %s" % (k, p.get("type"), p.get("title", "")))
'
echo "   the inspector builds the same form for the step on the canvas:"
m editor show --step check --json | grep -q '"x-plugin": "migration-check"'
echo "   and the editor can add one, with the palette's fields:"
m editor op '{"op":"insert_step","after":"check","name":"check-two","type":"migration-check","category":"review","fields":{"directory":"db"}}'
m editor show | grep -q 'check-two (migration-check)'
if out=$(m editor op '{"op":"set_field","step":"check-two","key":"directory","value":7}' 2>&1); then echo "a wrong setting was accepted" >&2; exit 1; fi
echo "   refused: $out"; echo "$out" | grep -q 'directory must be a string'
m editor discard

echo "== 5. a plugin that provides a parser is refused as not yet supported; the rest loads"
m extension list | grep -q '^junit-parser  refused'
m extension list | grep -q 'not yet supported'
m extension list | sed -n '/^junit-parser/,+1p'
m definition show | grep -q 'check'
if out=$(m extension approve junit-parser 2>&1); then echo "a refused extension was approved" >&2; exit 1; fi
echo "   $out"

echo "== 2. before approval the step fails, naming the command, and the plugin does not run"
t0=$(add "first task")
wait_state "$t0" failed 60
m task show "$t0" | grep -q 'needs approval: mason extension approve migration-check'
m task show "$t0" | grep 'needs approval' | head -1
[ ! -e "$root/ran.log" ] || { echo "the plugin ran without approval" >&2; exit 1; }
[ ! -e "$root/c/trust/web.toml" ]
echo "   (the plugin did not run; no approval is on record)"

echo "== 2. only the owner may approve; a stranger is refused and nothing changes"
if out=$(m --as mallory extension approve migration-check 2>&1); then echo "a stranger approved an extension" >&2; exit 1; fi
echo "   $out"; echo "$out" | grep -q "not the owner's identity"
[ ! -e "$root/c/trust/web.toml" ]
m extension list | grep -q '^migration-check  unapproved'

echo "== 2. the owner approves; the step runs, and its result is used"
m extension approve migration-check
h1=$(m extension list --json | python3 -c 'import json,sys; print([e["hash"] for e in json.load(sys.stdin)["extensions"] if e["name"]=="migration-check"][0])')
grep -q "$h1" "$root/c/trust/web.toml"
grep -q "by = \"$owner\"" "$root/c/trust/web.toml"
echo "   approved $h1, recorded in the config dir (outside the repository):"
sed 's/^/     /' "$root/c/trust/web.toml"
t1=$(add "second task")
wait_for_task "$t1" 'sign-off' 60
[ "$(lines)" = 1 ]
cat "$root/ran.log"
[ -d "$root/c/extensions/$h1" ]
echo "   it ran, from the approved copy in the config dir; the task went on to the next step"
echo "   (the script step's MASON_* variables:)"
wt="$root/d/w/web/$t1"
[ -d "$wt" ] || wt=$(find "$root/d/w/web" -maxdepth 1 -mindepth 1 -type d | head -1)
sed 's/^/     /' "$wt/mason-env.txt" | sed 's#=/.*#=…#'
grep -q "^MASON_TASK=$t1$" "$wt/mason-env.txt"
grep -q '^MASON_STEP=prepare$' "$wt/mason-env.txt"
grep -q '^MASON_RESULT_FILE=' "$wt/mason-env.txt"
echo "   a bad migration lands on the base; the plugin's verdict sends the task back:"
printf 'drop table a;\n' > "$root/pusher/migrations/002.sql"
git -C "$root/pusher" pull -q origin main 2>/dev/null || true
publish "a migration with no marker"
t2=$(add "third task")
wait_state "$t2" failed 60
m task show "$t2" | grep -q '1 of 2 migrations lack `-- reversible`: 002.sql'
m task show "$t2" | grep 'migrations lack' | head -1
git -C "$root/pusher" rm -q migrations/002.sql; publish "remove it again"

echo "== 3. changing any file in the folder changes the hash and needs a new approval"
printf '\n# a harmless comment\n' >> "$root/pusher/.mason/extensions/migration-check/check.py"
publish "touch the plugin"
sleep 1
h2=$(m extension list --json | python3 -c 'import json,sys; print([e["hash"] for e in json.load(sys.stdin)["extensions"] if e["name"]=="migration-check"][0])')
[ "$h1" != "$h2" ]
echo "   old hash $h1"; echo "   new hash $h2"
m extension list | grep -q '^migration-check  changed'
before=$(lines)
t3=$(add "fourth task")
wait_state "$t3" failed 60
m task show "$t3" | grep -q 'needs approval: mason extension approve migration-check'
m task show "$t3" | grep 'needs approval' | head -1
[ "$(lines)" = "$before" ]
echo "   (the old approval does not apply to the new hash; the plugin did not run)"
m extension approve migration-check
t4=$(add "fifth task")
wait_for_task "$t4" 'sign-off' 60
[ "$(lines)" = $((before + 1)) ]
echo "   approved again: it runs"

echo "== 4. an extension on a task branch never runs and is not offered"
git -C "$root/pusher" checkout -q -b feature
mkdir -p "$root/pusher/.mason/extensions/sneaky"
printf 'name = "sneaky"\nprotocol = 1\nprovides = ["step-type:sneaky-check"]\nrun = "./go.sh"\n' > "$root/pusher/.mason/extensions/sneaky/manifest.toml"
printf '#!/bin/sh\ntouch "%s/evil-ran"\n' "$root" > "$root/pusher/.mason/extensions/sneaky/go.sh"; chmod +x "$root/pusher/.mason/extensions/sneaky/go.sh"
$ci add -A; $ci commit -qm "sneaky"; $ci push -q origin feature
git -C "$root/repo" fetch -q origin
! m extension list | grep -q sneaky
! m editor show --json | grep -q sneaky-check
echo "   a branch of origin has the extension; the project does not list it or offer its step type"
touch "$root/evil-on"
t5=$(add "sixth task, whose branch rewrites the approved plugin and adds another")
wait_for_task "$t5" 'sign-off' 60
[ "$(lines)" = $((before + 2)) ]
grep -q 'evil-ran' "$root/d/w/web/$t5/.mason/extensions/migration-check/check.py"
[ -f "$root/d/w/web/$t5/.mason/extensions/sneaky/manifest.toml" ]
echo "   the task's own branch holds a rewritten check.py and a sneaky extension (it is in its worktree)"
[ ! -e "$root/evil-ran" ] || { echo "an extension from a task branch ran" >&2; exit 1; }
! m extension list | grep -q sneaky
echo "   but the plugin that ran was the approved one: the marker file does not exist"

echo "== 6. the conformance kit"
"$bin/conformance" plugin-kit docs/examples/plugins/migration-check
rm -rf "$root/broken"; cp -R docs/examples/plugins/migration-check "$root/broken"
perl -0pi -e 's/"name": "migration-check"/"name": "other"/; s/def run_step\(params\):\n/def run_step(params):\n    import time\n    time.sleep(60)\n/' "$root/broken/check.py"
if out=$("$bin/conformance" plugin-kit "$root/broken" --timeout 3 2>&1); then echo "the broken plugin passed" >&2; exit 1; fi
echo "$out" | sed 's/^/   /'
echo "$out" | grep -q 'FAIL initialize'
echo "$out" | grep -q 'FAIL run .*no answer within'

trap - EXIT; cleanup
if [ -z "${MASON_TESTS_ALREADY_RAN:-}" ]; then
  cargo test -q -p mason-plugins 2>&1 | grep 'test result' | head -3
  cargo test -q -p mason-definition --test plugins 2>&1 | grep 'test result'
fi
echo "s19 OK"
