#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 24, by hand: the phone apps on simulators and emulators. MANUAL: it needs Xcode with an iOS
# simulator, or an Android emulator, and the app built from clients/swift/ios or clients/compose/android
# (neither build was made in the stage; see docs/stages/s24.md). Nothing is asserted here.
#
# It starts a scratch masond with LAN access on and a demo script, prints the pairing URL, and
# lists what to check on each simulator or emulator:
#   1. The app asks for local-network permission (iOS) and finds "MASON on <host>" (Bonjour / NSD).
#      Note: the iOS simulator and the Android emulator share the host's network stack or sit behind
#      NAT; if discovery finds nothing, enter the host and port by hand.
#   2. Pairing with the code works once; a second try with the same code is refused.
#   3. A viewer-paired app shows everything and its action buttons are disabled; an owner-paired
#      app can cancel, skip, land now and answer a decision card.
#   4. Layout: the sidebar is the first screen, the count tiles scroll sideways above the Home row.
#   5. Revoke the device (`mason devices revoke ID`): the app drops to "not paired" within a few seconds.
#   6. Close the app, change something with `mason`, open it: it catches up from its last event.
# Press Return to stop.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s24m.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
mkdir -p "$root/c"
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port 0 --lan --lan-port 0 --demo 2>"$root/masond.log" &
daemon=$!
until m lan status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.2; done
m lan status
m pair
m pair --viewer
read -r -p "Press Return to stop " _
