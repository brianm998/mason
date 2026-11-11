#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s97 exit test: a long history is capped in the sidebar, with the true count.
#  0. cargo: the view tests (typical figures match the old code, the cap, no quadratic work).
#  1. A real masond with the whole project paused, so nothing runs. 520 tasks (more than
#     the 500-row cap) are filed and canceled.
#  2. A websocket client reads the sidebar: Canceled says count 520, sends 500 rows, and the
#     newest canceled (#519) is first.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
echo "== 0. cargo tests"
cargo test -q -p mason-daemon --lib views::
cargo test -q -p mason-daemon --test history_cap
root=$(mktemp -d /tmp/mason-s97.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
cdir="$root/c"; mkdir -p "$cdir/projects"
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$cdir" "$@"; }
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
cat > "$cdir/projects/p.toml" <<TOML
path = "$repo"
base = "main"
remote = "origin"
test = "true"
claude_bin = "/usr/bin/false"
TOML
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$cdir" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
m pause --for 2h >/dev/null

echo "== 1. file and cancel 520 tasks"
n=520
for i in $(seq 0 $((n - 1))); do
  m task add "t$i" --project p --body x >/dev/null
  m task cancel "$i" >/dev/null
done

echo "== 2. the sidebar over the websocket"
got=$(python3 -I scripts/exit/fixtures/s97/sidebar.py "$root/d" | sed -n 's/^canceled //p')
echo "   canceled: count rows first = $got"
[ "$got" = "$n 500 #$((n - 1))" ] || { tail -20 "$root/masond.log" >&2; echo "wanted [$n 500 #$((n - 1))]" >&2; exit 1; }
echo "s97 passed"
