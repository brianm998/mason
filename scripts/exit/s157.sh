#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s157 exit test: a real masond in a scratch data dir is sent SIGTERM; the log says
# `masond stopping: SIGTERM`, the exit status is 0 and boots.log ends with `stopped`
# for its sha. Started again, `mason project reload` applies an edited registration
# file without a restart, and refuses a change to a fixed field naming it.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s157.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT
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
cat > "$root/c/projects/scratch.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
TOML

echo "== SIGTERM is logged, exits 0, and is recorded as stopped"
start
kill -TERM "$daemon"
rc=0; wait "$daemon" || rc=$?
daemon=""
[ "$rc" -eq 0 ] || fail "exit status $rc, wanted 0"
grep -q "masond stopping: SIGTERM" "$root/masond.log" || fail "no 'masond stopping' line"
last=$(tail -n 1 "$root/d/runtimes/boots.log")
case "$last" in *" $sha stopped") ;; *) fail "boots.log ends with: $last" ;; esac

echo "== a registration edit is applied without a restart"
start
printf 'step_timeout_s = 777\n' >> "$root/c/projects/scratch.toml"
out=$(m project reload) || fail "reload failed: $out"
case "$out" in *step_timeout_s*) ;; *) fail "reload did not name the change: $out" ;; esac
out=$(m project reload)
case "$out" in *"Nothing changed"*) ;; *) fail "second reload: $out" ;; esac
sed -i.bak 's/^base = "main"/base = "other"/' "$root/c/projects/scratch.toml"
if out=$(m project reload); then fail "a base change was not refused: $out"; fi
case "$out" in *'`base`'*) ;; *) fail "refusal does not name base: $out" ;; esac
echo "s157: ok"
