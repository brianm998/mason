#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s830 exit test (#873): a narrowed remote check is sharded, a check that ended without a result
# is retried inside MASON, and a skip ends a check that waits.
#
# Not here: a scripted agent against a real masond and a fake GitHub over HTTP. The fake GitHub
# these tests use is the in-process one of crates/daemon/src/remote.rs, which drives the same
# `run_retrying` and `execute_sharded` that `run_check` calls.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

# 1. Exit-script shards: every selected script lands on exactly one shard, heaviest first onto
#    the lightest, by weights.txt.
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cat > "$work/weights.txt" <<'W'
s9001 300
s9002 200
s9003 100
s9004 100
s9005 50
W
list="scripts/exit/s9001.sh scripts/exit/s9002.sh scripts/exit/s9003.sh scripts/exit/s9004.sh scripts/exit/s9005.sh"
plan() { # plan I N
  printf '%s\n' $list | awk -v si="$1" -v sn="$2" -v wf="$work/weights.txt" -v dw=60 '
    BEGIN { while ((getline line < wf) > 0) { split(line, f, " "); if (f[1] ~ /^s[0-9]+$/ && f[2] ~ /^[0-9]+$/) w[f[1]] = f[2] + 0 } }
    { n++; path[n] = $0; name = $0; sub(/.*\//, "", name); sub(/\.sh$/, "", name); wt[n] = (name in w) ? w[name] : dw + 0 }
    END { for (k = 1; k <= sn; k++) tot[k] = 0
      for (a = 1; a <= n; a++) idx[a] = a
      for (a = 2; a <= n; a++) { v = idx[a]; b = a - 1; while (b >= 1 && wt[idx[b]] < wt[v]) { idx[b + 1] = idx[b]; b-- }; idx[b + 1] = v }
      for (a = 1; a <= n; a++) { i = idx[a]; best = 1; for (k = 2; k <= sn; k++) if (tot[k] < tot[best]) best = k; tot[best] += wt[i]; owner[i] = best }
      for (i = 1; i <= n; i++) if (owner[i] == si) print path[i] }'
}
# The real script's awk must be this one: compare its plan on a throwaway repository.
repo="$work/repo"
mkdir -p "$repo/scripts/exit" "$repo/scripts"
cp scripts/run-changed-exit-scripts.sh scripts/select-exit-scripts.sh "$repo/scripts/"
for s in s9001 s9002 s9003 s9004 s9005; do
  printf '#!/bin/sh\nexit 0\n' > "$repo/scripts/exit/$s.sh"
done
cp "$work/weights.txt" "$repo/scripts/exit/weights.txt"
(
  cd "$repo"
  git init -q -b main . && git -c user.name=t -c user.email=t@t add -A && git -c user.name=t -c user.email=t@t commit -qm base
  git checkout -q -b change
  for s in s9001 s9002 s9003 s9004 s9005; do echo "# edit" >> "scripts/exit/$s.sh"; done
  git -c user.name=t -c user.email=t@t commit -qam edit
)
got=""
for i in 1 2; do
  out=$(cd "$repo" && MASON_BASE=main MASON_EXIT_SHARD="$i/2" bash scripts/run-changed-exit-scripts.sh)
  got="$got$(echo "$out" | sed -n 's/^PASS //p' | sort | tr '\n' ' ')|"
  want=$(plan "$i" 2 | sed 's|.*/||; s|\.sh$||' | sort | tr '\n' ' ')
  [ "$(echo "$out" | sed -n 's/^PASS //p' | sort | tr '\n' ' ')" = "$want" ] || { echo "shard $i ran [$out], wanted [$want]" >&2; exit 1; }
done
all=$(for i in 1 2; do (cd "$repo" && MASON_BASE=main MASON_EXIT_SHARD="$i/2" bash scripts/run-changed-exit-scripts.sh); done | sed -n 's/^PASS s//p' | sort | tr '\n' ' ')
[ "$all" = "9001 9002 9003 9004 9005 " ] || { echo "the shards together ran [$all]" >&2; exit 1; }

# 2. The fake-GitHub tests: retry, skip cancel, narrowed shards; the narrowing decision; the keys.
cargo test -q -p mason-daemon --lib -- remote::tests::a_run_nobody_picks_up remote::tests::a_refused_push remote::tests::a_run_cancelled_at_the_limit remote::tests::two_misses remote::tests::a_red_run_is_a_result remote::tests::a_skip_while_a_run_waits remote::tests::a_narrowed_run_of_three_targets narrowed_shard_tests
cargo test -q -p mason-daemon --test it fallback_lease
cargo test -q -p mason-definition --test testing_tables narrowed_shard_keys_parse
echo "s830 ok"
