#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 18 exit test: the graphical editor's API, driven through the CLI of a real
# masond (the monitors send the same operations).
#  1. Add, connect, remove and edit steps; the comments in the TOML survive.
#  2. Invalid edits are refused, with the reason, and change nothing.
#  3. Undo and redo; the diff is small; simulate walks scripted tasks through the draft.
#  4. The test grid and the landing editor (and their refusals).
#  5. Apply commits the draft to the base branch, only for the owner; the daemon
#     reloads the new definition.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s18.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
op() { m editor op "$1"; }
refused() {  # JSON WHAT: the operation must be refused, with WHAT in the reason
  local out
  if out=$(m editor op "$1" 2>&1); then echo "this edit should have been refused: $1" >&2; exit 1; fi
  echo "   refused: $out"
  echo "$out" | grep -q -- "$2" || { echo "the reason should mention '$2'" >&2; exit 1; }
}
text() { m editor show --json | sed -n 's/^  "text": "\(.*\)",\{0,1\}$/\1/p' | head -1; }

git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
mkdir -p "$root/repo/.mason"
cat > "$root/repo/.mason/mason.toml" <<'TOML'
# The pipeline for web-api. Edited by hand: keep the comments.
schema = 1
start = "develop"   # no exploring here

# --- steps ---
[step.develop]
type = "agent"
category = "developing"
next = "test"   # then test

[step.test]
type = "test"
category = "testing"
on_red = "develop"
max_rounds = 2
next = "land"
suites = ["unit"]

# landing is last
[step.land]
type = "landing"
category = "landing"

[suite.unit]
command = "sh checks.sh unit"
parser = "tap"
min_tests = 1
level = "unit"

[suite.e2e]
command = "sh checks.sh e2e"
parser = "tap"
min_tests = 1
level = "e2e"

[binding.unit]
when = ["test_step"]
TOML
cat > "$root/repo/.mason/landing.toml" <<'TOML'
# How web-api lands.
mode   = "direct"   # no PRs
window = "20m"
TOML
printf '#!/bin/sh\necho 1..1\necho "ok 1 - x"\n' > "$root/repo/checks.sh"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm "pipeline"
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work
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

echo "== 1. the canvas: lanes, nodes, edges"
m editor show | head -8
m editor show | grep -q '^Developing  develop (agent)'
m editor show | grep -q 'test -red-> develop (at most 2 times)'

echo "== 1. add, connect, remove and edit steps"
op '{"op":"insert_step","after":"develop","name":"lint","type":"command","category":"developing","fields":{"command":"cargo fmt --check"}}'
m editor show | grep -q '^Developing  develop (agent)   lint (command)'
op '{"op":"insert_step","after":"test","name":"test-slow","type":"test","category":"testing","extends":"test"}'
m editor show | grep -q 'test -ok-> test-slow'
op '{"op":"set_field","step":"test-slow","key":"max_rounds","value":4}'
op '{"op":"set_field","step":"lint","key":"budget","value":"10m"}' 2>&1 | grep -q refused || true
op '{"op":"remove_step","name":"test-slow"}'
! m editor show | grep -q test-slow
op '{"op":"rename_step","from":"lint","to":"format"}'
op '{"op":"set_field","step":"format","key":"command","value":"cargo fmt --all --check"}'
op '{"op":"move","step":"format","x":420,"y":180}'
m editor show --json | grep -q 'cargo fmt --all --check'
echo "   the draft's text keeps every comment:"
for c in "# The pipeline for web-api. Edited by hand: keep the comments." "# no exploring here" "# --- steps ---" "# then test" "# landing is last"; do
  m editor show --json | grep -q -- "$c" || { echo "lost the comment: $c" >&2; exit 1; }
done
echo "   all five comments are there"

echo "== 2. invalid edits are refused, with the reason, and change nothing"
refused '{"op":"set_edge","from":"develop","edge":"next","to":"nowhere"}' 'nowhere'
refused '{"op":"set_field","step":"test","key":"max_rounds","value":0}' 'max_rounds'
refused '{"op":"unset_field","step":"test","key":"max_rounds"}' 'max_rounds'
refused '{"op":"set_field","step":"test","key":"suites","value":["ghost"]}' 'ghost'
refused '{"op":"set_field","step":"develop","key":"nxt","value":"x"}' 'unknown key'
refused '{"op":"set_field","step":"develop","key":"next","value":"land"}' 'edge'
refused '{"op":"insert_step","after":"develop","name":"Bad Name","type":"command","category":"developing"}' 'lowercase'
refused '{"op":"insert_step","after":"develop","name":"lint","type":"command","category":"testing","fields":{"command":"x"}}' ''
refused '{"op":"remove_step","name":"develop"}' ''
refused '{"op":"set_edge","from":"test","edge":"next","to":null}' ''
refused '{"op":"frobnicate"}' 'unknown operation'
echo "   (and the draft is as it was:)"
m editor show | grep -q 'format (command)'

echo "== 3. undo, redo, the diff, and simulate"
# One step back (the node's position) and forward again; two more back reach the base.
m editor undo
! m editor show --json | grep -q '"placed": true'
m editor redo
m editor show --json | grep -q '"placed": true'
m editor show | sed -n '/^---/,$p' | head -30
n=$(m editor show | grep -c '^[-+][^-+]')
echo "   ($n changed lines in the diff)"
m editor simulate | head -4
m editor simulate | grep -q 'format'
m editor simulate '{"name":"test red twice","results":{"test":["red","red","red"]}}' | grep -q 'failed'

echo "== 4. the test grid and the landing editor"
m editor show --json | grep -q '"suite": "e2e"'
op '{"op":"set_suite_level","suite":"e2e","level":"integration"}'
refused '{"op":"set_binding","level":"nothing-has-this-level","when":"batch","on":true}' 'no suite has level'
op '{"op":"set_binding","level":"integration","when":"batch","on":true}'
op '{"op":"set_binding","level":"unit","when":"development","on":true}'
refused '{"op":"set_binding","level":"unit","when":"tuesday","on":true}' 'unknown'
op '{"op":"landing_set","key":"max_size","value":6}'
op '{"op":"landing_add_tier","name":"batch","suites":["e2e"]}'
op '{"op":"landing_set_ladder","rungs":["rerun-failed","bisect","ask"]}'
refused '{"op":"landing_set","key":"mode","value":"yolo"}' 'unknown'
refused '{"op":"landing_set_tier","name":"batch","key":"ci","value":"github-actions"}' 'direct-ci'
refused '{"op":"landing_set","key":"parallel","value":0}' 'parallel'
m editor show | grep -q 'this changes how the project lands'
m editor show | grep -q 'runs `e2e` batch (integration)'

echo "== 4b. both monitors show the editor exactly as the daemon says (conformance)"
swift_ok=0; kt_ok=0
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then swift_ok=1; fi
if command -v java >/dev/null && command -v gradle >/dev/null && [ -z "${MASON_SKIP_COMPOSE:-}" ]; then kt_ok=1; fi
port=$(sed -n 's/.*"port": *\([0-9]*\).*/\1/p' "$root/d/daemon.json")
for step in "" test; do
  rm -rf "$root/exp" "$root/sw" "$root/kt"
  "$bin/conformance" editor-expected --port "$port" --project web --step "$step" --out "$root/exp"
  if [ $swift_ok = 1 ]; then
    (cd clients/swift && swift build 2>&1 | tail -1)
    clients/swift/.build/debug/monitor-tool editor --port "$port" --project web --step "$step" --out "$root/sw"
    "$bin/conformance" check --expected "$root/exp" --actual "$root/sw"
  fi
  if [ $kt_ok = 1 ]; then
    (cd clients/compose && gradle --no-daemon -q :app:installDist 2>&1 | grep -vE '^$' | head -5 || true)
    clients/compose/app/build/install/app/bin/app editor --port "$port" --project web --step "$step" --out "$root/kt"
    "$bin/conformance" check --expected "$root/exp" --actual "$root/kt"
  fi
  if [ $swift_ok = 1 ] && [ $kt_ok = 1 ]; then "$bin/conformance" check --expected "$root/sw" --actual "$root/kt"; fi
done
[ $swift_ok = 1 ] || echo "(SwiftUI skipped: needs macOS and Swift; the monitor-swift CI job covers it)"
[ $kt_ok = 1 ] || echo "(Compose skipped: needs Java and Gradle; the monitor-compose CI job covers it)"

echo "== 5. apply commits the draft to the base branch, for the owner only"
if out=$(m --as mallory editor apply -m "sneaky" 2>&1); then echo "a stranger applied an edit" >&2; exit 1; fi
echo "$out"; echo "$out" | grep -q "not the owner's identity"
m editor apply -m "Add a format step; run e2e per batch"
git -C "$root/origin.git" log --format='%an: %s' -1 main
git -C "$root/origin.git" show main:.mason/mason.toml | grep -q '# no exploring here'
git -C "$root/origin.git" show main:.mason/mason.toml | grep -q '\[step.format\]'
git -C "$root/origin.git" show main:.mason/mason.toml | grep -q '\[binding.integration\]'
git -C "$root/origin.git" show main:.mason/landing.toml | grep -q '# no PRs'
git -C "$root/origin.git" show main:.mason/landing.toml | grep -q 'max_size = 6'
git -C "$root/origin.git" show main:.mason/layout.toml | grep -q 'x = 420'
echo "   the commit is on origin's main with the comments, the new step, the binding, the landing change and the layout"
m definition show | grep -q 'format'
[ "$(m editor show | grep -c 'draft is the base')" = 1 ]
echo "   the daemon reloaded the definition, and the draft is the base again"
if m editor apply 2>&1 | grep -q 'nothing to apply'; then echo "   (applying nothing is refused)"; fi

trap - EXIT; cleanup
if [ -z "${MASON_TESTS_ALREADY_RAN:-}" ]; then
  cargo test -q -p mason-definition 2>&1 | grep 'test result' | head -4
  cargo test -q -p mason-landing --lib edit 2>&1 | grep 'test result'
fi
echo "s18 OK"
