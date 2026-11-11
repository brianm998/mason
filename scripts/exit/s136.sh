#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# `[tracker] require_label` (s136) exit test. A real masond, a fake GitHub per project, each
# holding three open issues by the owner: one labelled `mason`, one labelled `pipeline`, one
# with no label (a tracker shared with another pipeline).
#  1. Project "shared" (require_label = true): exactly one task, from the `mason` issue.
#  2. Project "plain" (the key absent): all three are imported, as before.
# Only task counts are asserted; the doctor's wording and the rule's edge cases are unit tests.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s136.XXXXXX); daemon=""; gh=""; gh2=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  [ -n "$gh" ] && kill -9 "$gh" 2>/dev/null || true
  [ -n "$gh2" ] && kill -9 "$gh2" 2>/dev/null || true
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
fail() { echo "FAILED: $*" >&2; set +e; m task list >&2 2>&1; m alerts >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
issue() {  # NUMBER TITLE LABELS-JSON
  printf ' {"number": %s, "node_id": "I_%s", "title": "%s", "body": "text", "state": "open", "user": {"login": "%s"}, "labels": %s}' "$1" "$1" "$2" "$owner" "$3"
}
gh_issues() {  # DIR then issue objects
  local dir=$1; shift; local out="[" sep=""
  for i in "$@"; do out="$out$sep$i"; sep=","; done
  printf '%s]\n' "$out" > "$dir/issues.json.tmp"; mv "$dir/issues.json.tmp" "$dir/issues.json"
}
count_of() { m task list | awk -v p="$1" '/^#/ && index($0, p) { n++ } END { print n + 0 }'; }
id_of() { m task list | grep "$1" | head -1 | sed -E 's/^#?([0-9]+).*/\1/'; }

echo "== setup: two repositories, two projects, two fake GitHubs"
mkdir -p "$root/gh" "$root/gh2" "$root/c/projects"
for g in gh gh2; do echo '[]' > "$root/$g/issues.json"; echo '{}' > "$root/$g/deps.json"; done
"$bin/fake-github" "$root/gh" "$root/gh.port" 2>"$root/gh.log" & gh=$!
"$bin/fake-github" "$root/gh2" "$root/gh2.port" 2>"$root/gh2.log" & gh2=$!
until [ -s "$root/gh.port" ] && [ -s "$root/gh2.port" ]; do sleep 0.05; done
port=$(cat "$root/gh.port"); port2=$(cat "$root/gh2.port")
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
mkdir -p "$root/repo/.mason"
cat > "$root/repo/.mason/mason.toml" <<'TOML'
schema = 1
start = "develop"

[step.develop]
type = "command"
category = "developing"
command = "true"
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
TOML
printf 'mode = "direct"\n' > "$root/repo/.mason/landing.toml"
echo base > "$root/repo/base.txt"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm "pipeline"
git -C "$root/repo" push -q origin main
git clone -q "$root/origin.git" "$root/repo2" 2>/dev/null
git -C "$root/repo2" checkout -q main
cat > "$root/c/projects/shared.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"

[tracker]
kind = "github"
api = "http://127.0.0.1:$port"
repo = "o/r"
token = "env:FAKE_GITHUB_TOKEN"
interval_s = 300
label = "mason"
require_label = true
TOML
cat > "$root/c/projects/plain.toml" <<TOML
path = "$root/repo2"
base = "main"
remote = "origin"
test = "true"

[tracker]
kind = "github"
api = "http://127.0.0.1:$port2"
repo = "o/r2"
token = "env:FAKE_GITHUB_TOKEN"
interval_s = 300
TOML
export FAKE_GITHUB_TOKEN=recorded-test-token
# Both trackers hold the same three open issues by the owner, each in its own numbers.
gh_issues "$root/gh" \
  "$(issue 701 'Shared labelled mason' '[{"name": "mason"}]')" \
  "$(issue 702 'Shared labelled pipeline' '[{"name": "pipeline"}]')" \
  "$(issue 703 'Shared no label' '[]')"
gh_issues "$root/gh2" \
  "$(issue 801 'Plain labelled mason' '[{"name": "mason"}]')" \
  "$(issue 802 'Plain labelled pipeline' '[{"name": "pipeline"}]')" \
  "$(issue 803 'Plain no label' '[]')"
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== wait for the first look at both trackers"
end=$((SECONDS + 60))
until [ "$(count_of 'Plain')" = 3 ]; do
  [ $SECONDS -lt $end ] || fail "the plain project imported $(count_of 'Plain') of 3 issues"
  sleep 0.3
done
sleep 1   # the shared project is polled at the same time; give it the same chance

echo "== 1. require_label = true: exactly one task, from the mason issue"
[ "$(count_of 'Shared')" = 1 ] || fail "the shared project has $(count_of 'Shared') tasks, not 1"
[ "$(count_of 'Shared labelled mason')" = 1 ] || fail "the one task is not the mason issue's"
m task refresh "$(id_of 'Shared labelled mason')" >/dev/null; sleep 2
[ "$(count_of 'Shared')" = 1 ] || fail "a later look imported more: $(count_of 'Shared') tasks"
echo "   one task: $(m task list | grep 'Shared')"

echo "== 2. the key absent: all three, as before"
[ "$(count_of 'Plain')" = 3 ] || fail "the plain project has $(count_of 'Plain') tasks, not 3"
m task list | grep 'Plain' | sed 's/^/   /'
echo "ok: require_label = true imports 1 of 3 issues; the key absent imports 3 of 3"
