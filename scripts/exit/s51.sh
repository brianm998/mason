#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# A constant daemon port (s51) exit test: real masond processes.
#  1. Started with no --port, masond listens on the default port (7423) and daemon.json says so;
#     killed and started again it is the same port.
#  2. A monitor-core-style client opened before the restart reconnects by itself and shows the same
#     tasks, also when the daemon comes back on another port (--port 0) and daemon.json changes.
#  3. With the port held by another process masond exits 1 with a message naming the port, the
#     holder and the way to change it; it does not pick another port.
#  4. With --port 0 two daemons (two data directories) run side by side.
# The default port must be free to run this: if something holds it, the script says so and stops
# (it never picks another port for the daemon under test).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
command -v python3 >/dev/null || { echo "s51 needs python3 (its WebSocket client)" >&2; exit 1; }
DEFAULT=7423
root=$(mktemp -d /tmp/mason-s51.XXXXXX); daemon=""; daemon2=""; client=""; holder=""
cleanup() {
  for p in "$client" "$holder"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done
  for p in "$daemon" "$daemon2"; do [ -n "$p" ] && kill -9 "$p" 2>/dev/null || true; done
  # Verified before it is killed: a stale pid in a registry may belong to someone else now.
  for hd in "$root"/d/h "$root"/d2/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1 || true; done
  rm -rf "$root"
}
trap cleanup EXIT
# Bound with SO_REUSEADDR as masond does, so a port in TIME_WAIT after a restart counts as free.
free_port() { python3 -c 'import socket,sys; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(("127.0.0.1",int(sys.argv[1])))' "$1" 2>/dev/null; }
if ! free_port "$DEFAULT"; then
  who=unknown
  if listing=$(lsof -nP -iTCP:$DEFAULT -sTCP:LISTEN 2>/dev/null); then who=$(printf '%s\n' "$listing" | sed -n 2p); fi
  echo "FAILED: port $DEFAULT, masond's default, is held by another process (${who:-unknown}); stop it and run again. This test does not pick another port." >&2
  exit 1
fi
cargo build -q --workspace --bins
bin=target/debug
ws=scripts/exit/fixtures/s51/wsclient.py
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { echo "FAILED: $*" >&2; tail -20 "$root/masond.log" >&2 || true; exit 1; }
wait_for() {  # SECONDS DESCRIPTION COMMAND...
  local end=$((SECONDS + $1)); local what=$2; shift 2
  until "$@" >/dev/null 2>&1; do [ $SECONDS -lt $end ] || fail "timed out waiting for: $what"; sleep 0.2; done
}
jq_port() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["port"])' "$1"; }
jq_pid() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$1"; }
start_daemon() {  # ARGS... (no --port unless given)
  "$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --max-concurrent 4 --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" "$@" 2>>"$root/masond.log" &
  daemon=$!
  until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
}
stop_daemon() { kill -9 "$daemon"; wait "$daemon" 2>/dev/null || true; daemon=""; sleep 0.5; }
# the last record the client wrote: FIELD
last() { tail -n 1 "$root/client.jsonl" | python3 -c "import json,sys; d=json.loads(sys.stdin.read()); print(d[sys.argv[1]])" "$1"; }
client_on() {  # PORT: the client's last view came from this port and shows both tasks
  [ -s "$root/client.jsonl" ] && [ "$(last port)" = "$1" ] && [ "$(last tasks)" = "$expect" ]
}

echo "== setup: a bare repository and a pipeline that holds a task in a long command"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
mkdir -p "$root/repo/.mason" "$root/c/projects"
cat > "$root/repo/.mason/mason.toml" <<'TOML'
schema = 1
start = "work"

[step.work]
type = "command"
category = "developing"
command = "sleep 600"
next = "land"

[step.land]
type = "landing"
category = "landing"
TOML
printf 'mode = "direct"\n' > "$root/repo/.mason/landing.toml"
echo base > "$root/repo/base.txt"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm pipeline
git -C "$root/repo" push -q origin main
cat > "$root/c/projects/web.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
TOML

echo "== 1. the default port, and the same one after a restart"
start_daemon
p1=$(jq_port "$root/d/daemon.json")
echo "   no --port: daemon.json says port $p1"
[ "$p1" = "$DEFAULT" ] || fail "expected the default port $DEFAULT, daemon.json says $p1"
m task add "first" --project web --body first >/dev/null
m task add "second" --project web --body second >/dev/null
expect="['#0', '#1']"  # how python prints the two task keys
pid1=$(jq_pid "$root/d/daemon.json")

echo "== 2. a monitor-core-style client opened before the restart"
python3 "$ws" monitor "$root/d" 120 "$root/client.jsonl" &
client=$!
wait_for 20 "the client to show two tasks" client_on "$DEFAULT"
before=$(last tasks); n0=$(last connects)
echo "   the client shows tasks $before (connection $n0)"
[ "$before" = "$expect" ] || fail "expected tasks $expect, the client shows $before"
stop_daemon
echo "   daemon killed; starting it again with no --port"
start_daemon
p2=$(jq_port "$root/d/daemon.json"); pid2=$(jq_pid "$root/d/daemon.json")
echo "   after the restart: port $p2 (was $p1), pid $pid2 (was $pid1)"
[ "$p2" = "$p1" ] || fail "the port changed over a restart: $p1 then $p2"
[ "$pid2" != "$pid1" ] || fail "the pid did not change: this was not a restart"
n1() { [ "$(last connects)" -gt "$n0" ]; }
wait_for 30 "the client to reconnect by itself" n1
after=$(last tasks)
echo "   the client reconnected (connection $(last connects)) and shows tasks $after"
[ "$after" = "$before" ] || fail "the tasks changed over the restart: $before then $after"

echo "== 2b. the daemon comes back on another port: the client re-reads daemon.json"
stop_daemon
start_daemon --port 0
p3=$(jq_port "$root/d/daemon.json")
echo "   --port 0: port $p3"
[ "$p3" != "$DEFAULT" ] && [ "$p3" != "0" ] || fail "--port 0 should give a free port, got $p3"
wait_for 30 "the client to follow the daemon to port $p3" client_on "$p3"
[ "$(last tasks)" = "$before" ] || fail "the tasks changed when the port moved: $(last tasks)"
echo "   the client is on port $p3 and shows tasks $(last tasks)"
kill "$client" 2>/dev/null || true; client=""
stop_daemon

echo "== 3. the default port held by another process"
python3 -c 'import socket,time; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(("127.0.0.1",'$DEFAULT')); s.listen(); time.sleep(600)' &
holder=$!
sleep 1
set +e
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --max-concurrent 4 --heartbeat-ms 100 --sweep-ms 3600000 2>"$root/taken.log"
code=$?
set -e
echo "   masond exited $code and said: $(cat "$root/taken.log")"
[ "$code" = 1 ] || fail "expected exit 1 with the port taken, got $code"
grep -q "port $DEFAULT on 127.0.0.1 is taken by" "$root/taken.log" || fail "the message does not name the port"
grep -q "pid $holder" "$root/taken.log" || echo "   (note: lsof did not name pid $holder; the message says 'another process' then)"
grep -q -- "--port" "$root/taken.log" && grep -q "daemon.toml" "$root/taken.log" || fail "the message does not say how to change the port"
grep -q "does not pick another" "$root/taken.log" || fail "the message does not say it will not pick another port"
kill "$holder"; holder=""

echo "== 4. --port 0: two daemons side by side"
start_daemon --port 0
pa=$(jq_port "$root/d/daemon.json")
mkdir -p "$root/c2"
"$bin/masond" --data-dir "$root/d2" --config-dir "$root/c2" --port 0 --heartbeat-ms 100 --sweep-ms 3600000 2>>"$root/masond2.log" &
daemon2=$!
wait_for 30 "the second daemon to announce itself" test -s "$root/d2/daemon.json"
pb=$(jq_port "$root/d2/daemon.json")
echo "   ports $pa and $pb"
[ "$pa" != "$pb" ] || fail "two daemons on the same port"
"$bin/mason" --data-dir "$root/d2" --config-dir "$root/c2" status >/dev/null || fail "the second daemon does not answer"
m status >/dev/null || fail "the first daemon does not answer"
echo "s51: ok"
