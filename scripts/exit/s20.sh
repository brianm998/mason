#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 20 exit test: the second adapters. Recorded responses and fakes only; no real
# service, no real Codex, no real Jenkins, no network.
#  1. Each adapter passes its conformance kit: Codex (plain and under a step host), Jenkins and
#     the generic build adapter (command and URL forms) on recorded HTTP, plain git as a host
#     with no features, and the sandbox wrappers (live only if the binary exists).
#  2. A real masond, a project whose agent is Codex (fake-codex replaying fixtures), and a custom
#     step built entirely with `mason editor op` run in one graph, with the retry ladder moving
#     up a model after a crash.
#  3. The task lands on a bare repository reached over ssh:// (through a local shim for ssh),
#     in direct mode, with no [host] section: no pull requests, labels, checks or merge queue.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s20.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for f in "$root"/d/h/*/host.kv; do
    [ -e "$f" ] || continue
    for k in host_pid child_pid; do pid=$(sed -n "s/^$k=//p" "$f"); kill -9 "-$pid" 2>/dev/null || kill -9 "$pid" 2>/dev/null || true; done
  done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1 to be $2" >&2; m task show "$1" >&2 || true; tail -20 "$root/masond.log" >&2; exit 1; }
    sleep 0.3
  done
}

echo "== 1. every adapter passes its conformance kit"
t() {  # LABEL TEST-ARGS...: run cargo tests and show the result line
  local label=$1; shift
  out=$(cargo test -q -p mason-adapters "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $label" >&2; exit 1; }
  echo "   $label: $(echo "$out" | grep 'test result' | sed 's/; finished.*//' | head -1)"
}
t "Codex: run_all (plain, and under a step host), reattach, parser on four fixtures" --test codex
t "Codex: parser and command unit tests" --lib codex
t "Jenkins: build_kit on recorded responses" --test jenkins
t "generic build adapter, command and URL halves" --test generic
t "plain git: plain_host_kit" --test plain
t "the conformance kit's own tests (fake agent, Claude, hosted)" --test conformance
out=$(cargo test -q -p mason-adapters --test sandbox -- --nocapture 2>&1) || { echo "$out" >&2; exit 1; }
echo "   sandbox wrappers: $(echo "$out" | grep 'test result' | head -1 | sed 's/; finished.*//')"
echo "$out" | grep -E 'live run|not installed' | sed 's/^/   /'
cargo test -q -p mason-adapter-api 2>&1 | grep 'test result' | head -1 | sed 's/^/   adapter-api: /'
cargo test -q -p mason-definition --lib ladder 2>&1 | grep 'test result' | head -1 | sed 's/^/   retry ladder rung: /'
cargo test -q -p mason-adapters --lib ladder 2>&1 | grep 'test result' | head -1 | sed 's/^/   retry ladder triggers: /'

echo "== 2. a bare repository over ssh://, and a project whose agent is Codex"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
mkdir -p "$root/repo/.mason"
cat > "$root/repo/.mason/mason.toml" <<'TOML'
# Codex-driven pipeline; the extra steps below are added through the editor's API.
schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
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
printf 'mode = "direct"\n' > "$root/repo/.mason/landing.toml"
echo base > "$root/repo/base.txt"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm "pipeline"
git -C "$root/repo" push -q origin main
# From here the only way to the remote is ssh://, through a shim that runs the command locally.
git -C "$root/repo" remote set-url origin "ssh://git@localhost$root/origin.git"
git -C "$root/repo" checkout -q -b owner-work
cat > "$root/ssh-shim" <<'SH'
#!/bin/sh
# Stands in for ssh: skips the options, drops the host, runs the remote command here.
echo "ssh $*" >> "$SSH_SHIM_LOG"
while [ $# -gt 0 ]; do
  case "$1" in
    -o|-p|-l|-i|-F|-J|-b|-c|-E|-L|-R|-D|-S) shift 2 ;;
    -*) shift ;;
    *) break ;;
  esac
done
shift   # the host
exec sh -c "$(printf '%s' "$*" | sed -e 's/^git-upload-pack/git upload-pack/' -e 's/^git-receive-pack/git receive-pack/')"
SH
chmod +x "$root/ssh-shim"
# (An environment may restrict git to file:// transports; this test needs ssh.)
export SSH_SHIM_LOG="$root/ssh.log" GIT_SSH_COMMAND="$root/ssh-shim" GIT_ALLOW_PROTOCOL=file:ssh

# The scripted Codex. Per invocation, in order, in the develop worktree:
#   1 dies at once (a crash)  -> the ladder retries on the next model, a fresh session
#   2 replays the plain fixture and submits nothing -> nudged by `codex exec resume <thread> -`
#   3 changes a file and submits
#   4 and 5: the two steps added through the editor (the command step runs no agent; "polish" does)
commit='git add -A && git -c user.name=t -c user.email=t@t commit -qm'
cat > "$root/script.json" <<JSON
{"steps": {"develop": [
  [{"exit": 3}],
  [{"replay": "$PWD/crates/adapters/fixtures/codex/plain.jsonl"}],
  [{"sh": "echo feature > feature.txt; $commit feature"}, {"call": {"tool": "submit_change", "args": {"summary": "feature", "head": "\$HEAD"}}}],
  [{"sh": "echo polished > polish.txt; $commit polish"}, {"call": {"tool": "submit_change", "args": {"summary": "polish", "head": "\$HEAD"}}}]
]}}
JSON
mkdir -p "$root/c/projects"
cat > "$root/c/projects/web.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
provider = "codex"
codex_bin = "$PWD/$bin/fake-codex"
ladder = ["gpt-small", "gpt-big"]
[agent_env]
FAKE_CODEX_SCRIPT = "$root/script.json"
TOML
! grep -q '^\[host\]' "$root/c/projects/web.toml"
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== 2. a custom step made entirely through the editor's API"
m editor op '{"op":"insert_step","after":"develop","name":"stamp","type":"command","category":"developing","fields":{"command":"echo stamped > stamp.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm stamp"}}'
m editor op '{"op":"insert_step","after":"stamp","name":"polish","type":"agent","category":"developing"}'
m editor show | grep -q 'stamp (command)'
m editor show | grep -q 'polish (agent)'
m editor apply -m "Add stamp and polish steps"
git -C "$root/origin.git" show main:.mason/mason.toml | grep -q '\[step.stamp\]'
git -C "$root/origin.git" show main:.mason/mason.toml | grep -q '# Codex-driven pipeline'
m definition show | grep -q 'stamp'
echo "   applied over ssh:// (the editor pushed the definition to the bare repository)"

echo "== 2/3. a task through develop (Codex), stamp (the editor's step), polish (Codex), test, and landing"
m task add "add a feature" --project web --explored --body "x" >/dev/null
wait_state 0 landed ${LAND_WAIT:-120}
for f in feature.txt stamp.txt polish.txt; do git -C "$root/origin.git" cat-file -e "main:$f"; done
echo "   task #0 landed: origin's main now has feature.txt (Codex), stamp.txt (the custom step), polish.txt (Codex)"
git -C "$root/origin.git" log --oneline -6 main | sed 's/^/     /'

echo "== 2. what the fake Codex was asked to do"
argv="$root/script.json.argv"
sed -e 's/^exec --json -C [^ ]* //' -e 's/-c [^ ]*//g' -e 's/  */ /g' "$argv" | awk '{print "     invocation " NR ": " $0}' 
grep -q -- '-m gpt-small' "$argv"
grep -q -- '-m gpt-big' "$argv"
[ "$(grep -n -m1 -- '-m gpt-small' "$argv" | cut -d: -f1)" -lt "$(grep -n -m1 -- '-m gpt-big' "$argv" | cut -d: -f1)" ]
echo "   the first invocation was on gpt-small; after the crash the step moved up to gpt-big"
grep -q 'resume 0199-codex-thread-1 -' "$argv"
echo "   the nudge resumed the fixture's thread with 'codex exec resume <thread> -'"
grep -q -- '-s workspace-write' "$argv"
grep -q 'approval_policy="never"' "$argv"
grep -q 'mcp_servers.mason.command=' "$argv"
echo "   developing steps run workspace-write, never ask for approval, and reach MASON through an MCP server entry"

echo "== 3. the bare repository was reached over ssh, and the host has no features"
[ -s "$root/ssh.log" ]
grep -q 'git-receive-pack' "$root/ssh.log"   # the landing push went over ssh
echo "   the ssh shim was invoked $(wc -l < "$root/ssh.log" | tr -d ' ') times, e.g.:"
sed 's#'"$root"'#$ROOT#g' "$root/ssh.log" | sort -u | head -3 | sed 's/^/     /'
! grep -q '^\[host\]' "$root/c/projects/web.toml"
echo "   the project has no [host] section: landing was direct (a branch push), no PR, no checks, no merge queue"

echo "== 3. the sandbox (optional)"
if command -v sandbox-exec >/dev/null || command -v bwrap >/dev/null; then
  echo "   a sandbox binary is installed here; its live run is in section 1"
else
  echo "   no sandbox-exec or bwrap here: only the argv and profile tests ran"
fi

trap - EXIT; cleanup
echo "s20 OK"
