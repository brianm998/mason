#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s162 exit test: a definition that binds a suite's level to test_step while its test step
# lists no suites. `mason doctor` warns, naming the step and the level; after `suites = [...]`
# is added to the step the warning is gone. A real masond; no network.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s162.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q --workspace --bins
bin=$PWD/target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { echo "FAILED: $*" >&2; [ ! -f "$root/doctor.out" ] || cat "$root/doctor.out" >&2; tail -20 "$root/masond.log" >&2; exit 1; }
doctor() { m doctor >"$root/doctor.out" 2>&1 || fail "mason doctor exited non-zero"; }

mkdir -p "$root/c/projects" "$root/repo/.mason"
git init -q -b main "$root/repo"
echo base > "$root/repo/base.txt"
write_def() { # $1: extra line for the test step
cat > "$root/repo/.mason/mason.toml" <<TOML
schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
$1
on_red = "develop"
max_rounds = 2
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.unit]
command = "true"
parser = "regex"
pass_regex = "ok"
level = "unit"

[binding.unit]
when = ["development", "test_step"]
TOML
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm "definition $2"
}
cat > "$root/c/projects/star.toml" <<TOML
path = "$root/repo"
base = "main"
test = "true"
TOML
write_def "" one
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port "$port" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== 1. the test step lists no suites"
doctor
grep 'bound to test_step' "$root/doctor.out" | sed 's/^/   /'
grep -q 'project star: .*step.test.suites.*`unit`.*test step `test` lists none' "$root/doctor.out" || fail "doctor does not warn about the unlisted suites"

echo "== 2. suites = [\"unit\"] is added"
write_def 'suites = ["unit"]' two
doctor
! grep -q 'bound to test_step' "$root/doctor.out" || fail "the warning is still there"
echo "   the warning is gone"
echo "s162 passed"
