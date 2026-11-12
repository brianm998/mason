#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# CPU gate fallback (s133) exit test, without load: a real masond whose sampler's primary reader
# always fails (MASON_CPU_TEST_PRIMARY_FILE names a file that does not exist) and whose fallback
# reads MASON_CPU_TEST_FALLBACK_FILE.
#  1. Fallback busy: `mason resources` names the fallback source and does not say "no gate now",
#     and a further compile is held (the first compile always starts, so one holds a slot first).
#  2. Fallback idle: the held compile starts.
# Not covered here (it needs a dozen real builds): the real-load case.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s133.XXXXXX); daemon=""; cc=""; first=""
cleanup() {
  [ -n "$first" ] && { touch "$root/release"; kill "$first" 2>/dev/null; } || true
  [ -n "$cc" ] && kill "$cc" 2>/dev/null || true
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { set +e; echo "FAILED: $*" >&2; m resources >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }

mkdir -p "$root/c" "$root/d"
echo 100 > "$root/fallback"
# MASON_CPU_FILE (the exit-script runner pins it) would replace both readers, so it is unset.
env -u MASON_CPU_FILE MASON_CPU_TEST_PRIMARY_FILE="$root/no-such-file" MASON_CPU_TEST_FALLBACK_FILE="$root/fallback" \
  "$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== 1. primary reader failing, fallback busy: the source is shown and a further compile is held"
m resources limit compile 2 >/dev/null
m run --needs compile -- sh -c "until [ -e '$root/release' ]; do sleep 0.1; done" 2>"$root/first.err" &
first=$!
end=$((SECONDS + 20))
until m resources | grep -q 'compile .* 1/2 in use'; do [ $SECONDS -lt $end ] || fail "the first compile did not start"; sleep 0.2; done
m run --needs compile -- touch "$root/compiled" 2>"$root/cc.err" &
cc=$!
end=$((SECONDS + 30))
until m resources | grep -q 'source loadavg'; do [ $SECONDS -lt $end ] || fail "mason resources does not name the fallback"; sleep 0.3; done
m resources | grep -q 'no gate now' && fail "mason resources still says no gate"
m resources | grep '^  cpu' | sed 's/^/   /'
sleep 2
[ ! -e "$root/compiled" ] || fail "the compile ran while the fallback said busy"
grep -q 'compile waiting: CPU 100% > 50%' "$root/cc.err" || fail "the wait was not explained: $(cat "$root/cc.err")"
m resources --json | grep -q '"source": "loadavg"' || fail "the health JSON has no cpu.source"
m resources --json | grep -q '"failed_reads"' || fail "the health JSON has no cpu.failed_reads"
grep -q 'cpu reading failed' "$root/masond.log" || fail "the failed read was not logged"
echo "   held, with the fallback named"

echo "== 2. fallback idle: the compile starts"
echo 0 > "$root/fallback"
end=$((SECONDS + 30))
until [ -e "$root/compiled" ]; do [ $SECONDS -lt $end ] || fail "the compile did not start when the fallback said idle"; sleep 0.2; done
wait "$cc"; cc=""
touch "$root/release"; wait "$first"; first=""
echo "   started"

trap - EXIT; cleanup
echo "s133 OK"
