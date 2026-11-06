#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 15, for real: `direct-ci` and `pr-batch` against a SCRATCH GitHub
# repository the owner has approved. MANUAL: never run by CI, and never by an
# agent on its own. It pushes branches, opens and merges pull requests and runs
# GitHub Actions in that repository.
#
# Needs, all explicit:
#   MASON_REAL_REPO=owner/scratch-repo   a repository that exists only for this
#   MASON_REAL_APPROVED=yes              you have read this script and approve running it
#   GH_TOKEN=...                         a token that can push, open and merge PRs and read Actions there
# and a repository whose default branch (it is read from the clone) takes pushes and
# merges without further rules, with Actions enabled.
#
# The agent is the scripted stand-in (no model is called). Two tasks are batched
# per mode: a clean one and a culprit that adds `red.integration`, which makes the
# repository's workflow fail, so the red goes through real CI and the ladder
# (rerun, then bisection over real candidate refs or pull requests).
# Afterwards `mason landing`, the pull requests and the remote branches are shown.
set -euo pipefail
: "${MASON_REAL_REPO:?name the scratch repository, owner/name}"
[ "${MASON_REAL_APPROVED:-}" = yes ] || { echo "refusing: set MASON_REAL_APPROVED=yes once you approve running this against $MASON_REAL_REPO" >&2; exit 2; }
: "${GH_TOKEN:?a token for the scratch repository}"
command -v gh >/dev/null || { echo "needs the gh CLI for the report at the end" >&2; exit 2; }
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s15r.XXXXXX); daemon=""
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
bin=$PWD/target/debug
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }

url="https://x-access-token:$GH_TOKEN@github.com/$MASON_REAL_REPO.git"
git clone -q "$url" "$root/repo"
cd "$root/repo"
git config user.name mason-s15; git config user.email mason@localhost
git remote set-url origin "https://github.com/$MASON_REAL_REPO.git"   # the token is not kept in the clone
git config credential.helper "!f() { echo username=x-access-token; echo password=\$GH_TOKEN; }; f"
base=$(git rev-parse --abbrev-ref HEAD)
mkdir -p .github/workflows .mason
cat > .github/workflows/mason.yml <<'YML'
name: mason
on:
  push:
    branches: ["mason/ci/**", "mason/pr/**"]
  pull_request:
jobs:
  integration:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: sh checks.sh
YML
printf '#!/bin/sh\n[ -f red.integration ] && { echo "red.integration is present"; exit 1; }\necho green\n' > checks.sh
cat > .mason/mason.toml <<'TOML'
schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "land"

[step.land]
type = "landing"
category = "landing"
on_red = "develop"
max_rounds = 2
TOML
turn='{"sh": "echo ok > file-$(basename \"$PWD\").txt; [ \"$(basename \"$PWD\")\" = %CULPRIT% ] && echo x > red.integration; git add -A && git -c user.name=t -c user.email=t@t commit -qm \"s15 task $(basename \"$PWD\")\""}, {"call": {"tool": "submit_change", "args": {"summary": "done", "head": "$HEAD"}}}'

for mode in direct-ci pr-batch; do
  echo "================ $mode against $MASON_REAL_REPO"
  printf 'mode = "%s"\nwindow = "15s"\nmax_size = 2\n' "$mode" > .mason/landing.toml
  git add -A; git commit -q -m "mason s15-real: $mode" --allow-empty; git push -q origin "$base"
  # Task ids restart for each daemon, so the culprit is task 1 in each run.
  rm -rf "$root/d" "$root/c"; mkdir -p "$root/c/projects"
  sed "s/%CULPRIT%/1/" <<<"{\"steps\": {\"develop\": [[$turn]]}}" > "$root/script.json"
  cat > "$root/c/projects/scratch.toml" <<TOML
path = "$root/repo"
base = "$base"
remote = "origin"
test = "true"
claude_bin = "$bin/fake-claude"
[host]
kind = "github"
repo = "$MASON_REAL_REPO"
token = "env:GH_TOKEN"
[ci]
kind = "github-actions"
repo = "$MASON_REAL_REPO"
token = "env:GH_TOKEN"
poll_s = 15
timeout_s = 1800
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
  "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" doctor || true
  "$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 2>>"$root/masond.log" &
  daemon=$!
  until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.2; done
  m task add "clean change" --explored --body "Add a file." >/dev/null
  m task add "culprit" --explored --body "Add a file and break the workflow." >/dev/null
  end=$((SECONDS + 1800))
  until [ "$(m task show 0 --json | jq_state)" = landed ]; do
    [ $SECONDS -lt $end ] || { echo "the clean task did not land in 30 minutes" >&2; m landing >&2 || true; exit 1; }
    sleep 10
  done
  echo "-- the clean task landed; the culprit:"
  m task show 1 | head -8
  echo "-- mason landing:"
  m landing
  echo "-- pull requests in $MASON_REAL_REPO:"
  gh pr list --repo "$MASON_REAL_REPO" --state all --limit 10
  echo "-- branches MASON left on the remote (there should be none):"
  git ls-remote origin 'refs/heads/mason/*' || true
  kill -9 "$daemon" 2>/dev/null || true; daemon=""
done
echo "s15-real finished: read the output above; the repository $MASON_REAL_REPO holds what it did"
