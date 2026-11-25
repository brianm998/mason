#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1075 exit test: the runner's no-timeout(1) fallback returns when a script ends and
# kills a script's whole process group at the limit; a narrowed shard runs without --doc.
set -euo pipefail
cd "$(dirname "$0")/../.."
if [ -z "${S1075_GUARDED:-}" ]; then S1075_GUARDED=1 exec perl -e 'alarm 120; exec @ARGV' bash "$0"; fi
unset MASON_EXIT_SHARD

d=$(mktemp -d); trap 'rm -rf "$d"' EXIT
cat > "$d/s21.sh" <<'X'
echo PASSING-MARK
X
cat > "$d/s22.sh" <<'X'
sleep 7731 &
sleep 7732
X
printf 's21 1\ns22 1\n' > "$d/weights.txt"

# (a) a passing script returns at once and leaves no watcher sleep.
out=$(MASON_EXIT_FALLBACK=1 MASON_EXIT_DIR="$d" MASON_EXIT_WEIGHTS="$d/weights.txt" \
  MASON_EXIT_SCRIPT_LIMIT_S=7733 MASON_EXIT_PLAN= bash -c 'mv "$MASON_EXIT_DIR/s22.sh" "$MASON_EXIT_DIR/x22"; bash scripts/run-exit-scripts.sh 2>&1; mv "$MASON_EXIT_DIR/x22" "$MASON_EXIT_DIR/s22.sh"') \
  || { echo "passing run failed"; echo "$out"; exit 1; }
echo "$out"
echo "$out" | grep -q '^s21: [0-9]* s' || { echo "no s21 timing line"; exit 1; }
! pgrep -f 'sleep 7733' >/dev/null || { echo "watcher sleep left"; exit 1; }

# (b) a hanging script with a descendant holding stdout is killed and the run ends.
rm "$d/s21.sh"
out=$(MASON_EXIT_FALLBACK=1 MASON_EXIT_DIR="$d" MASON_EXIT_WEIGHTS="$d/weights.txt" \
  MASON_EXIT_SCRIPT_LIMIT_S=2 bash scripts/run-exit-scripts.sh 2>&1) && { echo "should have failed"; exit 1; }
echo "$out"
echo "$out" | grep -q 'FAILED: scripts/exit/s22.sh\|FAILED: .*s22.sh (over 2 s)' || { echo "limit not reported"; exit 1; }
! pgrep -f 'sleep 773[12]' >/dev/null || { echo "descendant left"; exit 1; }
# (c) test-shard.py never puts --doc beside a target selector, which cargo refuses.
t="$d/t"; mkdir "$t"
printf '#!/bin/sh\n[ "$1" = --list ] && echo "a_t: test" && exit 0\necho "test a_t ... ok"\n' > "$t/a.sh"
cat > "$t/cargo.sh" <<X
#!/bin/sh
case " \$* " in
  *" --doc "*) case " \$* " in *" --test "*|*" --no-run "*) echo "can't mix --doc" >&2; exit 101 ;; esac
    echo "DOC-RUN" >> "$t/docs"; exit 0 ;;
esac
echo '{"reason":"compiler-artifact","profile":{"test":true},"executable":"$t/a.sh","target":{"name":"a","src_path":"$t/a.rs","kind":["test"]},"manifest_path":"$t/Cargo.toml","package_id":"a 0.1.0"}'
X
chmod +x "$t/a.sh" "$t/cargo.sh"
echo '{"shards":1,"shard_lists":[]}' > "$t/run.json"
shard() { MASON_SHARD=1 python3 -I scripts/test-shard.py --run-json "$t/run.json" -- "$t/cargo.sh" test "$@"; }
out=$(shard -p a --test a 2>&1) || { echo "narrowed shard failed"; echo "$out"; exit 1; }
echo "$out" | grep -q 'test a_t ... ok' || { echo "narrowed test did not run: $out"; exit 1; }
[ ! -e "$t/docs" ] || { echo "doc tests ran for a --test selection"; exit 1; }
out=$(shard -p a --doc 2>&1) || { echo "doc-only shard failed"; echo "$out"; exit 1; }
[ -e "$t/docs" ] || { echo "doc-only selection ran no doc tests: $out"; exit 1; }
echo "s1075 passed"
