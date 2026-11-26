#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1117 exit test: a 2 h pause is not listed as "in 1h", and the record of the resume that
# lifts it names the client and the pause. A real masond and the real command line.
#  1. `mason pause --mode all --for 2h`; `mason pause list` says "in 2h".
#  2. `mason resume`; the newest `resume` intervention says `via cli` and the lifted mode.
#  3. `mason pause list` then says who lifted it.
#  4. The daemon and command line tests for the same.
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$(mktemp -d /tmp/mason-s1117.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
fail() { echo "FAILED: $*" >&2; tail -20 "$root/masond.log" >&2 || true; exit 1; }
# The command line of a task's step calls itself that task's agent; here it is a person's.
unset MASON_TASK MASON_AS MASON_REASON

mkdir -p "$root/c/projects" "$root/d"
MASON_CPU_FILE="$root/cpu" "$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" \
  --heartbeat-ms 100 --sweep-ms 3600000 2>>"$root/masond.log" &
daemon=$!
until [ -s "$root/d/daemon.json" ]; do kill -0 "$daemon" || fail "masond exited"; sleep 0.1; done
cli() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }

echo "== 1. a 2 h pause"
cli pause --mode all --for 2h >/dev/null || fail "pause"
list=$(cli pause list)
case "$list" in *"in 1h"*) fail "an hour short: $list" ;; esac
case "$list" in *"until in 2h"*) ;; *) fail "no expiry in 2h: $list" ;; esac
echo "   $list"

echo "== 2. the resume is recorded"
cli resume >/dev/null || fail "resume"
detail=$(python3 -I - "$root/d/store.db" <<'PY'
import sqlite3, sys
db = sqlite3.connect(sys.argv[1])
row = db.execute("SELECT detail FROM interventions WHERE kind = 'resume' ORDER BY id DESC LIMIT 1").fetchone()
print(row[0] if row else "")
PY
)
case "$detail" in "/resume via cli: lifted #"*" all for everything, expires "*) ;; *) fail "detail: $detail" ;; esac
echo "   $detail"

echo "== 3. the list says who lifted it"
list=$(cli pause list)
case "$list" in *"nothing is paused"*"lifted "*" via cli"*) ;; *) fail "no lift line: $list" ;; esac

echo "== 4. the tests"
cargo test -q -p mason-daemon --test it pause_record::
cargo test -q -p mason-pipeline --bin mason ago_until
echo "s1117 OK"
