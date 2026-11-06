#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Spike: kill -9 the controller mid-output, start a new one, expect no loss.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
d=$(mktemp -d); trap 'pkill -P 1 -f "host.py $d" 2>/dev/null || true; rm -rf "$d"' EXIT
N=40
python3 "$here/controller.py" "$d" --start sh -c "i=1; while [ \$i -le $N ]; do echo line \$i; i=\$((i+1)); sleep 0.1; done; exit 7" &
c1=$!
sleep 1.2
kill -9 "$c1"; wait "$c1" 2>/dev/null || true
echo "controller killed after $(wc -l < "$d/received") lines; host alive: $(kill -0 "$(python3 -c "import json,sys;print(json.load(open('$d/registry.json'))['host_pid'])")" && echo yes)"
sleep 1.0   # output produced while no controller is attached
python3 "$here/controller.py" "$d"          # new controller reattaches
code=$(cat "$d/exit_status")
seq -f "line %g" 1 $N | diff - "$d/received" && echo "no loss, no duplicates ($N lines)"
[ "$code" = 7 ] && echo "exit status seen: $code"
hp=$(python3 -c "import json;print(json.load(open('$d/registry.json'))['host_pid'])")
echo "host session leader: $(ps -o sess=,pid= -p "$hp" 2>/dev/null | tr -s ' ' | head -1)"
echo "STEP-HOST SPIKE PASS on $(uname -s)"
