#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Numbers handed out by MASON (s43) exit test: a real masond with the scripted agent (fake-claude),
# a bare origin and a project that already holds Stage 20 and 42 (s20, s42, decisions up to 0020).
#  1. Three tasks claimed at the same moment get different numbers, and each prompt (explore and
#     develop) states only its own.
#  2. A task that adds a file under a number that is not its own (the base's s20, or someone
#     else's) is sent back before folding, with the file and the number it should use; it keeps
#     the same numbers through the retry and a daemon restart.
#  3. Tasks that each add a stage note, a decision record and an exit script land in either
#     order (the lower number last, in one round) with no conflict, and the README status block,
#     written by MASON when they land, lists them and passes scripts/check-status.sh.
#  4. A task titled "Stage 21" keeps s21 and reserves only a decision number.
#  5. A cancelled task's numbers are shown released and are not reused.
#  6. Filing says so when the range of ad-hoc exit numbers is used up.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s43.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  # Verified before it is killed: a stale pid in a registry may belong to someone else now.
  for hd in "$root"/d/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1; done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { echo "FAILED: $*" >&2; m task show ${DEBUG_TASK:-0} >&2 2>&1 || true; m task list >&2 2>&1 || true; tail -20 "$root/masond.log" >&2 || true; exit 1; }
state_of() { m task show "$1" --json 2>/dev/null | sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(state_of "$1")" = "$2" ]; do
    [ $SECONDS -lt $end ] || { m task show "$1" >&2 || true; fail "timed out waiting for task $1 to be $2"; }
    sleep 0.2
  done
}
add() { m task add "$1" --project web --body "${2:-do it}" | sed -n 's/^Filed task #\([0-9]*\) .*/\1/p'; }
num() {  # TASK KIND: the number reserved for the task
  m task show "$1" | sed -n "s/^  number    $2 \([0-9]*\):.*/\1/p"
}
start_daemon() {
  "$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
  daemon=$!
  until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
}
stop_daemon() { kill "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; daemon=""; }
ci="git -c user.name=t -c user.email=t@t"

echo "== setup: a bare origin that holds Stage 20 and s42, and a scripted agent that reads its numbers from its prompt"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
mkdir -p "$root/repo/.mason" "$root/repo/docs/stages" "$root/repo/docs/decisions" "$root/repo/scripts/exit"
cp scripts/gen-status.sh scripts/check-status.sh "$root/repo/scripts/"
printf '# Stage 20: second adapters\n\nSummary: the second adapters.\n' > "$root/repo/docs/stages/s20.md"
printf '# s42: dependencies\n\nSummary: task dependencies.\n' > "$root/repo/docs/stages/s42.md"
printf '# 0020\n' > "$root/repo/docs/decisions/0020-dependencies.md"
printf '#!/usr/bin/env bash\n' > "$root/repo/scripts/exit/s20.sh"
printf '# Repo\n\n<!-- status:begin -->\n<!-- status:end -->\n\nThe rest of the readme.\n' > "$root/repo/README.md"
cat > "$root/repo/.mason/mason.toml" <<'TOML'
schema = 1
start = "explore"

[step.explore]
type = "agent"
category = "exploring"
next = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
max_rounds = 1
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
max_rounds = 3
on_red = "develop"
TOML
printf 'mode = "direct"\n' > "$root/repo/.mason/landing.toml"
(cd "$root/repo" && git add -A && $ci commit -qm "base: stage 20 and s42" && git push -q origin main)
mkdir -p "$root/prompts" "$root/c/projects"
# The develop turns: the "agent" writes its stage note, decision record and exit script under the
# numbers in its prompt. A task whose text says WRONG takes s41 and the base's decision 0020 on the
# first try, and fixes it from the feedback on the second turn. SLOW waits a little first.
cat > "$root/agent.sh" <<'SH'
set -e
t=$(basename "$PWD")
printf '%s\n' "$FAKE_PROMPT" > "$PROMPTS/$t.$1.txt"
[ "$1" = explore ] && exit 0
N=$(printf '%s' "$FAKE_PROMPT" | sed -n 's/.*your stage note is docs\/stages\/s\([0-9]*\)\.md.*/\1/p' | head -1)
D=$(printf '%s' "$FAKE_PROMPT" | sed -n 's/.*your decision record is docs\/decisions\/\([0-9]*\)-<slug>\.md.*/\1/p' | head -1)
case "$FAKE_PROMPT" in *SLOW*) sleep 3;; esac
mkdir -p docs/stages docs/decisions scripts/exit
if [ "$1" = develop1 ]; then
  case "$FAKE_PROMPT" in *WRONG*) N=41; D=0020;; esac
  printf '# Notes of %s\n\nSummary: %s did its part.\n' "$t" "$t" > "docs/stages/s$N.md"
  printf '# Decision of %s\n' "$t" > "docs/decisions/$D-$t.md"
  printf '#!/usr/bin/env bash\n# %s\n' "$t" > "scripts/exit/s$N.sh"
else
  git mv docs/stages/s41.md "docs/stages/s$N.md" 2>/dev/null || true
  git mv docs/decisions/0020-$t.md "docs/decisions/$D-$t.md" 2>/dev/null || true
  git mv scripts/exit/s41.sh "scripts/exit/s$N.sh" 2>/dev/null || true
fi
git add -A && git -c user.name=t -c user.email=t@t commit -qm "work of $t"
SH
cat > "$root/script.json" <<JSON
{"steps": {
  "explore": [[
    {"sh": "PROMPTS=$root/prompts sh $root/agent.sh explore"},
    {"call": {"tool": "submit_plan", "args": {"summary": "write notes", "files": ["docs"], "approach": "write", "acceptance_criteria": ["notes exist"]}}}
  ]],
  "develop": [
    [{"sh": "PROMPTS=$root/prompts sh $root/agent.sh develop1"}, {"call": {"tool": "submit_change", "args": {"summary": "done", "head": "\$HEAD"}}}],
    [{"sh": "PROMPTS=$root/prompts sh $root/agent.sh develop2"}, {"call": {"tool": "submit_change", "args": {"summary": "renamed", "head": "\$HEAD"}}}]
  ]
}}
JSON
cat > "$root/c/projects/web.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
start_daemon

echo "== 1. three tasks claimed at the same moment get three different numbers, and only their own are in their prompts"
m pause --mode claims --for 1h >/dev/null
a=$(add "alpha" "SLOW first"); b=$(add "beta" "second"); c=$(add "gamma" "WRONG number third")
m resume --mode claims >/dev/null
for t in $a $b $c; do
  end=$((SECONDS + 30)); until [ -n "$(num $t exit)" ] && [ -n "$(num $t decision)" ]; do [ $SECONDS -lt $end ] || fail "task #$t has no numbers"; sleep 0.2; done
done
for k in exit decision; do
  all="$(num $a $k) $(num $b $k) $(num $c $k)"
  [ "$(echo $all | tr ' ' '\n' | sort -u | wc -l | tr -d ' ')" = 3 ] || fail "two tasks share a $k number: $all"
done
echo "   alpha #$a: s$(num $a exit), decision $(num $a decision); beta #$b: s$(num $b exit), $(num $b decision); gamma #$c: s$(num $c exit), $(num $c decision)"
[ "$(num $a exit)" -ge 43 ] && [ "$(num $a decision)" -ge 21 ] || fail "numbers do not start above the base's s42 and 0020"
m task show $a --json | tr -d ' \n' | grep -q '"numbers":\[{"kind":"decision"' || m task show $a --json | grep -q '"numbers"' || fail "--json does not list the numbers"
end=$((SECONDS + 60)); until [ -f "$root/prompts/t$a.explore.txt" ] || ls "$root"/prompts/*explore* >/dev/null 2>&1; do [ $SECONDS -lt $end ] || fail "no prompt was recorded"; sleep 0.2; done
sleep 2
ls "$root/prompts" | sed 's/^/   prompt file: /'
mine() { ls "$root"/prompts/*."$1".txt; }
prompt_of() {  # TASK KIND: the prompt file whose own task text is "do it"/SLOW etc: found by the task's number
  for f in $(mine "$2"); do
    if grep -q "docs/stages/s$(num "$1" exit).md" "$f"; then echo "$f"; return; fi
  done
}
for t in $a $b $c; do
  for kind in explore develop1; do
    f=$(prompt_of $t $kind); [ -n "$f" ] || fail "no $kind prompt carries task #$t's stage note"
    grep -q "docs/decisions/$(printf %04d $(num $t decision))-<slug>.md" "$f" || fail "the $kind prompt of #$t lacks its decision record"
    grep -q "scripts/exit/s$(num $t exit).sh" "$f" || fail "the $kind prompt of #$t lacks its exit script"
    grep -q "do not take another number" "$f" || fail "the $kind prompt of #$t does not forbid another number"
    for o in $a $b $c; do
      [ "$o" = "$t" ] && continue
      ! grep -q "s$(num $o exit).md" "$f" || fail "the $kind prompt of #$t shows the number of #$o"
    done
  done
done
echo "   every explore and develop prompt states its own stage note, decision record and exit script, and no one else's"

echo "== 2. a task that adds files under a number that is not its own is sent back, naming the number to use"
wait_state $c landed 180
r=$(m task show $c | grep -c "not its own" || true)
echo "   gamma #$c took s41 and 0020 first, then landed after feedback"
shown=$(m task show $c)
echo "$shown" | grep -q 'land         attempt 1  sentback' || fail "gamma was not sent back from landing"
reason=$(grep -rhao "uses numbers that are not its own[^\"]*" "$root/d" 2>/dev/null | sed -n 1p || true)
[ -n "$reason" ] || fail "the send-back reason was not recorded"
echo "   $reason"
case "$reason" in *docs/decisions/0023-*) ;; *) fail "the reason does not name the decision file to use";; esac
case "$reason" in *docs/stages/s45.md*) ;; *) fail "the reason does not name the stage note to use";; esac
n=$(num $c exit); d=$(num $c decision)
git -C "$root/origin.git" cat-file -e "main:docs/stages/s$n.md" || fail "gamma's stage note is not at s$n on origin"
git -C "$root/origin.git" show main:docs/stages/s20.md | grep -q 'second adapters' || fail "s20 on origin is no longer Stage 20's"
echo "   it landed as s$n and decision $d, and Stage 20's note is untouched"
stop_daemon; start_daemon
[ "$(num $c exit)" = "$n" ] && [ "$(num $c decision)" = "$d" ] || fail "gamma's numbers changed over a daemon restart"
echo "   after a daemon restart it still holds s$n and $d"

echo "== 3. tasks that all add a stage note and a decision record land in any order, with no conflict"
wait_state $a landed 180; wait_state $b landed 180
d1=$(add "delta" "SLOW fourth"); d2=$(add "epsilon" "fifth")
wait_state $d1 landed 180; wait_state $d2 landed 180
git clone -q "$root/origin.git" "$root/check" 2>/dev/null
(cd "$root/check" && git log --first-parent --format=%s | head -12 | sed 's/^/   /')
for t in $a $b $c $d1 $d2; do
  git -C "$root/check" cat-file -e "HEAD:docs/stages/s$(num $t exit).md" || fail "task #$t's stage note is missing on origin"
  git -C "$root/check" cat-file -e "HEAD:scripts/exit/s$(num $t exit).sh" || fail "task #$t's exit script is missing on origin"
  grep -q "^- \*\*s$(num $t exit)\*\*: $t did its part" "$root/check/README.md" || fail "the README status lacks s$(num $t exit)"
done
(cd "$root/check" && bash scripts/check-status.sh) || fail "the status block on origin is out of date"
[ "$(grep -c 'status:begin' "$root/check/README.md")" = 1 ] && grep -q 'The rest of the readme' "$root/check/README.md" || fail "the README was damaged"
subjects=$(git -C "$root/check" log --format=%s)
case "$subjects" in *"Regenerate the README status block"*) ;; *) fail "MASON never committed the status block";; esac
echo "   alpha, beta, gamma, delta and epsilon all landed; the README status block lists them and check-status.sh passes"

echo "== 4. a task titled Stage 21 keeps s21 and reserves only a decision number"
s21=$(add "Stage 21: a planned stage" "plan")
end=$((SECONDS + 30)); until [ -n "$(num $s21 decision)" ]; do [ $SECONDS -lt $end ] || fail "no decision number for the stage task"; sleep 0.2; done
[ "$(num $s21 exit)" = 21 ] || fail "the Stage 21 task does not keep stage 21 ($(num $s21 exit))"
m task show $s21 | grep -q '(its stage)' || fail "task show does not say the number is its stage"
wait_state $s21 landed 180
git -C "$root/origin.git" cat-file -e main:docs/stages/s21.md || fail "s21 did not land"
echo "   it keeps s21 (with decision $(num $s21 decision)) and landed docs/stages/s21.md"

echo "== 5. a cancelled task's numbers are shown released and not reused"
m pause --mode claims --for 1h >/dev/null
x=$(add "to be cancelled" "x")
m resume --mode claims >/dev/null
end=$((SECONDS + 30)); until [ -n "$(num $x exit)" ]; do [ $SECONDS -lt $end ] || fail "no numbers for the task to cancel"; sleep 0.2; done
xn=$(num $x exit); xd=$(num $x decision)
m task cancel $x >/dev/null
end=$((SECONDS + 30)); until m task show $x | grep -q 'released, not reused'; do [ $SECONDS -lt $end ] || fail "the numbers are not shown released"; sleep 0.2; done
y=$(add "after the cancel" "y")
end=$((SECONDS + 30)); until [ -n "$(num $y exit)" ]; do [ $SECONDS -lt $end ] || fail "no numbers for the next task"; sleep 0.2; done
[ "$(num $y exit)" -gt "$xn" ] && [ "$(num $y decision)" -gt "$xd" ] || fail "a released number was reused"
echo "   #$x held s$xn and $xd, released on cancel; the next task #$y got s$(num $y exit) and $(num $y decision)"
m task cancel $y >/dev/null

echo "== 6. filing says so when the range of ad-hoc numbers is used up"
stop_daemon
MASON_ADHOC_EXIT_START=99 start_daemon
z=$(add "takes the last number" "z")
end=$((SECONDS + 30)); until [ -n "$(num $z exit)" ]; do [ $SECONDS -lt $end ] || fail "no number for the task"; sleep 0.2; done
[ "$(num $z exit)" = 99 ] || fail "expected s99, got $(num $z exit)"
if m task add "one too many" --project web --body "w" >"$root/refused" 2>&1; then fail "filing was accepted with the range used up"; fi
grep -q "used up" "$root/refused" || fail "the refusal does not say the range is used up: $(cat "$root/refused")"
echo "   refused: $(tail -1 "$root/refused")"
m task add "Stage 22: still fine" --project web --body "w" >/dev/null || fail "a stage-numbered task was refused"
echo "   a task that names its stage is still accepted"
echo "s43 OK"
