#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 57 exit test: deep and light tiers with levels, mapped to real models and efforts by each
# provider's config. A real masond with the scripted agents (fake-claude, fake-codex); no model, no
# network. The same pipeline (explore: light at low, develop: deep at high) runs under both
# providers; the recorded launch arguments carry each provider's own model and effort.
#  1. no files: claude and codex built-in mappings; `agent = "fast"` runs as light; a runtime
#     override of the model still wins.
#  2. a provider file maps deep.high to another model: the launch changes, the definition does not;
#     a project-file entry overrides one level of it.
#  3. a provider file with an effort the adapter does not have is reported (daemon log, mason
#     doctor) and the built-in mapping is used.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s57.XXXXXX); daemon=""
stop_daemon() {
  if [ -n "$daemon" ]; then kill -9 "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; fi
  daemon=""
  for f in "$root"/d/h/*/host.kv; do
    [ -e "$f" ] || continue
    for k in host_pid child_pid; do pid=$(sed -n "s/^$k=//p" "$f"); kill -9 "-$pid" 2>/dev/null || kill -9 "$pid" 2>/dev/null || true; done
  done
}
cleanup() { stop_daemon; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
cdir=""   # the config directory of the phase being run
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$cdir" "$@"; }
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1 to be $2" >&2; m task show "$1" >&2 || true; tail -20 "$root/masond.log" >&2; exit 1; }
    sleep 0.3
  done
}

# The pipeline: the same text under both providers. $1 is the tier name explore uses.
def() {
  cat <<TOML
schema = 1
start = "explore"

[step.explore]
type = "agent"
category = "exploring"
agent = "$1"
effort = "low"
next = "develop"

[step.develop]
type = "agent"
category = "developing"
agent = "deep"
effort = "high"
next = "test"

[step.test]
type = "test"
category = "testing"
max_rounds = 2
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
TOML
}

mkproj() {  # ID PROVIDER EXPLORE-TIER [EXTRA project-file text]
  local id=$1 provider=$2 tier=$3 extra=${4:-} repo="$root/r-$1" origin="$root/o-$1.git"
  rm -rf "$repo" "$origin"
  git init -q --bare -b main "$origin" 2>/dev/null
  git clone -q "$origin" "$repo" 2>/dev/null
  git -C "$repo" checkout -q -b main
  mkdir -p "$repo/.mason"
  def "$tier" > "$repo/.mason/mason.toml"
  printf 'mode = "direct"\n' > "$repo/.mason/landing.toml"
  echo base > "$repo/README"
  git -C "$repo" add -A
  git -C "$repo" -c user.name=t -c user.email=t@t commit -qm pipeline
  git -C "$repo" push -q origin main
  git -C "$repo" checkout -q -b owner-work
  rm -f "$root/script-$id.json"*   # the scripted agents keep a turn count and an argv log beside it
  cat > "$root/script-$id.json" <<JSON
{"steps": {
  "explore": [[{"call": {"tool": "submit_plan", "args": {"summary": "add hello.txt", "files": ["hello.txt"], "approach": "write the file", "acceptance_criteria": ["hello.txt exists"]}}}]],
  "develop": [[
    {"sh": "echo hello > hello.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm hello"},
    {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "\$HEAD"}}}
  ]]
}}
JSON
  local bins script
  if [ "$provider" = codex ]; then
    bins="codex_bin = \"$PWD/$bin/fake-codex\""; script=FAKE_CODEX_SCRIPT
  else
    bins="claude_bin = \"$PWD/$bin/fake-claude\""; script=FAKE_CLAUDE_SCRIPT
  fi
  cat > "$cdir/projects/$id.toml" <<TOML
path = "$repo"
base = "main"
remote = "origin"
test = "test -f hello.txt"
$([ "${extra#\[provider}" = "$extra" ] && echo "provider = \"$provider\"")
$bins
$extra
[agent_env]
$script = "$root/script-$id.json"
TOML
}

launch_daemon() {
  "$bin/masond" --data-dir "$root/d" --config-dir "$cdir" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
  daemon=$!
  until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
}
phase() {  # NAME: a fresh config dir and data dir
  stop_daemon
  rm -rf "$root/d"
  cdir="$root/c-$1"; mkdir -p "$cdir/projects"
  : > "$root/masond.log"
}
run_task() {  # ID: add a task to the project and wait for it to land
  m task add "add hello" --project "$1" --body x >/dev/null
  wait_state "${next_task:-0}" landed ${LAND_WAIT:-120}
  next_task=$(( ${next_task:-0} + 1 ))
}
argv_of() { cat "$root/script-$1.json.argv"; }
# explore is the first start, develop the second
explore_line() { argv_of "$1" | sed -n 1p; }
develop_line() { argv_of "$1" | sed -n 2p; }
show() { echo "     $1 explore: $(explore_line "$2" | grep -oE -- '(--model [^ ]+|--effort [^ ]+|-m [^ ]+|model_reasoning_effort="[a-z]+")' | tr '\n' ' ')"
         echo "     $1 develop: $(develop_line "$2" | grep -oE -- '(--model [^ ]+|--effort [^ ]+|-m [^ ]+|model_reasoning_effort="[a-z]+")' | tr '\n' ' ')"; }
has() { grep -qF -- "$2" <<<"$1" || { echo "expected [$2] in: $1" >&2; exit 1; }; }
lacks() { ! grep -qF -- "$2" <<<"$1" || { echo "did not expect [$2] in: $1" >&2; exit 1; }; }

echo "== 1. no files: the built-in mappings"
phase builtin
next_task=0
mkproj cl claude light
mkproj cx codex light
mkproj fa claude fast
mkproj ov claude light
launch_daemon
run_task cl
show claude cl
has "$(explore_line cl)" "--model haiku"; lacks "$(explore_line cl)" "--effort"
has "$(develop_line cl)" "--model sonnet --effort high"
echo "   claude: light at low is haiku with no effort flag; deep at high is sonnet at high"
run_task cx
show codex cx
lacks "$(explore_line cx)" " -m "; has "$(explore_line cx)" 'model_reasoning_effort="minimal"'
lacks "$(develop_line cx)" " -m "; has "$(develop_line cx)" 'model_reasoning_effort="high"'
echo "   codex: no -m flag (its CLI's default model); light at low is minimal, deep at high is high"
run_task fa
show "agent=fast" fa
has "$(explore_line fa)" "--model haiku"
echo "   a definition that still says agent = \"fast\" runs as light"
m config set step.develop.model opus --for 1h --owner brian --project ov >/dev/null
run_task ov
show "override" ov
has "$(develop_line ov)" "--model opus --effort high"
echo "   a runtime override of the model still wins; the mapped effort stays"

echo "== 2. a provider file changes the launch with no change to the definition"
phase file
next_task=0
mkdir -p "$cdir/providers"
cat > "$cdir/providers/claude.toml" <<'TOML'
# machine-wide: an owner who wants Opus for deep work
[deep]
high = { model = "opus", effort = "max" }
[light]
high = { model = "sonnet", effort = "low" }
TOML
mkproj cl claude light
mkproj pe claude light '[provider]
use = "claude"
[provider.claude.light]
low = { model = "sonnet", effort = "medium" }'
launch_daemon
run_task cl
show "provider file" cl
has "$(develop_line cl)" "--model opus --effort max"
has "$(explore_line cl)" "--model haiku"
diff <(git -C "$root/r-cl" show main:.mason/mason.toml) <(def light) >/dev/null
echo "   deep.high now launches opus at max; the project's definition is byte-for-byte the one used before"
run_task pe
show "project entry" pe
has "$(explore_line pe)" "--model sonnet --effort medium"
has "$(develop_line pe)" "--model opus --effort max"
echo "   the project file's entry replaced light.low only; deep.high still comes from the provider file"
m doctor | grep -E "provider claude|deep +high|light +low" | sed 's/^/     /' | head -8
m doctor | grep -E "deep +high .*provider file" >/dev/null
m doctor | grep -E "light +low .*project file" >/dev/null
m doctor | grep -E "light +medium .*built-in" >/dev/null
echo "   mason doctor shows each entry and where it came from"

echo "== 3. a provider file with an effort the adapter does not have"
phase bad
next_task=0
mkdir -p "$cdir/providers"
cat > "$cdir/providers/claude.toml" <<'TOML'
[deep]
high = { model = "opus", effort = "ultra" }
TOML
mkproj cl claude light
launch_daemon
run_task cl
show "bad file" cl
has "$(develop_line cl)" "--model sonnet --effort high"
grep -q "ultra" "$root/masond.log"
echo "   the daemon log says: $(grep -m1 ultra "$root/masond.log" | sed 's/.*WARN//' | cut -c1-200)"
m doctor | grep -q "ultra"
echo "   mason doctor reports it too; the built-in mapping (sonnet at high) was launched"

stop_daemon
trap - EXIT; cleanup
echo "s57 OK"
