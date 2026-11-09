#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s84 exit test: a step's time limit counts from when its host started, so a daemon
# restart does not give a hung agent a fresh budget. A real masond with the scripted
# agent (fake-claude) whose develop step hangs, with `budget = "6s"`. The daemon is
# SIGKILLed 3 s into the step and restarted; the step must end "the step ran out of
# time" within the original budget plus a small margin (not 3 s + 6 s).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s84.XXXXXX); daemon=""
stop_daemon() {
  if [ -n "$daemon" ]; then kill -9 "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; fi
  daemon=""
}
cleanup() {
  stop_daemon
  target/debug/step-host-ctl reap "$root/d/h" >/dev/null 2>&1 || true
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
cdir="$root/c"; mkdir -p "$cdir/projects"
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$cdir" "$@"; }
die() { echo "$1" >&2; tail -20 "$root/masond.log" >&2; exit 1; }
start_daemon() {
  "$bin/masond" --data-dir "$root/d" --config-dir "$cdir" --port 0 --heartbeat-ms 100 --sweep-ms 3600000 --owner "${USER:-owner}" 2>>"$root/masond.log" &
  daemon=$!
  until m status >/dev/null 2>&1; do kill -0 "$daemon" || die "masond died"; sleep 0.1; done
}
now_ms() { python3 -I -c 'import time; print(int(time.time()*1000))'; }

repo="$root/r"; origin="$root/o.git"
git init -q --bare -b main "$origin" 2>/dev/null
git clone -q "$origin" "$repo" 2>/dev/null
git -C "$repo" checkout -q -b main
mkdir -p "$repo/.mason"
cat > "$repo/.mason/mason.toml" <<'TOML'
schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
budget = "6s"
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
printf 'mode = "direct"\n' > "$repo/.mason/landing.toml"
echo base > "$repo/README"
git -C "$repo" add -A
git -C "$repo" -c user.name=t -c user.email=t@t commit -qm pipeline
git -C "$repo" push -q origin main
git -C "$repo" checkout -q -b owner-work

echo '{"steps": {"develop": [[{"sleep_ms": 600000}]]}}' > "$root/script.json"
cat > "$cdir/projects/p.toml" <<TOML
path = "$repo"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
: > "$root/masond.log"
start_daemon

m task add "hangs" --project p --body x >/dev/null
end=$((SECONDS + 60))
until [ -f "$root"/d/h/t0r0-1/host.kv ]; do
  [ $SECONDS -lt $end ] || die "the develop host never started"; sleep 0.1
done
started=$(sed -n 's/^started_ms=//p' "$root"/d/h/t0r0-1/host.kv)
while [ "$(now_ms)" -lt $((started + 3000)) ]; do sleep 0.05; done
stop_daemon
[ ! -f "$root"/d/h/t0r0-1/exit.kv ] || die "the host ended before the restart"
echo "== daemon killed $(( $(now_ms) - started )) ms into the step; restarting"
start_daemon

end=$((SECONDS + 60))
until [ -f "$root"/d/h/t0r0-1/exit.kv ]; do
  [ $SECONDS -lt $end ] || { m task show 0 >&2; die "the hung step was never stopped"; }
  sleep 0.1
done
ended=$(sed -n 's/^ended_ms=//p' "$root"/d/h/t0r0-1/exit.kv)
took=$((ended - started))
echo "== the host ended $took ms after it started (budget 6000 ms)"
[ "$took" -ge 5900 ] || die "stopped before its budget ran out ($took ms)"
[ "$took" -le 8500 ] || die "the restart gave the step a fresh budget ($took ms; killed at 3000)"
end=$((SECONDS + 30))
until m task show 0 2>/dev/null | grep -q "the step ran out of time"; do
  [ $SECONDS -lt $end ] || { m task show 0 >&2; die "task show never said the step ran out of time"; }
  sleep 0.3
done
echo "s84 passed"
