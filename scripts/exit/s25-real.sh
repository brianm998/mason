#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 25, real tools, read-only: asks this machine's xcrun and adb what devices exist through the
# real provider, and prints them. Boots nothing and runs no suite. Not run in CI or by
# run-exit-scripts.sh (it needs Xcode, the Android SDK or a device, and says what it sees).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s25-real.XXXXXX); daemon=""
trap '[ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null; rm -rf "$root"' EXIT
cargo build -q --workspace --bins
bin=target/debug
for t in xcrun adb; do
  command -v "$t" >/dev/null && echo "found $t: $(command -v "$t")" || echo "no $t on PATH (its devices will not be listed)"
done
mkdir -p "$root/c/projects" "$root/repo"
git -C "$root/repo" init -q -b main
mkdir -p "$root/repo/.mason"
cp docs/examples/device-sample/.mason/mason.toml "$root/repo/.mason/mason.toml"
git -C "$root/repo" add -A; git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm sample
printf 'path = "%s"\nbase = "main"\ntest = "true"\nclaude_bin = "%s/%s/fake-claude"\n' "$root/repo" "$PWD" "$bin" > "$root/c/projects/sample.toml"
printf '[targets]\nprovider = "system"\npoll_ms = 1000\n' > "$root/c/daemon.toml"
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" 2>"$root/masond.log" &
daemon=$!
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
until m status >/dev/null 2>&1; do kill -0 "$daemon" 2>/dev/null || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.2; done
sleep 3
m targets list
echo "(copy this output into docs/stages/s25.md under 'Manual run')"
