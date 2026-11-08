#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 23, by hand: hear it. MANUAL: it needs a person, speakers and the built monitors (SwiftUI
# on a Mac, Compose on the desktop). Nothing is asserted here.
#
# It starts a scratch masond on a free port and raises conditions for you to listen to. Open
# one or two monitors, point them at the printed address, and check:
#   1. A pause raises one alert: the banner "Paused" shows, and you hear a short sound then one
#      sentence. (Pause is routed to sound and speech for this script only.)
#   2. With two monitors open, only one speaks; mute that one and, within about 30 s, the other
#      takes over. Muting never hides the banner or the attention count.
#   3. "Wants a decision" and "broke" sound different: use the voice button (the waveform) to hear
#      each family's sound, and pick another voice from the list the operating system offers.
#   4. Two announcements at once are spoken one after the other, never over each other.
#   5. Quit masond (Ctrl-C in its terminal is not needed: press Return here): after 15 s each
#      monitor raises "Daemon down" by itself, with a sound and a sentence unless muted.
# Press Return to stop.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s23m.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
mkdir -p "$root/c"
cat > "$root/c/notify.toml" <<'TOML'
[routing.paused]
sound = "0"
speech = "0"
[reminder]
after = "20s"
TOML
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port 0 --reload-ms 500 2>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.2; done
port=$(python3 -c "import json;print(json.load(open('$root/d/daemon.json'))['port'])")
echo "masond is on port $port. Start a monitor with:  MasonMonitor --port $port   (or the Compose app with --port $port)"
echo "Press Return to raise the first condition."; read -r _
m pause --mode all --for 1h
m pause --mode claims --for 1h
echo "Two conditions raised at once. Listen. Press Return to clear them (nothing more should be said)."; read -r _
m resume --mode all; m resume --mode claims
echo "Press Return to stop masond and watch for 'Daemon down' after 15 s."; read -r _
kill -9 "$daemon"; daemon=""
echo "Press Return when you have seen it."; read -r _
