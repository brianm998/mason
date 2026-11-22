#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s819 exit test: the monitors set the placement mode for MASON, a project and a task (#834).
# A real masond; the three owner set commands go through HTTP exactly as masond maps a monitor's
# typed `SetPlacement` command (POST /remote, POST /remote with a project, POST /tasks/N/run_on).
# `mason remote status --json` and `mason task show --json` must reflect each, with its source.
# Then the Rust, Swift and Compose tests run by name (Swift and Compose when swift / gradle are
# installed; MASON_SKIP_SWIFT or MASON_SKIP_COMPOSE skips one).
set -euo pipefail
trap 'rc=$?; [ "$rc" -eq 0 ] || echo "FAILED (exit $rc) at line $LINENO: $BASH_COMMAND" >&2' ERR
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s819.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q --workspace --bins
bin=$PWD/target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
port() { sed -n 's/.*"port": *\([0-9]*\).*/\1/p' "$root/d/daemon.json"; }
http() {  # WHO PATH JSON -> prints the status code
  curl -s -o "$root/last.json" -w '%{http_code}' -X POST "http://127.0.0.1:$(port)$2" \
    -H 'content-type: application/json' -H "x-mason-as: $1" -d "$3"
}
field() {  # JSON-FILE PYTHON-EXPR over `j`
  python3 -I -c 'import json,sys; j=json.load(open(sys.argv[1])); print(eval(sys.argv[2]))' "$1" "$2"
}
alpha='[p for p in j["projects"] if p["project"]=="alpha"][0]'
fail() { echo "FAILED: $*" >&2; tail -20 "$root/masond.log" >&2; exit 1; }

mkdir -p "$root/c/projects" "$root/repo"
git -C "$root/repo" init -q -b main
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -q --allow-empty -m base
cat > "$root/c/projects/alpha.toml" <<TOML
path = "$root/repo"
base = "main"
test = "true"

[remote_runs]
kind = "github-actions"
api = "http://127.0.0.1:9"
repo = "o/r"
token = "env:S819_TOKEN"
TOML
export S819_TOKEN=unused
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
m task add "Held" --project alpha --body "x" --proposed >/dev/null

echo "== a non-owner's set is refused"
[ "$(http someone-else /remote '{"mode":"remote"}')" = 403 ] || fail "global set by a non-owner"
[ "$(http someone-else /remote '{"mode":"remote","project":"alpha"}')" = 403 ] || fail "project set by a non-owner"
[ "$(http someone-else /tasks/0/run_on '{"to":"remote"}')" = 403 ] || fail "task set by a non-owner"
m remote status --json > "$root/s.json"
[ "$(field "$root/s.json" "j['mode']")" = local ] || fail "a refused set changed the mode"

echo "== 1. all of MASON"
[ "$(http "$owner" /remote '{"mode":"favor-local"}')" = 200 ] || fail "owner global set"
m remote status --json > "$root/s.json"
[ "$(field "$root/s.json" "j['mode']")" = favor-local ] || fail "global mode not reflected"
[ "$(field "$root/s.json" "$alpha['source']")" = global ] || fail "project source should be global"
m task show 0 --json > "$root/t.json"
[ "$(field "$root/t.json" "j['run_on']['mode']+' '+j['run_on']['from']")" = "favor-local global" ] || fail "task view: $(cat "$root/t.json")"

echo "== 2. one project"
[ "$(http "$owner" /remote '{"mode":"favor-remote","project":"alpha"}')" = 200 ] || fail "owner project set"
m remote status --json > "$root/s.json"
[ "$(field "$root/s.json" "$alpha['mode']+' '+$alpha['source']")" = "favor-remote project" ] || fail "project mode not reflected"
[ "$(field "$root/s.json" "j['mode']")" = favor-local ] || fail "the project set changed the global mode"
m task show 0 --json > "$root/t.json"
[ "$(field "$root/t.json" "j['run_on']['mode']+' '+j['run_on']['from']")" = "favor-remote project" ] || fail "task view after project set"

echo "== 3. one task"
[ "$(http "$owner" /tasks/0/run_on '{"to":"remote"}')" = 200 ] || fail "owner task set"
m task show 0 --json > "$root/t.json"
[ "$(field "$root/t.json" "j['run_on']['mode']+' '+j['run_on']['from']")" = "remote task" ] || fail "task mode not reflected"

echo "== Follow project, Follow MASON"
[ "$(http "$owner" /tasks/0/run_on '{"to":"default"}')" = 200 ] || fail "task default"
m task show 0 --json > "$root/t.json"
[ "$(field "$root/t.json" "j['run_on']['from']")" = project ] || fail "task did not follow its project"
[ "$(http "$owner" /remote '{"mode":"default","project":"alpha"}')" = 200 ] || fail "project default"
m task show 0 --json > "$root/t.json"
[ "$(field "$root/t.json" "j['run_on']['mode']+' '+j['run_on']['from']")" = "favor-local global" ] || fail "project did not follow MASON"

echo "== the tests, by name"
cargo test -q -p mason-daemon --test it placement_monitors
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
  (cd clients/swift && swift test --filter PlacementTests)
fi
if command -v java >/dev/null && command -v gradle >/dev/null && [ -z "${MASON_SKIP_COMPOSE:-}" ]; then
  (cd clients/compose && gradle --console=plain :core:test --tests '*PlacementTest' :app:test --tests '*PlacementControlTest')
fi
echo "s819 passed"
