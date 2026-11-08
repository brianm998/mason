#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 24 exit test: LAN access, pairing and roles. A real masond with the LAN listener on a random
# port beside loopback, and a fake phone (scripts/exit/fixtures/s24/fake_client.py) over TLS.
#  1. Default config: no LAN listener starts and daemon.json has no lan_port; a bad [lan] value is
#     refused naming daemon.toml.
#  2. An unpaired client is refused (401) on every route and the WebSocket upgrade; x-mason-as does
#     not open it.
#  3. A pairing code pairs an owner and a viewer; it works once; wrong codes are refused and five
#     lock them out. Only a trusted client issues codes.
#  4. The owner can change things; the viewer gets 200 on reads and subscribe and 403 on every POST
#     and typed command.
#  5. Revoking a device returns 401 at once and closes its sockets.
#  6. The advertised fingerprint is the served certificate's; a client pinning another refuses.
#  7. Neither a code nor a token is in the daemon log.
# Real mDNS discovery over Wi-Fi and real phones are s24-real.sh / s24-manual.sh (not run here).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
command -v python3 >/dev/null || { echo "s24 needs python3 (its fake phone)" >&2; exit 1; }
root=$(mktemp -d /tmp/mason-s24.XXXXXX); daemon=""
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
start() {  # EXTRA-ARGS...
  "$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port "$port" --owner "$owner" "$@" 2>>"$root/masond.log" &
  daemon=$!
  until m status >/dev/null 2>&1; do kill -0 "$daemon" 2>/dev/null || fail "masond did not start"; sleep 0.1; done
}
stop() { kill "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; daemon=""; }

echo "== 1. loopback is the default: no LAN listener, no setting changed"
start
grep -q lan_port "$root/d/daemon.json" && fail "daemon.json names a LAN port with LAN access off"
[ "$(m lan status --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["enabled"])')" = False ] || fail "LAN reported on by default"
out=$(m pair 2>&1 || true); case "$out" in *"LAN access is off"*) ;; *) fail "a code was issued with LAN access off";; esac
stop
echo "PASS: default is loopback only"
printf '[lan]\nenabled = "yes"\n' > "$root/c/daemon.toml"
if "$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port "$port" 2>"$root/bad.log"; then fail "a bad [lan] value was accepted"; fi
grep -q "daemon.toml: lan.enabled" "$root/bad.log" || fail "the bad value did not name the file: $(cat "$root/bad.log")"
echo "PASS: a bad [lan] value is refused, naming daemon.toml"
printf '[lan]\nenabled = true\nport = %s\n' "$lanport" > "$root/c/daemon.toml"

echo "== 2-7. the fake phone against the LAN listener"
start
[ "$(m lan status --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["port"])')" = "$lanport" ] || fail "[lan] port was not used"
python3 -I scripts/exit/fixtures/s24/fake_client.py "$lanport" "$port" "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" || fail "the fake phone failed"
[ "$(stat -f %Lp "$root/d/lan/key.der" 2>/dev/null || stat -c %a "$root/d/lan/key.der")" = 600 ] || fail "the private key is not mode 0600"
echo "PASS: the private key is mode 0600"
echo "== 7. secrets are not in the log"
python3 - "$root/masond.log" <<'PY' || fail "a code or token is in masond.log"
import re, sys
t = open(sys.argv[1]).read()
sys.exit(1 if re.search(r"mt_[0-9a-f]{16}", t) or re.search(r"code\W+\d{8}", t) else 0)
PY
echo "PASS: no token or code in masond.log"
echo "s24: all checks passed"
