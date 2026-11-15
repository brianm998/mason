#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 379 exit test: extra reachable hosts and a bind setting for the LAN listener.
#  1. A bad [lan] hosts or bind value is refused, naming daemon.toml.
#  2. With hosts and two bind addresses on loopback (127.0.0.1 and 127.0.0.2, else ::1), `mason lan
#     status` shows the hosts and both addresses, and the pairing URL has an alt= for each host.
#  3. A fake phone pairs and connects through the second address; an unpaired one is refused (401).
#  4. With no bind setting nothing about the default changes: it reports 0.0.0.0.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
command -v python3 >/dev/null || { echo "s379 needs python3 (its fake phone)" >&2; exit 1; }
root=$(mktemp -d /tmp/mason-s379.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
free() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
port=$(free); lanport=$(free)
mkdir -p "$root/c"
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { echo "FAIL: $*" >&2; tail -20 "$root/masond.log" >&2 || true; exit 1; }
start() {
  "$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port "$port" --owner "$owner" "$@" 2>>"$root/masond.log" &
  daemon=$!
  until m status >/dev/null 2>&1; do kill -0 "$daemon" 2>/dev/null || fail "masond did not start"; sleep 0.1; done
}
stop() { [ -n "$daemon" ] || return 0; kill "$daemon"; wait "$daemon" 2>/dev/null || true; daemon=""; }

echo "== 1. bad values are refused"
for bad in 'hosts = "x"' 'hosts = ["a b"]' 'hosts = [3]' 'bind = []' 'bind = ["nope"]' 'bind = "0.0.0.0"'; do
  printf '[lan]\nenabled = true\nport = %s\n%s\n' "$lanport" "$bad" > "$root/c/daemon.toml"
  if "$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port "$port" 2>"$root/bad.log"; then fail "accepted: $bad"; fi
  grep -q "daemon.toml: lan\.\(hosts\|bind\)" "$root/bad.log" || fail "did not name the setting for $bad: $(cat "$root/bad.log")"
done
echo "PASS: bad hosts/bind are refused, naming daemon.toml"

echo "== 4. the default binds 0.0.0.0"
printf '[lan]\nenabled = true\nport = %s\n' "$lanport" > "$root/c/daemon.toml"
start
m lan status --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["bind"]==["0.0.0.0"] and len(d["hosts"])==1, d' || fail "default bind or hosts changed"
stop
echo "PASS: no bind setting means 0.0.0.0, and only the .local name"

second=127.0.0.2
python3 -c 'import socket,sys; s=socket.socket(); s.bind((sys.argv[1],0))' "$second" 2>/dev/null || second=::1
echo "== 2-3. hosts, and two bind addresses (127.0.0.1 and $second)"
printf '[lan]\nenabled = true\nport = %s\nhosts = ["mac.example.ts.net", "100.64.0.9"]\nbind = ["127.0.0.1", "%s"]\n' "$lanport" "$second" > "$root/c/daemon.toml"
start
m lan status | grep -q "mac.example.ts.net, 100.64.0.9" || fail "lan status does not show the hosts: $(m lan status)"
m lan status | grep -q "127.0.0.1, $second" || fail "lan status does not show the bind addresses: $(m lan status)"
echo "PASS: mason lan status shows the hosts and bind addresses"
pj=$(m pair --json)
printf '%s' "$pj" | python3 -c 'import json,sys; d=json.load(sys.stdin); u=d["url"]; assert u.count("&alt=")==2 and "&alt=mac.example.ts.net&alt=100.64.0.9" in u and u.index("host=")<u.index("alt="), u; assert d["hosts"][1:]==["mac.example.ts.net","100.64.0.9"], d' || fail "pairing URL lacks alt= hosts: $pj"
echo "PASS: the pairing URL has host= then an alt= for each extra host"
fp=$(printf '%s' "$pj" | python3 -c 'import json,sys; print(json.load(sys.stdin)["fingerprint"])')
code=$(printf '%s' "$pj" | python3 -c 'import json,sys; print(json.load(sys.stdin)["code"])')
python3 -I scripts/exit/fixtures/s379/client.py "$second" "$lanport" "$fp" "$code" || fail "the fake phone failed through $second"
echo "s379: all checks passed"
