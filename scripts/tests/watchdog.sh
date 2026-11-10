#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Shell tests for scripts/mason-watchdog.sh against a fake server. No timing assertions:
# the script is called once per simulated minute (MASON_WATCHDOG_NOW).
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$(mktemp -d /tmp/mason-wd.XXXXXX); spid=""
cleanup() { [ -n "$spid" ] && kill -9 "$spid" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT

# Fake daemon: GET /status answers the JSON in $root/reply (or nothing, if $root/hang exists);
# POSTs (ntfy) are appended to $root/pushes.
cat >"$root/fake.py" <<'PY'
import http.server, os, sys, time
root = sys.argv[1]
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        if os.path.exists(root + "/hang"): time.sleep(30); return
        b = open(root + "/reply", "rb").read()
        self.send_response(200); self.send_header("Content-Length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0)); b = self.rfile.read(n)
        open(root + "/pushes", "ab").write(self.headers.get("Title", "").encode() + b"|" + b + b"\n")
        self.send_response(200); self.send_header("Content-Length", "0"); self.end_headers()
http.server.ThreadingHTTPServer.daemon_threads = True
s = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
open(root + "/port", "w").write(str(s.server_address[1]))
s.serve_forever()
PY
python3 -I "$root/fake.py" "$root" & spid=$!
until [ -s "$root/port" ]; do sleep 0.1; done
port=$(cat "$root/port")

cat >"$root/say" <<SH
#!/bin/bash
echo "\$*" >> "$root/said"
SH
chmod +x "$root/say"
printf '[ntfy]\nserver = "http://127.0.0.1:%s"\ntopic = "mason"\n' "$port" >"$root/notify.toml"

export MASON_WATCHDOG_STATE=$root/state MASON_WATCHDOG_DAEMON_JSON=$root/daemon.json
export MASON_WATCHDOG_NOTIFY_TOML=$root/notify.toml MASON_WATCHDOG_SAY=$root/say
export MASON_WATCHDOG_TIMEOUT=2
minute=0
run() { minute=$((minute + 1)); MASON_WATCHDOG_NOW=$((1000000 + minute * 60)) bash scripts/mason-watchdog.sh; }
reset() { rm -f "$root/said" "$root/pushes" "$MASON_WATCHDOG_STATE"; : >"$root/said"; : >"$root/pushes"; }
said() { wc -l <"$root/said" | tr -d ' '; }
expect() { # count substring
  [ "$(said)" = "$1" ] || { echo "expected $1 announcements, got $(said):" >&2; cat "$root/said" >&2; exit 1; }
  [ -z "${2:-}" ] || grep -q "$2" "$root/said" || { echo "no '$2' in:" >&2; cat "$root/said" >&2; exit 1; }
}
reply() { printf '{"pid":%s,"uptime_s":%s,"seq":1}' "$1" "$2" >"$root/reply"; }
alive() { printf '{"pid":1,"port":%s,"started_ms":1}' "$port" >"$root/daemon.json"; }

echo "== up: silent"
reset; alive; reply 100 5000
for _ in 1 2 3 4; do run; done; expect 0

echo "== down: not on the first failure, once on the second, repeated every 30 minutes"
reset; alive; reply 100 5000; run
printf '{"pid":1,"port":1,"started_ms":1}' >"$root/daemon.json"   # nothing listens on port 1
run; expect 0
run; expect 1 "is down"
grep -q "is down" "$root/pushes" || { echo "no push" >&2; exit 1; }
for _ in $(seq 1 10); do run; done; expect 1
for _ in $(seq 1 20); do run; done; expect 2 "still down"
for _ in $(seq 1 29); do run; done; expect 2
run; expect 3

echo "== hung: connects, never answers"
reset; alive; reply 100 5000; run; touch "$root/hang"
run; expect 0
run; expect 1 "is down"
rm "$root/hang"

echo "== recovered: said once"
reply 200 5; run; expect 2 "is back"
reply 200 5000; run; run; expect 2

echo "== missing daemon.json counts as down"
reset; rm -f "$root/daemon.json"; run; run; expect 1 "is down"

echo "== one restart is not a crash loop"
reset; alive; reply 100 5000; run; reply 101 3; run; reply 101 63; run; reply 101 123; run; expect 0

echo "== crash-looping: pid changes three runs in a row"
reset; alive; reply 100 5000; run
reply 101 200; run; reply 102 200; run; expect 0
reply 103 200; run; expect 1 "crash-looping"
reply 104 200; run; expect 1
reply 104 400; run; expect 2 "stopped restarting"

echo "== crash-looping: uptime under 60 three runs in a row"
reset; alive; reply 100 10; run; reply 100 20; run; expect 0
reply 100 30; run; expect 1 "crash-looping"

echo "== no say on a machine without it"
reset; alive; reply 100 5000; run
MASON_WATCHDOG_SAY=$root/nonexistent; export MASON_WATCHDOG_SAY
printf '{"pid":1,"port":1}' >"$root/daemon.json"; run; run; expect 0
grep -q "is down" "$root/pushes" || { echo "push skipped too" >&2; exit 1; }
echo "watchdog tests OK"
