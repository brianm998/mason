#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 24, for real: a masond on your Wi-Fi with LAN access on, and your own phone. MANUAL: it
# opens a TLS port on your network and advertises it over mDNS, so it is never run by CI or by an
# agent on its own. Nothing is asserted; you watch the phone.
#
# Needs: MASON_REAL_APPROVED=yes (you have read this script and approve running it).
#   1. This prints the address, the certificate fingerprint and a pairing URL (and a QR code when
#      `qrencode` is installed). Open the iOS or Android app, allow the local-network permission,
#      and the daemon should appear by its name "MASON on <host>" within a few seconds.
#   2. Pair by scanning the QR code or typing the 8-digit code within 120 s.
#   3. The sidebar is the first screen, tiles scroll sideways above the Home row; a row pushes
#      its detail. Cancel, land now and answering a card work for an owner and are refused for a
#      viewer (run again with VIEWER=1).
#   4. `mason devices list` shows the phone; `mason devices revoke ID` cuts it off at once.
# Press Return to stop.
set -euo pipefail
[ "${MASON_REAL_APPROVED:-}" = yes ] || { echo "refusing: set MASON_REAL_APPROVED=yes once you approve running this" >&2; exit 2; }
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s24r.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
mkdir -p "$root/c"
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port 0 --lan --lan-port 0 --demo-manual 2>"$root/masond.log" &
daemon=$!
until m lan status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.2; done
m lan status
if [ "${VIEWER:-}" = 1 ]; then out=$(m pair --viewer); else out=$(m pair); fi
echo "$out"
url=$(echo "$out" | grep '^mason://')
if command -v qrencode >/dev/null; then qrencode -t ANSIUTF8 "$url"; fi
read -r -p "Press Return to stop " _
