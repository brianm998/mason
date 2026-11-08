#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 22, by hand: the Health page in both monitors. MANUAL: it needs a person, a screen and
# the built monitors (SwiftUI on a Mac, Compose on the desktop). Nothing is asserted here.
#
# It starts a scratch masond on a free port with one planted retry loop, waits for the
# detector to file its proposal, then waits while you look. Open each monitor, point it at the
# printed address, choose Health in the sidebar, and check:
#   1. "Findings" lists the retry loop with its metric (retry_loop_rate) and the verdict "open".
#   2. "Recurring jobs" lists introspect and the analyst rows with when they last ran.
#   3. "Digest" shows landed, interventions, exploring skips by route (with how many came
#      back), forced landings with reasons, and "Tokens per landed task: not recorded".
#   4. "Triage" says it is off.
#   5. Both monitors show the same numbers.
# Press Return to stop.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s22m.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q --workspace --bins
cargo build -q -p mason-store --example plant
bin=target/debug
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
git init -q -b main "$root/shop"
mkdir -p "$root/shop/.mason" "$root/shop/crates/daemon/src" "$root/c/projects"
echo "// retries" > "$root/shop/crates/daemon/src/jobs.rs"
printf 'schema = 1\nstart = "develop"\n[step.develop]\ntype = "agent"\ncategory = "developing"\nnext = "land"\n[step.land]\ntype = "landing"\ncategory = "landing"\n' > "$root/shop/.mason/mason.toml"
printf 'mode = "direct"\n' > "$root/shop/.mason/landing.toml"
git -C "$root/shop" add -A; git -C "$root/shop" -c user.name=t -c user.email=t@t commit -qm init
printf 'path = "%s"\nbase = "main"\ntest = "true"\n' "$root/shop" > "$root/c/projects/shop.toml"
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --owner "${USER:-owner}" --introspect-ms 2000 --schedule-ms 500 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.2; done
m pause --mode claims --for 2h >/dev/null
target/debug/examples/plant attempts "$root/d/store.db" --project shop --task 1 --step develop --outcome fail \
  --reason "the agent failed: unknown model fable-9" --count 5 --retry-from 1
until m proposals | grep -q 'Retry loop'; do sleep 0.5; done
m health
echo
echo "masond is on: $(cat "$root/d/daemon.json" 2>/dev/null || echo 'see daemon.json in the data directory')"
echo "Open a monitor on it, choose Health, check the list above. Press Return to stop."
read -r _
