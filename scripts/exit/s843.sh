#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s843 exit test (#880): remote exit-scripts and cargo-workspace runs stay inside their limit.
#   (a) the shards of run-changed-exit-scripts.sh cover each selected script once;
#   (b) a script over MASON_EXIT_SCRIPT_LIMIT_S is killed, reported FAIL, and the later script still runs;
#   (c) scripts/test-shard.py kills an executable over MASON_TEST_EXE_LIMIT_S and still runs the others,
#       and without the variable has no deadline.
# Outcomes only, no timing assertions. Fakes are shell scripts; no binary is copied.
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fail() { echo "s843 FAIL: $*" >&2; exit 1; }

# ---- (a) and (b): a throwaway repository with fake exit scripts ----
w="$tmp/w"
mkdir -p "$w/scripts/exit"
cd "$w"
git init -q .
git config user.email t@example.invalid
git config user.name t
git config commit.gpgsign false
echo base > README
git add -A && git commit -q -m base
base=$(git rev-parse HEAD)
for n in 10 11 12 13 14 15 16 17; do
  printf '#!/usr/bin/env bash\nexit 0\n' > "scripts/exit/s$n.sh"
done
git add -A && git commit -q -m scripts

export MASON_BASE=$base
unset MASON_EXIT_SCRIPT_LIMIT_S MASON_EXIT_SHARD || true
all=$(bash "$repo/scripts/run-changed-exit-scripts.sh" | grep '^PASS ' | sort)
[ "$(echo "$all" | wc -l | tr -d ' ')" -eq 8 ] || fail "the unsharded run did not pass the 8 scripts: $all"
union=
for i in 1 2 3; do
  part=$(MASON_EXIT_SHARD=$i/3 bash "$repo/scripts/run-changed-exit-scripts.sh" | grep '^PASS s' || true)
  union="$union$part"$'\n'
done
union=$(printf '%s' "$union" | grep -v '^$' | sort)
[ "$union" = "$all" ] || fail "the shards' scripts differ from the unsharded selection: $union"

# (b) one script runs past a 1 s limit; a later one passes.
printf '#!/usr/bin/env bash\nsleep 60\n' > scripts/exit/s18.sh
printf '#!/usr/bin/env bash\nexit 0\n' > scripts/exit/s19.sh
git add -A && git commit -q -m slow
out=$(MASON_EXIT_SCRIPT_LIMIT_S=1 bash "$repo/scripts/run-changed-exit-scripts.sh" || true)
echo "$out" | grep -q '^FAIL s18 (over 1 s)' || fail "the slow script was not reported over its limit: $out"
echo "$out" | grep -q '^PASS s19$' || fail "the script after the slow one did not run: $out"
if MASON_EXIT_SCRIPT_LIMIT_S=1 bash "$repo/scripts/run-changed-exit-scripts.sh" >/dev/null; then
  fail "a run with an over-limit script exited 0"
fi

# ---- (c) test-shard.py with fake cargo and fake test executables ----
t="$tmp/t"
mkdir -p "$t/pkg"
cd "$t"
cat > pkg/slow.sh <<'EOF'
#!/bin/sh
if [ "$1" = "--list" ]; then echo "slow_t: test"; exit 0; fi
sleep 60
EOF
cat > pkg/fast.sh <<'EOF'
#!/bin/sh
if [ "$1" = "--list" ]; then echo "fast_t: test"; exit 0; fi
echo "test fast_t ... ok"
EOF
chmod +x pkg/slow.sh pkg/fast.sh
cat > fakecargo.sh <<EOF
#!/bin/sh
# Prints cargo's JSON artifact lines for the executables in FAKE_EXES; --doc runs nothing.
for a in "\$@"; do [ "\$a" = "--doc" ] && exit 0; done
for e in \$FAKE_EXES; do
  echo '{"reason":"compiler-artifact","profile":{"test":true},"executable":"'$t'/pkg/'\$e'.sh","target":{"name":"'\$e'","src_path":"'$t'/pkg/'\$e'.rs","kind":["test"]},"manifest_path":"'$t'/pkg/Cargo.toml","package_id":"'\$e' 0.1.0 (path+file:///x)"}'
done
EOF
chmod +x fakecargo.sh
echo '{"shards":1,"shard_lists":[]}' > run.json
shard() { MASON_SHARD=1 python3 -I "$repo/scripts/test-shard.py" --run-json run.json -- "$t/fakecargo.sh" test --workspace; }

rc=0
out=$(FAKE_EXES="slow fast" MASON_TEST_EXE_LIMIT_S=1 shard) || rc=$?
[ "$rc" -ne 0 ] || fail "an over-limit executable did not fail the shard: $out"
echo "$out" | grep -q 'timed out after 1 s' || fail "no timeout reason in: $out"
echo "$out" | grep -q 'test fast_t ... ok' || fail "the executable after the slow one did not run: $out"

rc=0
out=$(FAKE_EXES="fast" shard) || rc=$?
[ "$rc" -eq 0 ] || fail "a passing shard without a limit failed: $out"
echo "$out" | grep -q 'test fast_t ... ok' || fail "the unlimited run did not run the test: $out"

echo "s843 OK"
