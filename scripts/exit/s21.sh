#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 21 exit test: proposals and self-improvement. A real masond, fake agents (fake-claude)
# and a fake git host; no real service, no real agent CLI, no network.
#  1. Proposals: an agent files one through its tool and it becomes a task; duplicates and
#     filings without evidence are refused and file nothing; the caps hold; the policy and the
#     routing decide where a proposal goes; the owner approves, rejects and edits.
#  2. Improving MASON without write access: the change is made on its branch, pushed to the fork,
#     and the update channel switches to branch:<name> (built and self-tested, not taken over).
#  3. Publishing: the preview is redacted and attaches no client code; nothing is sent without
#     --confirm; a merged pull request returns the channel to main.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s21.XXXXXX); daemon=""
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
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1 to be $2" >&2; m task show "$1" >&2 || true; tail -20 "$root/masond.log" >&2; exit 1; }
    sleep 0.3
  done
}
refused() {  # WHAT CMD...: the command must fail; show why
  local what=$1; shift
  if out=$("$@" 2>&1); then echo "NOT REFUSED: $what: $out" >&2; exit 1; fi
  echo "   refused ($what): $(echo "$out" | sed 's/^error: //' | head -1)"
  last_refusal=$out
}
commit='git add -A && git -c user.name=t -c user.email=t@t commit -qm'
def='schema = 1
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
'
repo_of() {  # NAME: a bare "origin" and a clone with the pipeline's definition on main
  git init -q --bare -b main "$root/$1-origin.git"
  git clone -q "$root/$1-origin.git" "$root/$1" 2>/dev/null
  git -C "$root/$1" checkout -q -b main
  mkdir -p "$root/$1/.mason"
  printf '%s' "$def" > "$root/$1/.mason/mason.toml"
  printf 'mode = "direct"\n' > "$root/$1/.mason/landing.toml"
}

# --- MASON's own repository ("mason"): a fake host, a fork, and the pieces a build needs ---
repo_of mason
mkdir -p "$root/mason/crates/engine/src" "$root/mason/docs"
echo "// the engine" > "$root/mason/crates/engine/src/lib.rs"
echo "notes" > "$root/mason/docs/notes.md"
cat > "$root/mason/checks.sh" <<'SH'
echo "1..1"; echo "ok 1 - integration"
SH
git -C "$root/mason" add -A; git -C "$root/mason" -c user.name=t -c user.email=t@t commit -qm "mason"
git -C "$root/mason" push -q origin main
git -C "$root/mason" checkout -q -b owner-work
# --- a client project ("acmeshop") that must never leak ---
repo_of acmeshop
mkdir -p "$root/acmeshop/app"
echo "struct Pay { func chargeCard() {} }" > "$root/acmeshop/app/Pay.swift"
git -C "$root/acmeshop" add -A; git -C "$root/acmeshop" -c user.name=t -c user.email=t@t commit -qm "acme"
git -C "$root/acmeshop" push -q origin main
git -C "$root/acmeshop" checkout -q -b owner-work

# The scripted agents. acmeshop's agent notices things while it works: it files a proposal
# through its tool, the same one again (a duplicate), and one with no evidence.
prop='{"title": "Pay button charges twice", "body": "Tapping Pay twice quickly charges the card twice.", "evidence": "app/Pay.swift:1 chargeCard has no guard against a second call"}'
dup='{"title": "pay button charges twice", "body": "Tapping Pay twice quickly charges the card twice.", "evidence": "app/Pay.swift:1 chargeCard has no guard against a second call"}'
noev='{"title": "Something else is wrong", "body": "It is slow.", "evidence": "it just feels slow to me, honestly"}'
cat > "$root/acme.json" <<JSON
{"steps": {"develop": [[
  {"call": {"tool": "file_proposal", "args": $prop}},
  {"call": {"tool": "file_proposal", "args": $dup}},
  {"call": {"tool": "file_proposal", "args": $noev}},
  {"sh": "echo done > done.txt; $commit work"}, {"call": {"tool": "submit_change", "args": {"summary": "work", "head": "\$HEAD"}}}
]]}}
JSON
cat > "$root/mason.json" <<JSON
{"steps": {"develop": [[
  {"sh": "echo 'shorter log lines' > docs/shorter.md; $commit shorter"}, {"call": {"tool": "submit_change", "args": {"summary": "shorter", "head": "\$HEAD"}}}
]]}}
JSON
mkdir -p "$root/c/projects"
cat > "$root/c/projects/acmeshop.toml" <<TOML
path = "$root/acmeshop"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/acme.json"
TOML
cat > "$root/c/projects/mason.toml" <<TOML
path = "$root/mason"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/mason.json"

# MASON's own repository. No write access: improvements go to the owner's fork. The fake host
# keeps every branch, pull requests' heads included, in the one bare repository it is given, so
# that repository stands for the upstream and the fork both.
[self]
max_open = 20
per_week = 50
max_steps = 500
access = "fork"
fork = "$root/mason-origin.git"
switch = "check"

[host]
kind = "fake"
dir = "$root/host-state"
remote = "$root/mason-origin.git"
checks = "sh checks.sh"
TOML
# The update channel: a build is a copy of the binaries already built here.
bindir="$PWD/$bin"
cat > "$root/c/update.toml" <<TOML
repo = "$root/mason-origin.git"
channel = "stable"
build = "mkdir -p target/release; for b in masond step-host mason-mcp fake-claude mason; do cp '$bindir'/\$b target/release/\$b; done"
TOML
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
m pause --mode claims --for 2h >/dev/null

echo "== 1. an agent files a proposal through its tool"
m resume >/dev/null
m task add "work on the checkout" --project acmeshop --explored --body "x" >/dev/null
wait_state 0 landed ${LAND_WAIT:-120}
m proposals
[ "$(m proposals --json | grep -c '"task"')" = 1 ]   # one proposal: the other two were refused
grep -rl "Something else is wrong" "$root"/d --include=proposals-refused.txt >/dev/null
grep -rh "duplicate of #\|evidence" "$root"/d --include=proposals-refused.txt | sed 's/^/   refused for the agent: /'
m proposals --json | grep -q '"state": "proposed"'
echo "   the agent's proposal waits for the owner (the default policy), and its two bad ones filed nothing"
m pause --mode claims --for 2h >/dev/null

echo "== 1. duplicates and filings without evidence are refused, and file nothing"
m task add "second job" --project acmeshop --explored --proposed --body "x" >/dev/null
tasks_before=$(m task list | grep -c '^#')
refused "a duplicate" m proposal file --task 2 --title "Pay button charges twice" --body "Tapping Pay twice charges twice, again" --evidence "app/Pay.swift:1 chargeCard has no guard"
grep -q "duplicate of #" <<<"$last_refusal"
refused "empty evidence" m proposal file --task 2 --title "Cache never expires" --body "The cache keeps entries for ever." --evidence ""
refused "evidence of only spaces" m proposal file --task 2 --title "Cache never expires" --body "The cache keeps entries for ever." --evidence "     "
refused "evidence that names nothing" m proposal file --task 2 --title "Cache never expires" --body "The cache keeps entries for ever." --evidence "it seems to be quite wrong somewhere in there"
[ "$(m task list | grep -c '^#')" = "$tasks_before" ]
echo "   no task was created by any of the four refusals ($tasks_before tasks before and after)"

echo "== 1. caps: three a task, twenty a day for a project"
for t in "alpha widget leaks memory" "beta server drops packets"; do
  m proposal file --task 0 --title "$t" --body "$t, seen in testing." --evidence "app/Pay.swift:2 shows it" >/dev/null
done
# the agent's own proposal was task #0's first, and these two make three
refused "a fourth from one task" m proposal file --task 0 --title "gamma cache never expires" --body "The cache never expires." --evidence "app/Pay.swift:3 shows it"
grep -q "limit is 3" <<<"$last_refusal"

echo "== 1. policy and routing"
out=$(m proposal file --project acmeshop --title "Engine loses an event" --body "The engine drops an event when the queue is full." --evidence "crates/engine/src/lib.rs:1 drops it on a full queue")
echo "   $out"
grep -q "mason (pipeline)" <<<"$out"      # cites only MASON's files: routed to MASON, not the client project
out=$(m proposal file --task 2 --blocks --title "Parser rejects tabs" --body "The parser rejects tabs." --evidence "app/Pay.swift:4 rejects them")
echo "   $out"
grep -q "queued" <<<"$out"                # what blocks the filing agent's own task runs at once
m proposals | sed 's/^/     /'
m proposals --json | grep -q '"route": "pipeline"'
m proposals --json | grep -q '"route": "project"'

echo "== 1. the owner approves, rejects and edits"
agent_prop=1
refused "an edit with no evidence" m proposal edit "$agent_prop" --evidence "no file named here at all really"
m proposal edit "$agent_prop" --title "Pay button charges twice on a fast double tap" --evidence "app/Pay.swift:1 chargeCard has no guard; see also app/Pay.swift:9" >/dev/null
m proposal show "$agent_prop" | sed 's/^/     /' | head -4
m proposal approve "$agent_prop"
refused "an edit once it has started" m proposal edit "$agent_prop" --title "too late"
grep -q "started or is decided" <<<"$last_refusal"
rej=4
m proposal reject "$rej" --reason "not worth it"
refused "a rejected proposal filed again" m proposal file --task 2 --title "$(m proposal show "$rej" --json | sed -n 's/^ *"title": "\(.*\)",$/\1/p')" --body "again" --evidence "app/Pay.swift:2 again and again"
m pause --mode claims --for 2h >/dev/null
for t in $(m task list | sed -n 's/^#\([0-9]*\) *\(queued\|exploring\|developing\|proposed\).*/\1/p'); do m task cancel "$t" >/dev/null 2>&1 || true; done
m resume >/dev/null

echo "== 2. improving MASON without write access: on its branch, through a fake host"
secret_text="Shorten the log lines. Seen in acmeshop at $root/acmeshop/app/Pay.swift where chargeCard() wraps"
out=$(m improve "$secret_text")
echo "   $out"
n=$(echo "$out" | sed -n 's/^Filed #\([0-9]*\) in.*/\1/p')
branch="mason/improve/$n"
grep -q "branch $branch (fork access)" <<<"$out"
wait_state "$n" landed ${LAND_WAIT:-120}
git -C "$root/mason-origin.git" cat-file -e "$branch:docs/shorter.md"
! git -C "$root/mason-origin.git" cat-file -e "main:docs/shorter.md"
echo "   task #$n ran through develop and test, and its change is on $branch of the fork; main does not have it"
end=$((SECONDS + 180))
until grep -q "^channel = \"branch:$branch\"" "$root/c/update.toml"; do
  [ $SECONDS -lt $end ] || { echo "the channel never switched" >&2; m update status >&2; tail -20 "$root/masond.log" >&2; exit 1; }
  sleep 0.5
done
grep '^channel\|^repo' "$root/c/update.toml" | sed 's/^/     update.toml: /'
echo "   the daemon follows channel branch:$branch (built and self-tested; taking over is not run here)"
[ ! -d "$root/host-state/prs" ] || [ -z "$(ls "$root/host-state/prs" 2>/dev/null)" ]
echo "   nothing is upstream: the fake host has no pull request"

echo "== 3. publishing: the redacted version first, nothing without --confirm"
refused "--confirm before looking" m proposal publish "$n" --confirm
grep -q -- "--preview" <<<"$last_refusal"
refused "neither flag" m proposal publish "$n"
preview=$(m proposal publish "$n" --preview)
echo "$preview" | sed 's/^/     /'
for planted in acmeshop "$root/acmeshop" chargeCard Pay.swift; do
  if echo "$preview" | grep -qF "$planted"; then echo "LEAK in the preview: $planted" >&2; exit 1; fi
done
echo "$preview" | grep -q "attachments: none"
echo "$preview" | grep -q "changes docs/shorter.md"
! echo "$preview" | grep -q "app/"
[ ! -d "$root/host-state/prs" ] || [ -z "$(ls "$root/host-state/prs" 2>/dev/null)" ]
echo "   the preview named no client, path or code, listed only MASON's own file, and made no pull request"
out=$(m proposal publish "$n" --confirm)
echo "   $(echo "$out" | tail -1)"
pr=$(echo "$out" | sed -n 's/^Opened pull request #\([0-9]*\) upstream$/\1/p')
[ -n "$pr" ]
sent=$(cat "$root/host-state/prs/$pr.json")
for planted in acmeshop "$root/acmeshop" chargeCard Pay.swift; do
  if echo "$sent" | grep -qF "$planted"; then echo "LEAK in the pull request: $planted" >&2; exit 1; fi
done
echo "   pull request #$pr was opened with the previewed text; it carries no client name, path or code"
refused "publishing twice" m proposal publish "$n" --confirm

echo "== 3. upstream merges: the channel returns to main"
sed -i.bak 's/"state":"open"/"state":"merged"/' "$root/host-state/prs/$pr.json"
m proposal poll
grep -q '^channel = "main"' "$root/c/update.toml"
grep '^channel\|^repo' "$root/c/update.toml" | sed 's/^/     update.toml: /'
echo "   rejoined the main channel"

trap - EXIT; cleanup
echo "s21 OK"
