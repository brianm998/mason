#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s965 exit test: on a real masond with two projects, `mason project rename a b --yes`
# moves the registration file, `mason status` lists b and not a, and after a restart
# (replay) b is still there with a's id.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s965.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT
export USER="${USER:-owner}"
cargo build -q --workspace --bins
bin=$PWD/target/debug
sha=0123456789abcdef0123456789abcdef01234567
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { set +e; echo "FAILED: $*" >&2; tail -20 "$root/masond.log" >&2; exit 1; }
start() {
  "$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port 0 --runtime-sha "$sha" \
    --host-bin "$bin/step-host" --mcp-bin "$bin/mason-mcp" >>"$root/masond.log" 2>&1 &
  daemon=$!
  local end=$((SECONDS + 60))
  until m status >/dev/null 2>&1; do
    kill -0 "$daemon" 2>/dev/null || fail "masond exited early"
    [ $SECONDS -lt $end ] || fail "masond did not answer"; sleep 0.2
  done
}
mkdir -p "$root/c/projects" "$root/repo"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
echo base > "$root/repo/base.txt"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@example.com commit -q -m init
git -C "$root/repo" push -q origin main
for n in a other; do cat > "$root/c/projects/$n.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
TOML
done


echo "== a rename without --yes changes nothing"
start
out=$(m project rename a b) || fail "plan failed: $out"
[ -f "$root/c/projects/a.toml" ] || fail "the plan moved the file"
echo "== rename a b --yes"
out=$(m project rename a b --yes) || fail "rename failed: $out"
[ -f "$root/c/projects/b.toml" ] || fail "b.toml is missing"
[ ! -f "$root/c/projects/a.toml" ] || fail "a.toml is still there"
m --json status | grep -q '"b"' || fail "status does not list b"
if m --json status | grep -q '"a"'; then fail "status still lists a"; fi
echo "== a restart replays the rename"
kill -TERM "$daemon"; wait "$daemon" || true; daemon=""
start
m --json status | grep -q '"b"' || fail "after a restart status does not list b"
if m --json status | grep -q '"a"'; then fail "after a restart status lists a"; fi
echo "s965: ok"
