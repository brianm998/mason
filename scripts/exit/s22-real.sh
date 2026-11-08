#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 22, for real: the scheduled `cargo-mutants` run and the scheduled live adapter run.
# MANUAL: never run by CI, and never by an agent on its own. The mutants run takes long and
# the live adapter run calls a real agent CLI and spends real tokens.
#
# Needs, all explicit:
#   MASON_REAL_APPROVED=yes     you have read this script and approve running it
#   cargo-mutants               `cargo install cargo-mutants`
#   MASON_REAL_LIVE=yes         (optional) also run the live adapter test, which needs the
#                               `claude` CLI signed in and spends tokens
#
# Each runs as a recurring job of a project that is a scratch clone of this repository, twice.
# The first run records a baseline (nothing is filed). The second must file nothing new unless
# something regressed in between.
set -euo pipefail
[ "${MASON_REAL_APPROVED:-}" = yes ] || { echo "refusing: set MASON_REAL_APPROVED=yes once you approve running this" >&2; exit 2; }
command -v cargo-mutants >/dev/null || { echo "needs cargo-mutants (cargo install cargo-mutants)" >&2; exit 2; }
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s22r.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
git clone -q "$PWD" "$root/scratch"
git -C "$root/scratch" checkout -q -b main
mkdir -p "$root/c/projects"
live=""
if [ "${MASON_REAL_LIVE:-}" = yes ]; then
  live='
[schedule.live]
kind = "live"
every = "7d"
command = "cargo test -p mason-adapters --test real_claude -- --ignored"
timeout_s = 1800
'
fi
cat > "$root/c/projects/scratch.toml" <<TOML
path = "$root/scratch"
base = "main"
test = "true"

[introspect]
analyst = false

[schedule.mutants]
kind = "mutants"
every = "7d"
command = "cargo mutants -p mason-core --no-shuffle -j2"
timeout_s = 7200
$live
TOML
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --owner "${USER:-owner}" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.2; done
m pause --mode claims --for 2h >/dev/null
echo "== cargo-mutants, first run (records the survivors as the baseline)"
m schedule run scratch/mutants
echo "== cargo-mutants, second run (files only survivors that are new)"
m schedule run scratch/mutants
if [ "${MASON_REAL_LIVE:-}" = yes ]; then
  echo "== live adapter run, twice"
  m schedule run scratch/live
  m schedule run scratch/live
fi
m schedule list
trap - EXIT; cleanup
echo "s22-real finished: read the lines above; a survivor in the second run that the first did not have is a regression"
