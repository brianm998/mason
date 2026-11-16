#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Spike (task 462): can sccache cache workspace crates across per-task target dirs?
# Builds `cargo test --workspace --no-run` from a cold target dir in two worktrees of HEAD, one
# after the other, for each variant, and prints wall seconds, CPU seconds (user+sys) and the
# sccache counters for each build. Run it under `mason run --needs compile -- bash <this>`.
#
#   A  as today (incremental, shared sccache server)
#   B  CARGO_INCREMENTAL=0
#   C  CARGO_INCREMENTAL=0 + SCCACHE_BASEDIRS=<worktrees root>
#   D  CARGO_INCREMENTAL=0 + SCCACHE_BASEDIRS=<the two worktrees themselves>
#
# Each variant uses its own sccache server (own port, same disk cache) so its counters are its
# own and BASEDIRS (read when a server starts) takes effect. Afterwards it times the run_check
# case: a one-line edit in crates/daemon, rebuilt in the same worktree, incremental and not.
set -uo pipefail
cd "$(dirname "$0")/../.."
repo=$PWD
out=${1:-/Volumes/sp/mason/spike-sccache}
mkdir -p "$out"
out=$(cd "$out" && pwd)
head=$(git rev-parse HEAD)
wa=$out/wt/a
wb=$out/wt/b
for w in "$wa" "$wb"; do
  [ -d "$w" ] || git worktree add --detach "$w" "$head" >/dev/null
done
port=4990

stats() { SCCACHE_SERVER_PORT=$1 sccache --show-stats --stats-format json; }

# build <label> <port> <worktree> <target-dir> [env...]: one timed build, one result line.
build() {
  local label=$1 p=$2 wt=$3 td=$4
  shift 4
  local before after tf
  before=$(stats "$p")
  tf=$(mktemp)
  (cd "$wt" && env SCCACHE_SERVER_PORT="$p" "$@" \
    /usr/bin/time -l cargo test --workspace --no-run --target-dir "$td" >"$out/$label.log" 2>"$tf")
  local rc=$?
  after=$(stats "$p")
  python3 - "$label" "$rc" "$tf" "$before" "$after" <<'PY'
import json, re, sys
label, rc, tf, before, after = sys.argv[1:]
t = open(tf).read()
real = float(re.search(r"([\d.]+)\s+real", t).group(1))
cpu = float(re.search(r"([\d.]+)\s+user", t).group(1)) + float(re.search(r"([\d.]+)\s+sys", t).group(1))
b, a = json.loads(before)["stats"], json.loads(after)["stats"]
def d(f): return f(a) - f(b)
hits = d(lambda s: s["cache_hits"]["counts"].get("Rust", 0))
miss = d(lambda s: s["cache_misses"]["counts"].get("Rust", 0))
nc = {k: a["not_cached"].get(k, 0) - b["not_cached"].get(k, 0) for k in a["not_cached"]}
nc = {k: v for k, v in nc.items() if v}
print(f"RESULT {label} rc={rc} wall={real:.1f}s cpu={cpu:.1f}s rust_hits={hits} rust_misses={miss} not_cached={nc}")
PY
  rm -f "$tf"
}

edit() { echo "// spike edit $1" >>"$2/crates/daemon/src/lib.rs"; }

variant() { # <name> <port> <env...>
  local name=$1 p=$2
  shift 2
  SCCACHE_SERVER_PORT=$p sccache --stop-server >/dev/null 2>&1
  env SCCACHE_SERVER_PORT="$p" "$@" sccache --start-server >/dev/null
  rm -rf "$out/t-$name-a" "$out/t-$name-b"
  build "$name-cold-a" "$p" "$wa" "$out/t-$name-a" "$@"
  build "$name-cold-b" "$p" "$wb" "$out/t-$name-b" "$@"
  edit "$name" "$wa"
  build "$name-edit-a" "$p" "$wa" "$out/t-$name-a" "$@"
  git -C "$wa" checkout -- crates/daemon/src/lib.rs
  SCCACHE_SERVER_PORT=$p sccache --stop-server >/dev/null 2>&1
}

variant A $((port + 0)) SPIKE_VARIANT=A
variant B $((port + 1)) CARGO_INCREMENTAL=0
variant C $((port + 2)) CARGO_INCREMENTAL=0 SCCACHE_BASEDIRS="$out/wt"
variant D $((port + 3)) CARGO_INCREMENTAL=0 SCCACHE_BASEDIRS="$wa:$wb"
echo "spike commit $head, worktrees under $out"
